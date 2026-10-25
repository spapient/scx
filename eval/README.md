# scx 评测工具

本目录保存可重建的评测语料、冻结标签、盲审包和唯一评测入口。记忆库目录、
verbose 日志、配对统计 CSV 等均为**生成物**，不进入版本库。

在仓库根目录运行：

```bash
./bin/scx-eval --help          # macOS / Linux
python bin/scx-eval --help     # Windows
```

评测器只使用 Python 标准库，负责语料播种、基线构造、盲审编排与统计；检索和
证据门仍有且只有一份 Rust 实现，位于 `src/`。评测脚本不能在 Python 侧复刻排序。

## 目录

| 路径 | 用途 |
|---|---|
| `../bin/scx-eval` | 语料播种、基线构造、盲审、配对统计、包校验 |
| `confirm5.corpus.json` | confirm5：30 起公开事故，150 条存储 case |
| `v2.corpus.json` | v2 开发集：30 起公开事故，150 条存储 case |
| `real.corpus.json` | 真实事故集：11 起公开复盘，40 条 case |
| `confirm5-policy.labels.json` | 规则物化控制集，**不是人工标签** |
| `confirm5-ai.labels.json` | 384/384 AI 逐候选标签（`per-item-ai-v1`） |
| `confirm5-hao.labels.json` | 384/384 人工逐候选标签（`per-item-human-v1`） |
| `v2.labels.json` | v2 案例级标签，AI 预填后由两位审核者采纳 |
| `real.labels.json` | real 案例级标签（`judge.by=fei+hao`） |
| `confirm5-40.packet.tsv` | 冻结盲审包：40 case / 384 候选 |
| `confirm5-40-ai.tsv` | AI 输入包，保留 provenance |
| `confirm5-40-hao.tsv` | 人工输入包，含 literal 不变量修正 |

## 语料格式

语料 schema 与具体检索器解耦：

```json
{
  "name": "example",
  "expected_incidents": 1,
  "expected_cases": 1,
  "incidents": [{
    "id": "incident-id",
    "source": "https://example.invalid/postmortem",
    "root_cause": "one-line public root cause",
    "fix": "one-line public remediation",
    "cases": [{
      "store_query": "query used to store the case",
      "text_query": "optional first line of the stored text",
      "eval_query": "query used by --eval",
      "layer": "literal"
    }]
  }]
}
```

`text_query` 缺省时使用 `store_query`。播种后的文本形状固定为：

```text
<text_query>
公开复盘, 来源: <source>
根因: <root_cause>
```

2026-10-25 复验：三个归一化语料分别生成 confirm5 150/150、v2 150/150、
real 40/40，case hash 后缀与冻结基线一致；日期前缀变化不影响标签匹配。

## 标注契约

评估集应包含 30–50 个真实 case，`literal`、`paraphrase`、`semantic` 至少各 10 个。
每个 case 条目支持：

| 字段 | 含义 |
|---|---|
| `query` | 传给 `--eval` 的检索 query |
| `layer` | `literal` / `paraphrase` / `semantic` |
| `expected` | 一个或多个目标 case id（完整 id 或 hash 后缀） |
| `rel` | 候选 id → `2` 直接答案、`1` 相关、`0` 无关；未列出的 expected 默认 `1` |
| `judge` | `{by,date,method}`；`null` 表示尚未裁决 |
| `note` | 盲审提示；不得泄露 A/B 分数 |
| `judge_items` | 每个候选的 provenance，`--gate-human` 强制要求 |

三种门禁的证据强度不同：

- `--gate`：机械资格门，检查样本规模、层平衡、judge 元数据和基础指标。
- `--gate-items`：允许 AI 或非人工逐项裁决，同时明确标注证据等级。
- `--gate-human`：要求完整逐候选真人 provenance、弱相关/负例控制，且 NDCG A ≥ B。

自动标注、policy 物化、AI 预填、案例级签名都不能冒充独立人工证据。
逐候选盲审使用 `blind`；表格转 CSV 路径只用于机械目标标签。

## 复现 confirm5

```bash
./bin/scx-eval seed eval/confirm5.corpus.json \
  --mem-dir /tmp/scx-confirm5 --csv /tmp/scx-confirm5.csv --force
./bin/scx-eval baseline --csv /tmp/scx-confirm5.csv \
  --out /tmp/scx-confirm5-mechanical.json \
  --judge-by mechanical-target-confirm5-v1 \
  --judge-date 2026-10-23T00:00:00Z
./bin/run --eval /tmp/scx-confirm5-mechanical.json --gate --eval-verbose \
  --mem-dir /tmp/scx-confirm5
```

人工、AI 与 policy 门禁复用同一记忆库：

```bash
./bin/run --eval eval/confirm5-hao.labels.json --gate-human --eval-verbose \
  --mem-dir /tmp/scx-confirm5
./bin/run --eval eval/confirm5-ai.labels.json --gate-items --mem-dir /tmp/scx-confirm5
./bin/run --eval eval/confirm5-policy.labels.json --gate-human --mem-dir /tmp/scx-confirm5
```

预期：human `PASS / exit 0`；AI 只通过 `--gate-items`，`--gate-human` 必须
`FAIL / exit 2`；policy 同样 `FAIL / exit 2`。

## 配对统计

`stats` 读取确定性的 `--eval-verbose` 输出，计算配对 MRR/NDCG 差值、bootstrap
置信区间和 sign-flip permutation p 值。提供 `--mem-dir` 时由 case query 计算难度层；
也可传入 `case_index,tier` CSV。

```bash
./bin/run --eval eval/confirm5-hao.labels.json --gate-human --eval-verbose \
  --mem-dir /tmp/scx-confirm5 > /tmp/scx-confirm5-hao.txt
./bin/scx-eval stats \
  --baseline eval/confirm5-hao.labels.json \
  --verbose /tmp/scx-confirm5-hao.txt \
  --mem-dir /tmp/scx-confirm5 \
  --cluster-size 5 \
  --out /tmp/scx-confirm5-hao-stats.csv
```

`--cluster-size 5` 生成事故聚类敏感性行，不能替代 case 级预注册主分析。

## 盲审

```bash
./bin/scx-eval blind make \
  --baseline eval/confirm5-policy.labels.json --mem-dir /tmp/scx-confirm5 \
  --out /tmp/packet.tsv --manifest /tmp/manifest.json
./bin/scx-eval blind progress --packet /tmp/packet.tsv
./bin/scx-eval blind import \
  --baseline eval/confirm5-policy.labels.json --packet /tmp/packet.tsv \
  --out /tmp/human.json --judge <name> --date 2026-10-25T00:00:00Z
./bin/scx-eval blind check --baseline /tmp/human.json
./bin/scx-eval blind agree \
  --a /tmp/first-reviewer.json --b /tmp/second-reviewer.json
```

`blind make` 是确定性的。用 `confirm5-policy.labels.json` 和新播种记忆库重建
冻结包，应得到 SHA256
`b8eebf2fcb78d0fb745c9760b55dd34d832a9bb976b377f3dfb8c7cea264093f`。
`blind agree` 报告共同裁决项、原始一致率、Cohen's kappa 与分歧列表。
历史标签中的包路径字符串可能过时，应以 `judge.packet_sha256` 定位包。

## v2 与 real

```bash
./bin/scx-eval seed eval/v2.corpus.json \
  --mem-dir /tmp/scx-v2 --csv /tmp/scx-v2.csv --force
./bin/run --eval eval/v2.labels.json --gate --mem-dir /tmp/scx-v2

./bin/scx-eval seed eval/real.corpus.json \
  --mem-dir /tmp/scx-real --csv /tmp/scx-real.csv --force
./bin/run --eval eval/real.labels.json --gate-human --mem-dir /tmp/scx-real
```

v2 参与调参，且标签由 AI 预填后人工采纳，因此不是独立 holdout。`real` 只有案例级
签名；`--gate-human` 因缺少逐候选 provenance 而失败，这是预期证据边界，不是指标回退。

## 产物策略

- 保留语料、冻结标签、盲审包和预注册文档。
- 不提交记忆库、verbose 日志、gate 日志、rank-diff/strata/显著性 CSV 或草稿包。
- 引用结果时必须同时给出语料、标签集、二进制/配置、门禁模式、样本量和统计命令。
- CI 会在冻结语料上重建记忆库，并分别执行机械门、人工门、AI 逐项门、policy
  负向门和预注册优势断言；移动或重命名评测输入时，必须同步工作流路径。
- 旧 `eval-real/` 中间产物已删除，可从 git commit `5ada90d` 恢复。
