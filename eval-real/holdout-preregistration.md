# Held-out 预注册（2026-10-23，结果前冻结）

## 目的

在未参与 v1/v2 修复、query 修订和 qf 阈值选择的公开事故上，一次性检验：
`query_field_weight=0.5` 且仅当 `top1.sim < 0.30` 时启用的重排，是否让完整管线 A
相对朴素词面基线 B 产生可复现的质量优势。

## 语料

- 30 起公开复盘，每起 5 条：literal×2 / paraphrase×2 / semantic×1，共 150 条。
- 来源见 `eval-real/seed-holdout.ps1` 每条 `src`；与 v1/v2 使用的 URL 不重合。
- 入库文本只包含公开复盘 query、来源和根因摘要；`fix` 只取公开复盘已有处置。
- 本文件写入后，不再根据 A/B 结果修改 query、阈值或权重。

## 标签口径

- 这是 **target-only 机械标签**：每条的 expected 是其自身来源事故的 case，`rel=2`；
  其他候选未标注，按 0 计。
- `judge.by=mechanical-target-v1` 明确表示不是人工裁决；它不能替代 M4 要求的人工盲审，
  只用于 held-out 的排名/召回统计，避免弱相关标签口径漂移。
- 主指标只看 target 的排名，MRR 不受弱相关标签影响；NDCG 作为辅助。

## 预注册判定

- 主层：`tier:hard`（query 与目标 case query 的 token Jaccard < 0.2）。
- 主指标：MRR@10 的配对差 `Δ = MRR_A(qf) - MRR_B`。
- 规则：`Δ >= MDE` 且 paired bootstrap 95% CI 下界 `> 0`，且 permutation `p < 0.05`。
- `MDE=0.05`：与 v2 同一产品决策阈值；若 held-out 的 hard 层 n 明显低于 55，
  必须在报告中同时给出实际 n，不得把 v2 的功率依据直接移植。
- 同时要求 `--gate` PASS，且 literal 层不出现 A < B。
- 任何一层 A 的主指标低于 B：该层失败，不因其他层领先而改判。

## 运行顺序

1. 冻结本文件与 `seed-holdout.ps1` 的哈希。
2. 建库并生成 baseline，不先看召回结果。
3. 跑默认 A（qf=0）与 B，记录基线。
4. 复制库写入 qf=0.5，跑 qf A 与 B；只跑一次。
5. 输出 gate、分层指标、显著性、逐 case 排名；若未达标，保留失败结果，不调参重跑。
