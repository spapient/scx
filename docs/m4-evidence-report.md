# M4 证据评估报告（自动回归集 + 口径修正）

日期：2026-10-22
库：scx 0.4.1 + P4.4+ 优化⑤⑥⑦⑧⑨
状态：**自动回归集可复现；M4 独立质量证据未通过**

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
- M4 的**工具就绪**成立；M4 的**质量证据门**未成立。

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

**状态（不变）**：工具链就绪（`--annotate` 回归集 + `--annotate-raw` 盲测集）；
**独立人工/盲测质量证据未闭环前，M4 不宣称通过**。盲测裁决集 ≥30 组、每层 ≥10 组、
预声明主指标（r@5 + MRR）后，方为独立证据门。

**门禁（优化⑧ 后）**：97 库 + 8 CLI + 5 集成 + golden 29 例全绿，fmt/clippy 0，release ✓。
