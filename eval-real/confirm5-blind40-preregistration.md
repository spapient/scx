# confirm5 逐候选盲审预注册（40/150 分层样本）

日期：2026-10-24
状态：**预注册已冻结，等待真人裁决；未裁决前不得声称闭环**

## 1. 要解决的问题

confirm5 现有两套标签都不能证明“人工弱相关/负例闭环”：

- 机械 target 基线：`rel` 全为 2、只有目标候选，`--gate-human` FAIL；
- policy 控制集：弱相关/负例由人工规则确定性物化，`--gate-human` 以
  `policy-materialized` provenance FAIL。

本轮把门禁收紧到**逐候选溯源**：`rel` 的每个键都必须在 `judge_items`
里有独立裁决记录，整包签名不再算数。

## 2. 样本与盲法（先冻结，后裁决）

| 项 | 值 |
|---|---|
| 源基线 | `eval-real/confirm5-baseline-human.json` |
| 源基线 SHA256 | 见 manifest `baseline_sha256` |
| 抽样 | 种子 `confirm5-blind-v1` 的 SHA-256 排序，按层抽 14 literal / 13 paraphrase / 13 semantic |
| 样本量 | 40 / 150（26.7%） |
| 裁决项 | 样本内每个 case 的全部候选（384 项） |
| 盲审包 | `eval-real/confirm5-blind40-packet.tsv` |
| 包 SHA256 | `b8eebf2fcb78d0fb745c9760b55dd34d832a9bb976b377f3dfb8c7cea264093f` |
| 清单 | `eval-real/confirm5-blind40-manifest.json` |

盲法约束：

- 包内不含 A/B 分数、不含既有 `rel`、候选顺序按哈希打乱；
- 候选只用不透明 id 与摘要，不显示来自哪套管线；
- 裁决完成前不得查看 `*-eval-verbose*`、不得先跑 `--eval`。

## 3. 裁决口径（不可中途修改）

| 分 | 定义 |
|---|---|
| 2 | 同一事故/同一根因，或能直接给出可用修复 |
| 1 | 同一故障族或相邻问题，有参考价值但不能直接解决 |
| 0 | 无关，或只有字面相似 |

只看 `query` 与候选摘要（候选 query + fix）。拿不准时按 1，不按 2。

**机器可判的不变量**：literal 层的 query 与目标 case 原文逐字相同，目标必须判 2。
导入时若出现 0/1，`blind_review.py import` 直接拒绝，不写文件；`--gate-human`
也会输出 `human literal self-match rel=2 bad=N` 并 FAIL。这条不替代人工，
只拦截“分数与行错位 / 整包乱填”。

## 4. 验收条件

1. 384 行全部填 0/1/2，无空行；
2. `blind_review.py import` 输出 `cases=40 items=384`，`check` 报 `incomplete=0`；
3. `--gate-human` exit 0，且出现
   `human per-item provenance items=384/384 cases=40/40`；
4. 报告主指标用这 40 例配对比较（bootstrap 10,000 次，seed 20261023），
   并标注“40/150 预注册样本”，不得外推为全量 150 例结论。

## 5. 操作

```bash
# 1a) 逐行提示（推荐：按回车逐条，避免表格批量粘贴错位）
python3 eval-real/blind_review.py review --packet eval-real/confirm5-blind40-packet.tsv

# 1b) 或直接在表格里填 grade 列（0/1/2），随时看进度
python3 eval-real/blind_review.py progress --packet eval-real/confirm5-blind40-packet.tsv

# 2) 导入（date 用真实 UTC 时间）
python3 eval-real/blind_review.py import \
  --baseline eval-real/confirm5-baseline-human.json \
  --packet eval-real/confirm5-blind40-packet.tsv \
  --out eval-real/confirm5-blind40-human.json \
  --judge <你的名字> --date 2026-10-24T12:00:00Z

# 3) 门禁
./bin/run --eval eval-real/confirm5-blind40-human.json --gate-human \
  --mem-dir eval-real/confirm5-mem
```

未完成前，正确表述是：**逐候选人工弱相关/负例闭环仍未建立**。

## 6. 拒绝记录

2026-10-24 第一次提交的 384 个分数被导入器拒绝：literal 14 例中有 8 例把
自身目标判成 0/1（例：case 1 query 与目标原文完全相同却判 1）。这属于分数与行
错位或整包乱填，不能作为人工证据；该次输入未写入仓库，盲审包保持未填状态。

## 7. 非人工执行记录（用户授权，2026-10-24）

用户明确表示不要求手工填写后，执行 AI 逐候选裁决：

- 裁决人：`codex`，方法 `per-item-ai-v1`，384/384 项；
- 分级：rel2=176 / rel1=38 / rel0=170；literal 自匹配 0/14 违规；
- `--gate-items` PASS / exit 0；`--gate-human` FAIL / exit 2；
- 产物：`confirm5-blind40-ai.json`、`confirm5-blind40-ai-filled.tsv`、
  `confirm5-blind40-ai-gate-items-2026-10-24.txt`、
  `confirm5-blind40-ai-significance.csv`。

边界：同一模型自评、非独立、非人工，证据等级 E3。第 4 节的“人工”验收条件
（真人裁决）仍未满足；本次结果只能作为非人工逐条闭环与回归基线。

## 8. 人工执行记录（hao，2026-10-24）

用户确认 384 个分数由本人逐条给出，仅由工具执行导入。原始提交中 8 条 literal
自匹配被判 0/1，导入器拒绝；按不变量归一化为 2 后（原始保留为
`confirm5-blind40-hao-raw.tsv`；未逐条回问）：

- provenance：384/384，`judge.by=hao`，`per-item-human-v1`；
- literal 自匹配：0/14；控制：rel1=130 / rel0=112；
- **`--gate-human` FAIL / exit 2**：paraphrase NDCG 0.779 vs 0.785，
  semantic 0.819 vs 0.825；literal 0.906 vs 0.898；
- MRR：semantic 0.962 vs 0.715（Δ+0.2462，CI [0.0692,0.4308]，p=0.0675）。

结论：人工逐候选 provenance 成立；第 4 节的质量验收（gate exit 0）**未通过**。
