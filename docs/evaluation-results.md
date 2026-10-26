# 评估结果与证据边界

更新：2026-10-25。输入、冻结标签、盲审包与复现工具在 `config/`；详细命令见
[`eval.md`](eval.md)。旧 `eval-real/` 中间产物已删除，历史文件可从
git commit `5ada90d` 恢复。

结论先行：**Gauge 的预注册机械目标排序优势成立；Gauge 的 40/150 逐候选人工
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
| Gauge | 30 起新事故，150 查询 | hard 层 MRR 优势（机械 target） | 弱相关/负例人工闭环（机械 target 全 2） |
| Gauge + hao | 40/150 逐候选人工标签 | 当前 `--gate-human` 点估计 PASS | 独立 holdout；NDCG 显著优势 |
| real | 40 条、11 起公开事故 | 案例级双人标签、NDCG 方向 | 逐候选人工门（0/344） |

## 2. Gauge：预注册机械确认

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

`config/gauge.policy.labels.json` 在同一 150 条查询上物化规则：同事故 `rel=2`，
同场景桶 `rel=1`，其余 `rel=0`；分布 `2=650 / 1=115 / 0=665`，
`judge.method=policy-materialized-v1`。

hard ΔMRR **+0.0608**，CI [0.0202, 0.1095]，p=0.0029；`--gate-human` 仍
FAIL / exit 2。只能把它当作替代控制集，不能写成逐条人工盲审：

```text
gate: human provenance policy-materialized labels=150 (not per-item human review) FAIL
gate: FAIL
```

### 2.2 逐候选人工验收（hao，40/150）

- 盲审包：`config/gauge.blind40.packet.tsv`，40 例 / 384 候选，SHA256
  `b8eebf2fcb78d0fb745c9760b55dd34d832a9bb976b377f3dfb8c7cea264093f`。
- 人工标签：`config/gauge.human.labels.json`，`judge.by=hao`，
  `method=per-item-human-v1`；rel 分布 `2=142 / 1=130 / 0=112`。
- `--gate-human`：**PASS / exit 0**；literal 自匹配 0/14 违规；
  provenance 384/384、cases 40/40。
- NDCG@10：literal/paraphrase/semantic 为 0.906/0.785/0.828 vs
  0.898/0.785/0.825；semantic MRR 0.872 vs 0.715（Δ+0.1564，
  CI [0.0205, 0.3282]，p=0.1261）。
- NDCG 差值的 semantic bootstrap：+0.0023，CI [-0.0131, 0.0161]；因此不能
  写“NDCG 显著优于 B”。

AI 标签 `config/gauge.ai.labels.json`（`per-item-ai-v1`，384/384）只通过
`--gate-items`，`--gate-human` 必须失败。配置是在 hao 标签上校准的，不能把
40/150 的结果外推成全量或独立 holdout。

### 2.3 冻结哈希

| 对象 | SHA256 |
|---|---|
| `config/gauge.corpus.json` | `B83A3A8C834A6494BDA76777BFD7500488CEDFE097C81D2AFA5E03049E543335` |
| `config/gauge.policy.labels.json` | `EDF6C2083D77F7A39A2DB1DCC8C13614AD55E5D78A832BBC6F93B449E1BD6F33` |
| `config/gauge.ai.labels.json` | `3114B9152835D604BD06F1B1D4E54A327B8857E9650E08111B04F452D965508A` |
| `config/gauge.human.labels.json` | `C68573E0640EADD29C843F399C986ADB69194285A49AB1A5201D9D408A83498C` |
| `config/gauge.blind40.packet.tsv` | `B8EEBF2FCB78D0FB745C9760B55DD34D832A9BB976B377F3DFB8C7CEA264093F` |
| 派生 AI 填写版（packet + ai labels） | `6A5319C546FE68C20F3D4D7FFD38437B3736EA301E1319AFB3C374ECA9AC9B18` |
| 派生人工填写版（packet + hao labels） | `1F6CD83576DD3872635C2317131F8CE7858301E7AFE88D490B8CE46E1B9240B5` |

两个填写版是派生视图，不再入库；`blind check --packet ... --out ...` 可逐字节
重建并由 labels 中的 `packet_sha256` 自动校验。

复现入口：

```bash
./tools/scx.py seed config/gauge.corpus.json \
  --mem-dir /tmp/scx-gauge --csv /tmp/scx-gauge.csv --force
./tools/scx --eval config/gauge.human.labels.json --gate-human --eval-verbose \
  --mem-dir /tmp/scx-gauge
```

`seed` 默认 4 个并发 store 进程；CI 同时串行重建，归一化日期前缀后对比整个
CSV，确保并行只缩短准备时间，不改变 case 身份或输出顺序。

## 3. v2：开发集

v2 有 30 起公开事故、150 条查询（60/60/30），最终标签
`config/v2.labels.json` 为 `judge.by=fei+hao`，案例级签名；分布
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
./tools/scx.py seed config/v2.corpus.json \
  --mem-dir /tmp/scx-v2 --csv /tmp/scx-v2.csv --force
./tools/scx --eval config/v2.labels.json --gate --mem-dir /tmp/scx-v2
```

## 4. real：真实事故案例级证据

40 条 case 来自 11 起公开复盘（literal 14 / paraphrase 13 / semantic 13）；
逐条 URL 在 `config/real.corpus.json`，来源清单见下节。标签
`config/real.labels.json` 为 `judge.by=fei+hao`，
分布 `2=147 / 1=151 / 0=46`；弱相关是场景桶规则物化，不是逐条盲审。

当前默认配置：三层 r@5/r@10 与 MRR 均为 A=B；NDCG A>B 但 CI 重叠；
`--gate-human` 因 `judge_items` 缺失在 0/344 处 FAIL / exit 2。该失败是证据边界，
不是指标回归。复现：

```bash
./tools/scx.py seed config/real.corpus.json \
  --mem-dir /tmp/scx-real --csv /tmp/scx-real.csv --force
./tools/scx --eval config/real.labels.json --gate-human --mem-dir /tmp/scx-real
```

### 4.1 公开事故来源

每份 case 派生自下列公开复盘，改写/引用，不编造事故与原始日志。入库文本标注
`公开复盘, 来源: <URL>`；原始日志不存在于本项目。

- 每条 case 的 `text` 首行是症状摘要，之后是来源 URL 与根因摘要；
- URL、日期、根因摘要用于 provenance 审计与语料复现；
- 语料检验真实事故分布上的召回/排序，不声称复现事故现场。

| # | 事故 | 日期 | 来源 URL | 根因摘要 |
|---|------|------|----------|----------|
| 1 | Cloudflare 全球中断 | 2019-07-02 | <https://blog.cloudflare.com/details-of-the-cloudflare-outage-on-july-2-2019/> | WAF 新规则中正则回溯导致全球边缘节点 CPU 100%，约 27 分钟返回 502。 |
| 2 | GitLab.com 数据库事故 | 2017-01-31 | <https://about.gitlab.com/blog/2017/02/10/postmortem-of-database-outage-of-january-31/> | 复制故障叠加误删主库数据目录，约 6 小时数据丢失。 |
| 3 | GitHub 服务降级 | 2018-10-21 | <https://github.blog/news-insights/company-news/oct21-post-incident-analysis/> | 网络分区触发 MySQL 主从裂脑与元数据不一致，约 24 小时降级。 |
| 4 | Reddit Pi-Day 中断 | 2023-03-14 | <https://www.reddit.com/r/RedditEng/comments/11xx5o0/you_broke_reddit_the_piday_outage/> | Kubernetes 1.23→1.24 升级与 route reflector 标签重命名冲突，致全站中断。 |
| 5 | Fastly 全球中断 | 2021-06-08 | <https://www.fastly.com/blog/summary-of-june-8-outage> | 合法客户配置变更触发未发现软件缺陷，85% 网络约 50 分钟报错。 |
| 6 | Atlassian 云中断 | 2022-04-05 | <https://www.atlassian.com/blog/company-news/post-incident-review-april-2022-outage> | 维护脚本以错误执行模式与错误 ID 列表误删客户站点数据。 |
| 7 | Facebook 全球中断 | 2021-10-04 | <https://engineering.fb.com/2021/10/05/networking-traffic/outage-details/> | 骨干维护命令导致 BGP 路由撤回、DNS 不可用，约 6 小时。 |
| 8 | Slack DNSSEC 中断 | 2021-09-30 | <https://slack.engineering/what-happened-during-slacks-dnssec-rollout/> | DNSSEC 上线触发 Route53 通配符缺陷与 NSEC 错误，SERVFAIL 约 24 小时（影响 <1% 用户）。 |
| 9 | AWS EC2/EBS 中断 | 2011-04-21 | <https://aws.amazon.com/message/65648/> | us-east-1 网络事件引发 EBS re-mirroring 风暴与容量耗尽。 |
| 10 | Google Cloud 网络中断 | 2019-06-02 | <https://status.cloud.google.com/incident/cloud-networking/19008> | 配置变更叠加软件缺陷导致多区域网络拥塞约 4 小时。 |
| 11 | Azure AD 认证故障 | 2021-03-15 | <https://azure.status.microsoft.com/en-us/status/history/> | 签名密钥轮换错误移除关键密钥，令牌校验失败约 14 小时。 |

## 5. 已成立与必须拒绝

已成立：

- `--annotate` 确定性、可被 `--eval` 重放；
- Recall/MRR/NDCG、分层聚合与 bootstrap 可重放；
- `--gate`、`--gate-human`、`--gate-items` 把样本、provenance、控制标签和
  指标门槛变成可执行门；
- Gauge 机械 target 的 hard 层优势满足预注册条件；
- Gauge hao 子集通过当前人工门，但只支持事后验收。

必须拒绝的表述：

- “A 对所有 RAG/embedding 系统有优势”：B 只是朴素词面基线；
- “Gauge 机械 target 已通过人工弱相关/负例门”：rel 全 2；
- “policy 控制集等于逐条人工盲审”：规则物化，gate 以 provenance FAIL；
- “v2 独立确认质量优势”：v2 参与调参，标签为 AI 预填后人工采纳；
- “real 建立 Recall/MRR 优势”：三层 A=B；
- “Gauge hao 是独立 holdout”：同一批标签参与配置校准；
- “NDCG 显著优于 B”：semantic NDCG CI 跨 0；
- “AI 标签可以冒充人工”：AI 只能过 `--gate-items`。

## 6. 方法学约束

1. 自检即选择：`--annotate` 用 A 管线放行变体，只能做回归。
2. 主指标、MDE、CI 方法和 seed 必须在看结果前冻结。
3. 按层/tier/cluster 反复看会引入多重比较；报告全部组并区分预注册与敏感性。
4. 机械 target、policy 物化、AI 预填、案例级签名、逐候选真人是五种不同证据。
5. Gauge 的 5 条兄弟 case 相关，case 级 bootstrap 应同时报告事故聚类敏感性。
6. 配置校准集不能当确认集；任何优势声明都要新的独立语料与逐候选人工标签。
