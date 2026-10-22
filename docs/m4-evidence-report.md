# M4 证据评估报告（自动回归集 + 口径修正）

日期：2026-10-23（初版 2026-10-22）
库：scx 0.4.1 + P4.4+ 优化⑤⑥⑦⑧⑨ + real-eval 机械门
状态：**机械门 PASS（40 组/三层达标/judge=fei，exit 0）；M4 人工质量证据未闭环——全 rel=2、三层 A=B、语料为合成样例且单裁决人。§12.1 合成诊断集 A 在语义/多相关层领先，但不是独立人工证据。弱相关/负例与双人复核工具已就绪（§13），待真实历史事故语料与第二裁决人。**

## 1. 结论先行

`--annotate` 能稳定产出 30–50 组带证据链的回归标注，`--eval` 能重放 A/B 表；
这证明**评估工具链可用**，不证明 **A 的召回质量优于 B**。

原因：自动标注的 paraphrase/semantic 变体先经 **A 管线**自检（目标必须进 A 的
top-k）才放行，B 不参与放行。A 失败的变体被丢弃，因此评测集是“A 已成功的子集”，
存在选择偏差。没有独立人工标签或盲测，不能把 A/B 差异归因于统计管线更好。

## 2. 可复现语料与命令

原报告引用的 24-case 语料未入库，无法复现其精确数字。现提供可复现脚本：

```powershell
powershell -ExecutionPolicy Bypass -File scripts/m4-eval.ps1
```

脚本定义 24 个真实感 case（16 英文运维 + 8 中文/混排），执行：

```text
store × 24 → --annotate --seed 0 --annotate-max 0 → --eval
```

本次输出（Windows，release）：

```text
annotate: cases=36 rel2=30 rel1=6
  layer: literal=24
  layer: paraphrase=9
  layer: semantic=3
  dropped: paraphrase=1 semantic=0 (self-check filtered)
  skipped: paraphrase_no_synonym=14 semantic_no_root=21
```

## 3. A/B 对拍（本脚本可复现数字）

| 层 | n | A r@5 | B r@5 | A mrr | B mrr | A ndcg@10 | B ndcg@10 |
|---|---|---|---|---|---|---|---|
| literal | 24 | 1.000 | 1.000 | 0.958 | **1.000** | 0.969 | **1.000** |
| paraphrase | 9 | **1.000** | 0.889 | 0.778 | 0.778 | 0.836 | 0.807 |
| semantic | 3 | r@10: 1.000 | 1.000 | 0.833 | 0.833 | 0.877 | 0.877 |
| l1 | 33 | 29/33 = 0.879 | — | — | — | — | — |

**口径**：

- 只有 paraphrase 的 r@5 出现 A>B（1.000 vs 0.889）；MRR 持平；n=9，无置信区间，
  不能据此宣称统计显著。
- literal 的 MRR/NDCG **A 反而低于 B**（0.958/0.969 vs 1.000/1.000），
  因此“A ≥ B 全面成立”不成立；最多只能说“三层 r@5 不劣于 B”。
- semantic 只有 3 组，且 r@10 是事后放宽的指标；n=3 不能支撑质量结论。

## 4. 选择偏差漏斗

24 个 case 的理论上限是 72 条（literal+paraphrase+semantic），实际：

| 层 | 产出 | 未产出原因 |
|---|---|---|
| literal | 24 | 全部保留（同 query，A/B 都容易命中） |
| paraphrase | 9 | 14 个 query 无 SYN 可替换词；1 个自检未进 A top-5 |
| semantic | 3 | 21 个 case 无根因行/error_code 家族词；0 个自检失败 |

literal 占 36 组的 66.7%，semantic 仅 8.3%。计划 §8 要求“各层 ≥10 组”，
当前 24/9/3 不满足；这不是“30–50 组窗口达成”就能替代的分层门槛。

## 5. 方法学限制（必须随结论引用）

1. **自检即选择**：`self_check` 调用 A 管线，只保留 A 能召回的变体；A 失败的
   困难样本被丢弃，B 不参与。该设计适合做回归 smoke，不适合做 A/B 质量证明。
2. **指标事后切换**：semantic 从 r@5 改为 r@10，是在看到 r@5 结果后放宽；
   应预先固定指标，或同时报告 r@5/r@10 并明确主指标。
3. **样本量与显著性**：paraphrase n=9、semantic n=3；没有配对检验/置信区间，
   literal 的 MRR 差异（0.042）不能据此判定系统性优劣。
4. **语料来源**：24 个 case 是真实感合成语料，不是真实历史事故；没有人工
   relevance 标注，不能代表线上分布。
5. **A/B 定义**：B 是朴素词面基线，A 加了字段/PRF/衰减/MMR/反馈；即使 A 更好，
   也只能说明整条管线的组合效果，不能拆分归因到某个增强。

## 6. 哪些结论仍然成立

- `--annotate` 确定性（同 seed 逐字可重放）、带证据链、输出可被 `--eval` 解析。
- 自检丢弃数与未改写/无根因数现已输出，选择漏斗可审计。
- `--eval` 的 Recall@5/MRR/NDCG 计算与分层聚合可重放。
- M4 的**工具就绪**成立；`--eval --gate` 的**机械证据门**已于 2026-10-23 PASS
  （real-eval 40 组，literal 14 / paraphrase 13 / semantic 13，非 literal 26 条
  `judge.by=fei`，exit 0）。
- M4 的**质量优势证据**仍未成立：40/40 标签为 `2`，三层 A=B（判别层 0/3），
  语料是 `seed-40.ps1` 的真实感合成样例且只有单裁决人。只能写“A 不劣于 B”，
  不能写“A 优于 B”“统计显著”或“独立质量证据完全通过”。

## 7. 通过 M4 的最小闭环

1. 冻结一份独立语料（真实历史 query→case），至少 30 组、每层 ≥10 组。
2. 标注者不看 A/B 结果，先独立标 relevance；保留双人复核/仲裁记录。
3. 预先声明主指标（建议 r@5 + MRR）与置信区间方法，禁止看到结果后换指标。
4. 报告“全部尝试变体”与“自检放行变体”两套指标，量化选择偏差。
5. 在独立语料上重跑 A/B；若 A 仍不劣于 B，才可写“质量证据通过”。

## 8. 复现命令

```powershell
cd rs
cargo build --release --locked
powershell -ExecutionPolicy Bypass -File ..\scripts\m4-eval.ps1
```

## 9. 优化⑧处置记录（2026-10-22，外部评审第二轮）

**处置 1：literal MRR/NDCG < B 的退化例修复（P0）**

- 逐 case 定位（`--eval-verbose`）：退化例 = case `disk io read timeout`，
  A rank3（1) sim=0.13 score=6.00 [miss] 2) sim=0.13 score=6.00 [miss] 3) 目标 sim=0.95 score=4.57 [hit]）vs B rank1。
- 根因：PRF 扩展轮按 q2 打分，扩展词把无关 case 得分抬到 6.00；合并后按 score 混排（q2 与首轮口径不可比），真目标被挤出。
- 修复（recall.rs）：**PRF 严格补漏**——合并后不重排：首轮顺序权威，扩展新增（去重）按 q2 顺序追加尾部再截断。
  扩展词只补漏、永不改变首轮排序。
- 修复后重跑（36 组，自动口径）：literal A r@5=1.000 **mrr=1.000** **ndcg@10=1.000** = B 1.000；
  paraphrase A r@5=1.000（B 0.889）mrr=0.833（B 0.759）ndcg@10=0.877（B 0.792）——A 反超；
  semantic r@10 双 1.000；l1 0.909。**自动口径下 A 不再劣于 B 任何一层**。
  注意：本报告 §3 的 0.958/0.969 为修复前数字，已被本节替代。

**处置 2：选择偏差的工具化消除——`--annotate-raw` 盲测模式（新增）**

- 问题（评审 §5.1）：自检用 A 管线 = 用 A 自证；放行集是"A 已成功的子集"。
- 落地：`--annotate-raw`（与 `--annotate` 互斥）**不过自检**输出全变体集——
  含自检本会丢弃的变体（真阴性/假阳性对照），每条带盲测摘要
  `note: target=<hash16> hint=[原文首行] fix=[修复结论]`——盲测者只看 query 与目标摘要判 rel，
  不看任何 A/B 分数。裁决后 `--eval` 即独立证据，A/B 平等面对全集。
- 盲测 SOP：`--annotate-raw --seed 0 --annotate-max 0 --annotate-out baseline-raw.json` →
  人工逐条裁决 rel（2/1/0，或删除）→ `--eval baseline-raw.json --eval-verbose` →
  报告留档（裁决 JSON + 输出 + 裁决人/日期/seed）。任何一层 A<B 即失败，回到处置 1 类修复。
- **P0-1 盲测裁决链（2026-10-22，d2766dc）**：`--annotate-raw` 的 paraphrase/semantic 条目
  rel 不再预填（序列化 `"judge": null`）；`--eval` 遇未裁决条目直接报错（防"自动标注自证"）；
  新增 **`--judge [--judge-by NAME]` 交互裁决**——逐条打印 `[i/n] layer=.. query=".."` +
  盲测摘要（**零 A/B 分数**），stdin `2/1/0/s`（s=跳过，EOF 中止不写回，跳过条目保持
  judge:null 且 `--eval` 仍拒绝）；报告含裁决人/日期/seed 与 **bootstrap 95% CI**（固定
  seed `CI_SEED`、B=1000、r@5+MRR 预声明主指标），并输出 multi-rel/single-rel 计数
  （NDCG 判别强度可见，单相关条目 NDCG 接近二值、容易高估——评估必读口径）。

**状态（不变）**：工具链就绪（`--annotate` 回归集 + `--annotate-raw` 盲测集 + `--judge` 裁决链）；
**独立人工/盲测质量证据未闭环前，M4 不宣称通过**。盲测裁决集 ≥30 组、每层 ≥10 组、
预声明主指标（r@5 + MRR）与 bootstrap CI 后，方为独立证据门。

**门禁（P0-1..P1-5 后）**：104 库 + 8 CLI + 5 集成 + golden 29 例全绿，fmt/clippy 0，release ✓。
实施记录与整改单见 `docs/rag-implementation-log.md` §24。

## 10. `--gate` 机械门（2026-10-22，优化⑭）

`--eval --gate` 把 M4 的证据门变成可执行校验，任一条件失败即打印 `gate: FAIL` 并 exit 2：

| 条件 | 规则 |
|---|---|
| 总样本 | `total >= 30`（计划目标 30–50；超过 50 不惩罚） |
| 分层 | `literal` / `paraphrase` / `semantic` 各 `n >= 10` |
| 裁决元数据 | 非 literal 条目必须带 `judge.by` + `judge.date`；自动标注集（无 judge 对象）不得过门 |
| 主指标 | 每层 A 的预声明指标（literal/paraphrase r@5；semantic r@10）与 MRR 均 `>= B` |
| 未知层 | 出现三层之外的 layer 直接 FAIL，防止用其他层凑总数 |

复跑示例：

```powershell
cd rs
cargo build --release --locked
.\target\release\scx.exe --eval $env:TEMP\scx-demo-raw.json --gate --eval-verbose --mem-dir $env:TEMP\scx-demo-mem
```

实测：demo 16 case → 23 组（literal 16 / paraphrase 5 / semantic 2），
`--gate` 报 `total=23 FAIL`、`paraphrase n=5 FAIL`、`semantic n=2 FAIL`，exit 2；
自动标注集（23 组、无 judge 元数据）另报 `judge metadata ... FAIL`。
这证明机械门不会把 demo/自动集误判为 M4 证据。**在 §10 的时点，M4 状态仍为未通过**；
后续 2026-10-23 real-eval 机械门已 PASS（见 §11），但质量优势仍未成立。

## 11. real-eval 40 组机械门实测（2026-10-23）

### 11.1 输入与命令

```powershell
cd C:\Users\fei\.codex\skills\scx
powershell -ExecutionPolicy Bypass -File .\scripts\real-eval-build.ps1 `
  -Csv .\eval-real\cases.csv -Out .\eval-real\baseline.json
.\rs\target\release\scx.exe --judge .\eval-real\baseline.json --judge-by fei
.\rs\target\release\scx.exe --eval .\eval-real\baseline.json --gate `
  --eval-verbose --mem-dir .\eval-real\mem
```

本次真实执行结果：

```text
eval: cases=40 layers=literal,paraphrase,semantic multi-rel=0 single-rel=40
layer literal: n=14 A r@5=1.000 mrr=1.000 | B r@5=1.000 mrr=1.000
layer paraphrase: n=13 A r@5=1.000 mrr=1.000 | B r@5=1.000 mrr=1.000
layer semantic: n=13 A r@10=1.000 mrr=1.000 | B r@10=1.000 mrr=1.000
l1: hits=19 total=40 rate=0.475
gate: total=40 need>=30 (target 30-50) PASS
gate: layer literal n=14 need>=10 PASS
gate: layer paraphrase n=13 need>=10 PASS
gate: layer semantic n=13 need>=10 PASS
gate: judge metadata nonliteral=26 (need real by+date) PASS
gate: rel distribution 2=40 1=0 0=0
gate: warning all labels are 2 (no weak/negative controls)
gate: discrimination A!=B layers=0/3
gate: warning A=B on all layers (no measurable advantage)
gate: PASS
```

exit 0。裁决文件为 `eval-real/baseline.json`；非 literal 26 条均有
`judge.by=fei` 与 UTC `judge.date`，literal 14 条为原文对照并预填 `rel=2`。

同次代码门禁：`cargo test --locked --quiet` 通过 114 库 + 8 CLI + 5 集成
（golden 1 passed / 1 ignored）；`cargo clippy --all-targets -- -D warnings`
零告警。

### 11.2 证据强度边界

- 40 条标签全部为 `2`，没有 `1`（弱相关）或 `0`（无关）对照；这只能证明
  “目标能被找回”，不能证明系统能区分相关与干扰项。
- 三层 A/B 主指标与 MRR 全部相等（`discrimination A!=B layers=0/3`），
  没有可测出的 A 相对 B 优势。
- `eval-real/seed-40.ps1` 是脚本内写死的真实感样例，未附真实历史事故来源、
  脱敏记录或线上分布抽样说明；它不是已证实的独立历史语料。
- 只有一名裁决人 `fei`，没有第二人复核/仲裁记录。

### 11.3 结论

**机械门通过；M4 的质量优势未通过。** 当前可以引用“40 组、三层各 ≥10、
人工裁决元数据完整、`--gate` PASS、A 不劣于 B”；不得引用“A 优于 B”、
“显著提升”或“M4 独立质量证据完全闭环”。要闭环后者，仍需真实来源语料、
弱相关/负例、双人复核与可区分的 A/B 结果。

2026-10-23 后续合成诊断（§12）在截断 paraphrase 与 10 条多相关 query 上
测出 A 领先，但标签由已知目标/主题规则生成，不能替代上述独立人工证据。

## 12. 召回桶、零支持度与同义词扩展（2026-10-23）

用 40 组原 query 做前 2/3 词和去数字截断，生成 104 条诊断 query（不是人工
证据，只用于找退化），暴露了三个实现缺陷：

1. **桶判定的大小写 bug**：`bucket::lower` 误用了不做小写的 `norm_into`，
   `Jdbc.java`/`DATABASE` 匹配不到小写关键词表；`database query timeout`
   被 `timeout` 抢先归到 `network`，候选桶 `db` 时漏掉目标。
2. **网络桶优先级过宽**：`network` 含通用词 `timeout`，排在 `disk` 前面，
   使 `disk io read timeout`/`storage read timeout` 被归到 network；query
   `storage read` 的候选桶是 disk，目标被过滤。
3. **零支持度候选被当作命中**：没有 query 词命中的 case 仍以 `score=0`
   按库顺序返回；`replication delay`（与目标 `replica lag` 无词面交集）
   在 A rank 9 / B rank 5 被“碰巧命中”，指标虚高。

修复：

- `bucket::lower` 改为 `push_analysis_lower`（真正 Unicode 小写）；
- 家族优先级改为 `db / disk / build / runtime / network`，通用 `timeout`
  不再压过具体存储/构建/运行时信号；
- 只有通用 `timeout`/`connection`、没有 socket/dns/tcp 等专用词时返回空候选桶
  （全量扫），避免把跨 db/disk 的通用症状硬筛到 network；
- A 管线与 eval B 基线都过滤零词面/字段/实体支持度候选；quality/decay 只能
  给已有证据的 case 加权，不能单独把 case 带进 top-k；
- A 管线只使用高精度同义词扩展（`replication↔replica`、`delay↔lag`、
  `memory↔heap`、`failure↔failed` 等），不扩展 `db↔database` 这类高频宽词；
  B 基线保持纯词面；
- 新增大小写、disk-vs-network、通用网络词、零支持度、同义词回归测试；golden
  `memory_eval_verbose` 重冻（仅删除零分噪声候选，指标行不变）。

104 条诊断 query 复跑（同义词扩展后）：

```text
literal:   n=35 A r@5=1.000 mrr=1.000 | B r@5=1.000 mrr=1.000
paraphrase:n=39 A r@5=1.000 mrr=1.000 | B r@5=0.949 mrr=0.949
semantic:  n=30 A r@10=0.933 mrr=0.933 | B r@10=0.933 mrr=0.933
```

`replication delay` 的 A 现在通过 `replication→replica`、`delay→lag` 在 rank1
命中目标，B 仍是 `(no hits)`；`Java 进程内存耗尽` 的 A/B 都无词面命中。
零分兜底已移除，不再出现“按库顺序碰巧命中”。

另用 10 条 **合成多相关** 诊断（一个 query 对应 2–3 个 topic 相关 case，
rel=2/1；不是人工证据）测得：

```text
semantic: n=10 A r@10=0.867 mrr=0.950 ndcg=0.895
          B r@10=0.700 mrr=0.950 ndcg=0.780
```

A 的召回提升来自同义词扩展补到 pytest 等零字面重叠目标；但 A 仍会在
`test failure` 中插入低相关候选，不能把该诊断当成质量优势的最终证据。
复现：`pwsh -NoProfile -File scripts/recall-probe.ps1`。

结论边界：这些数字支持“同义词扩展 + 零支持度过滤 + 桶修复”是有效机制，
不构成 M4 独立人工证据；M4 的 rel=2 饱和、单一裁决人、真实来源缺失仍未解决。

代码门禁：`cargo test --locked --quiet` 114 库 + 8 CLI + 5 集成全绿，
golden 1 passed / 1 ignored；`cargo clippy --all-targets -- -D warnings`
零告警。

### 12.1 中英领域召回桥（同轮实验，2026-10-23）

104 条截断诊断里，语义层剩余漏召主要是**中英词面完全不相交**：中文查询
`进程内存耗尽` 与英文 case `out of memory heap` 没有共同 token。CJK 词表
已带 bigram/trigram，因此按 2 字词加中英领域对即可桥接，不需要 embedding：

- 名词对：`内存↔memory`、`存储↔storage`、`队列↔queue`、
  `连接↔connection`、`连接池↔pool`、`线程↔thread`、`崩溃↔crash`、
  `配置↔config`、`部署↔deploy`、`权限↔permission`；
- 宽泛→具体单向对：`memory→gc`、`disk→filesystem`。反向扩展实测有害：
  双向 `gc↔memory` 会让查询 `垃圾回收 gc…` 的 OOM case 反超 GC 目标，
  40 组门禁 semantic MRR 从 1.000 掉到 0.962（gate FAIL），故只保留单向；
- 不加入 `磁盘↔disk`：golden paraphrase `磁盘连接超时 conn_id 池` 的
  disk case 会以扩展词反超原词 `conn_id` 目标（MRR 1.000→0.500）。试过给
  扩展词统一半权可修复，但 L1 无收益且多一层机制，最终回退为删除该词对；
- 不加入 `证书↔certificate`：实测把 `ssl certificate expired` 抬到
  `tls handshake cert expired` 之前，semantic MRR 掉到 0.962；该中文查询
  已有 `tls` 字面桥，无需扩；
- 仍不收 `错误↔error`、`超时↔timeout`、`db↔database` 这类泛词。

同一 104 条诊断前后对比（A 管线；B 基线不变）：

```text
semantic  前: r@10=0.933 mrr=0.933 ndcg=0.933
          后: r@10=0.967 mrr=0.967 ndcg=0.967
literal   前/后: 1.000 / 1.000（无回归）
paraphrase 前/后: 1.000 / 1.000（无回归）
```

唯一剩余漏召回是截断产物 `return value`（目标 `rust mismatched types`
在“return/value”上无语义证据），不再为它加特例。10 条合成多相关诊断
同时改善：A r@10 0.867→0.900、NDCG 0.895→0.909；`memory` 现在能同时
召回 OOM、oom-killer 与 GC 三个目标。40 组门禁仍为三层全 1.000、exit 0。

结论边界不变：这是机制回归证据，不是独立人工质量证据；M4 的
“全 rel=2 / 单裁决人 / 合成语料”仍未闭环。

## 13. 弱相关/负例与双人复核工具（2026-10-23，就绪待真人）

为补上 M4 剩余两项缺口，新增两个脚本（策略层，未改核心）：

- `scripts/judge-audit.ps1`：对 baseline 每条 query 跑检索器 top-k，生成
  **盲测候选包**（只含 query + 候选摘要 + fix，不含 target id、不含 A/B 分数），
  人工逐候选打 `rel=2/1/0` 后 `-Import` 回填 rel 字典与 judge.by/date。
  这是产出弱相关/负例的唯一诚实途径。
- `scripts/judge-agreement.ps1`：比对两名裁决人回填后的 baseline，
  输出共同候选数、原始一致率、Cohen's kappa（0/1/2 三类）与逐条分歧。

机械自检（E1，非人工证据）：

```text
judge-audit 生成: 116 行（40 组 × top-10，40/40 target 均在候选池内）
judge-audit 回填: rel2=40 rel1=0 rel0=64（smoke-test 填充，验证机制）
judge-agreement: 两份相同 baseline → common=40 raw=1.000 kappa=1.000
```

**未闭环的两项输入只能由人提供，AI 不代劳：**

1. **真实历史事故语料**：仓库内只有 `eval-real/seed-40.ps1` 写死的合成夹具，
   没有真实事故。需要把真实事故日志逐条 `scx --query ... --store 1 --fix ...` 入库，
   再按 `docs/eval-baseline-template.md` 生成 CSV/baseline。AI 不会伪造历史事故。
2. **第二裁决人**：`judge.by` 必须是真人姓名；`judge-audit.ps1` 的包可以
   交给第二人独立填写，再跑 `judge-agreement.ps1`。AI 填的 rel 不算“独立人工证据”。

在两项输入补齐前，M4 结论只能是“机械门 PASS；质量优势未成立”。
