# 本地记忆库

状态：已实现（`scx 0.6.0`）。本文件同时承担设计、运维与变更记录，替代早期
`memory-design.md` / `memory-plan.md` / `memory-log.md` 三份阶段性文档。

记忆库是压缩引擎的旁路能力：`scx::compress` 仍是零 I/O 纯函数；记忆只影响
`--recall` 输出或召回排序，绝不改写当前输入的筛选正文。

## 1. 不变量

| 约束 | 含义 |
|---|---|
| 只删不改 | 压缩结果只包含当前输入原文行；记忆不向正文注入内容 |
| 纯函数引擎 | `compress` 不访问文件、环境变量、网络或时钟 |
| 零依赖 | 存储、JSON、索引、统计与评估均为本仓库实现 |
| 确定性 | 无随机源；固定时钟下相同 query、库快照和参数产生相同排序；评估与标注冻结时钟 |
| 可重建 | `cases/` 是资产；`index/` 是可丢弃缓存，可随时 `--rebuild` |
| 显式失效 | case 写入、反馈和删除推进库代际，旧索引、qmap 与会话缓存自动失效 |

## 2. 代码结构

```
src/memory/
├── mod.rs          # 唯一公开门面；内部模块不对外暴露
├── store.rs        # Case 模型、case.json 读写、目录枚举
├── meta.rs         # meta.toml、参数默认值、库代际与 L1 历史
├── ingest.rs       # 脱敏、指纹去重、摄取
├── classify.rs     # 判别字段抽取 + db/network/disk/build/runtime 场景桶
├── recall.rs       # BM25 + 字段/实体加权 + 桶过滤 + PRF + 重排
├── ranking.rs      # BM25、PRF 扩展词选择与 MMR 多样性
├── index.rs        # 倒排/实体索引、校验、候选提取与 postings 编解码
├── cache.rs        # 进程内会话快照 + 持久化 query 记忆图
├── edge.rs         # 小库线性路径的结构化实体一跳扩展
├── feedback.rs     # 状态机、质量分、词权与 L1 自适应
├── maintenance.rs  # rebuild / gc / delete / --stats 盘点
├── eval.rs         # 冻结 baseline 重放与指标
├── gate.rs         # 机械、逐项、人工证据门
├── annotate.rs     # 回归标注、盲测材料生成与逐候选裁决
├── json.rs         # 零依赖 JSON 子集
├── lock.rs         # 跨进程锁与原子写
└── util.rs         # FNV/UTC/脱敏/控制字符转义
```

切词、停用词、CJK 判定与中英领域同义词表统一归 `src/terms.rs`（压缩与记忆库共享）。

公开 API 只从 `scx::memory` 导出：`save`、`search`、`rate`、
`load`、`rebuild`、`gc`、`delete`、`evaluate` 以及少量查询与
维护辅助函数。实现模块保持私有，避免文件格式和内部缓存成为外部契约。

## 3. 磁盘布局

```text
<mem-dir>/
├── meta.toml                 # schema、case 数、代际、参数、DF 快照、L1 历史
├── cases/<date>_<hash16>/
│   └── case.json             # query、证据块、原始证据行、fix、字段、反馈
└── index/
    ├── postings.bin          # 倒排 + 实体索引 + 打分快照（可丢弃）
    ├── qmap.bin              # query 记忆图（可丢弃）
    ├── fingerprints.tsv      # 去重指纹 → case id（可丢弃）
    └── fingerprints.ready    # 指纹索引对应的 case 数（可丢弃）
```

`case.json` 采用固定字段顺序的紧凑 JSON；未知/缺失字段按默认值兼容读取，
读取上限 16 MiB（正文 1 MiB 加证据副本与最坏 JSON 转义后的安全余量）。
`postings.bin` 带版本、长度边界和 CRC32，坏段会报错并按需重建。
指纹索引按当前身份口径惰性重建，删除 case 后由 `ready` 标记失效。
`meta.toml` 是行式 `key = value` 子集，不是完整 TOML 实现。

## 4. 写入与去重

`--store 1` 的输入依次经过：

1. `sanitize`：脱敏已知的 password/token/Authorization/Bearer/URL 用户信息模式；
2. 指纹：query 保留数字身份，证据正文折叠数字噪声；
3. 同指纹查重：已有 case 只刷新时间和补录空 `fix`/证据，不覆盖已有结论；
4. 字段与场景桶抽取：错误码、文件位置、符号、数字指纹；
5. 原子写 `case.json`，推进库代际并使缓存失效。

限制：正文 ≤1 MiB，fix ≤4 KiB；`--max-bytes` 是写入前的软配额。脱敏不是通用 DLP。

## 5. 召回管线

```
query
  ├─ 会话缓存（进程内，30 分钟，带库代际）
  ├─ qmap 精确命中 → 直接还原 top-k 快照（变体 query 走 BM25）
  ├─ 候选集：线性扫描（小库）或倒排索引（≥10k）
  ├─ 场景桶过滤；低置信时回退全量候选，避免硬筛漏召
  ├─ BM25 + 结构化字段 + 实体边（索引路径复用 postings 快照）+ 质量/时间衰减
  ├─ 可选 query-field 重排与同事故兄弟收紧
  ├─ 低置信 PRF：扩展轮只补漏，不覆盖首轮顺序
  └─ 可选 MMR 去冗余
```

会话缓存与 qmap 都按请求的 `K` 复用：快照条数不足 `K` 时回退完整召回，
因此先用 `--recall 1` 暖缓存不会截断后续 `--recall 5`；同代际下保留更大的
快照，较小 K 查询不会覆盖它。qmap 快照另有 1 小时 TTL：最短时间衰减常数为
30 天，1 小时旧快照造成的相对分数漂移上界约 0.14%，同时覆盖同一排障会话内
的重复 CLI 查询；过期后重新走 BM25。

默认值为 `bm25_k1=1.2`、`bm25_b=0.75`；参数集中在 `meta.rs::DEFAULT_PARAMS`，
可在 `meta.toml [params]` 覆盖。索引候选携带 `Arc` 共享打分快照；query 的
df/idf/词权在每轮打分前只计算一次，宽查询不会为每个候选重复查表或开方。
线性与索引两条路径的 df/avgdl 均为全库口径，桶过滤只裁候选，不改变 BM25
统计量。

手工修改 `meta.toml [params]` 后，精确 query 快照（qmap）与进程内会话缓存仍按
库代际判断有效性，代际不会因手改文件自动前进。修改检索参数后请删除
`index/qmap.bin` 并重启进程，或换用新的库目录；`--rebuild` 负责重建
postings/df，不承诺清除 qmap。

`--recall-only` 严格只读：不创建库目录，不写 qmap、会话缓存、索引或 L1 历史。
索引缺失或损坏时直接回退线性扫描；需要自愈缓存时运行普通召回或 `--rebuild`。

## 6. 反馈与置信

- `--rate good|bad <id>`：更新 `good/bad`、`quality` 和状态机
  `open → solved → regressed`。
- 词级调权：good 词增量、bad 词减量，默认限制在 ±0.5。
- L1 自适应：学习路径记录 top1 相似度；反馈后以历史中位数更新 `l1_base`。
- 主流程 `--recall` 只有在 top1 达基线且领先 top2 足够时才输出 `mem:` 段；
  `--recall-detail` 额外显示摘要与原始证据，`fix:` 始终标注为候选答案。

## 7. 维护与评估

| 命令 | 作用 |
|---|---|
| `--stats` | case/桶/状态/fix 覆盖率/质量/磁盘占用 |
| `--rebuild` | 重建倒排、合并 qmap、写 DF 快照 |
| `--gc` | 清理长期 regressed 或陈旧且无 fix 的记录 |
| `--delete <id>` | 原子删除 case 并使缓存失效 |
| `--eval <baseline.json>` | 冻结标注 A/B 重放 |
| `--gate` / `--gate-items` / `--gate-human` | 机械 / 非人工逐项 / 人工证据门 |
| `--annotate` / `--annotate-raw` | 回归标注 / 盲测材料 |
| `--judge` | 逐候选交互裁决 |

评估语料、冻结标签与复现命令见 [`eval.md`](eval.md)，结果边界见
[`evaluation-results.md`](evaluation-results.md)。自动标注只能作为回归集；不能替代
独立人工盲审。`--gate-human` 失败是证据边界，不是可绕过的质量噪声。

## 8. 性能快照

Apple M5 Pro、release、零第三方依赖，2026-10-25 本机采样：

| 工作负载 | 结果 |
|---|---|
| 50k 行 / 2.34M 字符压缩 | 中位 11.7–12.0 ms（三轮 × 21 样本） |
| 10k case，稀疏查询 | p50 71–73 µs（三轮 × 21 样本） |
| 10k case，首次索引加载 | 11.6–11.8 ms（单独预热，不计入稳态 p99） |
| 10k case，全库候选宽查询 | p50 1.11–1.15 ms（三轮 × 7 样本） |
| 10k case，冷启动线性首查 | p50 0.95–0.97 s（三轮，负载敏感） |
| 10k case，全量重建索引 | 373–388 ms（三轮） |

复现：

```bash
cargo run --release --locked --example bench
cargo run --release --locked --example bench -- memory
```

数字用于同机回归，不是跨机器承诺。与同一会话中的 HEAD 基线交错测量：压缩
12.7–12.8 ms → 11.7–12.0 ms（约 6–8%）；稀疏召回 163–177 µs → 71–73 µs
（约 −57%）；宽查询 1.17–1.39 ms → 1.11–1.15 ms（差异受负载影响）；
冷启动线性首查 1.01–1.03 s → 0.95–0.97 s。
稀疏提速来自“先部分选择 top-k，再只排序 3k 候选窗口”；冷启动收益来自 case 读取
不再克隆整棵 JSON；宽查询仍以逐候选打分为主。
冷启动线性首查受系统负载与页缓存影响明显，表中给出三轮实测跨度。`bench -- memory`
单独报告索引首次加载，并同时测量线性冷启动、稳态倒排召回与全候选宽查询，
避免把索引 I/O 混入查询分布。基准库由 200 条种子复制为 1 万条，用于稳定测量
实现开销，不代表真实事故的规模与分布。

## 9. 变更记录

- 移除 `vec.bin`、重复的 BM25/PRF/qmap/cache 实现与 `model` 兼容层。
- 修复并发写入、同指纹去重、反馈丢失和索引失效窗口；原子写与跨进程锁统一。
- 修复低置信场景桶硬筛漏召；PRF 扩展轮改为严格补漏。
- 中文词表加入重叠 bigram/trigram；字段与实体加入结构化召回。
- `--recall-only` 去除初始化副作用；记忆内部模块收窄为私有。
- 会话快照并入 `cache`；qmap recency 改为真实 Unix 秒，旧格式自动作废。
- 索引打分快照改为 `Arc` 共享；query 的 df/idf/词权预计算，实体倒排由
  postings 快照派生，10k 全候选宽查询从约 15 ms 降至约 2.1 ms。
- 召回入口只解析一次 `meta.toml`，评估与标注复用同一快照；会话快照写入不再
  克隆 top-k 列表，稀疏召回 p50 从约 213–219 µs 降至约 170–195 µs。
- 召回 query 的切词、同义扩展、结构化字段与实体键归并为单次 `QueryPlan`；
  候选只保存文档下标、排序后再物化前 k 个 id，索引快照预解析 `updated_at`。
  10k 全候选宽查询从约 2.23 ms 降至约 1.17 ms；该轮稀疏路径为 172–177 µs
  （后续部分选择再降至 71–73 µs）。Gauge、v2、real 的线性与索引 A/B
  输出均逐字节不变。
- qmap 与会话快照按请求的 `k` 复用：条数不足 `k` 的快照一律回退完整召回，
  修复“先用 `--recall 1` 暖缓存、再用 `--recall 5` 只得到 1 条”的截断缺陷；
  同代际下保留更大的快照并刷新会话窗口，较小 k 查询不再覆盖它；`--rebuild`
  合并时同一秒内按写入顺序保留后写者。
- qmap 对同 query/代际、同前缀或更大 top-k 的重复快照做幂等跳过，重复 CLI 召回
  不再让 append-only 文件无限增长；分数变化仍追加。快照只复用 1 小时，避免
  时间衰减排序长期陈旧。`load` 以目录 id 为唯一资产身份，JSON 反序列化
  不再克隆整棵树或读取时钟，并对 `case.json` 设置 16 MiB 读取上限。
- 召回排序改为部分选择：默认只需 top-k（同族收紧最多看 3k），先
  `select_nth_unstable_by` 再排序候选窗口；MMR 开启时保留完整顺序。
  10k 稀疏召回 p50 从 161–169 µs 降至 67–69 µs。
- 压缩的 token 估算并入逐行分析循环；`--context 0` 的大输入不再为预算做
  整份 token 扫描。50k 行压缩中位 12.7–12.8 ms → 11.7–12.0 ms（约 6–8%）。
- 文本分类与归一化合并为 `analysis`；判别字段与场景桶合并为 `classify`
  （同一份“文本 → 稳定检索标签”口径）；标注与裁决、CLI 记忆输出归并到单一
  所有者，公开模块数与重复逻辑继续收敛；词表去重改为单趟完成并复用
  `build_doc_terms`。
- 移除历史 `eval-real/` 中间产物；可重建语料与冻结标签迁入 `config/`。
- `scx.py seed` 改为 stdin 字节输入 + 可配置并行 store，消除逐 case 临时文件；
  CI 对比串行/并行 CSV，语料与 `--eval-verbose` 解析增加单行、重复与覆盖校验。
- **记忆分层 + 晋升门控（P1，#69，2026-10-31）**：`Case` 新增 `tier: u8`（0/1/2，
  缺键兼容回退 L0）+ `tier_at`；新增 `tier.rs::promote` 只升不降门控
  （L0→L1：evidence 非空且 fix 非空；L1→L2：good ≥ 3 且 quality ≥ 0.4）。
  写入/补录/反馈三接入点；召回按 tier 先验加权（`tier_prior_l2/l1`，L0 不加，
  兜底展示不直答）。postings.bin 编码新增单字节 tier（POST_VER 3→4，旧缓存
  自动作废重建）；`--stats` 新增 `tier: l0=l1=l2=` 分布行。
- **冲突仲裁（P2，#70，2026-10-31）**：新增 `conflict.rs`，写入新 case 落盘前做
  minhash 近重复检测（加权 Jaccard，同桶候选优先、上限 `conflict_max=512`，
  `sim ≥ conflict_sim=0.8` 触发仲裁）。裁决：证据更全者胜 → quality 更高者胜 →
  先到先得（已存在 case 胜）；**不删除数据**，双方 `related` 双向互链
  （`Case.related` 新增字段，旧 case 缺键兼容为空；库内一侧加锁落盘，新 case 一侧
  内存 push 随写入落盘，避免对未落盘 case 重复加锁）；每次仲裁 append 一行
  `index/conflicts.log`（TSV 8 列：stamp/winner/loser/w_ev/l_ev/w_q/l_q/sim，
  原子追加，确定性无时钟源）。`--stats` 新增 `conflicts=` 计数行（读 log 行数）。

## L1 直答评测协议（fix 覆盖率 / 命中率口径）

L1 直答是召回后的确定性筛选层：`memory::hits` 在 `search` 命中后，
仅当 top1 满足 `feedback::qualified(top1_sim, top2_sim, l1_base,
l1_margin_min, ext_hit=1.0)` —— `top1_sim ≥ l1_base && (top1_sim - top2_sim)
≥ margin && ext_hit ≥ 0.30` —— 才保留命中（直答），否则清空候选退回普通
候选列表展示（不直答）。`l1_base` / `l1_margin_min` 由 meta.toml 提供，
缺省 `(0.55, 0.15)`，可随反馈自适应（见 feedback.rs 的 L1 历史学习）。

### 指标口径（全部可机械计算）

- **fix 覆盖率**：库内已 store 的 case 中，`fix` 非空的比例。L1 直答输出
  依赖 fix 覆盖——无 fix 的 case 只能作为候选展示，不能作为答案直答。
  口径：`fix 非空 case 数 / 全部 case 数`，case 以目录 id 为唯一资产身份。
- **直答触发率**：评测查询集中，`recall top1` 满足 `qualified` 的查询
  比例（分母为全部评测查询，含无命中查询）。
- **直答命中率**：触发直答的查询中，top1 的 case id 与冻结标签的
  expected tail16 匹配（指纹匹配）或人工 rel=2 判中的比例。**分子/分母
  均只统计触发直答的查询**；未触发直答的查询不计入（避免稀释口径）。
- **MRR 回归下界**：Gauge 机械 hard 层 ΔMRR ≥ 0（相对基线不为负），
  CI 下界不为负——L1 直答不得让词面召回退化（直答只删弱候选，不改排序）。

### 复现（命令与 eval.md 同构）

```bash
# 1) 建库并灌入 Gauge 语料（fix 标签在 corpus 内，随 case 入库）
./tools/scx.py seed config/gauge.corpus.json --mem-dir /tmp/scx-l1   --csv /tmp/l1.csv --force
# 2) 机械 hard 层目标基线（judge-by 由确认轮次决定，照抄 eval.md 写法）
./tools/scx.py baseline --csv /tmp/l1.csv   --out /tmp/l1-mechanical.json   --judge-by mechanical-target-gauge-v1
# 3) 显著性统计（mrr 为主指标；--mem-dir 用于分层）
./tools/scx.py stats --baseline /tmp/l1-mechanical.json   --verbose /tmp/l1-verbose.csv --out /tmp/l1-stats.csv
# 4) fix 覆盖率：库内 fix 非空 case 占比（scx --stats 或直接统计 mem-dir）
# 5) 直答触发/命中：对 Gauge 查询逐条跑 --recall 并核对 top1 tail16
```

> 直答触发率 / 命中率没有独立 CLI 汇总入口，用 `scx --recall 5 --recall-detail`
> 逐查询输出 `candidate, sim=..` 行 + 冻结标签对拍即可机械核算（每查询
> top1.sim 与 `--eval-verbose` 同口径）。

> 口径声明：本协议为**作者侧自测**（语料为合成/半合成事故集，标签为作者
> 冻结），非第三方独立评测；不用于对外发布级对比。ext_hit 在 CLI 门面恒为
> 1.0（`hits` 调用处固定），评测不模拟外部扩展命中。

- **RRF 融合召回 + 记忆类型（P4，#72，2026-10-31）**：`search_fused` 由线性加权
  改为**纯 RRF 秩融合**（k=60，零依赖）：BM25 秩 + 字段/实体秩（query 与 case
  侧 `classify::keys` 交集计数，全库零命中该路 None 不参与）+ minhash 语义秩 +
  外部 embed 秩（插拔），融合分值为四路秩和无权重乘积，`embed_weight/
  minhash_weight` 降级为路开关。`Case` 新增 `kind: String`（缺键回退
  "situation"），写入自动分类（procedure=≥2 行命令状行 / experience=error_code
  非空且 fix 非空 / semantic=定义标记 / situation 兜底；命令状行=行首空白 +
  22 词白名单）。新增 `filter_kind()`（剔除非 active 与 kind 不匹配、保序只减
  不增）；CLI `--recall-kind <k>` 全链路接入（word() free 白名单曾遗漏致值误
  判 --delete，已修复+回归测试）。`--stats` 新增 `kind:` 分布行。门禁 394 passed
  / clippy 0 / fmt 干净，CLI 四类摄取、kind 过滤双向、--recall-only 组合实测通过。
- **版本化 Revise/Retire（P3，#71，2026-10-31）**：`Case` 新增 `version: u32`、
  `life`（active/revised/retired，计划稿 `status` 与反馈状态机撞名故改
  `life`+`life_u8()`）、`supersedes`/`superseded_by`、`retire_reason`（缺键兼容）。
  `--revise <case-id>`（须配 `--store 1`）先写替换者（version 继承 +1、
  supersedes=旧 id）再原子标记旧 case revised+superseded_by（幂等、同指纹
  不自指、retired 拒改）；`--retire <case-id> --retire-reason <text>` 仅 tombstone
  （幂等、reason sanitize）。召回（索引与线性路径）过滤 revised/retired，
  df/avgdl 口径不变；postings 编码新增单字节 life（POST_VER 4→5 自动重建）；
  `--stats` 新增 `life: active=revised=retired=` 分布行。门禁 387 passed /
  clippy 0 / fmt 干净，CLI 全链路实测通过。
