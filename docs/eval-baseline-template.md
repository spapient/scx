# eval-baseline.json 标注采集模板（人工标注指南）

版本：v1.0（2026-10-22）
用途：为 P2/P4 阶段门收集 30-50 组分层人工标注，喂给 `scx --eval <baseline.json>` 出正式 A/B 对拍表与 L1 命中率。

> 自动标注（`--annotate`）只用于回归 smoke：它用 A 管线自检放行变体，存在选择偏差，
> 不能替代本指南的人工标注，也不能单独作为 M4 质量证据。自动标注结果须同时报告
> `dropped` / `skipped` 漏斗，并用独立人工样本复核。

> 采集原则：只标真实库里的 case（expected 必须是库中实际存在的 case id），query 变体按真实使用场景设计，
> 宁可少标不编造。标注集一经确定即冻结（--eval 确定性重放），后续库变化不影响对拍结论的可重放性。

---

## 1. baseline.json 格式

顶层为对象，唯一必填键 `cases`（数组）；每条 case 四个字段：

```json
{
  "cases": [
    { "query": "db timeout conn_id", "layer": "literal", "expected": ["2026-10-22_0adada28"], "rel": {"0adada28": 2} },
    { "query": "数据库连接池超时 连接池", "layer": "paraphrase", "expected": ["0adada28"], "judge": null, "note": "target=0adada28 hint=[ERROR db timeout conn_id=fkab pool exhausted]" },
    { "query": "存储空间不足 errno 磁盘", "layer": "semantic", "expected": ["2026-10-22_5f82b3ab"], "judge": null, "note": "target=5f82b3ab hint=[ERROR db write errno=28 no space left]" }
  ]
}
```

字段定义：

| 字段 | 类型 | 说明 |
|---|---|---|
| query | string | 评估时用的检索词（按真实用法写，可含字段/实体/中文混排） |
| layer | string | literal / paraphrase / semantic 三选一（分层定义见 §2） |
| expected | string[] | 相关 case id 列表（完整 id 或 hash16/hash8 后缀均可，后缀免日期漂移） |
| rel | object（可选） | case id → 相关度 0..2；缺省时 expected 内视为 1 |
| judge | object/null（可选） | 裁决元数据 `{"by":"姓名","date":"UTC","method":"可选"}`；`null` = 待 `--judge` 裁决。非 literal 条目必须有 by+date 才能过 `--gate`。`method=policy-materialized-*` 表示人工规则物化；`--gate-human` 会以 provenance 失败，不能再写“人工闭环” |
| note | string（可选） | 盲测摘要，建议 `target=<hash16> hint=[首行] fix=[修复结论]`；`--judge` 只显示 query+note，不显示 A/B 分数 |

## 2. 三层定义与造例规则

| 层 | 含义 | query 怎么造 | 示例（对同一 case） |
|---|---|---|---|
| literal | 词面命中 | 与 case.query / 证据块用词基本一致 | `db timeout conn_id` |
| paraphrase | 同义改写 | 换同义词/语序/中英混排，词面有偏移 | `数据库连接池超时 连接池`、`连接池 又 超时了` |
| semantic | 语义相关 | 说同一件事但几乎无共享词（最难的层） | 主题相同但换了 token：`存储空间不足 errno 磁盘` |

造例顺序：先从库选一个"真实发生过的问题"case，写它的 literal 原文 → 改写成 paraphrase → 再剥离成 semantic。每层至少 10 组（共 30-50 组）。

## 3. rel 分级语义（0..2）

| 值 | 语义 | 典型场景 |
|---|---|---|
| 2 | 强相关：该 case 直接就是答案 | 同 query 二次遇到、字段/实体精确对应 |
| 1 | 相关：参考价值，但不是直接答案 | 兄弟 case、相似根因 |
| 0 | 无关 | 干扰项（也可只列 expected 相关 case，0 可省） |

## 4. 怎么拿到真实 case id

```bash
# 对库中某 query 召回，看返回 id（--recall-detail 还带要点行，方便辨认是不是同一个问题）
scx --query "db timeout conn_id" --recall-only --recall 5 --recall-detail --mem-dir <库目录>
# 输出 mem: 段，每行一个 case：id score sim bucket [summary: ...]
```

取 `summary` 行的 error_code/file_loc 与你的问题核对，确认真是同一个根因再标 rel=2。

## 5. 推荐采集流程（照做即可）

> 不想手写 JSON：先运行 `powershell -ExecutionPolicy Bypass -File scripts\real-eval-build.ps1`。
> 首次会生成 `eval-real\cases.csv`（列：`query,layer,case_id,note`）；填 30+ 行后再运行一次，
> 脚本会生成 `eval-real\baseline.json`：literal 自动 `rel=2`，paraphrase/semantic 自动 `judge:null`。

1. 挑 30-50 个真实问题（日志/报错/diff 复盘记录），每个问题选 1 个代表 case。
2. 每个 case 写 1 条 literal + 1 条 paraphrase（同一问题可多条变体）；semantic 层按主题另造。
3. 全部 expected 用 `--recall-detail` 核对过 id 后填入。
4. 保存为 `baseline.json`（建议放 `docs/eval/` 下，与日期/库版本一起留档）。
5. 盲测裁决：`scx --judge baseline.json --judge-by <姓名>`（只对 `judge:null` 的 paraphrase/semantic 逐条输入 `2/1/0/s`；literal 可由指纹机械判定，保留 rel 即可）。
6. 先跑机械门：`scx --eval baseline.json --gate --eval-verbose --mem-dir <库目录>`；它只检查样本量、分层、裁决元数据和 A 不劣于 B，不代表弱相关/负例已闭环。
7. 闭环门：补完 0/1 控制标签后跑 `scx --eval baseline.json --gate-human --eval-verbose --mem-dir <库目录>`；只有 `gate: mode=human` + `gate: PASS` 才能写“人工弱相关/负例闭环”。
8. 把 `--judge` 后的 JSON、两次 gate 输出、裁决人/日期/seed 一起留档；报告行贴进 `docs/rag-implementation-plan.md` §7 P4.1 的验证记录。

## 5.1 弱相关/负例 + 第二裁决人（2026-10-23 新增脚本）

上面的 `--judge` 只对每条 query 的**单一目标**打 rel，产不出弱相关（1）和负例（0），
gate 会一直告警 `all labels are 2`。要补弱相关/负例并支持第二裁决人：

1. 生成盲测候选包（隐藏 target id 与 A/B 分数，只有 query + 候选摘要 + fix）：

   ```powershell
   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\judge-audit.ps1 -Baseline eval-real\baseline.json -MemDir eval-real\mem -Out eval-real\judge-packet.tsv
   ```

2. 人工在 `rel` 列逐候选填 `2/1/0`（或 `s` 跳过）。这是唯一产出弱相关/负例的诚实途径：
   候选池来自检索器 top-k，裁决人不知道哪条是 target，按 query 与候选摘要的相关度独立打分。
3. 回填（写入 rel 字典 + judge.by/date）：

   ```powershell
   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\judge-audit.ps1 -Import eval-real\judge-packet.tsv -Baseline eval-real\baseline.json -Judge <姓名>
   ```

4. 第二裁决人：用同一 `judge:null` 副本独立执行 1–3（不同 `-Judge` 姓名），然后比对一致性：

   ```powershell
   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\judge-agreement.ps1 -A first.json -B second.json
   ```

   输出共同候选数、原始一致率、Cohen's kappa（0/1/2 三类）与逐条分歧；分歧项需人工仲裁，
   仲裁后重新冻结 baseline。只有两人的裁决都写出 by+date 且 kappa 报告在案，才能写“双人复核”。

> 红线：包内没有 target id 也没有 A/B 分数；不要用自动 rel 或 AI 自动标替人裁决——
> `judge.by` 必须是真人姓名，AI/脚本标的一律不算“独立人工证据”。
> 例外仅限**已经冻结的人工裁决规则**：可以物化为 `judge.method=policy-materialized-v1`
> 的替代控制集，但 `--gate-human` 必须失败；只能报替代指标，不能写“gate-human PASS”
> 或“逐条人工盲审闭环”。

## 6. 约束与红线

- expected 必须是库中真实存在的 id——`--eval` 会按库实际召回计算，写错 id 等于污染结论。
- 同一 baseline 一旦用于结论，禁止再改（改了要重跑全流程并注明版本）。
- 分层标签要稳定：literal 只含词面命中例；paraphrase 必须与 literal 词面不同；semantic 几乎无共享词。
- 数量口径：30-50 组是下限建议，多多益善；每层独立统计，层内样本太少（<5）时报告会暴露 n，请勿拿单例下结论。

## 7. M4 两道门（`--gate` / `--gate-human`）与真实语料红线

`--eval --gate` 是**机械资格门**，只认同时满足以下条件的 baseline：

- 总组数 ≥30；literal / paraphrase / semantic 各 ≥10；
- 非 literal 条目必须有 `judge.by` + `judge.date`（`--judge` 自动写入）；
- 每层 A 的预声明主指标（literal/paraphrase r@5；semantic r@10）与 MRR 都不低于 B；
- 每层 A/B 的主指标均 ≥0.20 且 MRR 均 ≥0.10（绝对地板：拒绝全 miss 或退化基线下的“相对优势”）；
- 不允许三层之外的 layer 凑数。

`--eval --gate-human` 是**人工弱相关/负例闭环门**，在机械门之上再要求：

- 每条 case（含 literal）都有非机械/非 AI 的 `judge.by` + `judge.date`；
- `rel` 的每个候选键都必须在 `judge_items` 有独立真人条目，且
  `judge.method` 为 `per-item-*`；整包签名不算逐条裁决；
- 全语料至少有 10 条 `rel=1` 弱相关和 10 条 `rel=0` 负例；
- 至少两层同时包含弱相关与负例（防止控制全部堆在一层）；
- 每层 NDCG@10 A ≥ B（主指标持平时仍能验证弱相关/负例排序）。

`--eval --gate-items` 是**逐候选裁决门**：条件与 human 门相同，但允许非人类
裁决人（AI 记 `judge.by=codex`、`judge.method=per-item-ai-v1`）。输出固定包含
`adjudicator=... non-human`，PASS 只代表 E3 逐条裁决，不得写成人工闭环。

红线：

- confirm5 的 `rel` 全是 2，`--gate` 可以证明目标排序，`--gate-human` 必须失败；
- `eval-real/confirm5-baseline-human.json`（115 弱/665 负，policy 物化）`--gate-human` FAIL；只能报替代指标，不算逐条人工；
- `eval-real/baseline-final.json`（11/64 控制，逐条双人裁决）、
  `real-baseline-arb.json`（151/46）与 `v2-baseline-arb.json`（312/77）
  在旧门（仅案例级 `judge.by`）通过；逐候选门禁收紧后三者均为 FAIL
  （baseline 未带 `judge_items`，依次 0/116、0/344、0/821）；
- confirm5 的 40/150 预注册盲审包是当前唯一可直接补齐逐候选证据的路径
  （`eval-real/confirm5-blind40-preregistration.md`），未填完前闭环不成立；
- 自动标注集、AI 草案、机械 target 规则都不能冒充人工闭环。

因此：

- `demo.ps1` 的 23 组（16/5/2）**不能**过门，只能做流程演练；
- `--annotate` 自动标注集没有 judge 元数据，**不能**过门；
- 真实语料必须冻结后再调参，裁决时不得查看 A/B 分数；
- 只有你一人裁决时，报告写“single-annotator, author, blind to A/B”，不要写“独立人工”；要写独立证据，至少加第二人复核/仲裁。
