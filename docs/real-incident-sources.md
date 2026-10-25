# 真实历史事故语料来源（公开复盘）

本文档是 `eval/corpora/real.json` 真实事故语料库的**来源清单与出处**。
每一份入库 case 都派生自下列公开复盘，改写/引用，不编造事故与原始日志。
入库文本中标注 `公开复盘, 来源: <URL>`，原始日志不存在于本项目。

## 使用约定

- 每条 case 的 `text` 第一行为症状摘要（供召回与盲测裁决展示），第二行起为来源 URL 与根因摘要。
- 本文档记录 URL、日期、根因摘要，供 provenance 审计（M5 / VF3 可复现）。
- 语料目的：检验 `scx` 记忆库在真实事故分布上的召回与排序质量，不声称复现事故现场。

## 来源清单（11 起，均已公开）

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

## 派生 case 覆盖

语料共 40 条 case，覆盖上表 11 起事故，层分布为
literal 14 / paraphrase 13 / semantic 13。逐条 query、来源与根因 mapping
见 `eval/corpora/real.json`；最终案例级标签见 `eval/labels/real.json`。
