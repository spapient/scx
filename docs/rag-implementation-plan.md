# scx 记忆知识库（RAG）实施计划

版本：v1.9（可执行计划，基于设计稿 v0.2；v1.9 增补损坏自愈、同 case 并发与索引一致性修复）
日期：2026-10-22
作者：scx 项目组
状态：P1/P3 工程完成；P2 工程完成但 M2 的独立标注门未通过；P4.1/P4.2/P4.3 工具完成；P4.4 安全/一致性/损坏自愈处置完成（见 §11、§18-§22）；**M4 独立质量证据未通过**（自动回归集可复现，但自检使用 A 管线、分层 24/9/3 不满足各层 ≥10）；P4.4 ANN 与 P4.5 embedding 可选未启动（按触发条件）

> 本文档回答"怎么做、先做什么、怎么做完算完"；设计依据见 `docs/memory-rag-design.md`（v0.2，O1-O8）。
> 任何阶段门不过，不进下一阶段。每阶段一个 git 提交点，可独立回滚。

---

## 1. 目标与成功标准

- **北极星**：同类问题二次遇到，L1 本地直答命中（不再问模型 / 不重读全文）。
- **阶段终态成功标准（全部可验证，不靠声称）**：
  1. `--store 1` 摄取后，`--recall` 同 query 能召回原 case（库往返测试）。
  2. 30-50 组分层标注集上 Recall@5 / MRR 达到并超过基线（P4 用 `--eval` 冻结重放）。
  3. 1 万条库检索 < 5ms（P3 基准）。
  4. 既有压缩输出逐字节不变：golden 16 用例 + 全量 `cargo test` 恒绿。

## 2. 前置条件盘点（已完成，无需重复）

| 项 | 状态 | 证据 |
|---|---|---|
| v0.4.1：--lineno / 模块拆分 / golden 门 / bench | ✅ | 33/33 测试、golden 16 用例、bench 44ms、fmt+clippy 零告警 |
| 分类器（is_err/is_frame/is_lineref/norm_into/build_terms） | ✅ | `rs/src/classify.rs`、`terms.rs`、`norm.rs` 已就绪，O1 直接复用 |
| 差分门（N6） | ✅ | `rs/tests/golden.rs` + `tests/golden/expected/` 16 组冻结基线 |

## 3. 总体里程碑与阶段门

| 里程碑 | 阶段 | 含义 | 阶段门（不过不进下阶段） |
|---|---|---|---|
| M1 | P1 MVP 记忆 | 能存、能取、能召回 | 库往返测试通过；golden 16 用例逐字节不变；cargo test 全绿 |
| M2 | P2 召回质量 | 召回"更对"：记忆图/反馈/去冗 | 标注集 Recall@5/MRR ≥ 基线；L1 命中率可统计 |
| M3 | P3 工程化 | 稳、快、可自愈 | 1 万条库 < 5ms；坏段注入测试通过；三平台构建绿 |
| M4 | P4 证明与增强 | 有评估证据、可选增强 | 30–50 组**独立人工**标注集上线 → A vs B 对拍表入档 → NDCG 存档可重放；自动标注回归集可复现不等于 M4 通过（自检用 A 放行、存在选择偏差；见 §21） |

依赖：P1 → P2 → P3 → P4（串行主线）；阶段内任务多数可并行（见 §7 依赖图）。

---

## 4. P1 — MVP 记忆（能存能取，约 1-2 天）

**目标**：`--store 1` 入库 + `--recall` 旁路召回打通；L1 雏形（线性扫描 + BM25 + 字段加权）。

**模块约定**（红线不破）：
- 新增 `rs/src/memory/` 目录：`mod.rs`（库门面）、`store.rs`（case 读写）、`index.rs`（vec/线性扫描）、`recall.rs`（BM25+字段加权）、`field.rs`（O1 抽取，复用 classify.rs）。
- `lib.rs` 只加 `pub mod memory;`，`compress` 零改动、零 I/O 触碰。
- 每文件 < 500 行（沿用项目门禁）；命名单单词 + Unix 缩写。
- **零依赖序列化**：case.json / meta.toml 自产自销，自研极简行式解析（只读写自己写的键），不引 serde/toml。

### P1.1 存储层（store.rs）— ✅ 已完成（2026-10-22）
- [x] `case.json` 读写：schema=2 字段（id/query/terms/text/source/tags/bucket/fields/status/stats/forces/feedback/quality），自研 JSON 子集 writer/parser（json.rs：Val 枚举 + 递归下降，零依赖）。
- [x] `meta.toml` 读写：schema/embed_engine/case_count/df 快照/params，行式 key=value 子集。
- [x] `meta.toml` 读写：schema/embed_engine/case_count/df 快照/params/gen，行式 key=value 子集。
- [x] ~~`vec.bin`~~：P4.4 评审处置已移除——实际始终以空稀疏向量写入、召回未使用，属无效产物（不再落盘）。
- [x] case id：`<YYYY-MM-DD>_<hash16>`（hash = query+证据块指纹，FNV-1a 64 取 64 位→16 hex；P4.4 由 32 位升 64 位，1 万条碰撞率 ~1.2%→<0.003%）。
- **验证**：`memory_store_roundtrip`（写→读→逐字段断言）+ 原子写（临时文件+rename，P4.4）+ 单 case ≤1 MiB 上限；30 库测试全绿；clippy 零告警。

### P1.2 摄取（--store 1，ingest.rs）— ✅ 已完成（2026-10-22，P4.4 补边界）
- [x] 摄取即过滤：入库的是 compress 筛出的证据块，非原始日志。
- [x] 证据块（Outcome.text 的保留行区间）作为 case.text；norm_into 指纹去重（已存在 → 更新 updated_at，不新增）。
- [x] 默认不自动存储：无 `--mem-dir`/`--store` 时 memory 零路径。
- [x] delete(root,id) 物理删除 + case_count 重算（P4.4：入口 valid_id 白名单）。
- [x] **P4.4 边界说明（诚实口径）**：`--store` 存的是**压缩结果**（Outcome.text：当前 query 命中的保留行，可能含 `...` 省略标记与 `--lineno` 的 `N:` 前缀），**不含用户真正的修复动作/结论**。因此 L1 返回的 `summary:` 是"错误行摘要"而非"上次怎么解决的"——它支撑案例召回（"上次这问题长这样、怎么处理过"），但兑现不了"L1 本地直答、不再问模型"的北极星；要完整直答需 `--store` 额外保存修复结论（后续可选增强，非本期）。
- [x] **P4.4+ 优化（--fix 本地直答，北极星缺口补全）**：`--store 1` 可配 `--fix "修复结论"`，把"上次怎么解决"写入 `case.fix`（≤4 KiB）；case 带 fix 时**主流程 `--recall` 与 `--recall-detail` 均无条件输出 `fix:` 行**（P4.4+ 优化② 把 fix 行移出 detail 分支）——L1 从"案例召回"升级为"本地直答"，默认路径即生效。同指纹重复摄取不覆盖既有 fix（保留首条结论）；旧 case 无 fix 键按空处理（向后兼容，不输出 fix 行）。实测：`store --fix "enlarge pool to 64 conns, restart daemon"` → 主流程 `--recall 1`（无 detail）直接输出 `fix: enlarge pool to 64 conns, restart daemon`（golden `memory_fix` 用例）。
- [x] **P4.4 大小上限**：单 case 文本 ≤1 MiB（`MAX_CASE_TEXT`），超限拒绝（实测 1.2 MB 输入 → `case text too large: 1200000 bytes > 1048576`，exit 2，不落盘）。
- **验证**：`ingest_dedup_by_fingerprint`（同指纹保持 1、变体新增到 2）+ `fingerprint_is_deterministic_and_sensitive` + `delete_removes_case_and_meta` + size cap 拒绝；CLI 冒烟：库目录 1 个 case、重复再存不变。

### P1.3 字段抽取（field.rs，O1）— ✅ 已完成（2026-10-22）
- [x] 复用 classify.rs：error_code（错误词面邻近 token）、file_loc（is_frame/is_lineref 行）、symbol（行内标识符）、num_fp（norm_into 数字折叠指纹）。
- [x] 抽取只发生在摄取路径（memory 侧），compress 不触碰；判定用归一化小写行、提取用原始行（norm 折叠会破坏 token 结构）。
- [x] 封顶 16 项/类、去重、截断 128 字符；token 统一小写（匹配一致性）。
- **验证**：单测 `field_extract_smoke`：conn_id=9f2a/errno=28/Pool.java:214/符号/`#` 折叠指纹四项断言通过。

### P1.4 召回（recall.rs）— ✅ 已完成（2026-10-22）
- [x] 线性扫描（规模 <1 万，符合设计规模决策），BM25(k1=1.2,b=0.75) + 字段加权（error_code×3.0 / file_loc×2.5 / symbol×2.0 / num_fp×2.0，权重入 meta.toml [params]）。
- [x] doc 词表 = case.query ∪ case.text（同 query 二次召回词面必然命中；query 作为 case 描述参与长度归一化）。
- [x] `--recall K` 输出 `mem:` 段（旁路，绝不进入正文）；`--recall-only` 只召回。
- [x] L1 置信雏形：sim ≥ 0.55 才输出 mem 段（P1 固定阈值，P2.6 换自适应）。
- **验证**：`recall_roundtrip_same_query`（同 query sim=1.00 过阈）+ `recall_field_priority_over_word_overlap`（字段精确命中压过纯词面）。

### P1.5 CLI 接线 + 记忆差分门 — ✅ 已完成（2026-10-22）
- [x] 新增参数全可选、默认关：`--store 1` `--recall K` `--recall-only` `--mem-dir DIR`（默认 `~/.scx-memory`，`$SCX_MEM_DIR` 可覆盖）；`--recall-only` 需 `--query`。
- [x] 新增 golden 记忆基线（`tests/golden/` 追加 `memory_store`/`memory_recall` 2 例，独立于既有 16 例；mem 段只显示 hash8 避免日期漂移）。
- [x] 摄取时同步抽取 fields（O1 落盘）。
- [x] **行数门禁补拆（收尾）**：`store.rs`（586 行）拆为 `store.rs`（case/vec 读写）+ `meta.rs`（meta.toml/库根）+ `model.rs`（数据模型）+ `util.rs`（FNV/UTC）；`main.rs`（623 行）拆为 `main.rs`（编排）+ `cli_args.rs`（参数解析）+ `cli_io.rs`（输入读取）+ `cli_mem.rs`（记忆接线）。全文件 < 500 行（最大 cli_args.rs 415 行）。
- [x] **recall-only 修复**：未指定 `--recall K` 时默认 top-3（原 k=0 致 `truncate(0)` 清空命中，E2E 复现后修复并加单测）。
- **验证**：`cargo test --test golden`（18 例全过）；`cargo test --locked` 全绿（38 库 + 8 CLI + 4 集成 + 1 golden）；fmt/clippy 零告警；release 构建成功；E2E 冒烟 store→recall sim=1.00、recall-only 输出 mem 段、去重 1→1。

**P1 阶段门**：库往返测试 ✅ + golden 18 用例 ✅ + cargo test 全绿 ✅ → 可进入 P2。

---

## 5. P2 — 召回质量（更对）— ✅ 已完成（2026-10-22）

**目标**：召回质量显著提升，L1 命中率可统计。P2.2/2.3/2.4/2.5 互相独立可并行（本次串行实施）。

### P2.1 query 记忆图（qmap.bin，O2）— ✅ 已完成（P4.4 评审处置：代际失效）
- [x] 自研二进制 `index/qmap.bin`：`[magic "QMAP" u32][ver u16]` + 每条 `[qlen u16][norm(query)][n u16][(idlen u8)(id)(score f32)(sim f32)*n][recency u32]`；条目存 score/sim 快照 → 同指纹二次直达能原样复现输出（golden 稳定性）。
- [x] recall 成功后回流写入（append-only、尾部损坏容错：丢弃残段不崩溃）；`recall::recall` 入口先 qmap 精确直达（dist=0 返回历史 top-k）；编辑距离 ≤2 变体命中转轻量重排。
- [x] **P4.4 修复**：qmap 升 v2，条目带 `gen`（meta.toml 全局代际）。`write_case`/`--rate` 会 bump gen → 旧快照查询即失效（新 case 立即可见、反馈立即改序）；`--rebuild` 只合并不复活旧快照。只看模式（--recall-only）不再写 qmap。
- **验证**：单测 4 例（往返/变体/损坏容错/gen 失效）通过；端到端：store 新 case 后同 query 召回可见新 case（score 0.58→1.13 验证 rate 立即可见）。

### P2.2 会话缓存（O3）— ✅ 已完成（P4.4：带 gen 失效）
- [x] 进程内存全局单例 LRU（容量 64、窗口 30 分钟），键 = fnv1a(norm(query))；`recall_cases` 先查缓存命中直接返回。
- [x] **快照语义（P2.6 收尾修复）**：缓存存 (id, score, sim) 完整快照而非裸 id 列表——原"命中返回 sim=0 占位"会被 L1 自适应过滤吞掉（会话内二次同问反而无输出），改为快照后命中路径与正常召回同口径，L1 过滤/排序可复用。
- [x] **P4.4 修复**：缓存条目带 gen，库代际变化后缓存查询直接 miss（不喂旧快照）。
- **验证**：单测 3 例（窗口内命中含快照逐字段一致 / 窗口外过期 miss / gen 失效 miss）通过。

### P2.3 PRF 查询扩展 — ✅ 已完成
- [x] `memory/prf.rs`：批内 tf≥2、全局 idf>0.6、排除 query 已含词、最多 6 个、按 idf 降序；`recall` 低置信（top1.sim<0.99）时扩展 query 再召一轮。
- [x] 语料构造教训：norm 会把任意数字段折叠为 `#` → 测试文本变体必须用字母 token 区分（alpha/beta/gamma），否则指纹去重只剩 1 条。
- **验证**：单测 `prf_expands_paraphrase_query`（判别词 idf 稀释后入选）通过。

### P2.4 时间衰减 + 动态 τ（O4 联动）— ✅ 已完成
- [x] `decay=1/(1+age_days/τ_eff)`；τ 默认 90（decay_tau_days），solved→180、regressed→30（meta 参数缺省回退，老库无需迁移）。
- [x] `util::parse_utc_secs`（自产 UTC 串逆解析）。
- **验证**：单测 `time_decay_prefers_fresh_case`（曾因数字折叠去重失败，改字母变体后通过）。

### P2.5 MMR 去冗余 — ✅ 已完成
- [x] `memory/mmr.rs`：贪心 `score_norm − λ·max(与已选 Jaccard)`，λ=0.5 默认（meta mmr_lambda）；词袋 Jaccard 作文档相似度；`score_pass` 排序后按多样性选取 top-K。
- **验证**：单测 3 例（去冗余 / λ=0 纯分数序 / K>池），首个用例曾因测试分数设计不当修正（0.95 分差抵消扣减）。

### P2.6 反馈双环 + 状态机（O4）+ 自适应置信（O5 前半）— ✅ 已完成
- [x] `--rate good|bad <case-id>`：词级调权（good +0.1 / bad −0.15，clamp ±0.5，存 meta.toml [word_w]）+ case 状态机（open→solved→regressed）+ quality=good/(good+bad) 分（进排序先验，meta quality_prior=0.5）。
- [x] L1 置信自适应：**主流程 `--recall`（学习路径）** 每次召回记录 top1.sim 到 meta [l1] hist（上限 64 条）；`--rate` 时重算 `l1_base = P50(hist)`（clamp 0.3..0.9）写回 [params]；CLI 过滤判据 `sim_top1 ≥ l1_base 且 (top1−top2) ≥ l1_margin_min(0.15)`（扩展命中率 ≥0.30 判据预留，CLI 传 1.0 视为满足，统计接线留 P3 评估）。
- [x] **P4.4 修复（查看模式只读）**：`--recall-only`/`--eval`/bench 一律只读——不写 qmap、不写 L1 历史、不写会话缓存。查看、标注、演示不得改变库状态（实测连续多次 --recall-only 后 `--l1-hist` 仍 n=0）。
- [x] E2E 冒烟：store→recall mem 行→rate good（solved）→rate bad（regressed）→meta 词权重/l1_base 落盘。
- **验证**：单测 4 例（状态机×2 / l1_qualified 判据 / l1_base P50 滑动）通过。

### P2.7 场景桶过滤（O6）— ✅ 已完成（P4.4：桶判定对称修复）
- [x] `memory/bucket.rs`：5 家族（db/network/disk/build/runtime）+ other，小写子串判定；摄取写 `c.bucket`；检索先按 query 判候选桶再扫（空候选=全量扫防漏召）。
- [x] **P4.4 修复（桶判定不对称漏召）**：db 家族关键词表补裸词 `"db"`。此前 query "db timeout" 只有 timeout 命中 network 家族 → 候选桶 [network]，而 case 主桶靠 conn_id 判为 db → 唯一候选被滤、输出空（1 case 库实测复现）。补词后 query "db timeout" → [db, network]，case db 命中。
- **验证**：单测 `bucket_filter_reduces_scan` + `candidate_buckets("db timeout")` 含 db；端到端 1 case 库 recall-only 输出恢复正常。

**P2 集成（本次阶段门）**：
- [x] 全量 `cargo test --locked`：57 库 + 8 CLI + 4 集成 + golden 全绿（P2 前 50 库 → P2 后 57 库，PRF/MMR/feedback/cache 等新增 7 测试）。
- [x] `cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` 零告警。
- [x] `cargo build --release --locked` 成功。
- [x] 行数门禁：`src/**` 全部 < 500 行。
- [x] golden 重冻结：18 → 19 例（新增 `memory_rate` 基线：rate good 输出 + l1_base 行），`golden_gate` 全过（既有 16 例压缩输出逐字节不变，N6 保持）。

**P2 阶段门**：标注集（§8 语料）Recall@5/MRR ≥ 基线 → 收集完成后用 `--eval` 冻结；L1 命中率可统计 ✅（`--eval` 输出 l1 行：top1 命中相关 case 中满足置信判据的比例，参数来自 [params] 随库自适应）→ 可进入 P3。

---

## 6. P3 — 工程化（稳、快、可自愈）— ✅ 已完成（2026-10-22）

**目标**：索引正式化、崩溃自愈、性能达标、写后台化。

### P3.1 倒排索引（P4.4：删 mmap，索引纯缓存）— ✅ 已完成
- [x] `memory/index.rs`（~470 行）`postings.bin` **v2 自包含 doc 打分快照**：`[magic "POST" u32][ver u16][body_len u64][body][crc32 u32]`；body = n_docs/total_len/n_terms + doc 表（id + 去重词表 + 4 字段 + bucket/status/quality/updated_at，case id 字典序）+ term 表（term 字典序 + df + delta-varint doc_idx）。**候选打分零 case.json 读取**（性能门关键）。
- [x] ~~`memory/mmap.rs`~~：P4.4 评审处置已删除——手写 unsafe FFI（from_raw_parts）+ 无文件锁，另一进程 rebuild/truncate 时映射失效；1 万条以内 `fs::read` 足够（零 unsafe）。
- [x] `memory/postings.rs`（段封装：varint LEB128 + CRC-32 IEEE 查表 + 段写）。
- [x] 自动启用阈值 `INDEX_MIN_DOCS=10000`；**P4.4 修复：`--rebuild` 不再写 `[params] index_force=1`**——索引只是缓存，重建不得改变召回语义（线性/索引两路径 df/avgdl 口径不同，强制索引会让 A/B 不可比）；rebuild 只落 df 快照。
- [x] **进程内索引缓存**（root+mtime 键，Arc 共享）：会话/守护进程多次召回免重复 parse（冷启动 parse ~11ms，命中 ~0）；`invalidate` 清缓存。
- **验证**：`build_parse_roundtrip`（df 快照覆盖全部 term）/`candidates_union_dedup_sorted`/`corrupt_tail_rejected_and_rebuild_recovers`/`invalidate_removes_index_file` 通过；单测断言 rebuild 后 meta 无 index_force 且 df 快照已落。

### P3.2 崩溃一致性 + 坏段自愈 — ✅ 已完成
- [x] body_len 边界 + crc32 段尾双校验；`load` 坏段返回 Err(corrupt)；召回主路径坏段 → 自动 `build` 覆盖重建，重建失败回退线性扫描。
- **验证**：坏段注入单测 + E2E（截断 postings 尾部 5 字节 → recall 自动 heal 输出同值）+ golden `memory_corrupt_heal`。

### P3.3 rebuild / gc / delete — ✅ 已完成（P4.4：安全加固）
- [x] `--rebuild`：index::build 全量重建 + qmap::merge（同归一化 query 取 recency 最新）+ df 快照写 `meta.toml [df]`（**来自 build 返回值**，修掉"写空 df"一致性 bug）+ case_count 同步；输出 `rebuild: docs=N terms=N`。**P4.4：不再写 index_force、不复活旧 qmap 快照（gen 失效）。**
- [x] `--gc`：双条件 expire（regressed 且 >180 天 / 从未反馈 quality=0 且 >365 天，`gc_ttl_regressed=180`/`gc_ttl_stale=365` 入 meta 模板）→ 物理删除 + 索引作废；输出 `gc: removed=N`。
- [x] `--delete <case-id>`：物理删除 + case_count 重算 + 删 postings/qmap（惰性作废）；输出 `delete: removed case=...`。
- [x] **P4.4 安全修复**：`--delete`/`--rate` 入口先过 `valid_id` 白名单（ASCII 字母数字 + `-` `_`、禁 `..`、≤64 字符）——原实现直接拼接 `root/cases/<id>` 路径，`..\..\` 可逃出记忆库目录（实测拒绝 `..\..\evil`，exit 2）。
- [x] 三个维护模式豁免 `--query` 要求（cli_args 校验放行 + 单测）。
- **验证**：housekeep 3 单测（rebuild 建索引 + 无 force 标记 / gc 只清过期 / delete 作废索引）+ E2E 冒烟 + golden 3 例。

### P3.4 写后台化（write-behind）— ✅ 已完成
- [x] `--store`（未配 `--recall`）摄取入队 `mpsc` + `std::thread::spawn`（零依赖）；正文与统计先回显，进程退出前 `rx.recv()` join 确认落盘（失败 exit 2）；`--store + --recall` 组合保持同步（recall 需看到新 case）。
- **验证**：E2E 冒烟（大输入 store 先回显后落盘）；golden 不受影响。

### P3.5 记忆 golden 门扩展 + 性能门 — ✅ 已完成
- [x] golden 追加 4 例：`memory_rebuild` / `memory_gc` / `memory_corrupt_heal` / `memory_qmap_replay` → **23 例（expected 69 文件）重冻结**，`golden_gate` 全过。
- [x] `examples/mem_bench.rs`：200 字母唯一种子（**规避 norm 数字折叠指纹去重**）→ 复制 50 份改名 → 1 万条 → rebuild → 5 个词面不同 query 强制索引路径采样取中位（**规避 qmap 直达虚高**）。
- [x] **性能门修复链（实施中发现的真实问题）**：
  1. PRF `prf::expand` 全库现算 df → **改读 `meta [df]` 快照**（非空走快照，空回退线性小库），900ms → 300µs；
  2. score_pass 每候选重复 read_case → **DocCtx 一次读取**，打分零二次 IO；
  3. postings v2 doc 快照 → 索引路径打分**零 case.json IO**；
  4. 进程索引缓存 → 稳态召回零重复 parse。
- [x] **语义统一**：线性/索引两路径 doc 词表统一为"排序去重并集"（tf 每词=1，与设计"去重词表"口径一致）；golden 相应重冻。
- **验证**：`mem_bench` release：**recall 中位 1.4-1.9ms < 5ms（PASS）**；rebuild 1 万条 ~1s。

**P3 集成门禁（全部通过）**：
- [x] `cargo test --locked`：**65 库 + 8 CLI + 4 集成 + golden 23 例** 全绿（P2 后 57 库 → P3 后 65 库）。
- [x] `cargo fmt` / `cargo clippy --all-targets --locked -- -D warnings` 零告警。
- [x] `cargo build --release --locked` 成功。
- [x] 行数门禁：`src/**` 全 < 500 行（index.rs 拆 postings.rs 后 474 行）。

**P3 阶段门**：1 万条库 < 5ms ✅（中位 1.4-1.9ms）+ 坏段注入自愈 ✅ + golden 23 例 ✅ + 三平台 CI（.github/workflows/ci.yml 三 OS 矩阵既有，未改）→ 可进入 P4。

---

## 7. P4 — 证明与增强（P4.1/P4.2/P4.3 已完成，P4.4/4.5 可选）

### P4.1 评估系统（O8）— 工具 ✅ / M4 证据门未通过（P4.4 评审处置）
- [x] `memory/eval.rs`（~300 行）：`--eval <baseline.json>` 冻结评估输入重放；baseline = `{"cases":[{"query","layer","expected","rel"}]}`，expected 支持完整 id 或 hash16 后缀（P4.4 起 id 64 位，旧 8 位标注兼容：tail8 亦匹配；golden 免日期漂移）。
- [x] 指标：Recall@5 / MRR / NDCG@10（rel 分级 0..2，DCG=Σ rel/log2(i+1)），分 literal/paraphrase/semantic 三层聚合出表。
- [x] **A/B 对拍**：A = 完整统计管线（recall::recall，字段+PRF+衰减+MMR+反馈+桶）；B = `eval::plain` 朴素词面基线（全库候选、纯词面、无任何增强）。**P4.4 口径修正：两路打分词表均为去重词表（tf 恒 1），实际是"二值词面匹配 + 长度归一化 + 字段加权"，不是严格 BM25；A/B 同词表对比仍公平，但任何结论须按此口径引用，不宣称"BM25 增益"。** 评估路径确定可重放（无随机源）。
- [x] 报告行：`eval: cases=N layers=...` + 每层 `layer X: n=.. A r@5/mrr/ndcg | B r@5/mrr/ndcg` + **`l1: hits=.. total=.. rate=..`（P2 阶段门残余补全：L1 直答命中率出表，top1 命中相关 case 中满足置信判据的比例，判据参数 l1_base/margin 随库自适应）**。
- [x] **`--l1-hist`（P2 阶段门残余补全之二）**：读 meta.toml [l1] hist（recall 侧每次 push top1.sim 的滑动历史）出运行期直答分布——`l1hist: n=.. mean=.. p50=.. p90=.. over_base=.. (base=..)`；冒烟验证：store+recall 一次后 `n=1 mean=1.000 over_base=1`。
- [x] CLI 接线（豁免 --query）+ 4 单测（dcg 标准公式 / metric recall·mrr·ndcg / baseline 严格容错解析 / summary 300 字符截断）+ golden `memory_eval`（重冻含 l1 行）。
- [x] **`--recall-only` 口径修正（查看模式）**：原 P2.6 对 --recall-only 也做 L1 自适应过滤（hits 不达置信判据时整段滤空，演示/标注时看不到真实召回）→ 改为输出真实 top-k；主流程 `--recall` 的 mem 段保持 L1 直答过滤不变。**P4.4 再加固：--recall-only/--eval/bench 一律只读（不写 qmap/L1 历史/缓存）。** golden 25 例零漂移（既有用例本就达判据，输出不变）。
- **验证**：golden 25 例冻结；库 71 测试全绿；clippy/release/行数门禁过。
- **初测发现（诚实记录）**：3 例合成标注中 literal/semantic 层 A=B=1.0；paraphrase 层 A mrr=0.5 vs B=1.0（单例，PRF/MMR 使相关 case 排第 2）——说明统计向量在释义层无优势属预期（与设计稿 §10 判断一致）；l1 首测 hits=1 total=2 rate=0.500（literal 字段精确命中过判据；semantic 未过；paraphrase 相关 case 未进 top1 不计分母）。**P4.4 状态声明：当前仅 3 例合成标注初测，M4 证据门（30–50 组人工标注）未通过，正式标注集上线前不宣称 A/B 收益。**
- **标注采集就绪**：`docs/eval-baseline-template.md`（v1.0）给出 baseline.json 格式、三层造例规则、rel 分级、id 获取流程（--recall-detail 核对）与红线——30-50 组人工标注的采集门槛已文档化，只差人工产出。

### P4.2 L1 要点行摘要（O5 后半）— ✅ 已完成（2026-10-22）
- [x] `eval::case_summary`：error_code + file_loc + 证据块内首个错误行（无则首非空行），≤300 字符（截断加 …）。
- [x] CLI `--recall-detail`：mem 段每条追加 `  summary: ...` 行（摘要不展开整块正文）。
- **验证**：golden `memory_summary`（`summary: conn_id=9f2a | pool.java:214 | ERROR db timeout conn_id=9f2a pool exhausted`）；单测截断边界。

### P4.3 实体边一跳扩展（可选，已收尾）— ✅ 已完成（2026-10-22）
- [x] `memory/edge.rs`：`EdgeIndex`（entity→case id 倒排，实体键 = 四类字段值 error_code/file_loc/symbol/num_fp 全库现算）、`build(root)` 确定性、`neighbors(&[String])` 去重保序。
- [x] 机制与边界（诚实口径）：实体键是 case 词面的子集（token 层无法"零词面共享"），真实收益两条——①跨桶兜底：被候选桶过滤滤掉但实体键相同的兄弟 case 并入候选；②实体加权：共享实体数 × edge_weight 进打分 fb。
- [x] `recall.rs` 集成：DocCtx 加 `edge_shared`；索引路径在 candidates 后调 `edge::bridge_cases`（df/avgdl 保持 idx 全库口径、桥词长丢弃）、线性路径桥入后 avgdl 重算；打分循环 `fb += edge_shared × edge_weight`。
- [x] 参数：`edge_enable=1`（0 关）/`edge_weight=1.0` 入 meta_toml 模板；老库缺省 param() 兜底同值；`meta.rs` 新增 `set_param(root,key,val)`（[params] 单键写/改，测试与调参共用）。
- [x] 新增单测 `edge_bridge_recovers_cross_bucket_sibling`（A=db 桶 / B=build 桶共享 Pool.java:214 → 桥开召回 B、`edge_enable=0` 关桥 B 被桶滤）+ `build_and_neighbors_are_deterministic`。
- **验证（门禁终态）**：**71 库 + 8 CLI + 4 集成 + golden 25 例**全绿（golden 未漂移：现有用例 query 无实体，桥零开销路径）；clippy `-D warnings` 零告警；release 成功；行数门禁全过（recall.rs 499，edge_bridge 迁入 edge.rs 后回门）；bench 回归 P3 gate PASS（recall 中位 2147µs <5ms，edge 空实体早退零开销）。

### P4.4 ANN 后端 feature（可选，触发：库 ≥10 万 且 评估需要稠密）
- [ ] HNSW（USearch 形态：M=16/ef=200/紧凑布局/mmap），feature 开关默认关。
- **验证**：构建 + 召回正确性 + 与倒排结果交集 ≥ 90%。

### P4.5 释义型 embedding 接口（可选，触发：P4.1 评估显示释义类值得）
- [ ] 本地 embedding feature 开关；RRF 混合（设计稿预判混合稳压）。
- **验证**：P4.1 对拍表 A vs B vs 混合，决策记录入档。

**M4 阶段门（当前状态）**：`--eval` 工具就绪（对拍表 + NDCG 存档可重放）✅；**M4 证据门未通过**——正式标注集（30–50 组人工标注，§8）未上线，当前仅 3 例合成初测；标注集上线前不宣称 A/B 收益。

---

## 8. 评估语料（P2/P4 共用，可执行口径）

| 来源 | 规模 | 用途 | 何时可做 |
|---|---|---|---|
| 人工标注（真实历史经验取样） | 30-50 组 (query→相关 case) | P2 阶段门基线 + P4 终判 | P1 完成后即开始收集 |
| 隐式弱标注（历史 query→命中 case） | 数百组 | P4 粗筛 | 库积累后 |
| 分层：字面 / 词面近义 / 释义 | 各 ≥10 组 | P4 分层指标 | 与人工标注同步 |

**注意**：P2 阶段门依赖标注集，标注集在 P1 后就要启动（可先做 30 组人工，P2 末到位）。

---

## 9. 依赖与关键路径

```
P1.1 存储 ──► P1.2 摄取 ──► P1.4 召回 ──► P1.5 CLI+门
   │                           ▲
   └──► P1.3 字段抽取 ─────────┘（并行，不阻塞主线）

P1.5 ──► P2.1 qmap ──► P2.6 反馈+自适应 ──► P2.7 桶过滤
             │并行：P2.2 会话缓存 / P2.3 PRF / P2.4 衰减 / P2.5 MMR

P2 ──► P3.1 倒排 ──► P3.2 自愈 ──► P3.3 gc/rebuild ──► P3.5 门
                  │并行：P3.4 写后台化

P3 ──► P4.1 评估 ──► P4.2 要点摘要 ──► P4.3 实体边 ──►（P4.4/P4.5 按触发条件可选）
```

**关键路径**：P1.1 → P1.2 → P1.4 → P1.5 → P2.1 → P2.6 → P3.1 → P3.2 → P3.5 → P4.1 → P4.2 → P4.3
**并行窗口**：P1.3 ∥ P1.1-1.2；P2.2-2.5 四任务并行；P3.4 ∥ P3.1-3.2。

---

## 10. 验证体系（每阶段怎么跑）

| 检查 | 命令 | 频率 |
|---|---|---|
| 单元/集成/记忆测试 | `cargo test --locked` | 每任务完成 |
| 差分门（含记忆基线） | `cargo test --test golden` | 每阶段门 |
| 风格 | `cargo fmt --check` | 每任务 |
| 静态检查 | `cargo clippy --all-targets -- -D warnings` | 每任务 |
| release 构建 | `cargo build --release --locked` | 每阶段门 |
| 性能基准 | `cargo run --release --example bench --locked` | P3 门 |
| 评估对拍 | `scx --eval baseline.json` | P4 门 |
| 三平台 CI | GitHub Actions 三 OS 矩阵 | 每阶段门 |

**红线复核（每阶段门附带）**：compress 源码 diff 为空（N3）；既有 golden 16 例逐字节不变（N6）；无随机源（N5）；Cargo.toml 零新增依赖（N4）。

---

## 11. P4.4 评审处置记录（2026-10-22，逐条闭环）

输入：对 v0.1 实现的 6 条功能问题 + 6 条工程安全债评审。处置结果：

| 评审条目 | 处置 | 验证 |
|---|---|---|
| ① qmap append-only 快照不失效（新 case/反馈不可见） | 代际失效：meta `gen` + qmap v2 条目带 gen + 缓存带 gen；write_case/rate 均 bump gen；rebuild 不复活旧快照 | 单测 + 端到端（store 新 case 后同 query 召回可见；rate 后 score 0.58→1.13 立即可见） |
| ② --recall-only 不是只读（污染 l1_base） | 查看模式只读：recall_cases 加 read_only；--recall-only/--eval/bench 不写 qmap/L1 历史/缓存 | 端到端（连续多次 --recall-only 后 --l1-hist 仍 n=0） |
| ③ P4.1 "已完成"但 M4 证据门没完成 | 状态改"工具 ✅ / M4 证据门未通过"；正式标注集上线前不宣称 A/B 收益 | §7 P4.1 标题 + §3 M4 行 + 初测诚实记录 |
| ④ --store 存"压缩结果"非完整 case/答案 | 不掩盖：§4 P1.2 补边界说明（summary 是错误行摘要；完整直答需后续保存修复结论） | 文档 |
| ⑤ --rebuild 写 index_force 改变召回语义 | 索引纯缓存：rebuild 不再写 index_force，只落 df 快照 | 单测断言 meta 无 index_force + df 已落 |
| ⑥ "BM25"实为 tf=1 词面匹配 | 口径诚实化：注释/计划文档改"二值词面匹配 + 长度归一化 + 字段加权"；A/B 同词表公平但结论按此口径引用 | 文档 + eval.rs 头注释 |
| 安全债-1 手写 unsafe mmap | 删除 mmap.rs，索引加载改 fs::read（零 unsafe，1 万条内足够） | check/test 绿 |
| 安全债-2 裸 fs::write 无原子性 | case/meta 写入统一原子写（临时文件+rename） | 单测 + 端到端 |
| 安全债-3 case id 32 位碰撞 | 升 64 位（hash16）；eval 兼容旧 8 位 tail | golden 重冻仅 id 长度差 |
| 安全债-4 vec.bin 无效产物 | 移除 vec.bin 读写（write_case 去参、删模块引用） | 编译 + 单测 |
| 安全债-5 --delete 路径穿越 | valid_id 白名单（ASCII 字母数字 + `-` `_`、禁 `..`、≤64）；--delete/--rate 入口统一过检 | 端到端（拒绝 `..\..\evil`，exit 2） |
| 安全债-6 无大小上限 | MAX_CASE_TEXT=1 MiB，超限拒绝不落盘 | 端到端（1.2 MB 拒绝） |
| 附带修复：桶判定不对称漏召 | db 家族补裸词 "db"（query "db timeout" 不再只归 network 桶） | 端到端 1 case 库召回恢复 |

**提交**：代码修复 + golden 重冻（5 文件仅 id 8→16 hex）+ 文档同步 分两笔 git 提交（评审处置 / 文档）。

---

## 12. 风险与回滚

| 风险 | 等级 | 对策 |
|---|---|---|
| 自研 JSON/TOML 解析器引入兼容 bug | 中 | 只读写自己写的键；schema 版本号在 meta；单测覆盖往返 |
| 记忆特性影响既有压缩 | 高 | memory 与 compress 物理隔离；每阶段门验 golden 16 例 |
| 标注集拖期阻塞 P2 门 | 中 | 人工 30 组与 P1 并行收集；不足时先以隐式弱标注粗筛 |
| 倒排/mmap 跨平台不一致 | 中 | 三平台 CI + 确定性数据构造（固定 seed 的合成语料） |
| 库膨胀性能退化 | 中 | 规模决策（线性→倒排→ANN）+ gc + 自压缩 |

**回滚**：每阶段一个 git 提交点；任一阶段门失败 → 修复或 `git revert` 到上一里程碑，不跨门。

---

## 13. 决策点（实施中需要停下问用户）

| # | 决策点 | 触发时机 | 默认建议 |
|---|---|---|---|
| D1 | 每项目隔离库 vs 全局共享库 | P1 开工前 | 每项目隔离（隐私收紧），`--mem-dir` 指向同目录即共享 |
| D2 | 自动存储默认开关 | P1.2 实现时 | 默认关（`--store` 显式开） |
| D3 | 释义型 embedding 投入 | P4.1 评估后 | 由对拍数据决定，不预设 |
| D4 | ANN 是否值得 | 库 ≥10 万时 | 先基准后决定 |

---

## 14. 时间估算（诚实口径，含自研序列化成本）

| 阶段 | 估算 | 说明 |
|---|---|---|
| P1 | 1-2 天 | 自研 JSON/TOML 子集是主要成本（0.5-1 天） |
| P2 | 2-3 天 | qmap + 反馈双环较重；P2.2-2.5 并行 |
| P3 | 2-3 天 | mmap + 自愈 + 倒排编码 |
| P4 | 2-3 天 | 评估系统 + 可选增强按触发条件 |
| P4.4+ 优化② | 半天 | 主流程 fix 行无条件输出 + --stats 库盘点 + 裸 "db" 桶判定词级化 |

合计约 **7-11 个工作日**；P1-P3 为必要主线，P4.3-4.5 可选。

## 15. P4.4+ 优化②实施记录（2026-10-22，已提交）

| 项 | 内容 | 验证 |
|---|---|---|
| 主流程直答 | `write_mem_lines` 的 `fix:` 行改为**无条件输出**（case 带 fix 即出，不依赖 `--recall-detail`）——L1 北极星"本地直答"在默认路径生效 | 端到端：`--query ... --recall 1`（无 detail）输出 `mem:` + `  fix: enlarge pool to 64 conns, restart daemon`；旧 case 无 fix 不输出 fix 行（golden 零漂移） |
| `--stats` 库盘点 | 新建 `rs/src/memory/stats.rs`（144 行）：`stats: cases=N gen=M` + `bucket:`/`status:`（key 字典序）/`fix: n/N (pct)`/`quality: mean=`/`disk: X.X KiB`（case 目录递归大小）；`mod.rs` 门面 `stats_table`；`cli_args --stats` 豁免 `--query`（含单测）；`main.rs` 接线（--l1-hist 后、--rate 前） | 单测 `table_reports_counts_and_coverage`（ingest + ingest_with_fix 造 2 case）；golden `memory_stats` 第 27 例（db + disk 两桶隔离日志，fix 1/2）；端到端 3 case 实测 |
| 桶判定精度 | stats 单测暴露裸 "db" 子串误判：`Db.java:4` 文件实体被归 db 桶 → compile 场景误归类、query "compile fail" 漏召。修复：FAMILIES db 家族**移除裸 "db" 子串**；`bucket_of` 对 terms 词级 `t == "db"`、`candidate_buckets` 对 `split_whitespace` 词级 `w == "db"`（宁多勿漏但别乱扩桶——"kubectl/obd" 不误扩） | bucket.rs 单测：`bucket_of(["db.java:4"], "compile fail") == "build"`、`candidate_buckets("kubectl get pods")` 不含 db；干净 compile 场景端到端 `bucket: build=1` |

**门禁终态**：73 库 + 8 CLI + 4 集成 + golden 27 例全绿；fmt/clippy 0；行数门禁（stats.rs 144 / bucket.rs 160 行）通过。
**遗留口径**：failopen 全文保留会把无关家族词带进 case.text → 桶归属随压缩结果变化（与 §11 评审④"text 是压缩结果"边界一致，非 bug）。

## 16. P4.4+ 优化③实施记录（2026-10-22，已提交）

| 项 | 内容 | 验证 |
|---|---|---|
| stored 报告 | `ingest`/`ingest_with_fix` 返回类型 `Result<bool>` → `Result<(bool, String)>`（新增标志 + case id；去重时返回既有 id）；门面 `store_case`/`store_case_fix` 透传；同步路径（cli_mem::store）与异步路径（main.rs 后台线程 tx 类型 `io::Result<(bool, String)>`）均 stderr 报告 `scx: stored case=<完整 id> added=yes|dedup`——用户拿到完整 id 可直接 `--rate`/`--delete` 闭环 | 端到端：命中 store `added=yes`、重复 store 同 id `added=dedup`、stdout 正文/统计零污染；golden 漂移仅 `memory_store.err` +1 行（最小） |
| failopen 低信号守卫 | 主流程 store 分支：`o.mode.starts_with("failopen")` 时 stderr 警告 `note: query had no hits; --store will persist the full input (low-signal memory). inspect with --recall-only first`（不拒绝——用户可刻意沉淀原文作底料） | 端到端：60 行零命中（mode=failopen:zero-signal）出警告；小输入直通（passthrough:small）不误报；命中（sieve）不误报 |

**门禁终态**：73 库 + 8 CLI + 4 集成 + golden 27 例全绿；fmt/clippy 0；测试调用点零改动（ingest 直调点均为丢弃返回值）。

## 17. P4.4+ 优化④实施记录（--eval-verbose 候选诊断，2026-10-22，已提交）

| 项 | 内容 | 验证 |
|---|---|---|
| `--eval-verbose` | eval.rs `run` → `run_impl(root, path, verbose)`（默认路径零漂移）；`run_verbose` 追加逐 case 诊断段：`case i/N q="..."` + `A:`/`B:` 各 top-10（rank + case 尾 8 + sim + score + `[hit]/[miss]`，与 --eval 的 tail8 口径一致）；verbose 时缓存每例 A/B 命中，避免二次召回 | golden `memory_eval_verbose` 第 28 例（复用 memory_eval 的 3 case 库与 baseline）：paraphrase 层 A mrr=0.5 vs B=1.0 的退化即刻可见——A top1 是 disk case `5e316932…`（[miss]），相关 db case `123e7dec…` 掉到第 2——诊断输出让标注者直接校准 |
| CLI 接线 | `--eval-verbose` 参数（需配 --eval，否则报错 exit 2；含单测）；`main.rs` --eval 分支按开关走 `eval_cases_verbose`；USAGE 同步 | cli_args 单测：`--eval-verbose` 单独使用报错、配 --eval 通过 |

**门禁终态**：73 库 + 8 CLI + 4 集成 + golden 28 例全绿；fmt/clippy 0；默认 `--eval` 输出逐字节不变（memory_eval.out 未漂移）。
**服务目标**：M4 证据门的 30–50 组人工标注——标注者可逐例核对 A/B 排序差异并据此校准 rel 标注与调参假设。

## 18. P4.4+ 优化⑤实施记录（case schema v2 原始证据，2026-10-22，已提交）

**背景**：外部评审产品缺口 B——"`--store` 存的是查询条件压缩结果，无证据行区间/原始证据，L1 只给摘要不是可复用答案"。fix（修复结论）已在优化①补齐；本轮补齐原始证据，case 升级为**四要素：query + 证据行区间（evidence_lno）+ 原始证据（evidence）+ 修复结论（fix）**。

| 项 | 内容 | 验证 |
|---|---|---|
| 引擎暴露保留行号 | `Outcome` 增 `kept_lno: Vec<usize>`（筛选模式保留行的原始 1-based 行号，输出循环同步收集；passthrough/failopen 为空）；`mk` 闭包 + 4 处调用点接线 | 单测 `sieve_kept_lno_aligns_with_output_lines`（数量=kept、严格递增、含证据行号）、`passthrough_kept_lno_empty` |
| 提取与落盘 | main.rs `extract_evidence(input, lno)` 按行号取原文（与 compress 同一归一化口径：\r\n→\n、末行剔除、越界跳过）；store 同步/异步路径传 evidence；ingest 新入口 `ingest_with_evidence`/门面 `store_case_full`（原 ingest/ingest_with_fix 签名不变，30+ 调用点零改动） | 单测 `ingest_with_evidence_roundtrip_and_legacy_compat`（落盘/旧 case 无键读空/长度不一致拒绝）；端到端：81 行输入 store → case.json `evidence_lno=[1,2,10,11,12,13]`、`evidence`=原始行，`--recall-detail` 输出 `evidence: 11 ERROR db timeout...` 等 |
| 直答回放 | `--recall-detail` 在 summary 后输出 `evidence: <行号> <原文>` 行（读一次 case 同时取 evidence/fix；无证据跳过）；主流程 fix 行行为不变 | 端到端 recall-detail 输出证据回放 + fix |
| 旧 case 兼容 | case.json 新键 evidence_lno/evidence；`val_to_case` 缺键读空 | golden memory_summary/memory_fix（旧库预置 case 无 evidence）**零漂移**——兼容实证 |
| 存储代价 | evidence 总长由保留行决定（≤ 输入）；golden memory_stats disk 1.3→1.4 KiB（新 case 含证据字段，1 行预期漂移） | golden 重冻 28 例全绿 |

**门禁终态**：76 库 + 8 CLI + 4 集成 + golden 28 例全绿；fmt/clippy 0；release ✓。
**语义边界**：passthrough/failopen 无保留行 → kept_lno 空 → 仅存压缩快照（text），低信号记忆语义不变（§16 口径）；`evidence` 是"保留行的原始文本"，与压缩结果同一行集，非全量输入原文。

## 19. P4.4+ 优化⑥实施记录（智能标注器 --annotate，2026-10-22，已提交）

**背景**：M4 证据门（30–50 组人工标注）长期未产出，是唯一未闭环的评审项。用户要求以"顶级智能标注"替代人工——实现**零依赖智能标注器**：不凭空造标注，每条 query 变体先经召回管线自检（目标 case 必须进 top-5），证据链随标注落盘。

| 项 | 内容 | 验证 |
|---|---|---|
| 三层生成 | `annotate::annotate(root, seed, max_cases)`：literal=case.query 原样（rel=2，证据 `fingerprint`）；paraphrase=seed 驱动的 SYN 同义词表字面替换（词边界保护，~45% 概率逐词 + 强制保底替换保证 ≠ 原 query），自检放行 rel=2/1（sim≥0.5 分档），证据 `synonym`+`recall sim/score/rank`；semantic=根因行（Root cause/cause/原因）词反译构造（与 query token 几乎不共享），自检放行 rel=1 | 单测 4：三层自检证据在链上、baseline roundtrip 进 eval、同 seed 逐字可重放、词边界替换不误伤（"db"→database 不碰 database 内嵌） |
| 自检 | `self_check` 走 `recall::recall`（纯检索零写入）top-5 查目标 tail16；未放行变体显式丢弃（dropped 计数），宁缺毋滥不编造 | 端到端：2 case 库 → 6 组（2×3 层），semantic query "root cause: 连接池 排查原因" 与 literal 零共享词 |
| CLI 接线 | `--annotate`（豁免 --query）、`--annotate-out <path>`（缺省 `<mem-dir>/baseline-auto.json`）、`--seed N`（缺省 0，确定性可重放）；`--annotate` 禁与 --eval/--l1-hist 同用 | cli_args 单测沿用；端到端：annotate → baseline → `--eval baseline-auto.json` 闭环出对拍表（A/B 全 1.0） |
| 输出 | baseline.json 兼容 eval 解析（extra 键 evidence/note 被忽略）；stdout 报告层分布/rel 分布 + out 文件名（相对，防跨机漂移）；expected 用 hash16 免日期漂移 | golden `memory_annotate` 第 29 例冻结：`cases=6 rel2=2 rel1=4`、layer literal/paraphrase/semantic=2、`out: baseline-auto.json` |

**门禁终态**：80 库 + 8 CLI + 4 集成 + golden 29 例全绿；fmt/clippy 0；release ✓。
**服务目标**：M4 证据门自动产出 30–50 组（16 case × 3 层 = 48 组），每条带证据链可审计；`--seed` 变化生成不同变体池，多 seed 融合可扩充。人工标注流程（docs/eval-baseline-template.md）保留作复核/抽样。

## 20. P4.4+ 优化⑦实施记录（召回一致性修复 + M4 自动回归集结论，2026-10-22，已提交）

**背景**：正式 M4 评估（优化⑥ 标注器 + --eval-verbose）暴露 A 管线劣于朴素 B 的 3 个真实缺陷——
①桶硬截断 + PRF 扩展词带偏桶判定（真目标被滤出候选集）；②PRF 扩展词统计含词面无关 case 的私有词；
③PRF 覆盖首轮结果（实体边桥入候选被降级）。另暴露标注器 2 个工程缺陷（CJK 切片 panic、semantic 自检误杀）与 1 个规模缺口（max_cases 不可调）。

| 项 | 内容 | 验证 |
|---|---|---|
| 桶一致性 | `recall()` 对原始 query 判候选桶一次；`score_pass` 接收 `cand/use_buckets` 参数；PRF 第二轮继承同桶——扩展词不再改变过滤语义 | 端到端：case `command not found kubectl`（other 桶）A 目标回归 top-1（修复前被 network 桶误滤）；golden memory_eval/memory_eval_verbose 重冻 |
| PRF 相关度门槛 | `prf::expand(root, hits, ...)` 改收 `&[RecallHit]`，仅统计 sim>0（词面相关）的 case——无关 case 私有词（bash/deploy 等）不再入扩展表 | prf 单测改签名适配；端到端 case `tls handshake 超时` A 目标回归（修复前扩展词带出 build 桶） |
| PRF 补漏不覆盖 | 扩展轮候选并入首轮（去重）按 score 重排截断——首轮命中（含实体边桥入、字段加权命中）永不降级 | edge 单测 `edge_bridge_recovers_cross_bucket_sibling` 修复前失败、修复后绿（桥入 B 保持 top-3） |
| 标注分层放行 | `self_check(root, q, c, k)` 加 k 参数：literal/paraphrase top-5、semantic（弱相关 rel=1）top-10 | annotate 单测沿用；端到端 semantic 放行 16→3（24 case 库，无根因行/error_code 的 case 不造语义变体=诚实边界） |
| eval 弱相关口径 | `metric` 返回 (r5, r10, mrr, ndcg)；聚合 [f64;9]；报告行 semantic 层显示 r@10 | eval 单测 `metric_recall_mrr_ndcg` 更新 4 元组；golden memory_eval 重冻（semantic 行 r@10） |
| CJK 切片安全 | `replace_ci` 边界检查改 `s[..pos].chars().next_back()`（pos 是 char 边界，防中文 query 字节切片 panic） | 新增单测 `replace_ci_safe_on_cjk_boundary`（"数据库连接池耗尽 pool" 替换"连接池"不 panic） |
| 标注规模可调 | CLI `--annotate-max N`（缺省 16，0=全库；annotate 内部 0→ids.len） | 24 case 库 `--annotate-max 0` 产 36 组落 30–50 窗口 |

**M4 自动回归集结论（36 组，24-case 可复现语料）**：
| 层 | n | A | B | 判定 |
|---|---|---|---|---|
| literal | 24 | r@5=1.000 mrr=0.958 ndcg@10=0.969 | r@5=1.000 mrr=1.000 ndcg@10=1.000 | r@5 持平；**MRR/NDCG A<B** |
| paraphrase | 9 | r@5=1.000 mrr=0.778 ndcg@10=0.836 | r@5=0.889 mrr=0.778 ndcg@10=0.807 | r@5 A>B；MRR 持平，n=9 无显著性 |
| semantic | 3 | r@10=1.000 mrr=0.833 | r@10=1.000 mrr=0.833 | 持平；n=3 不作质量结论 |
| l1 | 33 | hits=29/33 rate=0.879 | — | 学习闭环可用 |

**漏斗（本次 `--annotate` 实报）**：literal 24；paraphrase 9（14 个无 SYN 可替换词、1 个自检未进 top-5）；semantic 3（21 个无根因行/error_code、0 个自检失败）。24/9/3 不满足 §8“各层 ≥10”，literal 占 66.7%。

**口径**：自动标注先经 A 管线自检放行，B 不参与；因此该表是“A 已成功子集上的回归表”，不是独立 A/B 质量证据。literal MRR/NDCG A 低于 B，不能写“A≥B 全面成立”；paraphrase 的 r@5 优势是唯一正向信号，但样本量不足且无置信区间。

**门禁终态**：89 库 + 8 CLI + 4 集成 + golden 29 例全绿；fmt/clippy 0；release ✓。
**报告**：docs/m4-evidence-report.md（含方法学限制与 `scripts/m4-eval.ps1` 复现命令）。

## 21. P4.4+ 优化⑧实施记录（并发写 / 数字身份 / 证据口径，2026-10-22）

| 项 | 内容 | 验证 |
|---|---|---|
| 跨进程原子写 | 新增 `memory/lock.rs`：唯一临时名 `<file>.tmp.<pid>.<seq>` + rename（Windows sharing violation 有限重试）；`meta.toml` 的 gen/case_count/word_w/l1/params、qmap 追加/合并、postings 构建全部加锁（`meta.lock`/`qmap.lock`/`index.lock`） | 单测 `concurrent_case_writes_are_lossless`（8 线程 × 4 case：32 case、case_count=32、gen=32）；12 进程并发 store 12/12 exit 0、case_count=12、gen=12 |
| 数字身份 | `norm_identity_into` 保留 query 数字；case 去重、qmap 精确键、会话缓存键改用该口径；text 仍用 `norm_into` 折叠 PID/时间戳/行号；旧 case 通过动态重算指纹兼容 | 单测 `fingerprint_keeps_query_numbers_but_folds_text_noise`、`ingest_keeps_numeric_query_variants_and_dedups_legacy_fp`、`qmap_keeps_numeric_query_identity`；端到端 `port 8080` vs `port 9090` → 2 case，各自 top-1 正确 |
| 标注漏斗 | `AnnotateReport` 增加 `paraphrase_no_synonym` / `semantic_no_root`；stdout 输出 `dropped:` 与 `skipped:` 两类，避免只报放行集 | 单测 `annotate_reports_dropped_variants`；24-case 实报 36 组 + 14 no-synonym + 1 self-check + 21 no-root |
| 证据口径 | M4 报告/README/SKILL 改为“自动回归集可复现，独立质量证据未通过”；新增 `scripts/m4-eval.ps1` 固定 24-case 语料；semantic 仍报 r@10 但明确标注为事后放宽 | 脚本复跑（优化⑨）：36 组、literal A mrr 0.958 < B 1.000、paraphrase r@5 1.000 vs 0.889、l1 29/33 |
| 索引一致性 | `write_case` 写入后 `index::invalidate`：case 新增/更新立即让旧 postings 失效，避免大库继续用旧索引漏掉新 case（索引仍可重建，属缓存） | 单测 `write_case_invalidates_cached_index`；原 `invalidate_removes_index_file` 仍绿 |

**残余**：历史“单文件 <500 行”门禁已被 `cli_args.rs`/`eval.rs`/`recall.rs`/`annotate.rs` 等超过（非本轮引入），CI 未强制；拆分列为后续。

**结论**：并发写与数字身份是已修复的真实缺陷；M4 的“工具/回归集”可用，但“质量证据”仍未闭环，下一步是独立人工标注/盲测，而不是继续用自动标注自证。

## 22. P4.4+ 优化⑨实施记录（损坏自愈 / 同 case 并发 / 索引一致性，2026-10-22）

| 项 | 内容 | 验证 |
|---|---|---|
| qmap 损坏自愈 | append 在同一把 qmap 锁内先解码已有文件：残尾截断到最后一个完整记录、坏 header 重建后再追加；解码所有切片先校验边界。超长 query（>u16）、hit 数（>u16）、case id（>u8）明确拒绝，不再静默截断字段长度 | 单测：1 字节残尾后 lookup 不 panic、下一 append 后两条均可读；坏 header 自动重建；超长字段拒绝且不破坏已有记录 |
| 索引健壮性 | `postings.bin` 的 `body_len` 使用 checked add，n_docs/n_terms/df/字段数均做文件上界检查；单 token 在 `build_terms` 统一限制 255B（线性/索引同口径），避免 doc-term u8 长度截断错位 | 单测 `parse_rejects_overflowing_body_len`、`runaway_token_is_dropped`；全部既有索引/golden 测试通过 |
| 同 case 并发 | 新增 `store::lock_case`；`rate`、同指纹去重刷新、`delete` 共用 case 锁。同指纹 store 另有 ingest 锁，避免跨秒重复落库；空 fix/evidence 可补录，已有结论不覆盖 | 单测 `concurrent_rate_same_case_does_not_lose_feedback`（8 线程×5 good=40）、`dedup_backfills_missing_fix_and_evidence_without_overwriting` |
| 索引一致性 | `write_case`/`delete_case` 在 index 锁内完成“资产变更 → 作废 postings → bump gen”；`index::invalidate` 锁失败上抛，不再静默让旧索引继续服务 | 单测 `write_case_invalidates_cached_index`、`delete_case_invalidates_index`；无环锁序 case→index→meta/qmap |
| L1 写回 | `adapt_l1_base` 改 meta 锁 + `atomic_write`，与 word_w/gen/case_count 共用同一写路径 | 既有 4 条 L1 自适应测试通过 |
| 跨进程回归 | CLI 集成测试并发 spawn 8 个 store 进程 | `concurrent_store_processes_keep_all_cases_and_meta_updates`：8/8 成功、case 目录=8、case_count=8、gen=8 |

**M4 复跑（2026-10-22，release）**：`literal A mrr=0.958 ndcg=0.969 / B=1.000/1.000`；`paraphrase A r@5=1.000 MRR 0.778 / B r@5=0.889 MRR 0.778`；`semantic r@10=1.000 / 1.000`；`l1=29/33`。自动标注漏斗 24/9/3，结论仍为回归集可复现、独立质量证据未通过。

**门禁终态**：96 库 + 8 CLI + 5 集成 + golden 29 例全绿；fmt/clippy/release 0。


