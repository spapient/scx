# scx 性能与能力深度优化分析（多专家联合）

> 版本：v2（2026-11-10）。状态：**实施完成，全门禁双轨绿、golden 零漂移**。
> 方法：只读源码调查，所有结论标注证据等级——`confirmed`（源码/文档直接表达）、
> `inferred`（基于结构的推断，需实测）、`unknown`（证据不足）。
> 配套：事件自动化总方案见 `docs/automation.md`，本文的实施项映射到其阶段门。
>
> 回填摘要：P1/P3 由 `src/par.rs` 统一并行度；P2 经核实进程内索引缓存已存在
> （`src/memory/index.rs`，`(SystemTime,len)` 键 + Mutex 兜底）；P4 校准冻结
> 1.787；A1 哈希 `step` 同源；C1 `--word-boundary` 落地；A2 行谓词合并为
> `mark_by_predicate`+`MarkOut`。

## 1. 现状基线（先承认它已经很强）

| 指标 | 已实测值 | 来源 |
|---|---|---|
| 50k 行压缩中位 | 6,048 µs（50ms 回归门 PASS） | docs/benchmarks.md:22 |
| 稀疏召回 p50 | 135 µs（较新口径 71–73 µs） | docs/benchmarks.md:46,56 |
| 全候选宽查询 p50 | 3.299 ms，性能门 <5ms PASS | docs/benchmarks.md:50-51 |
| 全量重建索引 | 373–388 ms | docs/benchmarks.md:47 |
| 70 例重放 | 100% 确定性，准确率 82.01% | docs/benchmarks.md:74 |

结论：常规热路径已无明显"低垂果实"，**真实提升点集中在三处：大输入的并行口径、
冷启动/索引加载延迟、以及通过表驱动合并带来的代码精简**。

## 2. 性能专家视角

### P1. 并行线程数硬编码 12，不随核数伸缩 — `inferred` ✅ 已落地

- 证据：`src/compress/plan.rs` 原硬编码 `12`；同仓 `src/sem.rs` 已使用
  `std::thread::available_parallelism()`，两套口径不一致。
- 问题：在 4 核迷你机/小核 VM 上仍派生 12 线程，造成过度订阅与调度开销；在更多
  核机器上又吃不满。
- 落地：新建 `src/par.rs`，`par::count()` = `available_parallelism()`（CAP=32，
  失败回退 1），plan.rs 与 sem.rs 全部改调它；分块保序，输出逐字节确定。

### P2. MCP 长驻进程内未缓存索引 — `inferred` ✅ 核实：缓存已存在

- 证据：冷启动线性首查 0.95–1.765 s、索引首次加载 11.6–14.5 ms
  （docs/benchmarks.md:46,48）。
- 核实结果：`src/memory/index.rs` 已实现进程级缓存
  `OnceLock<Mutex<Option<Entry>>>`，键 `(SystemTime, len)`，`load(root)` 命中即
  返回 `Arc<Index>`，含 NotFound 与并发重建兜底；`src/memory/cache.rs` 另有
  QMAP 查询记忆（gen 代际失效）。无需重复建设，保持现状。

### P3. 并行脚手架重复 — `confirmed` ✅ 口径已统一

- 证据：`plan.rs` 有 `split_parts` + 多处 `thread::scope`；`render.rs` 另有一套
  scope；`tally`/`tally_par` 同构双份。
- 落地：线程数统一改调 `par::count()`（P1），消除两套并行口径；分块保序结构
  保持，golden 零漂移。

### P4. 安全系数 2.0 是标注在案的临时值 — `confirmed` ✅ 已校准冻结

- 证据：`src/terms.rs` 原明确写"未经留出分位数校准……重跑 `scx-tooling
  calibrate` 冻结真实 P95 比率后再收紧"。
- 落地：运行独立二进制 `scx-tooling calibrate`（内置 cl100k_base 真值，370
  样本）：P50=1.215，P90/P95/P99/P100=1.787；据此把 `BUDGET_SAFETY_FACTOR`
  从 2.0 冻结为 **1.787**，保守余量收紧、安全保证不降级，golden 保持绿。

## 3. 算法与能力专家视角

### C1. 子串匹配缺词边界 — `confirmed` ✅ 已落地（默认关）

- 证据：`plan.rs` 对每个锚词直接 `ll.contains(t)`。短词存在跨词命中，
  如 `"error"` 命中 `"terror"` 类子串。
- 落地：Config 新增 `word_boundary: bool`（policy 默认 false，零漂移），配套
  flag/help/MCP 接入；plan.rs 新增 `term_hit(ll, t, word_boundary)`，边界用
  `is_ascii_alphanumeric` 判定，非 ASCII 锚词退化为子串匹配；含 term_hit 单测。

### C2. 锚词扩展 O(n²) 去重 — `confirmed` ➖ 评估后保留现状

- 证据：`src/compress.rs:150` 用 `v.contains(&f)` 线性去重；词表上限 256
  （terms.rs），实测开销可忽略。
- 结论：顺序保持的 HashSet 去重需额外结构，收益不抵复杂度，保留线性去重。

### C3. 恒等统计与全事件触发 — 交叉引用

- 已在 `docs/automation.md` §4–§5 设计（S1/S3），本分析不重复；其统一统计模型
  同时服务这里的校准结果展示。

## 4. 架构与精简专家视角

| 编号 | 重复/冗余（confirmed） | 落地情况 |
|---|---|---|
| A1 | `compress::hash::Hasher` 与 `plan::Finger` 是同一乘法散列、仅种子不同 | ✅ plan.rs 抽出 `SEED_A/SEED_B`+`step`，Finger 复用，compress.rs 哈希模块改调 `plan::step` |
| A2 | `score.rs` 中 num/sym/zh/list 行谓词同构：谓词判定 + 写 keep/reason | ✅ 合并为 `mark_by_predicate`+`MarkOut` 统一骨架（score.rs 净 −50/+68 行内含新骨架，4 个包装各约 10 行） |
| A3 | setup 三个注册函数 | ✅ HookSpec 事件表 + merge_hooks 幂等写入（automation S2） |
| A4 | 统计渲染三处：`ops::recall_report`、`meter::render`、hook 拼装 | ✅ 统一 `src/stat.rs` 模型 + `Stat::card` 单渲染器（automation S3） |
| A5 | `args.rs` 全仓最大文件；Flag 表与 parse 分支偏长 | ➖ 维持现状，风险收益不匹配，后续单独小步处理 |

注：A 列原行数为结构推断的方向估计，实际以 gate 与 golden 零漂移为准，
不把估计当承诺。

## 5. 优先级与阶段映射

| 优先级 | 项 | 理由 | 映射阶段 |
|---|---|---|---|
| 1 | P4 校准、P2 索引缓存 | 直接提升能力/延迟，语义不变 | S3 统计同步、独立门 |
| 2 | P1+P3 并行工具统一、A1 哈希合并 | 一处改动同时减代码与统一口径 | S1/S4 |
| 3 | C1 词边界、C2 去重 | 质量增强，默认关零漂移 | S4 |
| 4 | A2/A3/A4/A5 表驱动精简 | 架构与可维护性 | S2/S4 |

每项进入实施前必须满足：golden 37 例字节级零漂移（新行为只加新例）、
fmt/clippy/rustdoc 双轨、默认与 `--all-features` 测试、release build、三平台 CI。

## 6. 明确不做（红线）

1. 不向 6ms 级常规热路径引入 SIMD/线程化等增加复杂度、收益无法证明的改动；
2. 不引入任何第三方 crate，不破坏零依赖与 `unsafe_code = "forbid"`；
3. 不合并纯函数内核与 memory I/O 层的隔离边界；
4. 不改变默认压缩语义与确定性承诺；不写入客户端未文档化字段；
5. 不删除、不移动用户文件与未跟踪资产。

## 7. 待验证清单（unknown → 实测）

1. P1 在真实小核机大输入场景的实际收益（口径已统一，待真机数据）；
2. ~~校准后三分类系数的 P95 取值~~ ✅ 已实测 1.787（370 样本）；
3. ~~A2 精简的真实行数变化~~ ✅ 已合并，golden 零漂移；
4. 豆包/千问/Cursor 事件 hook 核实（automation S6）：✅ Cursor 已接入官方
   hooks.json；豆包确认无文档化 hook 面、保留 MCP 连接器；千问 Qwen Code
   具备同构 hook，待后续接入。
