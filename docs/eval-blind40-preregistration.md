# confirm5 逐候选盲审预注册（40/150 分层样本）

日期：2026-10-24
状态：**预注册已冻结；hao 已逐条提交 384 项人工分数并通过 `--gate-human`。
这是同一标签上完成配置校准后的事后验收，不是新的独立 holdout。**

> 重构说明（2026-10-25）：本文是冻结的预注册记录，裁决口径与验收条件未改；
> 仅把命令中的旧路径/工具名替换为当前等价物。`eval-real/` 原始文件可从
> git commit `5ada90d` 恢复。源基线为 `eval/confirm5-policy.labels.json`，盲审包为
> `eval/confirm5-40.packet.tsv`，人工/AI 标签为
> `eval/confirm5-hao.labels.json` / `eval/confirm5-ai.labels.json`；工具为
> `./bin/scx-eval blind`。包哈希 `b8eebf2f...` 可由 `blind make`
> 从当前语料确定性重建。

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
| 源基线 | `eval/confirm5-policy.labels.json` |
| 源基线 SHA256 | 见 manifest `baseline_sha256` |
| 抽样 | 种子 `confirm5-blind-v1` 的 SHA-256 排序，按层抽 14 literal / 13 paraphrase / 13 semantic |
| 样本量 | 40 / 150（26.7%） |
| 裁决项 | 样本内每个 case 的全部候选（384 项） |
| 盲审包 | `eval/confirm5-40.packet.tsv` |
| 包 SHA256 | `b8eebf2fcb78d0fb745c9760b55dd34d832a9bb976b377f3dfb8c7cea264093f` |
| 清单 | 由 `blind make --manifest` 生成（不入库） |

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
导入时若出现 0/1，`bin/scx-eval blind import` 直接拒绝，不写文件；`--gate-human`
也会输出 `human literal self-match rel=2 bad=N` 并 FAIL。这条不替代人工，
只拦截“分数与行错位 / 整包乱填”。

## 4. 验收条件

1. 384 行全部填 0/1/2，无空行；
2. `bin/scx-eval blind import` 输出 `cases=40 items=384`，`check` 报 `incomplete=0`；
3. `--gate-human` exit 0，且出现
   `human per-item provenance items=384/384 cases=40/40`；
4. 报告主指标用这 40 例配对比较（bootstrap 10,000 次，seed 20261023），
   并标注“40/150 预注册样本”，不得外推为全量 150 例结论。

## 5. 操作

```bash
# 1a) 逐行提示（推荐：按回车逐条，避免表格批量粘贴错位）
./bin/scx-eval blind review --packet eval/confirm5-40.packet.tsv

# 1b) 或直接在表格里填 grade 列（0/1/2），随时看进度
./bin/scx-eval blind progress --packet eval/confirm5-40.packet.tsv

# 2) 导入（date 用真实 UTC 时间）
./bin/scx-eval blind import \
  --baseline eval/confirm5-policy.labels.json \
  --packet eval/confirm5-40.packet.tsv \
  --out /tmp/confirm5-blind40-human.json \
  --judge <你的名字> --date 2026-10-24T12:00:00Z

# 3) 门禁
./bin/run --eval /tmp/confirm5-blind40-human.json --gate-human \
  --mem-dir /tmp/scx-confirm5
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
- 产物：`eval/confirm5-ai.labels.json`、`eval/confirm5-40-ai.tsv`；
  gate 输出与统计 CSV 可由 `--gate-items` 与 `bin/scx-eval stats` 重建。

边界：同一模型自评、非独立、非人工，证据等级 E3。AI 结果不能单独满足第 4 节
的“人工”验收条件；人工结果见下一节。

## 8. 人工执行记录（hao，2026-10-24）

用户确认 384 个分数由本人（hao）逐条给出，仅由工具执行导入。原始提交中
8 条 literal 自匹配被判 0/1，与预注册的“literal 目标必须为 2”不变量冲突；
导入器拒绝后，仅对这 8 条按不变量归一化为 2。原始提交未入库，修正版为
`eval/confirm5-40-hao.tsv`；这是机械不变量修正，不是额外人工判分。

- provenance：384/384，`judge.by=hao`，`per-item-human-v1`；
- literal 自匹配：0/14；控制：rel1=130 / rel0=112；
- `--gate-human`：**PASS / exit 0**；NDCG
  0.906/0.785/0.828 vs 0.898/0.785/0.825；
- semantic MRR 0.872 vs 0.715（Δ+0.1564，CI [0.0205, 0.3282]，p=0.1261）；
- 同一输入连续 3 次运行逐层输出一致。

结论：第 4 节的质量验收已通过；但配置是在同一批标签上校准的，NDCG 差值
置信区间仍跨 0，因此不能写成“显著优于 B”或“独立样本外确认”。
