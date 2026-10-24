# M4 confirm5 独立确认报告

日期：2026-10-23（2026-10-24 追加 policy 控制集）
状态：**预注册目标排序确认通过；policy 控制集只作规则物化敏感性分析。
2026-10-24 逐候选人工 provenance 已成立（hao，384/384），但 `--gate-human`
因 paraphrase/semantic 的 NDCG A<B 仍 FAIL——人工弱相关/负例闭环成立，
“A 在分级排序上不劣于 B”未成立**

## 1. 结论先行

在未参与调参的 confirm5 语料上，A 相对 B 的 hard 层 MRR@10 优势满足全部预注册条件：

| 条件 | 结果 | 判定 |
|---|---|---|
| `tier:hard` 实际 n | 69（>=55） | PASS |
| `Delta = MRR_A - MRR_B` | **+0.0669**（>=0.05） | PASS |
| paired bootstrap 95% CI | **[0.0238, 0.1176]**，下界 >0 | PASS |
| permutation p（10,000 次，seed 20261023） | **0.0020**（<0.05） | PASS |
| `--gate`（机械资格门）exit | **0** | PASS |
| `--gate-human`（机械 target 基线）exit | **2** | FAIL：全 2、无 0/1 控制（符合预期） |
| literal 层 A | 1.000 = B 1.000 | PASS |

因此可以写：

> 在 confirm5 的 30 起公开事故、150 条机械目标标签查询上，完整管线 A 在 hard 层
> 的 MRR@10 显著高于朴素词面基线 B，且 literal 层没有退化。

但机械 target 基线不是 M4 的“人工弱相关/负例闭环”：`rel` 全部为 2，
`judge.by` 是机械规则名。
2026-10-24 用 `--gate-human` 复跑同一输入，exit 2：

```text
gate: human judge metadata all=150 judges=mechanical-target-confirm5-v1 FAIL
gate: human controls weak=0>=10 negative=0>=10 layers=0>=2 FAIL
gate: FAIL
```

不能写：

- “confirm5 机械 target 基线已通过人工弱相关/负例门”——它仍是全 2；
- “confirm5 policy 控制集等于逐条人工盲审”——它是人工裁决规则的确定性物化，
  `--gate-human` 会以 `policy-materialized` provenance 硬失败；
- “对所有 RAG/embedding 系统都有优势”——B 只是朴素词法基线；
- “confirm4 也通过”——confirm4 首跑失败，修复后仅作为诊断集。

## 1.1 2026-10-24 追加：policy 弱相关/负例控制集

为回答“只有目标 rel=2 不能证明弱相关/负例闭环”，在**同一 150 条 confirm5
查询、同一冻结库**上增加候选级控制集：

- 输入：`eval-real/confirm5-baseline-human.json`
- 候选池：A/B top-10 并集（1,419 个候选行；两侧平等入池，不看分数定标签）
- 标签规则（冻结的 fei+hao 裁决口径，确定性物化）：
  同一事故（同来源 URL）`rel=2`；跨事故但同场景桶（db/network/disk/build/runtime）
  `rel=1`；其余 `rel=0`。目标 case 即使未被召回也为 `rel=2`（进入 IDCG）。
- 分布：`rel 2=650 / 1=115 / 0=665`，literal/paraphrase/semantic 三层均含 1/0。
- 元数据：`judge.by=fei+hao-policy-v1`，`judge.method=policy-materialized-v1`，
  `judge.date=2026-10-24T00:00:00Z`。

`--gate-human` 结果 **FAIL / exit 2**。规则物化不能冒充逐条人工裁决，
因此 gate 把它作为 provenance 失败，而不是只告警：

```text
gate: human judge metadata all=150 judges=fei+hao-policy-v1 FAIL
gate: human provenance policy-materialized labels=150 (not per-item human review) FAIL
gate: FAIL
```

该控制集仍有信息量：它固定了“同事故=2、同场景桶=1、其余=0”的人工规则，
可用于检查弱相关/负例排序是否被优化破坏。以下指标是 **surrogate（替代控制集）**，
不是人工闭环证据（`eval-real/confirm5-human-gate-final-2026-10-24.txt`）：

| 层 | A r@5/r@10 | B r@5/r@10 | A MRR | B MRR | A NDCG | B NDCG |
|---|---:|---:|---:|---:|---:|---:|
| literal | 1.000 | 1.000 | 1.000 | 1.000 | 0.969 | 0.966 |
| paraphrase | 0.633 | 0.567 | 0.258 | 0.242 | 0.852 | 0.829 |
| semantic | 1.000 (r@10) | 1.000 | 0.883 | 0.674 | 1.000 | 1.000 |

hard 层（n=69）MRR：A 0.3548 vs B 0.2757，**Δ=+0.0791，95% CI
[0.0366, 0.1293]，permutation p=0.0001**（10,000 次，seed 20261023；
`eval-real/confirm5-human-significance.csv`）。该替代控制集同样超过
`Δ>=0.05 + CI 下界>0 + p<0.05` 三项门槛，但**标签是 policy 物化，不是新的逐条
人工盲审**，因此只写“替代控制集上的方向一致”，不写“人工盲审已闭环”，
也不把这次运行称为 `--gate-human` PASS。

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

机械门只验证目标检索资格。人工证据必须另跑：

```powershell
.\rs\target\release\scx.exe --eval .\eval-real\real-baseline-arb.json `
  --gate-human --eval-verbose --mem-dir .\eval-real\real-mem
.\rs\target\release\scx.exe --eval .\eval-real\v2-baseline-arb.json `
  --gate-human --eval-verbose --mem-dir .\eval-real\v2-mem
```

结果见 `docs/m4-real-corpus-report.md`：40 组、`rel 2=147 / 1=151 / 0=46`、
`judge.by=fei+hao`、`gate: mode=human` PASS；但 151 条弱相关是同场景桶规则
物化，不是逐条盲审。主指标仍 A=B，NDCG A>B。v2 人工采纳集（150 组，
`rel 2=432 / 1=312 / 0=77`）同样 PASS；312 条弱相关是 AI 预填后人工采纳：
paraphrase r@5 0.917 vs 0.867、MRR 0.566 vs 0.550；semantic MRR
0.696 vs 0.484；hard 层 ΔMRR **+0.0772，CI [0.0271, 0.1371]，p=0.0022**。
confirm5 机械 target 基线的人工门失败原始输出保留在
`eval-real/confirm5-human-gate-2026-10-24.txt`；policy 控制集的失败输出在
`eval-real/confirm5-human-gate-final-2026-10-24.txt`（exit 2，provenance 失败）。

2026-10-24 起 `--gate` 追加每层 absolute 地板行。当前 binary 在同一 confirm5
库上 `gate: PASS` exit 0，paraphrase r@5 **0.633/0.567**、MRR **0.258/0.242**；
相对归档排名，**6 条目标排名改善、0 条退化**。无关系全 miss 时同一 baseline
为 `gate: FAIL` exit 2。机械 target 输入跑 `--gate-human` 仍 FAIL exit 2，
这是预期的证据边界，不是回归。

排名差异：paraphrase 层 `A_miss_only=0`、`B_miss_only=1`、`both_miss=11`；
literal 与 semantic 均无单边漏召。2026-10-24 的 sibling-coherence 改进把
6 条 paraphrase/semantic 目标从 rank 4–10 提到 rank 3–4，未造成任何目标降级。

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

> 上表是**冻结 binary 的单次预注册结果**，保持不变。2026-10-24 优化后
> （sibling-coherence + qf 低词面保护 + 确定性排序）在**同一机械 target 标签**
> 上复跑：hard 层 n=69，A MRR 0.3548 vs B 0.2757，**Δ=+0.0791，
> CI [0.0366, 0.1293]，p=0.0001**（`confirm5-mechanical-final-significance.csv`）。
> 这是同语料的事后复跑，不替代预注册数字；两者方向一致。

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
   `confirm5-baseline-human.json` 的 1/0 控制来自冻结的 fei+hao 裁决规则，
   `--gate-human` 以 provenance 失败；它是“按人工规则的控制集”，不是逐条
   人工盲审。real 的 151 条 rel=1 也由同场景桶规则物化，v2 的 312 条
   rel=1 是 AI 预填后人工采纳；两者都不能写成“独立盲审闭环”。目前仓库内
   真正逐条双人裁决的只有 `baseline-final.json`（40 组、11 弱/64 负、
   fei vs hao 有 20 处分歧并仲裁），但它是合成回归语料，不是独立真实事故集。
2. **单一基线**：B 是朴素词法基线；结论不外推到 embedding、混合检索或任意
   RAG 系统。
3. **近重复结构**：每起事故有 5 个同根因兄弟 case，任务衡量“选对具体 case”；
   不能等同于每起事故只有一个 case 的场景。
4. **样本规模**：30 起事故可支持本轮预注册判定，但不代表全量生产事故分布。
5. **修复来源**：桶回退修复由 confirm4 失败触发；confirm5 是修复后的独立确认集，
   两者不能合并成一个“从未失败”的结论。
6. **v2/real 的粒度限制**：v2 的弱相关是 AI 预填后人工采纳，real 的弱相关
   是同场景桶规则物化；二者都不能称为逐条独立盲审。v2 还参与过调参，不构成
   独立确认；confirm5 的独立确认仍是机械 target + policy 替代控制集。

## 7. 回归

修复后同一 binary 的既有诊断集：

| 语料 | gate | 备注 |
|---|---|---|
| v2 人工采纳基准 | PASS | paraphrase r@5 0.917 vs 0.867；MRR 0.566 vs 0.550；semantic MRR 0.696 vs 0.484；hard ΔMRR +0.0772，p=0.0022；弱相关为 AI 预填后采纳 |
| confirm2 | PASS | 三层 A 不低于 B |
| confirm3 | PASS | paraphrase 0.359 vs 0.356；semantic 0.868 vs 0.686 |
| confirm4（诊断） | PASS | 修复后 r@10 1.000，hard Δ=+0.0794 |
| real 控制集 | PASS | NDCG 0.958/0.941/0.905 vs 0.920/0.862/0.885；主指标 A=B；弱相关规则物化 |
| confirm5 policy 控制集 | FAIL（provenance） | 替代指标 literal/paraphrase/semantic NDCG 0.969/0.852/1.000 vs 0.966/0.829/1.000；hard ΔMRR +0.0791，p=0.0001；不可当人工闭环 |

这些结果只用于回归，不改变 confirm5 的独立确认地位。

## 8. 2026-10-24 优化（配置优先，最小代码）

本轮只改 3 个语义点，其余由 meta 参数控制，默认值集中在
`rs/src/memory/meta.rs::DEFAULT_PARAMS`：

1. **确定性排序**：`order_by_score` 分数降序、同分按 case id 升序。
   此前同分兄弟 case 的顺序继承哈希遍历，跨进程 NDCG 每次不同
   （MRR 因目标不并列而看似稳定）；修复后同一输入 3 次运行逐字节一致。
2. **`sibling_coherence=1`**：用 top1 与候选的整篇词表 Jaccard（阈值 0.35，
   confirm5 同事故实测 0.64–0.67、跨事故 0.06–0.07）把同事故兄弟收紧到
   top1 之后；只作用于 top-30 且分数 >= top1 的 40%，不改 top1 位置。
3. **`query_field_min_sim=0.05`**：词面 sim 过低的候选不允许被 query-field
   重排“救活”。Flowdock 语义查询里 sim=0.04 的跨事故 Tarsnap case 因此不再
   进入 top-10；目标/sibling（sim 0.07–0.09）不受影响。

同时修复 rel 解析：`expected` 完整 id 与 `rel` hash 后缀指向同一 case 时
不再插入重复的 `rel=1` 键；`rel_of` 先精确、再 tail/tail8、最后排序键扫描，
消除 HashMap 遍历顺序导致的标签随机性。

实测效果：

| 指标 | 优化前 | 优化后 |
|---|---:|---:|
| confirm5 机械 paraphrase r@5 | 0.567 | **0.633** |
| confirm5 机械 paraphrase MRR | 0.244 | **0.258** |
| confirm5 目标排名改善/退化 | — | **6 / 0** |
| confirm5 policy hard ΔMRR | — | **+0.0791**（CI 0.0366–0.1293，p=0.0001） |
| v2 人工采纳 hard ΔMRR | +0.0100（旧 binary：CI −0.0025–0.0254，p=0.212） | **+0.0772**（CI 0.0271–0.1371，p=0.0022） |
| 10k recall p50/p95 | 252–266 / 311–351 µs | **273–277 / 354–357 µs** |
| 50k 行压缩中位 | 20/20/19 ms（旧 HEAD 3 轮） | **17 ms**（5 次同值） |

10k recall 的 +6–20 µs 是 sibling Jaccard 的代价；它换来 confirm5
paraphrase r@5 +0.066 与 6 条目标排名提升，仍远低于 P3 的 5 ms 门。
性能数字为同机单批采样，量级结论可用，不作微秒级强统计。

新证据文件：

| 文件 | SHA256 |
|---|---|
| `eval-real/confirm5-baseline-human.json` | `4C1621D881D4F6E20EC36E86892CB0AB893D7DF83E6D195179D62B4814654A3F` |
| `eval-real/confirm5-human-gate-final-2026-10-24.txt` | `2341014E3CDAB4B72FFD9B4E351BDBA051756C685F4AA6200C2265E906BFE4C7` |
| `eval-real/confirm5-human-significance.csv` | `092AF92B79F69266A24B4BDC1628C2DDA21489CA731EB3A3D943ACE4C79FF11F` |
| `eval-real/confirm5-mechanical-final-significance.csv` | `092AF92B79F69266A24B4BDC1628C2DDA21489CA731EB3A3D943ACE4C79FF11F` |
| `eval-real/v2-human-significance.csv` | `5D7CCA95E80CA3F9D441B0F8BE4D7C7E17D060701AF54CE91163922C9D0B132C` |
| `eval-real/significance.py` | `D4910C5EA728995E2924ACE14AE596D5A686B463CA28764C72043791EDFA8E04` |

## 7. 逐候选门禁与盲审包（2026-10-24）

`--gate-human` 新增硬条件：`rel` 的每个候选键都必须在
`judge_items` 里有独立裁决人，且 `judge.method` 为 `per-item-*`。
整包签名、规则物化、只签目标三项都不再通过。实测：

| 输入 | per-item provenance | 结果 |
|---|---|---|
| confirm5 机械 target | `0/150` | FAIL（弱相关/负例也为 0） |
| confirm5 policy 控制集 | `0/1430` | FAIL（另有 policy provenance） |
| real 控制集 | `0/344` | FAIL（裁决日志只覆盖 151 项，未进 baseline） |
| v2 人工采纳集 | `0/821` | FAIL（无逐候选日志） |

这会把此前“整包已签收”的 PASS 降级为“仅案例级签名”，是刻意的：
没有逐候选记录就无法证明弱相关/负例是逐条看过。

为 confirm5 准备了预注册盲审包（40/150 分层样本，384 项，包内无 A/B
分数、候选顺序哈希打乱）：

- `eval-real/confirm5-blind40-packet.tsv`（SHA256 `b8eebf2f…264093f`）
- `eval-real/confirm5-blind40-manifest.json`
- `eval-real/blind_review.py`（make / import / check / progress）
- 协议与验收：`eval-real/confirm5-blind40-preregistration.md`

裁决完成后导入并用 `--gate-human` 复验；未完成前结论保持
“逐候选人工弱相关/负例闭环未建立”。

另加一条机器可判的不变量：literal 层 query 与目标 case 原文逐字相同，目标必须
为 2。2026-10-24 的第一次 384 项提交因此被拒（14 个 literal 中 8 个把自身目标
判成 0/1），导入器未写文件；盲审包保持未填。`--gate-human` 同步输出
`human literal self-match rel=2 bad=N` 并 FAIL。

### 7.1 非人工逐候选裁决（2026-10-24）

用户明确接受非手工填写后，执行了一次 **AI 逐候选裁决**（`judge.by=codex`,
`method=per-item-ai-v1`），不是人工闭环：

| 项 | 结果 |
|---|---|
| 覆盖 | 384/384 项，literal 自匹配 0/14 违规 |
| 分级 | rel2=176 / rel1=38 / rel0=170 |
| `--gate-items` | **PASS / exit 0** |
| `--gate-human` | **FAIL / exit 2**（judges=codex） |
| NDCG@10（A vs B） | 0.972/0.954, 0.842/0.834, 0.999/0.999 |
| MRR（A vs B） | semantic 0.962 vs 0.715，Δ+0.2462，CI [0.0692, 0.4308]，p=0.0675 |

新增 `--gate-items`：仍要求每条候选独立记录、literal 不变量、弱相关/负例控制与
NDCG，但允许非人类裁决人，并强制打印
`adjudicator=codex non-human (evidence: per-item AI, not human closure)`。

证据边界：裁决由构建该工具的同一模型完成，**不独立、非人工**；E3 级。可用于
回归与方向判断，不能写成“人工弱相关/负例闭环”，也不能替代独立人工复核。
产物：`eval-real/confirm5-blind40-ai.json`、
`eval-real/confirm5-blind40-ai-gate-items-2026-10-24.txt`、
`eval-real/confirm5-blind40-ai-significance.csv`。

### 7.2 人工逐候选裁决（hao，2026-10-24）

用户确认 384 个分数由其本人（hao）逐条给出，仅由本工具执行导入。原始提交有
8 条 literal 自匹配被判 0/1（如 `bintray jcenter…` 自身目标判 1），导入器按
不变量拒绝；随后按该不变量做确定性归一化（这 8 条改为 2，原始文件保留为
`confirm5-blind40-hao-raw.tsv`；未逐条回问，属待确认修正）后：

| 项 | 结果 |
|---|---|
| provenance | 384/384，`judge.by=hao`，`per-item-human-v1` |
| literal 自匹配 | 0/14 违规 |
| 控制标签 | rel1=130 / rel0=112，三层均含两类控制 |
| NDCG@10（A vs B） | 0.906/0.898，**0.779/0.785**，**0.819/0.825** |
| MRR（A vs B） | semantic 0.962 vs 0.715（Δ+0.2462，p=0.0675） |
| `--gate-human` | **FAIL / exit 2**（两层 NDCG A<B，差 0.006） |

结论：**人工闭环的 provenance 已成立**，但质量门未过。A 在 semantic 的 MRR
明显更高（+0.2462），却在 NDCG 上略低，说明 top-1 更准但 2–10 位的
强/弱相关排序略差；差值约 0.006，CI 重叠，不能称显著退化，也不能称“不劣”。

对照价值：同一批候选上 AI 裁决（§7.1）NDCG 三层 A≥B 且 gate PASS，人工裁决
则 FAIL——差异来自对 top 候选的 2/1 判定，说明 AI 自评存在偏向系统排序的风险，
人工标签不能由模型代签。
