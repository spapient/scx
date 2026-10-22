# M4 confirm5 独立确认报告

日期：2026-10-23
状态：**预注册确认通过；目标排序质量优势成立**

## 1. 结论先行

在未参与调参的 confirm5 语料上，A 相对 B 的 hard 层 MRR@10 优势满足全部预注册条件：

| 条件 | 结果 | 判定 |
|---|---|---|
| `tier:hard` 实际 n | 69（>=55） | PASS |
| `Delta = MRR_A - MRR_B` | **+0.0669**（>=0.05） | PASS |
| paired bootstrap 95% CI | **[0.0238, 0.1176]**，下界 >0 | PASS |
| permutation p（10,000 次，seed 20261023） | **0.0020**（<0.05） | PASS |
| `--gate` exit | **0** | PASS |
| literal 层 A | 1.000 = B 1.000 | PASS |

因此可以写：

> 在 confirm5 的 30 起公开事故、150 条机械目标标签查询上，完整管线 A 在 hard 层
> 的 MRR@10 显著高于朴素词面基线 B，且 literal 层没有退化。

不能写：

- “M4 人工相关性证据已完全闭环”——confirm5 使用目标机械标签，不是双人盲审；
- “对所有 RAG/embedding 系统都有优势”——B 只是朴素词法基线；
- “confirm4 也通过”——confirm4 首跑失败，修复后仅作为诊断集。

## 2. 确认链

### 2.1 confirm4 首跑失败

冻结版 confirm4 首跑 `exit=2`：

```text
layer paraphrase: n=60 A r@5=0.617 mrr=0.287 | B r@5=0.617 mrr=0.288
layer semantic:   n=30 A r@10=0.967 mrr=0.632 | B r@10=1.000 mrr=0.348
gate: FAIL
```

逐 case 归因发现两条低置信 query 的查询侧桶与目标主桶不一致：

- paraphrase case `... JVM upgrade ... thread ...`：候选桶 `runtime`，目标桶 `db`；
- semantic case `机器重启时选错了磁盘分区 ... ext4 ...`：候选桶 `disk`，目标桶 `db`。

场景桶在排序前硬筛掉了目标；这不是字段重排参数问题。

### 2.2 修复

`recall.rs` 新增低置信桶回退：

- 新参数 `bucket_bypass_sim=0.30`（写入新库 meta.toml）；
- 首轮 top1 `sim < 0.30` 时，后续所有轮次改为无桶全量候选打分；
- 高置信查询仍保留桶过滤，不牺牲常规扫描路径；
- 新增线性/索引双路径回归测试
  `low_confidence_query_bypasses_bucket_filter_linear_and_index`。

confirm4 修复后诊断 `exit=0`，hard Δ=+0.0794，CI [0.0372, 0.1283]，p=0.0005。
但 confirm4 已被用于定位和验证修复，**不再具备确认资格**。

### 2.3 confirm5 独立确认

confirm5 在修复后新建，30 起事故与 v1/v2/holdout/confirm2/confirm3/confirm4
均不重复；在运行 A/B 前完成预注册与语料封印。

## 3. 语料

| 项 | 值 |
|---|---|
| 公开事故 | 30 |
| 每起 case | 5（literal×2 / paraphrase×2 / semantic×1） |
| 总 case | 150 |
| 评估查询 | 150（60/60/30） |
| 查询去重 | 240/240 唯一 |
| 与六套前序语料重叠 | 0 URL / 0 查询 |
| hard 估计 | n=69（paraphrase 54 / semantic 15） |
| 标签 | target=2，其余未标注=0；`judge.by=mechanical-target-confirm5-v1` |

来源为 `https://github.com/danluu/post-mortems` 索引中的公开复盘；每条 case 的
`text` 保存来源 URL 和根因摘要，中文为翻译/改写，不编造事故与日志。

## 4. 冻结与复现

| 对象 | SHA256 |
|---|---|
| release binary | `0469182BD8FED2A32F5288D188728ECCC8852129D7124C38643C4DE18F7D6FCD` |
| source/measurement manifest | `9856A9CFE3D69D437A446028DFFF50EE77A41F1E8D0F2845826BD0E279FCCCD7` |
| pre-registration | `1BC18AF752D60E56C88749EB505694CE563E90CA080FA7D3C4F9A9D1A8A56E20` |
| seed script | `AA7D90BDA0251E8E1D34AE8E7B8834310481785690E3297B64F16DE830792BDC` |
| incidents JSON | `E12FDD3C62E6E5C4B192BE423DC6F3070097DE033D41D0426E95E6E543765704` |
| verbose output | `84B6DD195ED5F7818C40652818B96E13F947B573AFFC95FEF93CB5F603D76D3B` |
| significance CSV | `B1818132171E830E191EDF871B0CF40C03DC47F89099A4D031D0857404C438EB` |

关键文件：

- `eval-real/confirm5-preregistration.md`
- `eval-real/confirm5-source.sha256`
- `eval-real/confirm5-incidents.json`
- `eval-real/seed-confirm5.ps1`
- `eval-real/confirm5-corpus.sha256`
- `eval-real/confirm5-eval-verbose.txt`
- `eval-real/confirm5-exit.txt`
- `eval-real/confirm5-rank-diff.csv`
- `eval-real/confirm5-strata.csv`
- `eval-real/confirm5-significance.csv`

## 5. 单次确认运行

```powershell
.\rs\target\release\scx.exe --eval .\eval-real\confirm5-baseline.json `
  --gate --eval-verbose --mem-dir .\eval-real\confirm5-mem
```

结果：`exit=0`。

```text
layer literal: n=60 A r@5=1.000 mrr=1.000 ndcg@10=1.000 | B r@5=1.000 mrr=1.000 ndcg@10=1.000
layer paraphrase: n=60 A r@5=0.567 mrr=0.244 ndcg@10=0.378 | B r@5=0.567 mrr=0.242 ndcg@10=0.373
layer semantic: n=30 A r@10=1.000 mrr=0.883 ndcg@10=0.913 | B r@10=1.000 mrr=0.674 ndcg@10=0.751
l1: hits=60 total=87 rate=0.690
gate: rel distribution 2=150 1=0 0=0
gate: discrimination A!=B layers=2/3
gate: PASS
```

排名差异：paraphrase 层 `A_miss_only=0`、`B_miss_only=1`、`both_miss=11`；
literal 与 semantic 均无单边漏召。

预注册统计：

| 组 | n | A MRR | B MRR | Δ | 95% CI | p | 优势 |
|---|---:|---:|---:|---:|---|---:|---|
| literal | 60 | 1.0000 | 1.0000 | 0 | [0, 0] | 1.0000 | 否 |
| paraphrase | 60 | 0.2439 | 0.2423 | +0.0016 | [-0.0007, 0.0056] | 1.0000 | 否 |
| semantic | 30 | 0.8833 | 0.6739 | +0.2094 | [0.1028, 0.3306] | 0.0005 | 是 |
| **tier:hard** | **69** | **0.3425** | **0.2757** | **+0.0669** | **[0.0238, 0.1176]** | **0.0020** | **是** |
| all | 150 | 0.6742 | 0.6317 | +0.0425 | [0.0186, 0.0705] | 0.0004 | 否（<0.05） |

`all` 的 `advantage=False` 是因为点估计 0.0425 低于 MDE 0.05；主判定层是
预注册的 `tier:hard`，该层五项条件全部满足。

### 5.1 聚类敏感性（事后稳健性，不替代预注册）

每起事故贡献 5 个兄弟 case，case 级 bootstrap 可能低估同事故内的相关性。
因此另外做了一次**按事故聚类**的敏感性分析：以事故为抽样单位（30 个 cluster）
做 10,000 次 bootstrap，并按 cluster 整体做 10,000 次符号置换。该分析不改变
预注册主判定，只检查结论是否依赖“把兄弟 case 当独立样本”的假设。

| 组 | case / cluster | A MRR | B MRR | Δ | 95% CI | p | 优势 |
|---|---:|---:|---:|---:|---|---:|---|
| `tier:hard:cluster` | 69 / 30 | 0.3425 | 0.2757 | +0.0669 | **[0.0273, 0.1138]** | **0.0038** | 是 |

复现：`eval-real/cluster-significance.ps1`，输出
`eval-real/confirm5-significance-cluster.csv`。结论：在更保守的按事故重采样口径下，
hard 层优势仍然成立；但优势仍主要来自 A 对部分语义兄弟 case 的排序改善，不能
外推为所有事故类型都稳定领先。

## 6. 证据边界

1. **机械标签**：confirm5 的 rel 由目标 case 机械确定，不是人工相关性盲审；
   它证明了目标排序泛化，不替代 M4 的人工弱相关/负例证据。
2. **单一基线**：B 是朴素词法基线；结论不外推到 embedding、混合检索或任意
   RAG 系统。
3. **近重复结构**：每起事故有 5 个同根因兄弟 case，任务衡量“选对具体 case”；
   不能等同于每起事故只有一个 case 的场景。
4. **样本规模**：30 起事故可支持本轮预注册判定，但不代表全量生产事故分布。
5. **修复来源**：桶回退修复由 confirm4 失败触发；confirm5 是修复后的独立确认集，
   两者不能合并成一个“从未失败”的结论。

## 7. 回归

修复后同一 binary 的既有诊断集：

| 语料 | gate | 备注 |
|---|---|---|
| v2 真人基准 | PASS | paraphrase MRR 0.554 vs 0.550；semantic MRR 0.734 vs 0.484 |
| confirm2 | PASS | 三层 A 不低于 B |
| confirm3 | PASS | paraphrase 0.359 vs 0.356；semantic 0.868 vs 0.686 |
| confirm4（诊断） | PASS | 修复后 r@10 1.000，hard Δ=+0.0794 |

这些结果只用于回归，不改变 confirm5 的独立确认地位。
