# M4 真实历史事故语料报告

## 1 结论

- 真实语料：11 起公开事故复盘派生 40 条 case，literal 14 / paraphrase 13 / semantic 13。
- 控制门 `--eval --gate-human`：旧门（案例级签名）**PASS**；2026-10-24 收紧为
  逐候选 `judge_items` 后 **FAIL（0/344）**。弱相关标签来自人工规则物化
  （legacy JSON 未声明 `judge.method`），**不是逐条人工盲审闭环**。
- 控制标签：`rel 2=147 / 1=151 / 0=46`，`judge.by=fei+hao`；三层均含 1/0 控制。
- A/B 主指标：三层 **A ≥ B**；literal/semantic/paraphrase 的 Recall 与 MRR 全部持平
  （paraphrase MRR 0.744 vs 0.744）。NDCG 三层 A>B。
- NDCG@10：literal **0.958 vs 0.920**、paraphrase **0.941 vs 0.862**、
  semantic **0.905 vs 0.885**；区间有重叠，只说明弱相关排序不劣且略高，
  不等于主指标优势或统计显著。
- `discrimination A!=B layers=0/3`：MRR/Recall 无可测差异；只能表述为
  “未观察到 A 劣于 B”，**不能**表述为“A 优于 B”。

## 2 语料与来源

- 来源清单：`docs/real-incident-sources.md`（11 起事故的公开 URL + 根因摘要）。
- 入库文本：`eval-real/real-mem`；每条 `text` 第二行标注 `公开复盘, 来源: <URL>`。
- 红线：只改写/引用公开复盘，不编造事故与原始日志。
- 构建脚本：`eval-real/seed-real.ps1`（`-Force` 仅清理工作区内 `real-mem`）。
- 评估输入：`eval-real/real-cases.csv` → `eval-real/real-baseline.json`。

## 3 复核命令（E1）

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File eval-real\seed-real.ps1 -MemDir eval-real\real-mem -Csv eval-real\real-cases.csv -Force
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\real-eval-build.ps1 -Csv eval-real\real-cases.csv -Out eval-real\real-baseline.json
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\judge-audit.ps1 -Baseline eval-real\real-baseline.json -MemDir eval-real\real-mem -Out eval-real\judge-packet-real.tsv
.\rs\target\release\scx.exe --eval .\eval-real\real-baseline-arb.json --gate-human --mem-dir .\eval-real\real-mem
```

## 4 裁决与统计

裁决规则（由裁决人给出，脚本确定性物化；`apply-adjudication.ps1`）：

- 目标 case → rel=2。
- 同一公开事故（同来源 URL）的其他 case → rel=2。
- fei：其余候选 → 0。
- hao：跨事故但同场景桶（根因域相近、不可复用）→ 1；其余 → 0。

结果：

| 项 | 值 |
|---|---|
| 候选行 | 344 |
| fei rel2/1/0 | 147 / 0 / 197 |
| hao rel2/1/0 | 147 / 151 / 46 |
| fei vs hao | common=344 raw=0.561 kappa=0.407 |
| 分歧 | 151（全部 fei=0 / hao=1） |

仲裁：沿用上一轮决议口径——分歧项采用 hao 的独立判定（一致项保留），逐条写入
`eval-real/arbitration-log.tsv`；最终 `real-baseline-arb.json`，`judge.by=fei+hao`，
rel2/1/0 = **147 / 151 / 46**。

`judge-packet-real-fei.tsv` 的 40/107/197 是**改判前草案**（同源=1）；改为同源=2 后为 147/0/197。

## 5 Gate 输出（E1）

```text
eval: cases=40 layers=literal,paraphrase,semantic multi-rel=34 single-rel=6 (NDCG strength)
layer literal: n=14 A r@5=1.000 mrr=1.000 ndcg@10=0.958 ci95=[0.930,0.981] | B r@5=1.000 mrr=1.000 ndcg@10=0.920 ci95=[0.851,0.973]
layer paraphrase: n=13 A r@5=1.000 mrr=0.744 ndcg@10=0.941 ci95=[0.903,0.973] | B r@5=1.000 mrr=0.744 ndcg@10=0.862 ci95=[0.802,0.913]
layer semantic: n=13 A r@10=1.000 mrr=0.962 ndcg@10=0.905 ci95=[0.871,0.937] | B r@10=1.000 mrr=0.962 ndcg@10=0.885 ci95=[0.848,0.924]
gate: mode=human
gate: human judge metadata all=40 judges=fei+hao PASS
gate: human controls weak=151>=10 negative=46>=10 layers=3>=2 PASS
gate: layer literal ndcg@10 A=0.958 vs B=0.920 PASS
gate: layer paraphrase ndcg@10 A=0.941 vs B=0.862 PASS
gate: layer semantic ndcg@10 A=0.905 vs B=0.885 PASS
gate: rel distribution 2=147 1=151 0=46
gate: discrimination A!=B layers=0/3
gate: warning A=B on all layers (no measurable advantage)
gate: PASS
```

exit 0。裁决文件为 `eval-real/real-baseline-arb.json`；40 条 case 均带
`judge.by=fei+hao` 与 UTC `judge.date`。
原始输出：`eval-real/real-human-gate-2026-10-24.txt`；
2026-10-24 优化后输出：`eval-real/real-human-gate-final-2026-10-24.txt`；
单裁决人对照：`eval-real/baseline-final-human-gate-2026-10-24.txt`。

## 6 诚实性限制（M2 / M5）

1. **hao 的 rel=1 是人工裁决规则物化，不是逐条手工标注**：151 条“根因域相近”由同一场景桶
   （db/network/disk/build/runtime）判定，规则可复现但粒度较粗；这是已声明的证据限制（M2）。
   该 legacy baseline 没有写 `judge.method`，所以当前 gate 只能按 by+date 放行；
   它不是“逐条人工盲审”证据。若按新 provenance 规则标注，应 FAIL。
2. **fei 的 rel=1 为 0**：草案中的 107 条同源弱相关已按裁决规则改为 2。
3. **A/B 优势未成立**：三层 Recall/MRR 全部持平，`discrimination 0/3`；
   只有 NDCG 高于 B。结论只能是“未观察到退化”。
   2026-10-23 的默认行为变更（`mmr_enable=0`、`bucket_allow_other=1`）把
   paraphrase MRR 从 0.756 降到 0.744；这是为 v2 判别集修复 MMR 挤出精确目标
   的代价，已由 v2 机械门验证（见 `docs/m4-v2-benchmark.md`）。
4. **语料规模**：40 条，刚过 gate 下限 30；统计功效有限。
5. **NDCG 分层**：`multi-rel=34 single-rel=6`，NDCG 非二值；但相关度分级来自规则，
   不宜解读为人工细粒度相关性。
6. **主指标持平**：控制门通过不等于 A 的主指标优于 B；本轮只证明规则物化弱相关/
   负例排序 NDCG 不劣且略高，不能写成显著优势或独立人工闭环。

## 7 本轮发现并修复的缺陷

| 缺陷 | 触发 | 修复 | 证据 |
|---|---|---|---|
| O6 桶过滤漏召 | `primary failover during network partition` 被 network 词硬筛，db 桶主库 case 漏召 | db 家族新增 `failover/replica/replication/主库/主从/裂脑`；回归测试 `candidate_buckets` 含 db | 修复前 40 条中 1 条目标不在 top-10；修复后 40/40 |
| PRF 排序不可重放 | 扩展候选 idf 并列时继承 HashMap 随机序，跨进程同 query 候选数 6..10 抖动 | `sort_expansion_candidates` 增加词面次级键；回归测试 `expansion_sort_ties_break_by_term_asc` | 同 query 10 次进程调用候选数恒定 |
| URL 污染场景桶 | 公开复盘 URL 的 `https` 含 `http`，v2 150 条中 120 条误归 network | `bucket::lower` 剥离 URL；`bucket_allow_other=1` 放行未分类 case | v2 top-10 目标漏召 6→0 |
| MMR 挤出精确目标 | λ=0.5 时同事故兄弟 case 优先，v2 paraphrase r@5 0.783 | `mmr_enable` 默认 0，需要多样性时显式开启 | v2 paraphrase r@5 0.867，gate PASS |
| 同分排序跨进程漂移 | 同分兄弟 case 继承 HashMap 遍历序，NDCG 每次运行不同 | `order_by_score` 同分按 case id 升序 | 同一输入 3 次运行的 layer 行 md5 一致 |
| rel 键重复 | `expected` 完整 id 与 `rel` hash 后缀指向同一 case，`or_insert(1.0)` 插入重复键 | `parse_baseline` 按 id_matches 合并；`rel_of` 精确→tail→tail8→排序键 | 新增回归测试 `expected_does_not_override_tail_label_with_one` |
| 同事故兄弟散落 | 跨事故弱候选插入同事故 rel=2 兄弟之间 | `sibling_coherence=1`：top1 同族（Jaccard≥0.35）收紧到其后 | confirm5 目标排名 6 改善 / 0 退化；v2 hard ΔMRR +0.010→+0.077 |

验证（E1）：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、
`cargo test --locked`（130 库 + 9 CLI + 5 集成 + 2 golden 活跃，1 ignored）全绿；
release 重建后旧门 `--gate-human` PASS；逐候选门禁收紧后 FAIL（0/344）。
