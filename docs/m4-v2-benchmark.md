# M4 v2 判别型基准（150 组）

## 1 结论先行

- v2 语料：30 起公开事故 × 5 条 = **150 case**（literal 60 / paraphrase 60 / semantic 30）。
- 机械门（`--gate`）在**机器草案裁决**上 **PASS**：三层 A ≥ B；但
  `discrimination A!=B layers=1/3`，paraphrase MRR 仅高 0.008，
  hard 层 ΔMRR=+0.010，paired bootstrap 95% CI [-0.0025, 0.0254]，permutation p=0.212。
- 在**旧 binary** 的 v2 语料上结论只能是“**未观察到 A 劣于 B**”，不是“A 优于 B”。
  **旧 binary 的 v2 质量优势未成立。**
- **后续 confirm5 已在另一套 30 起公开事故、150 条机械目标标签语料上完成
  预注册独立确认**：hard n=69，ΔMRR=+0.0669，95% CI [0.0238, 0.1176]，
  permutation p=0.0020，gate exit 0。完整证据见
  `docs/m4-confirm5-report.md`。该确认不能倒灌为 v2 的优势证据。
- **v2 AI 辅助基准已补**（`eval-real/v2-baseline-codex.json`）：312 行同桶跨事故边界
  按“具体故障机制”逐对裁决，`rel 2=432 / 1=192 / 0=197`，`--gate` PASS（exit 0）。
  MRR 与草案完全相同，只改变 NDCG；这**不是独立人工证据**，仅用于人工审核前的草案。
- **AI 预填的人工审核已完成**（2026-10-23）：fei/hao 各自审核 312/312 行，
  `review_state=human-reviewed`；但原始建议均为 `suggested_by=codex-ai`，两人
  全部采纳，`raw=1.000 / kappa=1.000`，0 处分歧。这是“人工采纳”，不是
  独立盲审；不能据此写“人工闭环已独立建立”。
- 最终人工采纳基准 `v2-baseline-arb.json`：`rel 2=432 / 1=312 / 0=77`，gate PASS。
  但 312 条弱边界全部为 `1`，没有产生弱相关负例；旧 binary 的质量优势未成立。
- **2026-10-24 优化后复跑（v2 开发集，非独立确认）**：sibling-coherence +
  query-field 低词面保护 + 确定性排序后，hard 层 ΔMRR **+0.0772**，
  CI **[0.0271, 0.1371]**，p=**0.0022**；paraphrase r@5 0.917 vs 0.867
  （MRR 0.566 vs 0.550），semantic MRR 0.696 vs 0.484，三层 NDCG A>B，
  旧门 `--gate-human` PASS；逐候选 `judge_items` 收紧后 FAIL（0/821）。
  312 条弱相关是 AI 预填后由 fei/hao 采纳，
  不是独立盲审；v2 还参与过调参，因此这是“人工采纳标签上的开发集证据”，
  不能当独立确认。confirm5 的机械 target 独立确认仍然成立，但其 policy
  控制集因 provenance 规则明确 FAIL，人工弱相关/负例闭环仍未建立。

### 1.1 优化后复跑明细（2026-10-24）

| 组 | n | A MRR | B MRR | Δ | 95% CI | p |
|---|---:|---:|---:|---:|---|---:|
| literal | 60 | 1.0000 | 1.0000 | 0 | [0, 0] | 1.000 |
| paraphrase | 60 | 0.5662 | 0.5496 | +0.0166 | [0.0034, 0.0333] | 0.0481 |
| semantic | 30 | 0.6959 | 0.4842 | +0.2117 | [0.1033, 0.3433] | 0.0039 |
| **tier:hard** | 55 | **0.4072** | **0.3300** | **+0.0772** | **[0.0271, 0.1371]** | **0.0022** |
| all | 150 | 0.7656 | 0.7167 | +0.0490 | [0.0223, 0.0798] | 0.0002 |

目标排名相对归档：18 条变化，16 条改善、2 条退化（rank 2→5、4→5，均仍在
top-5，r@5 不变）。复现：

```bash
python3 eval-real/significance.py \
  --baseline eval-real/v2-baseline-arb.json \
  --verbose eval-real/v2-human-gate-final-2026-10-24.txt \
  --strata eval-real/v2-strata.csv \
  --out eval-real/v2-human-significance.csv
```

## 2 语料与构建

| 项 | 值 |
|---|---|
| 事故数 | 30（公开复盘，URL 见 `eval-real/seed-v2.ps1` 的 `src`） |
| case 数 | 150（每起 5 条：literal×2 / paraphrase×2 / semantic×1） |
| 入库 | `eval-real/v2-mem`（每条 `text` 标注 `公开复盘, 来源: <URL>`） |
| 评估输入 | `eval-real/v2-cases.csv` → `eval-real/v2-baseline.json` |
| 构建脚本 | `eval-real/seed-v2.ps1` |

构建命令：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File eval-real\seed-v2.ps1 -Force
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\real-eval-build.ps1 -Csv eval-real\v2-cases.csv -Out eval-real\v2-baseline.json
```

### 2.1 query 修订

初版有 23 条 query 含 “the rollback / the recovery / the fix was” 等指代，脱离上下文
后与目标 case 零词面重叠，目标不可能进入候选集。修订只改 `pb/pd/sf`（评估 query），
不改入库文档，保留改写语义并补一个实体锚点。脚本：
`eval-real/patch-v2-queries.ps1`（逐条断言恰好命中一次）。

修订后 top-20 候选包 `targets_missing=0`；top-10 包同样 `targets_missing=0`。

## 3 难度分层与 near-miss

难度 = query 与目标 case **query 摘要**的 token Jaccard：
easy >0.5 / medium 0.2–0.5 / hard <0.2。中文按 CJK unigram+bigram 切分。
near-miss = A top-10 中与目标**同场景桶、不同来源 URL**的 case。

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File eval-real\v2-strata.ps1
```

| 层 | n | easy | medium | hard | 含 near-miss |
|---|---:|---:|---:|---:|---:|
| literal | 60 | 60 | 0 | 0 | 52 |
| paraphrase | 60 | 12 | 8 | 40 | 49 |
| semantic | 30 | 0 | 15 | 15 | 23 |

明细：`eval-real/v2-strata.csv`。

## 4 本轮发现并修复的召回缺陷

| 缺陷 | 触发 | 修复 | 证据 |
|---|---|---|---|
| URL 污染场景桶 | 来源 URL 的 `https` 含 `http`，150 条中 120 条被误归 network | `bucket::lower` 先剥离 URL；桶只看正文/实体 | `bucket_of` 新增 URL 回归断言 |
| `other` 被硬筛 | 目标桶未识别为具体家族时被桶过滤丢弃 | `bucket_allow_other=1` 默认放行 `other` | v2 top-10 漏召从 6 降到 0 |
| 领域词缺失 | `resolver/servfail/bgp/route/prefix/ebs/blob/volume/云盘` 未入桶表 | 扩充 disk/network 关键词 | 新增 5 条桶分类断言 |
| MMR 挤出精确目标 | λ=0.5 时同事故兄弟 case 被优先，paraphrase r@5 0.783→0.867（关闭后） | `mmr_enable` 默认 0；需要多样性时显式开启 | `eval-real/v2-ablate/*.txt` |
| PRF 并列序不可重放 | HashMap 迭代序影响扩展词截断 | idf 降序 + 词面升序（已在 `prf.rs`） | `expansion_sort_ties_break_by_term_asc` |

消融命令：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File eval-real\ablate-v2.ps1
```

关键消融（v2，机器草案标签）：

| 变体 | paraphrase r@5 / MRR | semantic r@10 / MRR |
|---|---|---|
| A 完整（MMR 开） | 0.783 / 0.530 | 0.933 / 0.461 |
| A−MMR | 0.867 / 0.546 | 1.000 / 0.468 |
| A−MMR+allow-other | **0.867 / 0.558** | **1.000 / 0.484** |
| B 基线 | 0.867 / 0.550 | 1.000 / 0.484 |

## 5 机械门输出（草案与 AI 辅助基准）

最终草案：`eval-real/v2-baseline-arb-draft.json`（`judge.by=draft-rule`，规则物化）。

```text
eval: cases=150 layers=literal,paraphrase,semantic multi-rel=124 single-rel=26 (NDCG strength)
layer literal: n=60 A r@5=1.000 mrr=1.000 ndcg@10=0.977 | B r@5=1.000 mrr=1.000 ndcg@10=0.951
layer paraphrase: n=60 A r@5=0.867 mrr=0.558 ndcg@10=0.900 | B r@5=0.867 mrr=0.550 ndcg@10=0.867
layer semantic: n=30 A r@10=1.000 mrr=0.484 ndcg@10=0.946 | B r@10=1.000 mrr=0.484 ndcg@10=0.919
l1: hits=69 total=93 rate=0.742
gate: rel distribution 2=432 1=312 0=77
gate: discrimination A!=B layers=1/3
gate: PASS
```

复现：

```powershell
.\rs\target\release\scx.exe --eval .\eval-real\v2-baseline-arb-draft.json --gate --mem-dir .\eval-real\v2-mem
```

### 5.1 AI 辅助基准（已补，非人工）

`eval-real/v2-baseline-codex.json` 的 `judge.by=codex-ai-2026-10-23`。312 行
“同桶跨事故”边界不再一律标 1，而是按 **具体故障机制** 裁决：
同机制（如 DB failover、BGP 路由、DNS 解析、CPU 饱和、存储配置、密钥轮换、
机房供电/制冷、操作员误删）标 `1`，只有粗场景桶相同但机制不同标 `0`。
共 109 个 targetIncident→candidateIncident 判定，逐行理由在
`eval-real/v2-weak-pair-decisions.csv`；复现脚本
`eval-real/fill-codex-weak-review.ps1`。

```text
eval: cases=150 layers=literal,paraphrase,semantic multi-rel=86 single-rel=64 (NDCG strength)
layer literal: n=60 A r@5=1.000 mrr=1.000 ndcg@10=0.983 | B r@5=1.000 mrr=1.000 ndcg@10=0.963
layer paraphrase: n=60 A r@5=0.867 mrr=0.558 ndcg@10=0.898 | B r@5=0.867 mrr=0.550 ndcg@10=0.867
layer semantic: n=30 A r@10=1.000 mrr=0.484 ndcg@10=0.952 | B r@10=1.000 mrr=0.484 ndcg@10=0.933
l1: hits=69 total=93 rate=0.742
gate: rel distribution 2=432 1=192 0=197
gate: discrimination A!=B layers=1/3
gate: PASS
```

复现：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File eval-real\fill-codex-weak-review.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File eval-real\merge-weak-review.ps1 `
  -Weak eval-real\v2-judge-review-weak-codex.tsv `
  -Out eval-real\v2-judge-compact-codex.tsv
Copy-Item eval-real\v2-baseline.json eval-real\v2-baseline-codex.json
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\judge-audit.ps1 `
  -Import eval-real\v2-judge-compact-codex.tsv `
  -Baseline eval-real\v2-baseline-codex.json -Judge codex-ai-2026-10-23
.\rs\target\release\scx.exe --eval .\eval-real\v2-baseline-codex.json --gate --mem-dir .\eval-real\v2-mem
```

机械门全文：`eval-real/v2-gate-codex.txt`。

### 5.2 预注册统计（MRR@10 为主）

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File eval-real\v2-significance.ps1
```

`MDE=0.05`（一行依据：v2 的 hard 层 n=55，按 80% 功效、α=0.05 的双侧配对检验，
可检测的 MRR 绝对差约 0.05；低于此值的差异不改变产品决策）。

| 组 | n | A MRR | B MRR | Δ | paired bootstrap 95% CI | permutation p |
|---|---:|---:|---:|---:|---|---:|
| literal | 60 | 1.000 | 1.000 | 0 | [0, 0] | 1.000 |
| paraphrase | 60 | 0.5579 | 0.5496 | +0.0083 | [-0.0028, 0.0222] | 0.288 |
| semantic | 30 | 0.4842 | 0.4842 | 0 | [-0.0050, 0.0050] | 1.000 |
| **tier:hard** | 55 | 0.3400 | 0.3300 | **+0.0100** | **[-0.0025, 0.0254]** | **0.212** |
| all | 150 | 0.7200 | 0.7167 | +0.0033 | [-0.0012, 0.0092] | 0.253 |

决策：hard 层 CI 下界 ≤ 0 且 Δ < MDE(0.05) → **优势不成立**；
只能写“未观察到 A 劣于 B”。草案明细：`eval-real/v2-significance.csv`；
AI 辅助基准明细：`eval-real/v2-significance-codex.csv`；
真人基准明细：`eval-real/v2-significance-human.csv`。

AI 辅助基准复算后统计量不变（`eval-real/v2-significance-codex.csv`）：
paraphrase Δ=+0.0083，CI [-0.0028, 0.0222]，p=0.288；semantic Δ=0；
hard Δ=+0.0100，CI [-0.0025, 0.0254]，p=0.212；全部 `advantage=False`。

### 5.3 标签敏感性

把同桶跨事故从“一律 1”收紧为“同机制才 1”后，只有 NDCG 变化，MRR 与
A/B 排序差异不变：

| 标签口径 | rel 2/1/0 | literal NDCG | paraphrase NDCG | semantic NDCG | A!=B layers |
|---|---|---:|---:|---:|---:|
| 宽松（草案） | 432 / 312 / 77 | 0.977 / 0.951 | 0.900 / 0.867 | 0.946 / 0.919 | 1/3 |
| 严格（AI 辅助） | 432 / 192 / 197 | 0.983 / 0.963 | 0.898 / 0.867 | 0.952 / 0.933 | 1/3 |

两种口径下 paraphrase MRR 均为 0.558 vs 0.550，hard ΔMRR 均为 +0.010；
结论对弱相关边界不敏感，仍不支持“A 优于 B”。

### 5.4 AI 预填的人工审核结果（2026-10-23）

fei/hao 两份审核文件均为 `312/312 human-reviewed`，`reviewed_by` 分别为
`fei`/`hao`。两人对 312 行同桶跨事故全部判 `1`；仲裁无分歧：

```text
judge-agreement: common=821 raw=1.000 kappa=1.000
  matrix 2,2=432 1,1=312 0,0=77 off-diag=0
disagreements: 0
arbitration log: header only
```

最终人工采纳基准 `eval-real/v2-baseline-arb.json`（`judge.by=fei+hao`）门禁：

```text
layer literal: n=60 A r@5=1.000 mrr=1.000 ndcg@10=0.977 | B r@5=1.000 mrr=1.000 ndcg@10=0.951
layer paraphrase: n=60 A r@5=0.867 mrr=0.558 ndcg@10=0.900 | B r@5=0.867 mrr=0.550 ndcg@10=0.867
layer semantic: n=30 A r@10=1.000 mrr=0.484 ndcg@10=0.946 | B r@10=1.000 mrr=0.484 ndcg@10=0.919
gate: rel distribution 2=432 1=312 0=77
gate: discrimination A!=B layers=1/3
gate: PASS
```

人工统计与草案一致（`eval-real/v2-significance-human.csv`）：
paraphrase ΔMRR=+0.0083，CI [-0.0028, 0.0222]，p=0.288；
hard Δ=+0.0100，CI [-0.0025, 0.0254]，p=0.212；全部 `advantage=False`。
因此可以写“312 条 AI 预填建议被两名审核人全部采纳、机械门 PASS”，
但**不能写“独立人工盲审闭环”**，且质量优势仍不成立；312 条弱边界全部为 1，
说明这轮标签没有提供弱相关负例区分度。

## 6 证据强度与限制

1. **AI 草案不是独立人工证据**：`v2-baseline-codex.json` 是 AI 按机制规则逐对裁决，
   可复现、可审计，但没有第二名真人盲测；`v2-baseline-draftcompact.json` 的 rel 由
   “target/同来源=2，其余=0”机器预填；`v2-baseline-hao-draft.json` 的 rel=1 由
   “同桶跨事故=1”规则物化。这些标签都是规则/机制物化，不是逐条人工盲测。
2. **v2 是诊断集，不是 held-out**：本轮修复（URL/桶/MMR/query 修订）是在观察
   v2 失败后做的，不能把 v2 结果当作独立泛化证据。
3. **优势未成立**：paraphrase MRR 0.558 vs 0.550（Δ=0.008，CI 重叠）；
   literal/semantic 主指标持平；`discrimination 1/3`。
4. **v1 仍 PASS**：40 组真实语料在默认行为变更后仍 exit 0，但
   `discrimination 0/3`（MMR 关闭后 paraphrase MRR 由 0.756 降到 0.744，与 B 持平）。
   详见 `docs/m4-real-corpus-report.md`。
5. **v2 人工审核已完成，但不是独立盲审**：fei/hao 均完成 312/312 审核，
   一致率和 kappa 都是 1.000；但两人都全盘采纳了 AI 预填，且同桶跨事故全为 1，
   弱相关负例为 0 条。v2 质量优势不成立。
   机械门 PASS 只证明 A 不劣于 B，不能外推为 v2 的召回质量优势。
   confirm5 的独立确认是另一套语料上的结论，见 `docs/m4-confirm5-report.md`。

## 7 AI 预填人工审核记录与复现命令（已完成）

压缩包 `eval-real/v2-judge-compact.tsv`：822 行 / 150 case（约 5.5 行/案），
每条含 target + 同来源兄弟 + 同桶 near-miss + 异桶对照，行内已确定性打乱。
2026-10-23 的最终结果见 §5.4；以下是当时的执行路径，保留用于复现/审计。

```powershell
# 0) 生成两份 AI 预填建议（review_state=ai-suggested-pending-human）
powershell -NoProfile -ExecutionPolicy Bypass -File eval-real\prefill-human-reviews.ps1

# 1) fei / hao 各自审核 312 行；-Blind 隐藏预填建议，强制独立输入 1/0
powershell -NoProfile -ExecutionPolicy Bypass -File eval-real\review-weak-human.ps1 `
  -In eval-real\v2-judge-review-weak-fei.tsv -Reviewer fei -ReviewAll -Blind
powershell -NoProfile -ExecutionPolicy Bypass -File eval-real\review-weak-human.ps1 `
  -In eval-real\v2-judge-review-weak-hao.tsv -Reviewer hao -ReviewAll -Blind

# 2) 两份都完成后一键校验、合并、回填、一致性、仲裁、最终 gate
powershell -NoProfile -ExecutionPolicy Bypass -File eval-real\close-v2-human.ps1

# 以下是分步等价命令（便于排查）
# 1) 只需人工判 312 行“同桶跨事故”边界（1/0）；target/同来源=2、异桶=0 由规则给出
#    eval-real\v2-judge-review-weak-fei.tsv
#    eval-real\v2-judge-review-weak-hao.tsv

# 2) 把人工边界合并回 compact 草案
powershell -NoProfile -ExecutionPolicy Bypass -File eval-real\merge-weak-review.ps1 `
  -Weak eval-real\v2-judge-review-weak-fei.tsv `
  -Out eval-real\v2-judge-compact-fei-filled.tsv

# 3) 回填
Copy-Item eval-real\v2-baseline.json eval-real\v2-baseline-fei.json
Copy-Item eval-real\v2-baseline.json eval-real\v2-baseline-hao.json
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\judge-audit.ps1 `
  -Import eval-real\v2-judge-compact-fei-filled.tsv `
  -Baseline eval-real\v2-baseline-fei.json -Judge fei

# 4) 一致性 + 仲裁 + 门禁
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\judge-agreement.ps1 `
  -A eval-real\v2-baseline-fei.json -B eval-real\v2-baseline-hao.json
powershell -NoProfile -ExecutionPolicy Bypass -File eval-real\arbitrate.ps1 `
  -Fei eval-real\v2-baseline-fei.json -Hao eval-real\v2-baseline-hao.json `
  -Out eval-real\v2-baseline-arb.json -Log eval-real\v2-arbitration-log.tsv
.\rs\target\release\scx.exe --eval .\eval-real\v2-baseline-arb.json --gate --mem-dir .\eval-real\v2-mem
```

`v2-judge-compact-draft.tsv` 只是预填草案，供审改，不是独立裁决。
