# 评估结果与证据边界

更新：2026-10-25。输入、冻结标签、盲审包与复现工具在 `eval/`；详细命令见
[`eval/README.md`](../eval/README.md)。旧 `eval-real/` 中间产物已删除，历史文件可从
git commit `5ada90d` 恢复。

结论先行：**confirm5 的预注册机械目标排序优势成立；confirm5 的 40/150 逐候选人工
子集在当前配置下通过 `--gate-human`，但这是同一批标签上的事后校准，不是独立
holdout，且 NDCG 差值 CI 跨 0。v2 是开发集；real 主指标 A=B，逐候选人工门因
provenance 缺失而失败。仓库内仍没有“独立 holdout + 双人逐条盲审 + 预注册配置”
的完整闭环。**

## 1. 证据阶梯

| 阶段 | 语料 / 标签 | 能证明 | 不能证明 |
|---|---|---|---|
| 自动标注 | `--annotate` 回归集 | 管线确定、可重放、变体选择漏斗 | A/B 质量优势：变体先经 A 自检放行 |
| 合成 40 组 | 真实感夹具 + 案例级人工标签 | 机械门可用、弱相关/负例工具链 | 真实事故分布、独立盲审 |
| v2 | 30 起公开事故，150 查询；AI 预填后双人采纳 | 机械门 PASS；开发集 hard Δ+0.0514 | 独立确认；逐候选人工闭环 |
| confirm2–4 | 历史确认集 | confirm4 暴露低置信桶硬筛缺陷 | confirm4 确认资格（修复由它触发） |
| confirm5 | 30 起新事故，150 查询 | hard 层 MRR 优势（机械 target） | 弱相关/负例人工闭环（机械 target 全 2） |
| confirm5 + hao | 40/150 逐候选人工标签 | 当前 `--gate-human` 点估计 PASS | 独立 holdout；NDCG 显著优势 |
| real | 40 条、11 起公开事故 | 案例级双人标签、NDCG 方向 | 逐候选人工门（0/344） |

## 2. confirm5：预注册机械确认

语料为 30 起公开事故、150 条查询（literal 60 / paraphrase 60 / semantic 30），
每起事故 5 条兄弟 case。使用 150 条 target-only 机械标签，`rel=2` 为 target，
其余未标注为 0；主指标预注册为 hard 层 MRR@10。

| 条件 | 结果 | 判定 |
|---|---|---|
| hard 实际 n | 69（>=55） | PASS |
| Δ = MRR_A − MRR_B | **+0.0669**（>=0.05） | PASS |
| paired bootstrap 95% CI | **[0.0238, 0.1176]**，下界 >0 | PASS |
| permutation p（10,000 次，seed 20261023） | **0.0020** | PASS |
| `--gate` exit | 0 | PASS |
| `--gate-human`（机械 target 基线） | 2 | 预期失败：全 2、无 0/1 控制 |

按事故聚类的敏感性：CI [0.0273, 0.1138]，p=0.0038。该聚类是敏感性检查，
不替代 case 级预注册分析。当前默认配置的复跑值见下节；历史预注册 binary 的
点值仍是上表。

### 2.1 policy 控制集

`eval/confirm5-policy.labels.json` 在同一 150 条查询上物化规则：同事故 `rel=2`，
同场景桶 `rel=1`，其余 `rel=0`；分布 `2=650 / 1=115 / 0=665`，
`judge.method=policy-materialized-v1`。

hard ΔMRR **+0.0608**，CI [0.0202, 0.1095]，p=0.0029；`--gate-human` 仍
FAIL / exit 2。只能把它当作替代控制集，不能写成逐条人工盲审：

```text
gate: human provenance policy-materialized labels=150 (not per-item human review) FAIL
gate: FAIL
```

### 2.2 逐候选人工验收（hao，40/150）

- 盲审包：`eval/confirm5-40.packet.tsv`，40 例 / 384 候选，SHA256
  `b8eebf2fcb78d0fb745c9760b55dd34d832a9bb976b377f3dfb8c7cea264093f`。
- 人工标签：`eval/confirm5-hao.labels.json`，`judge.by=hao`，
  `method=per-item-human-v1`；rel 分布 `2=142 / 1=130 / 0=112`。
- `--gate-human`：**PASS / exit 0**；literal 自匹配 0/14 违规；
  provenance 384/384、cases 40/40。
- NDCG@10：literal/paraphrase/semantic 为 0.906/0.785/0.828 vs
  0.898/0.785/0.825；semantic MRR 0.872 vs 0.715（Δ+0.1564，
  CI [0.0205, 0.3282]，p=0.1261）。
- NDCG 差值的 semantic bootstrap：+0.0023，CI [-0.0131, 0.0161]；因此不能
  写“NDCG 显著优于 B”。

AI 标签 `eval/confirm5-ai.labels.json`（`per-item-ai-v1`，384/384）只通过
`--gate-items`，`--gate-human` 必须失败。配置是在 hao 标签上校准的，不能把
40/150 的结果外推成全量或独立 holdout。

### 2.3 冻结哈希

| 对象 | SHA256 |
|---|---|
| `eval/confirm5.corpus.json` | `B83A3A8C834A6494BDA76777BFD7500488CEDFE097C81D2AFA5E03049E543335` |
| `eval/confirm5-policy.labels.json` | `EDF6C2083D77F7A39A2DB1DCC8C13614AD55E5D78A832BBC6F93B449E1BD6F33` |
| `eval/confirm5-ai.labels.json` | `3114B9152835D604BD06F1B1D4E54A327B8857E9650E08111B04F452D965508A` |
| `eval/confirm5-hao.labels.json` | `C68573E0640EADD29C843F399C986ADB69194285A49AB1A5201D9D408A83498C` |
| `eval/confirm5-40.packet.tsv` | `B8EEBF2FCB78D0FB745C9760B55DD34D832A9BB976B377F3DFB8C7CEA264093F` |
| `eval/confirm5-40-ai.tsv` | `6A5319C546FE68C20F3D4D7FFD38437B3736EA301E1319AFB3C374ECA9AC9B18` |
| `eval/confirm5-40-hao.tsv` | `1F6CD83576DD3872635C2317131F8CE7858301E7AFE88D490B8CE46E1B9240B5` |

复现入口：

```bash
./bin/scx-eval seed eval/confirm5.corpus.json \
  --mem-dir /tmp/scx-confirm5 --csv /tmp/scx-confirm5.csv --force
./bin/run --eval eval/confirm5-hao.labels.json --gate-human --eval-verbose \
  --mem-dir /tmp/scx-confirm5
```

## 3. v2：开发集

v2 有 30 起公开事故、150 条查询（60/60/30），最终标签
`eval/v2.labels.json` 为 `judge.by=fei+hao`，案例级签名；分布
`rel 2=432 / 1=312 / 0=77`。312 条弱相关由 AI 按具体故障机制预填，fei/hao
两份审核全盘采纳；因缺少逐候选 `judge_items`，`--gate-human` 在
`human per-item provenance` 处 0/821 FAIL。

当前默认配置的机械门 PASS。显著性（10,000 次，seed 20261023）：

| 组 | ΔMRR | 95% CI | p | advantage |
|---|---:|---:|---:|---:|
| paraphrase | +0.0040 | [-0.0017, 0.0122] | 0.5004 | 0 |
| hard | +0.0514 | [0.0080, 0.1036] | 0.0254 | 1 |

hard 数值达到门槛，但 v2 参与调参，不能写成独立确认。复现：

```bash
./bin/scx-eval seed eval/v2.corpus.json \
  --mem-dir /tmp/scx-v2 --csv /tmp/scx-v2.csv --force
./bin/run --eval eval/v2.labels.json --gate --mem-dir /tmp/scx-v2
```

## 4. real：真实事故案例级证据

40 条 case 来自 11 起公开复盘（literal 14 / paraphrase 13 / semantic 13）；
来源清单见 [`real-incident-sources.md`](real-incident-sources.md)，逐条 URL 在
`eval/real.corpus.json`。标签 `eval/real.labels.json` 为 `judge.by=fei+hao`，
分布 `2=147 / 1=151 / 0=46`；弱相关是场景桶规则物化，不是逐条盲审。

当前默认配置：三层 r@5/r@10 与 MRR 均为 A=B；NDCG A>B 但 CI 重叠；
`--gate-human` 因 `judge_items` 缺失在 0/344 处 FAIL / exit 2。该失败是证据边界，
不是指标回归。复现：

```bash
./bin/scx-eval seed eval/real.corpus.json \
  --mem-dir /tmp/scx-real --csv /tmp/scx-real.csv --force
./bin/run --eval eval/real.labels.json --gate-human --mem-dir /tmp/scx-real
```

## 5. 已成立与必须拒绝

已成立：

- `--annotate` 确定性、可被 `--eval` 重放；
- Recall/MRR/NDCG、分层聚合与 bootstrap 可重放；
- `--gate`、`--gate-human`、`--gate-items` 把样本、provenance、控制标签和
  指标门槛变成可执行门；
- confirm5 机械 target 的 hard 层优势满足预注册条件；
- confirm5 hao 子集通过当前人工门，但只支持事后验收。

必须拒绝的表述：

- “A 对所有 RAG/embedding 系统有优势”：B 只是朴素词面基线；
- “confirm5 机械 target 已通过人工弱相关/负例门”：rel 全 2；
- “policy 控制集等于逐条人工盲审”：规则物化，gate 以 provenance FAIL；
- “v2 独立确认质量优势”：v2 参与调参，标签为 AI 预填后人工采纳；
- “real 建立 Recall/MRR 优势”：三层 A=B；
- “confirm5 hao 是独立 holdout”：同一批标签参与配置校准；
- “NDCG 显著优于 B”：semantic NDCG CI 跨 0；
- “AI 标签可以冒充人工”：AI 只能过 `--gate-items`。

## 6. 方法学约束

1. 自检即选择：`--annotate` 用 A 管线放行变体，只能做回归。
2. 主指标、MDE、CI 方法和 seed 必须在看结果前冻结。
3. 按层/tier/cluster 反复看会引入多重比较；报告全部组并区分预注册与敏感性。
4. 机械 target、policy 物化、AI 预填、案例级签名、逐候选真人是五种不同证据。
5. confirm5 的 5 条兄弟 case 相关，case 级 bootstrap 应同时报告事故聚类敏感性。
6. 配置校准集不能当确认集；任何优势声明都要新的独立语料与逐候选人工标签。
