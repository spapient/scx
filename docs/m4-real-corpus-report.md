# M4 真实历史事故语料报告

## 1 结论

- 真实语料：11 起公开事故复盘派生 40 条 case，literal 14 / paraphrase 13 / semantic 13。
- `--eval --gate`：**PASS**（exit 0）。
- A/B 主指标：三层 **A ≥ B**；literal/semantic/paraphrase 的 Recall 与 MRR 全部持平
  （paraphrase MRR 0.744 vs 0.744）。NDCG 三层 A>B。
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
.\rs\target\release\scx.exe --eval .\eval-real\real-baseline-final.json --gate --mem-dir .\eval-real\real-mem
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

```
eval: cases=40 layers=literal,paraphrase,semantic multi-rel=34 single-rel=6 (NDCG strength)
layer literal: n=14 A r@5=1.000 mrr=1.000 ndcg@10=0.959 | B r@5=1.000 mrr=1.000 ndcg@10=0.920
layer paraphrase: n=13 A r@5=1.000 mrr=0.744 ndcg@10=0.932 | B r@5=1.000 mrr=0.744 ndcg@10=0.862
layer semantic: n=13 A r@10=1.000 mrr=0.962 ndcg@10=0.914 | B r@10=1.000 mrr=0.962 ndcg@10=0.885
gate: rel distribution 2=147 1=151 0=46
gate: discrimination A!=B layers=0/3
gate: warning A=B on all layers (no measurable advantage)
gate: PASS
```

## 6 诚实性限制（M2 / M5）

1. **hao 的 rel=1 是规则物化，不是逐条手工标注**：151 条“根因域相近”由同一场景桶
   （db/network/disk/build/runtime）判定，规则可复现但粒度较粗；这是已声明的证据限制（M2）。
2. **fei 的 rel=1 为 0**：草案中的 107 条同源弱相关已按裁决规则改为 2。
3. **A/B 优势未成立**：三层 Recall/MRR 全部持平，`discrimination 0/3`；
   只有 NDCG 高于 B。结论只能是“未观察到退化”。
   2026-10-23 的默认行为变更（`mmr_enable=0`、`bucket_allow_other=1`）把
   paraphrase MRR 从 0.756 降到 0.744；这是为 v2 判别集修复 MMR 挤出精确目标
   的代价，已由 v2 机械门验证（见 `docs/m4-v2-benchmark.md`）。
4. **语料规模**：40 条，刚过 gate 下限 30；统计功效有限。
5. **NDCG 分层**：`multi-rel=34 single-rel=6`，NDCG 非二值；但相关度分级来自规则，
   不宜解读为人工细粒度相关性。

## 7 本轮发现并修复的缺陷

| 缺陷 | 触发 | 修复 | 证据 |
|---|---|---|---|
| O6 桶过滤漏召 | `primary failover during network partition` 被 network 词硬筛，db 桶主库 case 漏召 | db 家族新增 `failover/replica/replication/主库/主从/裂脑`；回归测试 `candidate_buckets` 含 db | 修复前 40 条中 1 条目标不在 top-10；修复后 40/40 |
| PRF 排序不可重放 | 扩展候选 idf 并列时继承 HashMap 随机序，跨进程同 query 候选数 6..10 抖动 | `sort_expansion_candidates` 增加词面次级键；回归测试 `expansion_sort_ties_break_by_term_asc` | 同 query 10 次进程调用候选数恒定 |
| URL 污染场景桶 | 公开复盘 URL 的 `https` 含 `http`，v2 150 条中 120 条误归 network | `bucket::lower` 剥离 URL；`bucket_allow_other=1` 放行未分类 case | v2 top-10 目标漏召 6→0 |
| MMR 挤出精确目标 | λ=0.5 时同事故兄弟 case 优先，v2 paraphrase r@5 0.783 | `mmr_enable` 默认 0，需要多样性时显式开启 | v2 paraphrase r@5 0.867，gate PASS |

验证（E1）：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、
`cargo test --locked`（118 库 + 8 CLI + 5 集成 + 1 golden）全绿；release 重建后 gate PASS。
