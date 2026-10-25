# 本地记忆库

状态：已实现（`scx 0.4.1`）。本文件同时承担设计、运维与变更记录，替代早期
`memory-design.md` / `memory-plan.md` / `memory-log.md` 三份阶段性文档。

记忆库是压缩引擎的旁路能力：`scx::compress` 仍是零 I/O 纯函数；记忆只影响
`--recall` 输出或召回排序，绝不改写当前输入的筛选正文。

## 1. 不变量

| 约束 | 含义 |
|---|---|
| 只删不改 | 压缩结果只包含当前输入原文行；记忆不向正文注入内容 |
| 纯函数引擎 | `compress` 不访问文件、环境变量、网络或时钟 |
| 零依赖 | 存储、JSON、索引、统计与评估均为本仓库实现 |
| 确定性 | 相同输入、库快照和参数产生相同输出；评估与标注固定种子 |
| 可重建 | `cases/` 是资产；`index/` 是可丢弃缓存，可随时 `--rebuild` |
| 显式失效 | case 写入、反馈和删除推进库代际，旧索引、qmap 与会话缓存自动失效 |

## 2. 代码结构

```
rs/src/memory/
├── mod.rs          # 唯一公开门面；内部模块不对外暴露
├── store.rs        # Case 模型、case.json 读写、目录枚举
├── meta.rs         # meta.toml、参数默认值、库代际与 L1 历史
├── ingest.rs       # 脱敏、指纹去重、摄取
├── field.rs        # 错误码/文件位置/符号/数字指纹抽取
├── bucket.rs       # db/network/disk/build/runtime 场景桶
├── recall.rs       # BM25 + 字段/实体加权 + 桶过滤 + PRF + 重排
├── ranking.rs      # BM25、PRF 扩展词选择与 MMR 多样性
├── synonyms.rs     # 中英领域词扩展
├── index.rs        # 倒排/实体索引、校验、候选提取与 postings 编解码
├── query_cache.rs  # 进程内会话快照 + 持久化 query 记忆图
├── edge.rs         # 小库线性路径的结构化实体一跳扩展
├── feedback.rs     # 状态机、质量分、词权与 L1 自适应
├── maintenance.rs  # rebuild / gc / delete / --stats 盘点
├── eval.rs         # 冻结 baseline 重放与指标
├── gate.rs         # 机械、逐项、人工证据门
├── annotate.rs     # 回归标注、盲测材料生成与逐候选裁决
├── json.rs         # 零依赖 JSON 子集
├── lock.rs         # 跨进程锁与原子写
└── util.rs         # FNV/UTC/编辑距离/脱敏/控制字符转义
```

公开 API 只从 `scx::memory` 导出：`store_case`、`recall_cases`、`rate_case`、
`read_case`、`rebuild`、`gc`、`delete_case`、`eval_cases` 以及少量查询与
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

`case.json` 采用固定字段顺序的紧凑 JSON；未知/缺失字段按默认值兼容读取。
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
  ├─ qmap 精确命中 → 编辑距离 ≤2 的轻量重排
  ├─ 候选集：线性扫描（小库）或倒排索引（≥10k）
  ├─ 场景桶过滤；低置信时回退全量候选，避免硬筛漏召
  ├─ BM25 + 结构化字段 + 实体边（索引路径复用 postings 快照）+ 质量/时间衰减
  ├─ 可选 query-field 重排与同事故兄弟收紧
  ├─ 低置信 PRF：扩展轮只补漏，不覆盖首轮顺序
  └─ 可选 MMR 去冗余
```

默认值为 `bm25_k1=1.2`、`bm25_b=0.75`；参数集中在 `meta.rs::DEFAULT_PARAMS`，
可在 `meta.toml [params]` 覆盖。索引候选携带 `Arc` 共享打分快照；query 的
df/idf/词权在每轮打分前只计算一次，宽查询不会为每个候选重复查表或开方。

`--recall-only` 严格只读：不创建库目录，不写 qmap、会话缓存或 L1 历史。

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

评估语料、冻结标签与复现命令见 [`eval/README.md`](../eval/README.md)，结果边界见
[`evaluation-results.md`](evaluation-results.md)。自动标注只能作为回归集；不能替代
独立人工盲审。`--gate-human` 失败是证据边界，不是可绕过的质量噪声。

## 8. 性能快照

Apple M5 Pro、release、零第三方依赖，2026-10-25 本机采样：

| 工作负载 | 结果 |
|---|---|
| 50k 行 / 2.34M 字符压缩 | 中位 12–13 ms（三轮 11–17 ms 样本） |
| 10k case，稀疏查询 | p50 204–207 µs，p95 236–292 µs（三轮） |
| 10k case，全库候选宽查询 | p50 1.9–2.1 ms（三轮） |
| 10k case，冷启动线性首查 | p50 约 0.97 s（三轮） |
| 10k case，全量重建索引 | 约 374–382 ms（三轮） |

复现：

```bash
cargo run --release --locked --manifest-path rs/Cargo.toml --example bench
cargo run --release --locked --manifest-path rs/Cargo.toml --example mem_bench
```

数字用于同机回归，不是跨机器承诺。`mem_bench` 同时报告线性冷启动、倒排召回与
全候选宽查询，避免只展示稀疏命中路径。

## 9. 变更记录

- 移除 `vec.bin`、重复的 BM25/PRF/qmap/cache 实现与 `model` 兼容层。
- 修复并发写入、同指纹去重、反馈丢失和索引失效窗口；原子写与跨进程锁统一。
- 修复低置信场景桶硬筛漏召；PRF 扩展轮改为严格补漏。
- 中文词表加入重叠 bigram/trigram；字段与实体加入结构化召回。
- `--recall-only` 去除初始化副作用；记忆内部模块收窄为私有。
- 会话快照并入 `query_cache`；qmap recency 改为真实 Unix 秒，旧格式自动作废。
- 索引打分快照改为 `Arc` 共享；query 的 df/idf/词权预计算，实体倒排由
  postings 快照派生，10k 全候选宽查询从约 15 ms 降至约 2.1 ms。
- 文本分类与归一化合并为 `analysis`；标注与裁决、CLI 记忆输出归并到单一所有者，
  公开模块数与重复逻辑继续收敛；词表去重改为单趟完成并复用 `build_doc_terms`。
- 移除历史 `eval-real/` 中间产物；可重建语料与冻结标签迁入 `eval/`。
