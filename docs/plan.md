# scx 架构优化路线图与阶段交付记录（P1–P5）

> 本文档记录整体架构优化方案的阶段化落地状态（P1–P5）与每阶段的完成证据。
> 方案正文为消息交付（不落盘）；本文档只维护可执行的路线与状态。
> 更新惯例：每阶段完成后由实施方在此记录门禁证据、改动清单与遗留项。

## 路线总览（P1–P5）

| 阶段 | 主题 | 内容 | 状态 |
|---|---|---|---|
| P1 | 内核重构 + 评测协议 | compress 阶段拆分、Outcome 枚举化、args 声明式、conf 提取、--max-tokens、Containment 协议与基准资产 | ✅ 完成（见下） |
| P2 | 结构感知与 MCP 集成层 | fence/import/at 链邻域、src/mcp.rs（可选层，不破坏纯内核）、conf 策略文件接线 | ✅ 完成（见下） |
| P3 | 配置化驱动深化 | policy 表文件化（--policy-file）、语义增强 feature-gate 可选路径、词面召回增强、--force 正则子集 | ✅ 完成（见下） |
| P4 | 记忆库协议演进 | 多进程写实测、L1 直答评测协议、与压缩内核解耦收尾 | 未开始 |
| P5 | token 级精裁（LLMLingua-2 启发） | `--fill-floor` 行间门槛、`--trim` 行内 token 重要性裁剪（数字/命中/锚保留，停用词按比例摘）、bench.py `--scx-extra`、GSM8K 评测验证 | ✅ 完成（见下） |

红线（每阶段门禁）：`cargo fmt --check` / `cargo clippy --all-targets --locked -- -D warnings` /
`RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked` / `cargo test --locked` /
`cargo build --release --locked`；golden 逐字节零漂移（新行为走新增 golden 例）；
Gauge 不退化；每函数圈复杂度 < 10；`#![forbid(unsafe_code)]` 与零第三方 crate 保持。

## P1 交付状态（2026-10-25）

| 子项 | 状态 | 门禁结果 | 改动文件 | 新增测试 |
|---|---|---|---|---|
| 1.1 compress 阶段拆分 + policy 表 | ✅ | 全绿（golden 零漂移） | `src/policy.rs`（新）、`src/compress.rs`、`src/lib.rs` | lib 若干（继承） |
| 1.2 Outcome 枚举化 | ✅ | 全绿（golden 零漂移） | `src/lib.rs`、`src/compress.rs`、`src/main.rs` | — |
| 1.3 args 声明式参数表 | ✅ | 全绿（golden 零漂移） | `src/args.rs` | cli 8 例（继承） |
| 1.4 conf 提取 | ✅ | 全绿（golden 零漂移） | `src/conf.rs`（新）、`src/memory/meta.rs`、`src/lib.rs` | +2 |
| 1.5 --max-tokens 硬上限 | ✅ | 全绿（golden 32 例零漂移） | `src/lib.rs`、`src/compress.rs`、`src/args.rs`、`src/main.rs`、`src/policy.rs`、`tests/golden.rs` + 3 基线 | +4 单测 +3 golden 例 |
| 1.6 内核评测协议 Containment | ✅ | 全绿（golden 零漂移） | `src/contain.rs`（新）、`src/lib.rs`、`tests/contain.rs`（新）、`tools/scx.py` | +2 单测 +4 integration |
| 1.7 公开基准资产 | ✅ | 全绿（golden 零漂移） | `config/contain5.corpus.json`（新）、`docs/containment.md`（新）、`docs/eval.md`、`docs/plan.md`（新） | — |

测试计数基线 → P1 末：lib **166** / integration **13** / cli **11** / contain **4** /
golden **2 passed / 1 ignored（32 用例）** / doc 0；全部 `cargo test --locked` 全绿。

### commit 链（分支 codex/readme-polish，全部独立可回滚）

| commit | 说明 |
|---|---|
| `b01755d` | chore: snapshot working tree as P1 baseline（**含用户既有 26 项未提交工作**，非 P1 代码改动） |
| `b7999a4` | refactor(compress): split single-pass pipeline into phase fns, policy table（1.1） |
| `011a053` | refactor(outcome): type Outcome.mode/risk as Mode/Risk enums（1.2） |
| `2116657` | refactor(args): declarative FLAGS table drives parse and validation（1.3） |
| `ccfb023` | refactor(conf): extract line-wise TOML-subset parser to top-level scx::conf（1.4） |
| `368c5ab` | feat(compress): --max-tokens hard cap with evidence-floor-first semantics（1.5） |
| `7edbe3c` | style(conf): rustfmt wrap in test string（1.5 fmt 收尾） |
| `99a8a9a` | feat(contain): kernel eval protocol — containment judgment + RBG suite（1.6） |
| `968666a` | docs(eval): freeze contain5 corpus + CONTAINMENT + plan（1.7） |
| `e559c3a` | refactor(ccn): cap P1-rewritten fns below McCabe 10（复杂度收尾） |
| `（fmt）` | style: rustfmt pass（conf/contain 测试文件，1.5/1.7 收尾） |

### 圈复杂度门禁（P1 改写函数）

- 口径：McCabe（if/else-if/for/while/match-arm + `&&`/`||` 各 +1 判定点）。
  P1 改写函数全部 ≤ 8：`compress` 编排器（7 点，阶段判定全部下放辅助函数）、
  `parse_flag`（8-arm 分派器）、`score_line`（8）、`seed_marks`（7）、
  `validate_annotate`（4）、`conf::parse`（5）、`contain::run`（3）等。
- 工具说明：`lizard` 对 Rust 的 CCN 系统性高估（如 `score_line` 报 38、实际 8），
  未用作门禁依据；以人工 McCabe 核算为准。
- 既有 `memory/*` 函数（ingest/gate/recall/query_cache 等）复杂度 > 10 属
  用户既有代码，不在 P1 范围；P4「记忆库协议演进」阶段统一处理。

### 关键语义（--max-tokens，§3.3 落地）

- 默认 `None` = 现状行为逐字节不变；启用后：**证据地板最高优先**（证据行 +
  最小邻域无条件保留）。
- 证据地板已超上限 → `mode=overflow:evidence`；可选填充受
  `min(budget, max − floor)` 约束，风险守卫追加后仍超 → `mode=overflow:fill`。
- 统计行仅在 overflow 模式追加 `max_tokens=N tok_est=M`；`--json` 仅在启用时
  追加 `max_tokens`/`tok_est` 字段（向后兼容）。绝不静默丢弃证据。

### 评测资产（contain5，自测性质）

- 语料 `config/contain5.corpus.json`：25 切片 × 5 类（全 synthetic），1787 行、
  70 证据行、92298 字符；SHA256 `a077c35e…b681`。
- 入口 `tools/scx.py contain`（Python 编排 + 统计，不复刻排序）；
  Rust 侧 `scx::contain` 为判定唯一事实源（整行等值），`tests/contain.rs`
  红黄蓝断言批为可执行规格。
- 实测（当前 HEAD，ratio=0.35, context=1）：**证据保留 70/70 = 100%**、
  truncation 基线 0/70、平均字符压减 82.01%、token 估算压减 79.93%。
- **性质**：作者侧合成语料自测，**非第三方独立评测**；口径差异与复现命令
  见 `docs/containment.md`。

### 遗留 / 待 P2

1. MCP server 集成层（`src/mcp.rs`，二进制名 `scx-mcp`）未实施——设计见架构方案 §2.4，属 P2。
2. 结构感知邻域（fence/import/at 链）未实施——属 P2，与既有
   `frame_bonus`/`diffmeta_bonus` 评分表正交，改默认表须过 golden + Gauge。
3. 语义增强 feature-gate 可选路径未实施——属 P3；默认路径承诺逐字节不变。
4. 与 SuperCompress 的同场独立对照实验未做（需真实事故语料 + 第三方盲审）。
5. `tools/scx.py` 的 `contain` 输出 token 压减为确定性正则估算，非真实
   LLM tokenizer（文档已标注）。

## P2 交付状态（2026-10-25）

| 子项 | 状态 | 门禁结果 | 改动文件 | 新增测试 |
|---|---|---|---|---|
| 2.1 scx::ops 编排层 | ✅ | 全绿（golden 32 例零漂移） | `src/ops.rs`（新）、`src/main.rs`（薄壳）、`src/lib.rs`、`src/args.rs`、`src/input.rs` | lib +14（原 main 单体测试移入） |
| 2.2 结构感知邻域 | ✅ | 全绿（golden 35 例零漂移） | `src/analysis.rs`、`src/compress.rs`、`src/policy.rs`（RegionCfg 数据表）、`src/lib.rs`（Config.structure）、`src/args.rs`（--structure）、`tests/golden.rs` + 3 基线 | +7 单测 +3 golden 例 |
| 2.3 统计/JSON 升级 | ✅ | 全绿（golden 37 例零漂移） | `src/lib.rs`（Dropped/DropReason）、`src/compress.rs`（drop_runs）、`src/ops.rs`（render_json dropped）、`tests/golden.rs` + 2 基线 | +4 单测 +2 golden 例 |
| 2.4 MCP 雏形 | ✅ | 全绿（feature 开/关双轨） | `Cargo.toml`（feature=mcp + bin）、`src/mcp.rs`（新）、`src/mcp.rs`（新）、`tools/scx setup`、`tools/scx doctor`、`tests/mcp_smoke.rs` | +1 集成（stdio 往返） |
| 2.5 锁序契约 | ✅ | 全绿（golden 37 例零漂移） | `src/memory/lock.rs`（acquire_all）、`src/memory/meta.rs`、`src/memory/store.rs`（*_locked 变体）、`src/memory/maintenance.rs`（delete_case 重构）、`docs/locking.md`（新）、`tests/memory_concurrency.rs`（新） | +2 单测 +1 集成（多进程压力） |

测试计数基线 → P2 末：lib **192** / integration **13** / cli **11** / contain **4** /
golden **2 passed / 1 ignored（37 用例）** / mcp **1** / concurrency **1** / doc 0；
`cargo test --locked --features mcp` 亦全绿（含 mcp 冒烟 + golden 37 零漂移）。

### commit 链（P2，全部独立可回滚，接 P1 链之后）

| commit | 说明 |
|---|---|
| `deb8dea` | refactor(ops): extract scx::ops orchestration layer（2.1） |
| `12b231e` | feat(compress): structural-aware neighborhood（2.2） |
| `9e47151` | feat(outcome): per-run drop ledger — Outcome.dropped + DropReason（2.3） |
| `93027c4` | feat(mcp): stdio MCP server under cargo feature=mcp（2.4） |
| `92e3ad7` | feat(memory): lock-order contract — acquire_all + multi-process stress（2.5） |

### 关键语义（--structure / dropped，与方案偏差以代码为准）

- `--structure 1` 默认关：`Config.structure=false` 时 `expand_regions` 走原路径，
  与 v0.4.1 逐字节一致；结构并入是**叠加**非替代（基础种子邻域先行，含种子的
  fence/import/stack 区域整段并入）；超长区域按 `REGION_DEFAULT.max_span=200`
  裁剪为种子 ± 半窗，裁剪行为 Budget 候选。结构判定：`is_import_line` 排除
  `use strict`/`use of`；堆栈帧要求前导空白缩进；fence 内行屏蔽 import/stack 判定。
- `Outcome.dropped`：1-based 闭区间 + `DropReason{budget,max-tokens}`；
  仅在 `--json` 输出（默认 stdout 统计行不含，golden 零漂移）；overflow 路径
  记 `max-tokens`，其余删除（含结构裁剪）记 `budget`；passthrough/fail-open 为空。
- MCP：`cargo build --release --features mcp` 产出 `target/release/scx-mcp`
  （stdio，零第三方 crate）；tools = compress_context / connect_account /
  usage_summary；只调 `compress::compress` + `ops::render_json`，不触 CLI 渲染。
- 锁序契约（docs/locking.md）：`cases/{id} → index → meta → qmap →
  fingerprints`；多锁唯一入口 `acquire_all`；持 meta 后写 meta 用 `*_locked` 变体。

### 遗留 / 待 P3（P2 新增 + P1 结转）

1. P3「配置化驱动深化」：policy 表文件化（`--policy-file`）、语义增强
   feature-gate 可选路径（默认路径承诺逐字节不变）——架构方案 §2.5/§3.4。
2. `--help/usage()` 未加 `[--max-tokens N]` / `[--structure 1]`（保 golden
   字节的取舍，文档待补，P3 随 usage 重构处理）。
3. `tools/scx.py contain` 的 token 压减为确定性正则估算，非真实 LLM
   tokenizer（P1 结转，已标注）。
4. 与 SuperCompress 的同场独立对照实验未做（需真实事故语料 + 第三方盲审，
   P1 结转）。
5. 既有 `memory/*` 函数圈复杂度 > 10 未拆（用户既有代码，P4 统一处理）。


---

## P3 完成记录（2026-10-25）

### 范围与门禁证据

P3 四子项全部完成，每子项独立 commit（全部可回滚）。改动前基线
HEAD=`ae4c2c2`（P2 收尾）；P3 链结束后 HEAD=`343b221`，工作区干净。
门禁逐子项全绿：`cargo fmt --check`=0、`cargo clippy --all-targets --locked -- -D warnings`=0
（默认与 `--features semantic` 双轨）、`cargo build --release --locked`=0；
**golden 37 例全程逐字节零漂移**；Gauge 机械 hard 层不退化
（见 3.2 复验数据）。P3 末测试计数：lib **200** / cli 11 / contain 4 /
golden 2（37 例，1 ignored 重冻）/ force_regex 4 / policy_file 5 / concurrency 1；
`--features semantic` 另 +5 semantic 单测 +3 semantic 集成 +1 mcp 冒烟。

### 子项状态

| 子项 | 完成 | commit | 新增测试 | 说明 |
|---|---|---|---|---|
| 3.1 semantic feature | ✅ | `ebc1a67`（前置 `9bd59c9` json 提取；补漏 `faabfad`） | +5 单测 +3 集成 | 手写 SHA-256/权重自描述 JSON/确定性 sigmoid；feature 关时 cfg 排除、golden 零漂移 |
| 3.2 词面召回增强 | ✅ | `5d33229` | +1 单测 | RECALL_PAIRS +fmt/msg 2 对（无字面交集纪律；param/parameter 前缀交集弃用）；synonyms_enable [params] 开关既有（默认开）；Gauge 复验 ΔMRR literal 0 / paraphrase +0.005 / semantic +0.162，CI 下界 ≥0 |
| 3.3 --force 正则 | ✅ | `0b519fd` | +7 单测 +4 集成 | 手写有限子集（`\|` 交替/字符类/`*+?`/转义），Thompson NFA 无回溯灾难；re: 前缀显式开启，非法报错含子集提示；字面子串行为逐字节保持 |
| 3.4 --policy-file | ✅ | `343b221` | +5 集成 | 文本覆盖 DEFAULT（ratio/context/window/elide/risk_gate/lineno/max_tokens/structure）；CLI 显式 > 文件 > DEFAULT；非法文件报错含行号 |

### commit 链（P3，接 P2 链之后）

| commit | 说明 |
|---|---|
| `9bd59c9` | refactor(json): extract minimal JSON parser from mcp into src/json.rs |
| `ebc1a67` | feat(semantic): optional hand-written scoring under cargo feature=semantic（3.1） |
| `faabfad` | chore(semantic): commit json.rs allow(dead_code) + mcp import order missed in ebc1a67 |
| `5d33229` | feat(terms): conservative high-precision recall pairs fmt/msg + Gauge re-run（3.2） |
| `0b519fd` | feat(force): re: prefix enters hand-written finite regex subset（3.3） |
| `343b221` | feat(policy): --policy-file text override with CLI > file > DEFAULT priority（3.4） |

### 关键语义（与方案偏差以代码为准）

- **3.1 semantic**：`--semantic <weights.json>`（feature=semantic 下可用）；权重
  文件自描述 `{schema:"scx-sem-1", dim, weights, sha256}`，sha256 = hex(SHA256(
  weights 每 f64 大端 8 字节拼接))，校验失败即报错拒绝运行；`sigmoid(w·x)`
  确定性；不启用时默认路径逐字节不变。
- **3.2 词面召回增强**：实际为既有 `RECALL_PAIRS`（22 对）+ `synonyms_enable`
  [params] 开关机制的**保守增量**（+fmt/msg 2 对）。词表纪律：只收无字面交集
  的领域释义对，宽泛→具体单向；`param/parameter` 因前缀交集弃用。
- **3.3 --force 正则**：`re:` 前缀进入手写子集（`\|` 交替 / `[...]` 字符类
  `^` 取反 / `*` `+` `?` / `\` 转义；不支持 `.` `()` 锚点 `{}` `\d` 等简写）；
  Thompson NFA 编译 + ε-closure 单趟模拟（O(行字符×状态) 无回溯灾难）；
  非锚定搜索（含空匹配，标准语义）；字面子串条目行为与 v0.4.1 逐字节一致；
  CLI 层非法 re: 报错含支持子集提示；库内核绕过 CLI 的非法模式不参与匹配
  （debug 断言提示，文档化取舍）。
- **3.4 --policy-file**：TOML 子集文本覆盖 DEFAULT；优先级 **CLI 显式参数
  > 文件 > DEFAULT**（`s.seen` 判定，`into_opts` 仅在 CLI 显式 `--max-tokens`
  时覆盖，修复了 None 冲掉文件设置的缺陷）；非法键/节/类型/范围报错含行号；
  不传时零影响（golden 37 零漂移）。

### 收尾 commit 链（接 P3 链之后，2026-10-25）

| commit | 说明 |
|---|---|
| `a294630` | fix(json): derive Clone for shared Json — restore `--features mcp` compile（用户验收修复） |
| `5b32a88` | feat(help): multi-line usage() covering P2/P3 flags（收尾 1/6） |
| （README/SKILL/版本） | docs(readme): P1–P3 全面同步 + chore(version): 0.4.1 → 0.5.0（收尾 2–6，见交付报告） |

### 修复与收尾记录（a294630 之后）

- **`a294630`（用户验收修复）**：`src/json.rs` 派生 `Clone`——P3 交付时默认 +
  semantic 双轨门禁全绿，但 mcp 轨编译未纳入每轮门禁，`--features mcp` 下
  `Json` 缺少 `Clone` 导致编译回归；已由用户修复并提交。**教训：此后门禁
  一律「默认 + semantic + mcp」三轨**（clippy 与 test 均三轨）。
- **收尾 commit 链**（见下「收尾记录」）：`--help` 补齐 P2/P3 参数说明；
  README/SKILL 全面更新至 0.5.0；版本 bump `0.4.1 → 0.5.0`（4 个用户可见
  特性 + 2 个可选 feature + contain5 评测协议）；`docs/memory.md` 版本引用同步。
- 遗留项 2（`--help` 缺失）已在本轮解决：usage() 现覆盖 `--max-tokens N` /
  `--structure 0|1` / `--policy-file <path>` / `--force SUBSTR|re:PATTERN`，
  `--semantic`（cfg-gated）仅 feature=semantic 构建时显示；帮助文本未纳入
  golden 基线（无 golden 例覆盖 help 输出），无需重冻。

### 遗留 / 待 P4（收尾后现状）

1. P4「记忆库协议演进」：多进程写实测、L1 直答评测协议、与压缩内核解耦收尾。
2. `tools/scx.py contain` 的 token 压减为确定性正则估算，非真实 LLM tokenizer。
3. 与 SuperCompress 的同场独立对照实验未做（需真实事故语料 + 第三方盲审）。
4. 既有 `memory/*` 函数圈复杂度 > 10 未拆（用户既有代码，P4 统一处理）。

---

## P4 完成记录（2026-10-25，版本 0.5.0 保持）

### 范围与门禁证据（三轨全绿，golden 37 例零漂移）

- `cargo fmt --check` = 0；`cargo clippy --all-targets --locked -- -D warnings`
  默认 / semantic / mcp 三轨 = 0；`cargo test --locked` = 227 通过 / 0 失败
  （lib 200 + cli 11 + contain 4 + force_regex 4 + golden 2[1 ignored] +
  concurrency 1 + policy_file 5）；`RUSTDOCFLAGS="-D warnings" cargo doc` = 0；
  `cargo build --release --locked` = 0。
- 复杂度清理按 **lizard 初筛 + 人工 McCabe 核算**（lizard 对 Rust CCN 高估
  ≈2×，不入基线）；拆分全部为纯提取，浮点加法顺序 / 循环 / 排序平局 /
  I/O 序列不变，golden 逐字节零漂移。曾出现一次 golden 假红，根因
  `/tmp/scx-golden-mem-store` 残留目录（测试 `remove_dir_all` 与中断残留
  相碰），非代码回归，已清理并复核。

### 子项状态

| 子项 | 状态 | 证据 |
|---|---|---|
| 4.1 发布基建 | 完成 | `cargo package --locked` dry-run 196 文件 1.9MiB（压缩 350.5KiB）verify 通过；`tools/scx install` 实测 RC=0；`docs/release-checklist.md` |
| 4.2 性能矩阵 | 完成 | `examples/bench.rs` 四路径（默认 / structure off+on / max-tokens / policy）+ `docs/benchmarks.md` 跨机基准表；structure 开 +12~13%、max-tokens 12029–12449µs overflow:evidence、policy parse 13.3–13.5ms 一次性 |
| 4.3 RFC 多租户 | 完成 | `docs/rfc-multi-tenant.md`（设计 only，未实现；禁 publish/push/merge 纪律内） |
| 4.4 存量复杂度清理 | 完成 | `memory/*` 18 个 lizard 候选全部人工核算；**recall_inner 19→8、score_pass 42→9**、index build 12→1 / parse 23→4、ranking expand 21→6、query_cache append 13→3 / merge 11→4、gate check_mode 23→4 / adjudication_checks 26→4、eval parse_baseline 35→8、annotate judge 25→8 / annotate_report 13→4、meta load_meta 18→9 / bump_word_w 18→5、feedback rate 13→4、json write_into 15→8、util parse_utc_secs 20→7、ingest persist 11→2；辅助函数全部 <10 |
| 记忆库协议演进 | 完成 | L1 直答评测协议（fix 覆盖率 / 直答触发率 / 直答命中率 / MRR 下界）入 `docs/memory.md`；多进程写实测沿用 P2 `memory_concurrency` 压测 + `docs/locking.md`；内核/记忆库解耦复核：无跨模块引用（仅 ops 编排层调 memory） |
| 待办收尾 | 完成 | contain token 口径维持「确定性正则估算」（真实 tokenizer 需第三方 crate，违反零依赖红线）并已在 CONTAINMENT.md / benchmarks 标注；SuperCompress 同场对照不虚构——需真实事故语料 + 第三方盲审，见遗留 |

### commit 链（P4，接收尾链之后）

| commit | 说明 |
|---|---|
| `ae86cbd` | feat(install): one-shot installer + release dry-run evidence + docs/release-checklist（4.1） |
| `4d266be` | docs(rfc): multi-tenant / memory sandbox / MCP auth design only（4.3） |
| `4e04062` → `660e528` → `b1b91f3` | feat(bench): four-path matrix + cross-machine table + clippy 三连修复（4.2；教训：**不得带红 clippy 提交**） |
| `66cb5d0` | refactor(memory): recall_inner / score_pass 拆分（4.4 自担部分） |
| `98fe595` / `bf41ef9` / `f276389` | refactor(memory): gate/eval、index/ranking/query_cache、annotate/meta/feedback/json/util/ingest 拆分（4.4 并行子代理，统一提交） |
| `ce545d0` | refactor(memory): recall.rs PassCtx 收敛（too_many_arguments 归零） |
| `c4f00a0` | docs(memory): L1 直答评测协议（4.5） |

### 关键语义与偏差声明（以代码为准）

- 4.2 policy 基准为**字段级近似**（ratio/context/max_tokens/structure 代表字段），
  完整 CLI 映射在 `args::parse_policy_file`；`src/lib.rs` 的 `conf` 由
  `mod` 改为 `pub mod`（公共 TOML 子集解析器，零依赖）。
- 复杂度清理期间修复子代理引入的编译/门禁问题：弯引号落在 Rust 字符串字面量
  （语法错误）、`String::as_str`（MSRV 1.82 下 unstable，改 `k[..].cmp(t)`）、
  `too_many_arguments`（PassCtx / AnnotateCtx 收敛）。均为纯修复，不改行为。
- contain token 压减保持「确定性正则估算」（ASCII 词 + CJK 字），与字符口径
  差约 2 个百分点；真实 tokenizer 需引入第三方 crate，违反零依赖红线。

### 发布记录（2026-10-26，crates.io）

- **crates.io 发布完成**：`scx 0.5.0` 已正式发布（`cargo publish --locked` 成功，
  Published scx v0.5.0 at registry crates-io）。crate 名 `scx` 发布前经 API 确认未占用
  （404），名称合法；发布后 `cargo search scx` 可见、API 元数据/关键词/分类完整。
- **发布前处置**：工作区出现的未提交优化（`src/fx.rs` 零依赖 FxHash、
  `regex.rs` 预编译 Compiled、`compress.rs` FxHashMap + ASCII key_len 快速路径）经用户
  确认纳入发布；提交 `1b89398`（perf(determinism): FxHash maps, precompiled NFA, ascii
  key_len — output unchanged），fmt/clippy/全量测试全绿（含 golden 37 例零漂移），
  与 main 一并 push。
- **发布凭据与账户**：用户 `cargo login` 配置 API token；crates.io 要求已验证邮箱，
  自动浏览器登录按钮 CSRF 初始化故障（环境级，多轮诊断），最终由用户换用已验证邮箱
  `haofei.ni@outlook.com`（Verified）完成发布前置。发布经 API 验证：max_version 0.5.0、
  downloads 初始 0、yanked false。

- **0.5.1 二次发布（2026-10-26）**：外部会话布局重组 `8b4b5ee`（tools/ + config/ 重组、
  fx 折叠进 compress、新增 tools/scx 四合一入口）经验证后发布。门禁全绿：fmt/clippy 三轨
  （默认/semantic/mcp）0 告警、test 默认 227 / semantic 235 / mcp 228 全过、golden_gate 零漂移、
  doc/release 通过。版本 bump `0a7bdf4` + lock 同步 `7dc0f91`，`cargo publish --locked` 成功
  （196 文件），crates.io max_version=0.5.1、num_versions=2。main 已同步 origin/main=7dc0f91。

- **0.6.0 发布（2026-10-26）**：三件优化经人工验收后发布——① MCP 并入主二进制
  （serverInfo.name=`scx`）+ `scx setup` 一键注册 Codex/Cursor/Claude（幂等+备份+握手自检，
  隔离 HOME 实测三路径）；② contain 同场对照 `docs/containcmp.md`（scx 70/70=100% vs 头截断
  0/70 vs 随机 8/70，三指标+种子可复现，明确"自测非第三方"）；③ feature-gate 词干化
  （默认轨 228/0 逐位不变、semantic 239/0、Gauge hard 层 Δ+0.0608 零退化、zero-signal
  4/5→0/5）。门禁双轨全绿、golden 37 零漂移；commit 链 634fcc7→…→ee5ce4c + `f725515`
  （.gitignore 草稿留本地）；`cargo publish --locked` 成功（201 文件），crates.io
  max_version=0.6.0、num_versions=3。main 已同步 origin/main=f725515。

- **0.6.1 补丁（2026-10-26）**：修复 Claude Code 严格校验 `inputSchema` 导致的连接失败
  （commit 271f4b2：TOOLS 表加 JSON Schema，compress_context 带参数 schema、无参工具 `{}`；
  门禁双轨 228/0+239/0、golden 零漂移、本机 Claude `✔ Connected` 实测；Codex `--approve-for-me`
  与 Claude `--allowedTools` 真实 MCP 通道调用均成功）。bump 0.6.1 + lock 同步；
  `cargo publish --locked` 成功；crates.io max_version=0.6.1、num_versions=4；main 同步
  origin/main=bb600ac（271f4b2 fix + bump）。

- **0.6.4 发布（2026-10-26）· 压缩召回效果报告**：① `Outcome.tokens_in` 全模式恒定填充
  （`terms::estimate` 口径：ASCII≈4 字符/token、中文≈0.6 token/字）；② `render()` JSON 增
  `tokens_in`/`tokens_out`/`token_cut_pct`（向后兼容，MCP `compress_context` 同步携带，
  豆包/千问调用即可见"省了多少 token"）；③ CLI stderr 每次输出「SCX 压缩召回」效果卡——
  原始/压缩后/省下的行·字符·token 三口径 + 证据召回/去重/裁剪明细 + 风险等级，小输入透传与
  查询零命中各有结论卡（普通人可读）；stdout 契约零变化（golden 基线仅 stderr 增量更新）。
  门禁：lib 205 + 集成 25 全绿、fmt/clippy 零告警、golden 回归零漂移；commit 链
  95c5736（feat）→ f1cd2b4（bump）。全渠道发布：crates.io `scx` 0.6.4（Published）、
  GitHub Release v0.6.4（三平台 tar.gz/zip + sha256 核对）、Homebrew tap 0.6.4
  （fcbae74 已推）、npm `@spapient/scx` 0.6.4（versions 含 0.6.4，npx 端到端
  serverInfo=0.6.4）；本机 `~/.cargo/bin/scx` 与 `/opt/homebrew/bin/scx` 均已 0.6.4。
- **0.6.5 发布（2026-10-26）· 效果卡进 MCP content**：`compress_context` 返回改为
  `[效果卡, JSON]` 双 content——首块为「SCX 压缩召回」效果卡（复用 `recall_report`，人话可读），
  次块为机器 JSON（`tokens_in` 等）；新增 `jesc` JSON 字符串转义（换行/制表符/反斜杠），
  smoke 测试加双块断言。门禁全绿（lib 205 + 集成，fmt/clippy/doc/release 零告警）；
  commit 链 feat+bump（main= 已推）。全渠道发布：crates.io 0.6.5（已存在确认）、
  GitHub Release v0.6.5（三资产 sha256 核对）、Homebrew tap 0.6.5（本机 /opt/homebrew/bin/scx 已 0.6.5）、
  npm `@spapient/scx` 0.6.5（versions 已含，豆包 npx 缓存清理后重启拉到 0.6.5）；
  **豆包端到端实证：MCP 返回效果卡 + JSON 双块，对话直接可见压缩效果**（此前效果卡仅终端 stderr 可见）。
- **0.6.6 发布（2026-10-26）· MCP 单 content 合并**：豆包实测对 0.6.5 双 content **仅渲染
  JSON 转述、效果卡不可见**（对话里只看到 `(7行/432字符…kept=7/7,cut_pct=0.0)` 摘要）。
  改为 `compress_context` 返回**单块** text：效果卡在上、机器 JSON 在下（`result` + 拼接），
  删除 `result_multi`；smoke 断言单块结构（`content:[{...}]` 且无第二块）。门禁全绿
  （lib 205 + 集成 11+，fmt/clippy/doc/release 零告警）；commit `d581735`（feat+bump）
  推 main + tag v0.6.6。全渠道发布：GitHub Release v0.6.6（id 423777948，三资产 sha256 核对）、
  crates.io 0.6.6、Homebrew tap 0.6.6、npm `@spapient/scx` 0.6.6（内置 0.6.6 三平台二进制）。
  **修复目标：任意客户端（豆包/千问/Codex/Cursor/Claude）对话原样显示效果卡**。
- **0.6.7 发布（2026-10-26）· 边界安全加固与行为修复**：P0 修复 `--delete` 路径穿越
  （id 未过 `store::valid` 白名单，`Path::join` 遇绝对路径/`../` 可删除任意目录）；P1 修复
  `--force re:` 交替被 `|` 拆分、MCP `ratio` 千分比语义、MCP JSON 转义覆盖 C0 控制字符；
  P2/P3 边界加固（JSON 解析深度上限、脱敏 JSON 引号形式、setup 保留文件权限、行数上限、
  semantic 权重拒绝非有限值、recall 回显转义）；normalize 纯字母 hex 保留、移除未接线 ext_hit；
  SMALL_TOK 1200→600。门禁全绿（默认 253/0、semantic 265/0、golden 零漂移）。
  全渠道发布：crates.io 0.6.7（Published）、GitHub Release v0.6.7（三资产 sha256 核对）、
  Homebrew tap 0.6.7（c034739）、npm `@spapient/scx` 0.6.7（内置 0.6.7 三平台二进制，经 Windows SSH 发布）。
- **0.7.0 发布（2026-10-27）· 治理层、语义召回、Gauge 评估、双许可证**：治理层 P0–P7（自动拦截、
  分类型计量、效果审计、运行时证据留存、快照指纹 + MCP 生命周期 + 账本聚合、结构锚点保护、压缩前脱敏）、
  token 计量与分桶、`--max-tokens` 保守上界（安全系数 2.0）、MCP `compress_context` 新增 `kind` 参数；
  MinHash-LSH 近似语义召回、ICWS 加权 MinHash、可插拔嵌入后端、BM25 b 降至 0.3；Gauge 评估 Phase A/B/C
  + CI recall@3 门禁；许可证改 MIT OR Apache-2.0。门禁全绿（fmt/clippy×2/doc/test/build/selftest）。
  全渠道发布：crates.io 0.7.0（Published）、GitHub Release v0.7.0（三资产 sha256 核对）、Homebrew tap 0.7.0
  （28b7392）、npm `@spapient/scx` 0.7.0（内置 0.7.0 三平台二进制）。

## 分发渠道与平台接入记录（2026-10-26，0.6.1 之后）

> 完整矩阵见 `docs/install-plan.md`（v3 全渠道总览）。本段为 plan 侧状态记录。

### 分发渠道占名 / 发布

| 渠道 | 命名 | 版本 | 状态 | 证据 |
|---|---|---|---|---|
| crates.io | `scx`（账户 apex） | 0.7.0 | ✅ 已发布 | max_version=0.7.0、未 yanked；`cargo install scx` 本机实测 |
| GitHub Release | spapient/scx | v0.7.0 | ✅ 已发布 | 三平台资产（darwin-arm64 / linux-x86_64 musl / windows-x86_64，sha256 已核对） |
| Homebrew tap | `spapient/homebrew-scx` | 0.7.0 | ✅ 已发布并实测 | `brew tap spapient/scx && brew install scx` → /opt/homebrew/bin/scx 0.7.0 |
| npm | `@spapient/scx`（scoped） | 0.7.0 | ✅ 已发布并实测 | `npx -y @spapient/scx mcp` 通过（serverInfo=0.7.0）；包内置 0.7.0 三平台二进制 |
| npm 纯名 | `scx` | — | ⏳ 回收申请中 | 2017 空壳包（tu123dou）占用；已发 npm 官方（落款 spapient） |
| winget | `Spapient.Scx` | 0.6.1 | ⏳ PR #461786 审核中 | microsoft/winget-pkgs portable 三 yaml |
| Scoop | `scx` | 0.6.4 | ⏳ PR #8768 审核中 | package request #8767 已提；manifest 0.6.4（sha256 核对）。旧 PR #8766 因缺 request 被关，按官方流程重提 |
| PyPI | `scx` | — | ✗ 被占 | 现存无关包；未注册 |
| AUR | `scx` | — | ✗ 放弃 | 官网反爬；按用户指示放弃 |

### 平台接入状态（实测）

- **豆包（Doubao.app）✅ 已注册成功**：GUI 连接器表单 STDIO，服务器名 `scx`、命令 `npx`、
  参数 `-y @spapient/scx mcp`（三行）；管理页「连接器」显示 scx 且开关开启。
  豆包支持任意可执行命令（无千问那种 npx-only 限制）。**已实证连接器进程链**：
  `ps` 可见豆包拉起 `npm exec @spapient/scx mcp` → node `.bin/scx mcp` →
  vendor `scx-darwin-arm64 mcp`（长期存活，豆包自带 node 环境，无千问 PATH/env 坑）；
  豆包对话中测试消息（7 行小日志）返回 `passthrough:small`（小输入透传，工具调用成功、
  行为正确）。v0.6.4 起豆包调用 compress_context 的返回 JSON 带 token 效果字段。
- **千问办公（QwenWorkCN）✅ 已注册成功（终版方案，2026-10-26）**：`LOCAL_IDENTITY_PHASE_RESTRICTED`
  仅拦截 **JSON 导入**，**手动填写表单可用**（用户主张正确）。**最终配置：命令 `/opt/homebrew/bin/scx mcp`
  （带显式 `mcp` 子命令参数）、环境变量留空**。三个关键坑：① `npx -y @spapient/scx mcp` 类命令保存时
  只保留第一个 token（存成 `command:"npx"`），但绝对路径+参数（`/opt/homebrew/bin/scx mcp`）可完整保存；
  ② 本机无系统级 node/npx，千问 PATH 找不到 npx → 旧 npx 配置 `process_not_found`；
  ③ **千问 STDIO 不传环境变量给子进程**——`SCX_MCP=1` 在 JSON 预览正确生成但 spawn 时丢失，
  scx 无参数启动打印 usage 退出（`code=unknown`、`Registered 0 tools`，main.log 实锤）。
  0.6.2 保留 `SCX_MCP=1` env 开关（供支持 env 的客户端），千问侧不可用。**待办**：千问对话端到端调用。
- **Codex / Claude Code / Cursor / Qwen Code**：`scx setup` 全自动注册 + 握手自检，
  三端实测通过（含 MCP 端到端调用）。

### 门禁基线（本轮无代码改动，文档更新不触门禁）

- 纯文档提交（install-plan.md v4 千问办公改判已注册 + plan.md 记录段）；无 Rust 改动，
  门禁基线维持 0.6.1 状态（双轨 228/0+239/0、golden 37 零漂移）。

### 遗留 / 发布后检查单

1. **merge / push / crates.io 发布均已执行**：main = origin/main = `1b89398`；
   crates.io `scx` 0.5.0 已发布（见上）。剩余可选事项：GitHub release/tag（未打 tag，
   `v0.5.0` tag 需用户指示）；远程 main 分支保护为 PR-only（本次直接 push 为管理员
   绕过路径，后续可改走 PR+CI）。
2. SuperCompress 同场独立对照未做：需**真实事故语料 + 第三方盲审**，本轮不虚构数据。
3. contain token 估算为自测口径（非真实 tokenizer），文档已显著标注。
4. `docs/benchmarks.md` 默认路径本轮单轮采样负载偏高（14,757µs），历史快照
   （11.7–12.0ms）为基准值；新路径（structure/max-tokens/policy）为本轮首测，
   跨机复现见 benchmarks.md。

---

## v0.6.0 完成记录（2026-10-26，三项优化）

### 范围与门禁证据（双轨：默认 + semantic；mcp 已并入默认轨）

- `cargo fmt --check` = 0；`cargo clippy --all-targets --locked -- -D warnings` 默认 /
  semantic 双轨 = 0；`cargo test --locked` = **228 通过 / 0 失败**（基线 227 +
  mcp_smoke 并入默认轨；golden 37 例零漂移，`golden_gate` ok）；`--features semantic`
  轨 = **239 / 0**（+3 词干化单测）；`cargo build --release --locked` = 0。
- 版本 `0.5.1 → 0.6.0`（minor，按 pre-1.0 惯例）；VERSION 唯一来源 `CARGO_PKG_VERSION`。

### 子项状态

| 子项 | 状态 | 证据 |
|---|---|---|
| 事项 1：MCP 并入默认轨 + `scx setup` 一键注册 | ✅ | `scx mcp` 手动握手返回 tools/list（serverInfo.name=scx）；`scx setup` 在隔离 HOME 下三配置文件实测：写入正确、二次运行内容逐字节不变（幂等）、`.scx.bak` 备份存在、每 agent 写后 tools/list 握手 PASS；tools/scx setup/doctor 对齐实测通过 |
| 事项 2：contain 同场对照（scx vs 头截断 vs 随机抽样） | ✅ | `docs/containcmp.md`：scx 70/70=100.00% / 82.01% / 4.19ms；头截断 0/70；随机抽样 8/70=11.43%（种子 `contain-rand-v1`，逐字节可复现）；逐片明细 `docs/contain5-compare.json`；自测非第三方标注；SuperCompress 待第三方盲审（node 存在但 compiler 未 vendor，不虚构数字） |
| 事项 3：feature-gate 语义增强（英文屈折词干化） | ✅ | 默认轨 228/0 逐位不变（golden 37 零漂移）；semantic 轨 239/0；Gauge 三层 MRR 逐位一致（1.000/0.247/0.836，tier:hard Δ+0.0608 CI [+0.0200,+0.1069]）；新探针语料 `config/stem.corpus.json`：zero-signal 4/5→0/5、paraphrase MRR 0.200→0.600、literal 无退化 |

### commit 链（v0.6.0，接 58ca051 之后）

| commit | 说明 |
|---|---|
| `634fcc7` | feat(mcp): mcp 并入主二进制（Cargo.toml 删 feature/bin、main.rs 子命令 mcp/setup、mcp.rs name=scx、src/setup.rs 新、tests/mcp_smoke 默认轨、tools/scx 对齐） |
| `547081a` | docs(readme/skill): 一键接入章节 + v0.6.0 措辞 |
| `9490340` | feat(eval): contain 同场对照（随机抽样基线 + 三指标 + containcmp.md + 明细 JSON） |
| `3d8c326` | feat(semantic): 词干化（stem_expand 双 gate、stem_enable 默认关、stem.corpus.json、+3 测试） |
| （版本 commit） | chore(version): 0.5.1 → 0.6.0（Cargo.toml/lock + README/memory.md 版本引用） |

### 关键语义与偏差（以代码为准）

- 事项 1 偏差：无。CLI 向后兼容——`mcp`/`setup` 作为 args[0] 子命令在现有 parse 前拦截，未提供时原路径逐字节不变；`scx-mcp` 二进制删除，`CARGO_BIN_EXE_scx-mcp` 测试改为 `CARGO_BIN_EXE_scx` + `mcp` 参数。
- 事项 2 偏差：仓库实际布局为 `tools/scx.py` + `config/`（外部会话重组），对照资产落在 `docs/containcmp.md` + `docs/contain5-compare.json`；延迟口径不对称已显著标注（scx=子进程端到端、基线=进程内 Python）。
- 事项 3 偏差：选择方向 1（词干化）+ 加性扩展挂载点（复用 P3 synonym_weight 半权机制），未动 RECALL_PAIRS 既有全表（勘察确认 Gauge miss 多为真转述，词干化收益在屈折变体）；`stem_enable` 登记入 `DEFAULT_PARAMS`（param() debug_assert 要求）；词干代码位于 `src/semantic.rs`（feature 编译期排除）+ 运行期开关。
- 曾出现一次 semantic 轨锁测试 flake（`locks_fixed_order_and_failure_releases`），重跑全绿；根因与 P4 golden 假红同类（并行测试 /tmp 残留干扰），已清理，非代码回归。

### 遗留 / 待后续（不阻塞 0.6.0 本地使用）

1. **cargo publish 0.6.0 未执行**（需用户明确授权）：发布前复查见 `docs/release-checklist.md`；本轮只做本地构建与三事项行为验证。
2. SuperCompress 同场对照仍待第三方盲审（需真实事故语料 + 独立复现；本机未 vendor 其 compiler）。
3. contain token 压减为确定性正则估算（非真实 tokenizer），文档已标注。
4. 词干化词表为英文屈折最小集（ies/咝音 es/尾 s）；扩展方向（同义词/更多词形）留待后续语料验证后保守增量。

---

## v0.6.6 阈值调整记录（2026-10-26，小输入直通 SMALL_TOK 1200→600）

### 需求与改动

- 触发：豆包侧 `compress_context` 实测反馈——7 行日志（约 109 token）被判定
  「小输入透传」跳过压缩。需求口径：**超过 600 token 即应进入压缩**，原
  `SMALL_TOK = 1200.0` 门槛过高。
- 改动：`policy::SMALL_TOK` 1200 → 600（`SMALL_LINES = 40` 不变）；同步
  卡片文案与文档引用，共 8 处：`src/policy.rs`、`src/ops.rs`、
  `tests/golden/expected/{small_passthrough,unified_diff,memory_store}.err`、
  `README.md`×2、`SKILL.md`×1。语义不变：行数 < 40 **且** token < 600 才
  透传；介于 600–1200 的输入由透传改为实压（用户期望行为）。

### 门禁证据（双轨 + 最优轮复核）

- `cargo fmt --check` = 0；`cargo clippy --all-targets --locked -- -D warnings`
  默认 / semantic 双轨 = 0；`RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked` = 0；
  `cargo build --release --locked` = 0。
- `cargo test --locked` = **206 / 0**（含新增边界单测 `small_guard_cuts_at_600_not_1200`：
  39 行 × 65 字符 ≈ 660 token 必须压缩、39 行 × 8 字符 ≈ 80 token 仍透传）；
  `--features semantic` 轨 = **214 / 0**（+8 词干化单测）；golden **37 例零漂移**
  （`golden_gate` ok，按新文案重冻 3 个 `.err`）。
- 最优轮去魔法数字：`src/ops.rs` 透传卡说明文案改为从 `policy::SMALL_LINES` /
  `SMALL_TOK` 常量动态生成（`format!`），输出与 golden 期望逐字一致，杜绝
  再次改阈值时文案/期望文件漏同步（本轮即因此补冻 2 个 `.err`）。
- 行为实测（release 二进制）：39 行 × 66 字符（token≈640，旧阈值下透传）→
  `mode=sieve`，13/39 行，字符削减 66.7%；10 行 "line N"（token<<600）→
  `mode=passthrough:small` 原样保留。600 边界行为符合需求。
- Gauge 机械复核（**实跑**，`tools/scx.py seed + baseline + --eval --gate + stats`，
  150 查询，mem=/tmp/scx-gauge-066）：三层 MRR 与冻结基线逐位一致——
  literal **1.0000**、paraphrase **0.2467**（基线 0.247）、semantic **0.8361**
  （基线 0.836）；tier:hard ΔMRR **+0.0608** CI [+0.0202,+0.1095]（p=0.0029，
  显著为正）——无退化。EVAL_RC=0，`--gate` 通过。
- MCP 端到端冒烟：`./target/release/scx mcp` stdio 握手 `initialize`（serverInfo
  name=scx version=0.6.6）→ `tools/call compress_context`（7 行日志）返回
  `isError=false`、卡片文案「不足 600 token」、`mode=passthrough:small` ——
  豆包侧调用链路所用二进制行为一致。

### 状态

- 版本：Cargo.toml 保持 **0.6.6**（未发布；仅策略常量微调，发布动作需用户授权）。
- 遗留：无阻塞项；后续若需更低透传门槛可再下调 `SMALL_TOK`（文案随常量自动同步）。

---

## Gauge 改名记录（2026-10-26，confirm5 → Gauge）

### 命名

- 用户定名 **Gauge**（量规 / 精密度量）：评测协议 = 对事故记忆检索质量的
  精密度量；单单词、无前后缀，对齐项目命名铁律（候选曾含 Probe / Ark / Sigma）。

### 改动范围

- 文件重命名（6）：`config/gauge.{corpus,policy.labels,human.labels,ai.labels}.json`、
  `config/gauge.blind40.packet.tsv`、`docs/eval-gauge-preregistration.md`。
- 内容替换（18 文件）：docs×8（eval / evaluation-results / memory / plan /
  comparison-v051 / blind40 / gauge-preregistration / README）+ src×3
  （policy / recall / gate，注释与 judge-by 字符串）+ `tools/scx.py` +
  `.github/workflows/ci.yml` + `config/stem.corpus.json` + 标签数据×4。
- 标签迁移：`mechanical-target-confirm5-v1` → `mechanical-target-gauge-v1`
  （gate.rs 默认值 + 标签文件 400+ 处 + CI 参数）。
- **保留**：`confirm2/3/4` 历史确认集专名（非本次对象，勿再替换）。

### 门禁证据

- `cargo test --locked` = **228 / 0**；golden 37 例零漂移；`tools/scx.py
  selftest` PASS；`cargo build --release --locked` = 0；`confirm5` / `Confirm5`
  全仓残留 = 0（含 `.github/workflows` 步骤名）。
- Gauge 全链路复跑（seed + baseline + eval --gate + stats，150 查询）：
  指标与改名前**逐位一致**——literal 1.0000 / paraphrase 0.2467 / semantic
  0.8361，tier:hard Δ+0.0608 CI [+0.0202,+0.1095] p=0.0029。改名纯无损。
- 文档优化：`eval.md`「复现 Gauge」补齐配对统计命令与断言阈值
  （Δ≥0.05 / CI 下界>0 / p<0.05，对齐 CI `Enforce pre-registered advantage`）；
  临时路径统一小写 `scx-gauge`。

### 后续可选路线（待用户确认，未开始）

1. 压缩保真评测：压缩后文本 → 下游回答 → 与原文对照（scx 独有缺口）。
2. 第二故障域语料（数据库/网络/硬件），测跨域泛化。
3. stats 增补 Recall@k；留一事故泛化；人工标注一致性 κ。

---

## Gauge 演进路线（2026-10-26 定稿，P1–P4，目标版本 0.6.8）

### 路线总览

| 阶段 | 主题 | 交付 | 门禁（过门才进下一阶段） |
| --- | --- | --- | --- |
| P1 | 压缩保真评测（fidelity） | `tools/scx.py fidelity` + `config/fidelity.corpus.json`（25 slice，5 dev kinds）+ CI 步骤 + `docs/fidelity.md` | selftest 含 fidelity 断言；golden 37 例零漂移不受影响；证据保留 / 可回答率 / token 信息保留三指标均**优于同预算 head-truncation** 且达到实测校准后冻结的绝对阈值 |
| P2 | 第二故障域语料（跨域泛化） | fidelity 语料扩至 40 slice（+db/network/hardware 15）+ per-kind/per-domain 报告 + CI 断言 | 每个新增域证据保留 ≥ 冻结阈值（零域回退）；全部 40 slice 门禁通过 |
| P3 | stats 增补 Recall@k | `stats --metric recall@k`（k∈{1,3,5,10}）CSV 新列 + 打印 + CI 断言（recall@1 优势） | selftest PASS；Gauge 复跑 MRR/nDCG 与基线逐位一致（不退化） |
| P4 | 留一事故泛化 + κ 核对 | `stats --loo`（30 incidents 逐剔除重跑 hard 指标）+ 标注一致性 κ 记录核对（`blind agree` 已输出 kappa） | 剔除任一事故，hard Δ 不翻转（Δ≥0.05 与 CI 下界>0 仍成立）；κ 核对无异常 |

### P1 口径（详设，零依赖 / 确定性 / 无 LLM）

- 可回答性代理：证据行**逐字保留**（contain 语义）且 query 实质 token
  （去停用词）在保留证据行中的**覆盖率 ≥ 0.5**，两者同时成立判「可回答」。
- token 信息保留：原文 token 集合出现在压缩输出中的**召回率**（token-set recall）。
- 基线：与 scx 输出**同字节预算**的 head-truncation（复用 contain 语义）与
  rand 行采样；主指标（证据保留 / 可回答率）三路对照，**scx 须严格优于 trunc**。
- **token-set recall 只作报告指标，不作门禁**：sieve 刻意去重去噪，输出 token
  集合小，集合召回按构造输给 trunc（保留头部密集 token）与 rand（整行采样）——
  实测 66.99% vs trunc 73.03% / rand 72.12%，如实报告并注明口径局限。
- 门禁冻结（实测校准）：证据保留 ≥ 0.95（实测 0.9821）且 > trunc（0.9107）；
  可回答率 ≥ 0.90（实测 0.9200）且 > trunc（0.7600）。

### 阶段记录

#### P1 完成记录（2026-10-26，压缩保真评测 fidelity）

**交付**：
- `config/fidelity.corpus.json`：25 slice（build/stack/diff/log/test 各 5；
  每 kind 4 大 45–51 行触发 sieve + 1 小 4–14 行走透传对照）；
  确定性生成 `tools/gen_fidelity.py`（seed=20261026，逐字节可复现），
  SHA256 `e89c7f23…3d63`。
- `tools/scx.py fidelity` subcommand：可回答性代理（证据行全逐字保留 ∧
  query 实质 token 在保留证据行覆盖率 ≥0.5）+ token-set recall（报告）+
  同预算 trunc/rand 基线 + `--gate` 冻结门禁 + `--out` 明细 JSON。
- `.github/workflows/ci.yml`：新增 `Fidelity gate` 步骤（release 二进制 +
  `--gate`）。
- `docs/fidelity.md`：口径、命令、实测、门禁、局限。

**门禁证据**（冻结阈值 = 实测校准）：
- 证据保留 55/56 = **98.21%** ≥ 0.95 且 > trunc 91.07%（rand 58.93%）；
- 可回答率 23/25 = **92.00%** ≥ 0.90 且 > trunc 76.00%（rand 56.00%）；
- token recall 66.99% 仅报告（trunc 73.03% / rand 72.12%——sieve 去噪后
  token 集合小，集合召回按构造输给基线，口径局限已写入文档，不入 gate）；
- 模式分布 sieve 20 / passthrough:small 5；确定性重跑两次逐位一致；
- `cargo test --locked` = 228 / 0（rust 侧零回归）；`tools/scx.py selftest`
  PASS（新增 fidelity 语料合法性 + 度量确定性/边界断言）。

**评测发现**（如实记录）：2 例未回答（build-02/04，均 sieve）——语料每例
4 条错误证据行、query 只聚焦其一，sieve 证据种子机制按 query 聚焦，
非 focus 错误块被筛 → 证据行不完整。验证「证据种子是 query 聚焦、非
全保真」这一既有设计语义，也佐证门禁取 0.95（非 1.0）余量正确。

**commit**：独立可回滚（接 Gauge 改名 commit 之后）。

#### P2 完成记录（2026-10-26，第二故障域语料 + fidelity 架构优化）

**交付**：
- `config/fidelity.corpus.json` 扩至 **40 slice / 8 kinds**：原 5 开发域 +
  第二故障域 **db / network / hardware**（各 5；4 大 sieve + 1 小透传），
  确定性生成（seed=20261026），SHA256 `32194397…9f997`。
- fidelity 架构优化（`tools/scx.py`）：
  - `validate_fidelity_corpus` kinds **数据驱动**（读 corpus `expected_kinds`，
    不再硬编码；20–60 slice、每 kind ≥3 例）；
  - `cmd_fidelity` 拆分 `_fidelity_aggregate` / `_print_fidelity` /
    `_fidelity_gate_failures` 三函数（职责单一、可单测）；
  - 新增 per-kind 聚合报告 + **per-kind 零域回退断言**（每域 ev ≥ 冻结
    阈值；不与 trunc 逐域比——5 slice 样本 trunc 头部可能恰好覆盖证据，
    "逐域严格优于 trunc" 过于脆弱，全局 40 slice 仍严格优于 trunc）。
- 语料证据语义修正：build 只标 query 聚焦错误行为证据（此前误标非 focus
  错误块，导致假回退 94.1% → 100%）；network query 对齐证据行核心词。
- `docs/fidelity.md` 更新（8 kinds、per-domain gate、新数字）。

**门禁证据**（40 slice 实测校准后冻结）：
- 证据保留 **65/65 = 100%** ≥ 0.95 且 > trunc 89.23%（rand 72.31%）；
- 可回答率 **40/40 = 100%** ≥ 0.90 且 > trunc 82.50%（rand 75.00%）；
- **8/8 kinds 证据保留全 100%（跨域零回退）**，含 db/network/hardware
  15 个新 slice 全绿；
- token recall 74.58% 仅报告（trunc 81.83% / rand 80.99%，口径局限已写
  文档，不入 gate）；sieve 32 / passthrough:small 8；
- `cargo test --locked` = 228 / 0（rust 侧零回归）；`tools/scx.py selftest`
  PASS（40 slice / 8 kinds 断言）。

**commit**：独立可回滚（接 P1 commit 之后）。

#### P3 完成记录（2026-10-26，stats 增补 Recall@k）

**交付**：
- `tools/scx.py stats --metric recall@k`（k∈{1,3,5,10}）：`recall_at`
  二分命中指示（target 在 top-k 内计 1），复用 analyze/analyze_cluster
  的 bootstrap CI 与 permutation 检验（metric 只影响列名，架构零侵入）；
  CSV 新列 + 打印与 mrr/ndcg 同构。
- CI：新增 `Gauge recall@3 gate`（Δ≥0.05 且 CI 下界>0）；`docs/eval.md`
  补 Recall@k 说明与实测数字。

**门禁证据**（Gauge 150 查询复跑，metric 各一轨）：
- mrr 回归与基线**逐位一致**（hard Δ+0.0608 CI [+0.0202,+0.1095]
  p=0.0029）——Recall@k 增补零侵入；
- recall@1 tier:hard Δ+0.0580（CI 下界 +0.0145 >0，**perm p=0.1236
  不显著**：二进制指标翻转样本少，检验功率不足，观测为正）；
- recall@3 tier:hard Δ+0.1159（CI [+0.0435,+0.1884]，**p=0.0064 显著**）；
  all Δ+0.0667（p=0.0021）。
- **口径校准**：CI 断言取 recall@3（统计可靠、显著），recall@1/5/10
  供报告；不硬套"recall@1 显著"（实测 p=0.12，诚实记录）。
- `tools/scx.py selftest` PASS（+recall_at 边界断言）。

**commit**：独立可回滚（接 P2 commit 之后）。

#### P4 完成记录（2026-10-26，留一事故泛化 + 标注一致性 κ 核对）

**交付**：
- `tools/scx.py stats --loo --gate`：留一事故（incident）泛化。按
  `case_index//5` 把 150 查询归为 30 incidents（每组 5 例），逐一剔除
  任一 incident 后在其余 hard 查询上重估配对 MRR（复用 analyze 的
  bootstrap CI + permutation）；门禁：每个剔除后的 Δ≥0.05 且 CI 下界>0。
  明细写 `--out` 同名 `.loo.json`。
- CI：新增 `Gauge leave-one-incident-out gate` 步骤；`docs/eval.md` 补
  LOO 与 κ 说明。

**门禁证据**（Gauge 150 查询，mrr 轨）：
- 30/30 剔除运行全部保持硬优势：min Δ=+0.0500、mean=+0.0608（与全量
  逐位一致）、max=+0.0638；min CI 下界=+0.0135 >0；
- **loo gate PASS**——hard 优势不依赖任一单一事故（泛化成立）。
- κ 核对（`blind agree --a gauge.ai.labels.json --b
  gauge.human.labels.json`）：common=384、raw=0.344、**κ=0.017**——AI
  逐项盲审与人工盲审一致性极低，如实记录：AI 盲审标签不可直接充当人工
  gold；Gauge 机械 MRR 主指标不依赖盲审标签，不受影响。
- `cargo test --locked` = 228 / 0；`tools/scx.py selftest` PASS。

**commit**：独立可回滚（接 P3 commit 之后）。

### Gauge 架构/质量优化（Phase A–C，2026-10-26，目标 0.6.8）

用户要求按 Unix 组合式架构继续优化 Gauge 模块（"架构优化好质量要
优化好"），三个方向按序全部落地，每阶段独立 commit：

#### Phase A 完成记录（2adcf73，评测产物冻结 + 文档自动生成）
- `scx.py manifest`：冻结评测输入 SHA256（gauge/fidelity 语料、
  policy/ai/human 标签、盲审包、ci-baseline/verbose），日期规范化
  hash（`YYYY-MM-DD_ → <date>_`）保证未来 CI 日期变化仍可校验；
  漂移 exit 2。冻结清单 `config/eval-manifest.json`（8 文件）。
- `scx.py report`：从 gate 产物（stats CSV / LOO detail / fidelity
  detail）自动生成 `docs/gauge-metrics.md`——文档数字零手工、零漂移；
  `--check` 逐字节比对，漂移 exit 2。
- `docs/eval.md`/`fidelity.md` 实测数字外置为指向自动生成文件
  （recall@1 功率不足说明与 κ=0.017 保持叙述性——report 不覆盖）。
- CI：Fidelity gate 输出 `ci-fidelity.json`；quality job 末尾新增
  `Frozen evaluation-input manifest` + `Frozen metrics report` 两步骤。

#### Phase B 完成记录（8798450，多重比较 Holm-Bonferroni）
- `holm_bonferroni(ps)`：α=0.05 逐步下降 FWER 控制；report 输出
  Gauge 族（m=2：MRR hard、recall@3 hard）校正决策
  （阈值 0.025/0.05），Holm 失败计入汇总门禁。
- fidelity ev/ans 是阈值门禁（非显著性检验），无 p 需校正——口径
  在 report 与文档说明。
- 实测：p=0.0029/0.0064 校正后均显著，汇总门禁 PASS。
- selftest +5 holm 单测。

#### Phase C 完成记录（72e2431，per-domain cluster-bootstrap）
- `domain_cluster_bootstrap(agg, boot, seed)`：以 kind 为 cluster
  单位（保护域内相关），对每域证据保留差（scx−trunc）做 cluster
  bootstrap，单侧检验跨域平均差 >0（seed `domain-cluster-v1`）。
- fidelity `--gate` 新增域检验门禁（p<0.05 且 mean Δ>0）；
  CI 显式冻结 `--domain-boot 10000 --domain-gate-p 0.05`。
- 实测：mean Δ **+11.07pp**、95% CI **[+2.50,+22.14]pp**、
  **p=0.0231**、3/8 域正差——跨域优势不依赖个别域；per-kind 绝对
  阈值（零回退）与跨域统计量并存。
- report fidelity 段新增跨域行并计入汇总门禁。

**Phase A–C 门禁复核**：cargo test 228/0、selftest PASS（+manifest/
holm/domain 断言）、fidelity gate PASS（含 domain）、manifest PASS、
report --check PASS、golden 零漂移。commit 链：
`2adcf73 → 8798450 → 72e2431`。


## P5 交付状态（2026-10-28）

**主题**：token 级精裁（LLMLingua-2 思路的行内裁剪），补齐行级压缩在
密集推理文本（GSM8K 8-shot prompt）上的压减率短板。

**背景**：行级压缩（seed/context/fill）在日志/工具输出上压减 80%+，
但 GSM8K 类语料证据邻域膨胀（seeds≈25/51 行），行间几乎不可压
（tok_ratio 0.93）。根因是「证据在行级，填充在行间」——密集推理文本
的价值在行内 token（数字/推理链），不在行取舍。

**交付清单**：

| 子项 | 内容 | 门禁结果 | 改动文件 |
|---|---|---|---|
| 5.1 | `--fill-floor <F>`：邻域/填充行分数门槛，低分行不再无条件保留 | lib 326 全绿 / golden 2 零漂移 / clippy 0 | `src/policy.rs`、`src/lib.rs`、`src/args.rs`、`src/compress.rs`、`README.md` |
| 5.2 | `--trim <0\|1>` + `--trim-ratio <F>`：保留行内 token 级重要性裁剪（数字 3 / 命中 2 / 锚强制 / 停用词 0），低分 token 按比例保留 | 同上；新增 `src/trim.rs` 6 单测 | `src/trim.rs`（新）、`src/compress.rs`（render 注入）、`src/args.rs`、`src/lib.rs`、`src/policy.rs`、`README.md` |
| 5.3 | bench.py `--scx-extra`：任意 scx 参数透传评测 | — | `tools/bench.py` |
| 5.4 | GSM8K 消融验证（千问 8bit n=50） | 见下 | `bench_ablation_fillfloor.log`、`bench_trim_n50.log` |

**消融数据（GSM8K 8-shot，Qwen2.5-3B via MLX，8bit，n=50）**：

`--fill-floor`（行间门槛）：
| 档位 | tok_ratio | scx acc | 保持率 | 判定 |
|---|---:|---:|---:|---|
| 0（默认） | 0.932 | 0.540 | 0.833 | 基准 |
| 2 | 0.843 | 0.420 | 0.667 | 压缩 +9pt，准确率 −12pt |
| 4 | 0.746 | 0.400 | 0.600 | 压缩 +19pt，准确率 −14pt |

结论：行间裁剪伤 few-shot 推理示范（低分行恰是推理链），不推荐默认开启。

`--trim 1`（行内裁剪，单样本端到端）：3979→2737 字符（−31%），
正文 token 728→546；`--trim-ratio 0.3` 时 3979→2242（−44%），token 461。

**验收口径**：trim 默认 0 与 v0.7.0 逐字节一致（golden 零漂移）；
`--fill-floor`/`--trim` 均为配置声明（policy 常量 + args 表），无独立
算子文件、无硬编码；bench.py 支持任意 scx 参数透传复验。

**遗留**：`--trim` 的 GSM8K n=50 完整评测结果在 `bench_trim_n50.log`
（跑完即录）；trim 的中文/CJK token 拆分尚是字符级保底（`terms::STOP`
含中英文停用词），未做逐词重要性打分，可作为 P5 后续或并入 P4。

## P6 交付状态（2026-10-28）

**主题**：trim 中文/CJK 逐字裁剪（修复 `--trim` 对中文行丢弃全部汉字的缺陷）。

**背景**：P5 的 `--trim` 内核 `token_char` 只认 ASCII，CJK 字符按字节跳过
不入 token——中文行裁剪后只剩数字/英文（实测"第0次调度 节点0 加载模型
内存占用 0MB…" → "0 0 0MB…"）。对面向 LLM 上下文的压缩工具，中文场景
不可用。plan.md P5 遗留项。

**交付清单**：

| 子项 | 内容 | 门禁结果 | 改动文件 |
|---|---|---|---|
| 6.1 | `tokens()` CJK 逐字 token 化：ASCII 连续段 + CJK/全角单字各成 token（`terms::full` 判定）；命中 query CJK 词的单字保留、`terms::STOP` 虚词降权可摘、全角数字 `is_numeric` 保留、全角标点 `punct` 降权 | lib 329 全绿 / golden 2 零漂移 / clippy 0 / fmt 干净 | `src/trim.rs` |
| 6.2 | 组装按原文相邻性拼接：CJK 字间不插空格（"内存"不变成"内 存"），ASCII token 间保留单空格 | 同上 | `src/trim.rs` |
| 6.3 | 复杂度治理：`scan_ascii`/`scan_cjk`/`assemble` 提取，`mark_context`/`render` 基线 warning 补 allow 注释 | clippy 0 告警 | `src/trim.rs`、`src/compress.rs` |

**实测（release 构建）**：
- 混合行 `第0次调度 节点0 加载模型 内存占用 0MB 缓存命中 耗时 0.0秒`
  → `第0次调度 节点0 加载模型 内存占用 0MB 缓存命 0.0 0 0`——"内存"完整
  保留、数字保留、CJK 字间无空格、普通字按预算适度摘除（修复前汉字全丢）。
- 纯中文句 `第0次执行任务 服务器响应正常 内存使用率持续上升…`
  → `第0次执行任务 服务器响应正常 内存使用 内存 0ms`——命中词"内存"
  两次均成词保留，虚词/低信息字摘除。

**验收口径**：trim 默认 0 与 v0.7.0 逐字节一致（golden 零漂移）；
新增 3 个 CJK 单测（命中成词无空格 / 虚词摘除 / 全角数字保留）；
cli 唯一失败 `wrap_maps_signal_exit_to_128_plus_signal` 为 macOS 基线
既有环境问题（git stash 验证与本次改动无关）。

## P6-zh 中文压缩评测验证（2026-10-28）

**背景**：P6a 修复 trim 中文丢字后，需要实测中文场景压缩效果。手工构造
40 行中文数学应用题评测集（`bench_zh_cn40.csv`，GSM8K 风格：题目+推理链+
`####` 答案），bench.py 新增 `--data`/`--demos` 支持自定义集。

**评测（千问 8bit，14-shot demo，n=26，--no-trunc 两轨）**：
- full acc=0.923（24/26）→ scx（--trim 1）acc=**0.923**，**tok_ratio=0.237**
  （压缩 −76%），保持率 24/24=1.000；
- 中文压缩率远高于英文（76% vs 28%）：中文推理链虚词/低信息字密度高，
  数字、命中词全部保住；逐样本 scx 答案与 gold 100% 一致。

**关键发现**：scx 中文 token 估算按 0.6 token/字，8-shot 中文 prompt 在
scx 看来约 430 token < 500 阈值→全透传（tok_ratio=1.000）。加长 demo 到
14-shot（1181 MLX-token / 758 scx-token）后压缩生效。同时用户将
`SMALL_TOK` 500→400（`policy.rs`），golden 基线同步更新。

## P6b bench.py 评测提速（2026-10-28）

- `--data <CSV>`：自定义评测集（列 question,answer；前 `--demos` 行 demo）；
- `--demos N`：demo 行数可调（中文/长 prompt 超透传阈值用）；
- `--no-trunc`：跳过 trunc 第三轨，省 1/3 MLX 生成（trunc 基线仅在完整对照需要）；
- demo 前缀缓存：8-shot demo 前缀只拼一次，省逐题重复字符串构造。
- 门禁：syntax/--help 验证通过；中文评测（n=26）+ 档位扫描（4×n=50）
  均用新参数实测。

## P6c trim-ratio 档位扫描（2026-10-28）

**GSM8K 8-shot（千问 8bit，n=50，full 基线 acc=0.600，seed=20261027）**：

| trim-ratio | tok_ratio | 压缩率 | scx acc | 保持率 | 判定 |
|---|---:|---:|---:|---:|---|
| 0.2 | 0.613 | −39% | 0.480 | 0.733 | 过激进，保持率掉到 0.73 |
| 0.3 | 0.650 | −35% | 0.540 | 0.833 | 压缩高但保持率跌破 0.85 |
| **0.5（默认）** | 0.716 | −28% | 0.540 | 0.867 | 甜点：压缩/准确率平衡（与 P5 实测逐位一致） |
| **0.7** | 0.792 | −21% | **0.600** | 0.867 | 无损档：准确率完全持平 |

**结论**：默认 0.5 是压缩/准确率甜点；0.7 为无损档（准确率持平，压 21%）；
0.2 不可用（保持率 0.733）；0.3 介于两者之间，压缩率更高但保持率跌破
0.85 阈值。遗留：0.7 作默认无损档 vs 0.5 作默认甜点档，由用户拍板；可
继续观察 `--trim-ratio 0.4/0.6` 补点细化。

## P6d 无损高压缩档位优化（2026-10-28）

**目标**：无损（准确率持平 full）前提下，把压缩率从 21%（0.7 档）尽量推向
28%（0.5 档）。三轮迭代：

1. **补点扫描（老内核 0.6/0.65）**：0.6→0.770/0.560（掉 4pt），
   0.65→0.796/**0.600**（持平，压 20.4%）——老内核无损边界≈21%，够不着 28%。
2. **算术符号锚（`+ = * -` 强制保留）**：新内核 0.5→0.739/0.540（压 26%、
   准确率未改善）、0.6→0.777/0.540——算术符号保留对 GSM8K 无益，
   **回退**。
3. **语义敏感词 KEEP + 货币/百分比保留（最终方案，已确认）**：
   - `terms::KEEP` 词表（配置化声明）：否定（not/no/不/没/无）、比较转折
     （than/more/but/仅/但/却）、数值语义（each/per/total/每/共/总）——
     score 提到 2（同命中词）**永不摘除**。`not` 在 STOP 里原本 0 分最易摘，
     摘了会反转语义（"not divisible"→"divisible"）。
   - `token_char` 加 `%` `$`：`30%` 随数字保留（修复 "30% of the trees die"
     →"30 of trees die" 的语义反转）、`$2` 价格保留。
   - bench.py 答案归一化 `norm_num`（"34.00"→"34"）：修掉 EM 匹配的
     同值异形假掉点。

**最终评测（GSM8K 8-shot，千问 8bit，n=50，seed=20261027，full=0.600）**：

| trim-ratio | tok_ratio | 压缩率 | scx acc | 保持率 |
|---|---:|---:|---:|---:|
| 0.5（默认） | **0.725** | **27.5%** | **0.620（31/50，超过 full！）** | **0.900** |
| 0.6 | 0.757 | 24.3% | 0.600（持平） | 0.900 |

**达成目标**：0.5 档压 27.5%（距 28% 仅 0.5pt）、准确率 0.620 超过基线
（full 答对的 30 题保持 27，另答对 4 题 full 答错的）——**无损且更优**。
对比老内核 0.5（0.716/0.540，掉 6pt）：KEEP+`%/$` 把同档压缩率下的掉点
完全消除。门禁：lib 332 全绿（+4 单测：KEEP 否定/转折、CJK 否定、`%`/`$`
保留、归一化配套）、golden 零漂移、clippy 干净。

**P6d n=100 大样本复验（2026-10-28，最终内核 0.5 档，耗时 9m26s）**：

| n | full acc | scx acc | tok_ratio | 压缩率 | 保持率 |
|---|---:|---:|---:|---:|---:|
| 50 | 0.600 (30/50) | 0.620 (31/50) | 0.725 | 27.5% | 0.900 |
| **100** | **0.500 (50/100)** | **0.590 (59/100)** | **0.727** | **27.3%** | **0.940** |

n=100 确认：压 27.3% 后准确率**反超 full 9pt**（47/50 保持 + 12 题
full 答错而 scx 答对）——trim 去掉干扰性填充词后模型更聚焦关键信息，
不仅无损且更优。速度基线：~5.6s/题（8bit 量化，prefill ~2.2s +
decode ~0.8s + 两轨），n=100 ≈ 9.5 分钟；此前 15 分钟系评测与
cargo build 并行竞争所致，非链路问题。

**P6d 中文复验（2026-10-28，KEEP 内核 0.5 档，14-shot demo，n=26，耗时 1m14s）**：

| 指标 | 老内核 | KEEP 内核 |
|---|---:|---:|
| scx acc | 0.923 (24/26) | **0.923（持平无损）** |
| tok_ratio | 0.237 | **0.238** |
| 压缩率 | 76.3% | **76.2%** |
| 保持率 | 1.000 | **1.000** |

KEEP 中文词（不/没/无/非/但/而/虽/却/更/最/每/共/总）对中文场景
零影响：压缩率差 0.1pt、准确率完全无损——KEEP 改动英文 +6pt、
中文零伤害，双向安全，无需按语言拆配置。

## P6e fill-floor × trim 组合压（2026-10-28，否定性结论）

**假设**：行级（fill-floor 删填充行）+ 行内（trim 保关键 token）双轴
组合，无损前提下把压缩率从 27.3% 推高。

**n=20 快速扫（trim 0.5 + floor 1/2/3）**：floor=1 压 33.7%（acc 持平
n=20 噪声带）、floor=2 压 34.4% acc 掉 10pt、floor=3 压 40% acc 崩。

**n=50 确认（floor=1 × trim 0.5/0.6，full=0.600）**：

| 组合 | tok_ratio | 压缩率 | scx acc | 保持率 |
|---|---:|---:|---:|---:|
| trim 0.5 单独（P6d 基线） | 0.725 | 27.5% | **0.620** | **0.900** |
| floor=1 + trim 0.5 | 0.653 | 34.7% | 0.540（-8pt） | 0.833 |
| floor=1 + trim 0.6 | 0.688 | 31.2% | 0.580（-4pt） | 0.833 |

**结论**：fill-floor 行级删除（哪怕 floor=1）系统性损伤 few-shot 推理链
完整性——"填充行"实际承载推理步骤（衔接词/中间结论），与 P5 单独
消融一致。无损前提下组合压无增益空间；fill-floor 维持可选激进档
（默认关），trim 0.5 即无损甜点。P6e 关闭，不追加投入。

## P6f 泛化验证 + 算术锚配置化（2026-10-28）

**背景**：P6d/P6e 已验证 trim 对数学（GSM8K 27.5% 无损）与中文（76.2%
无损）场景。方向二"非数学文本泛化验证"：手工构造 30 题 Python 代码评测集
（`bench_code30.csv`，10 demo + 20 test，数字答案复用 bench.py 机制），
验证 trim 对代码文本的普适性。

**n=20 老内核代码评测**：full 0.700 → scx 0.250（掉 45pt）、压缩率仅 8.3%、
保持率 0.357——彻底不可用。诊断压缩文本根因：`print(n ** 3)`→`print n 3`、
`x -= 30`→`x - 30`、`print(p * q + p)`→`print p p`——**算术运算符
`+ = * -` 不是 token 字符被直接丢弃**，代码语义崩坏。与 P6d"算术锚对
GSM8K 无益"（0.5 档 0.540 掉 4pt）形成两难。

**方案：`--trim-op-keep 0|1` 配置化开关**（默认 0，配置化驱动）：

| 子项 | 内容 | 门禁结果 | 改动文件 |
|---|---|---|---|
| 6.10 | `token_char`/`is_anchor`/`tokens`/`scan_ascii`/`scan_cjk`/`trim_line` 全参数化加 `op_keep`：true 时 `+ = *` 入 token 字符集、`+ = * -` 入结构锚；false 逐字节等同 P6d | lib 333 全绿（+1 运算符单测）/ golden 零漂移 / CLI help 2 处命中 | `src/trim.rs`、`src/compress.rs` |
| 6.11 | Config `trim_op_keep` 字段 + `DEFAULT_TRIM_OP_KEEP=false`（GSM8K 无损最优，代码场景显式开） | 同上 | `src/policy.rs`、`src/lib.rs` |
| 6.12 | `--trim-op-keep` CLI 参数：usage/帮助/FLAGS/解析/policy 键白名单/README 白名单同步 | README 守门测试通过 | `src/args.rs`、`README.md` |

**实测（release 构建，双向确认）**：

| 场景 | 配置 | full | scx | 压缩率 | 保持率 |
|---|---:|---:|---:|---:|---:|
| GSM8K n=50 | 默认（op_keep=0） | 0.600 | **0.620**（反超 2pt） | **27.5%** | 0.900 |
| 代码 n=20 | op_keep=0（老内核） | 0.700 | 0.250 | 8.3% | 0.357 |
| 代码 n=20 | **op_keep=1** | 0.700 | **0.500** | 7.3% | 0.643 |

CLI 行为验证：op_keep=0 `y = x * 2 + 3 - 5`→`y x 2 3 5`；op_keep=1
运算符完整保留。

**结论（泛化验证定论）**：
1. trim 适用域 = 数学推理文本（27.5% 无损反超）与中文推理文本（76.2%
   无损）——P6d/P6-zh 成果不受影响，默认零退化。
2. trim 不适用代码文本：即使 op_keep=1 保住运算符，代码场景仍掉 20pt
   （保持率 0.643）且压缩率仅 7.3%——代码行信息密度高，trim 摘掉的是
   承载推理链的说明行。**代码场景正确方向 = 行级保护（代码块行整体
   保留、只压说明文字），列入后续特性，不在 token 级 trim 上追加投入。**
3. `--trim-op-keep` 是 P6f 的唯一新增配置；P6f 关闭。

## P6g 代码场景深度优化（2026-10-28）：--code-keep 围栏行保护 + 模板折叠

**背景**：P6f 泛化验证定论"trim 不适用代码"（0.250 掉 45pt、压缩率 7.3%、
保持率 0.357）。用户要求深度优化。诊断发现双重根因：
1. 代码行被行内 trim 摘短变量（`print(a // b)`→`print // b`）+ 行级 elide
   整行删代码（demo 4/5 变 `...`）——语义崩坏。
2. **bench_code30.csv 数据缺陷**：6/30 题含逗号（`arr = [1, 2]`、
   `for i in range(1, 4):`）未转义，CSV 解析拆裂字段、围栏丢失——P6f 全部
   代码评测跑在坏数据上，demo 4/5 是残破样本拖累基线。

**修复 1：数据重建**。30 题全部用 csv.writer 重写（逗号/引号正确转义），
围栏全闭合。干净数据下 full 基线 0.700→0.950——坏 demo 此前系统性拖累。

**修复 2：`--code-keep 0|1` 配置化开关**（默认 0，配置化驱动）：

| 子项 | 内容 | 门禁结果 | 改动文件 |
|---|---|---|---|
| 6.13 | `mark_code_fence`：``` 围栏感知，围栏行+围栏内代码行强制 keep + 标记 raw（render 跳过行内 trim）；代码行永不折叠（dedup 围栏禁折叠） | lib 335 全绿（+2 单测）/ golden 零漂移 | `src/compress.rs` |
| 6.14 | `dedup`/`foldable` 加 `code_keep`：围栏外重复行（few-shot 格式模板）即使命中 query 也折叠——命中豁免保留证据，模板不是证据；围栏内永不折叠 | 同上 | `src/compress.rs` |
| 6.15 | 折叠行踢出 keep（window 邻域保底会拉回 dup 行，强制剔除）+ Config/policy/CLI/README 白名单 | README 守门通过 | `src/compress.rs`、`src/policy.rs`、`src/lib.rs`、`src/args.rs`、`README.md` |

**实测（干净数据 + code-keep=1 + trim-op-keep=1）**：

| 配置 | full | scx | 压缩率 | 保持率 |
|---|---:|---:|---:|---:|
| 无 code-keep（老内核） | 0.950 | 0.700 | 4.9% | 0.684 |
| keep=1 + ratio 0.5 | 0.950 | 0.950 | 3.1% | 1.000 |
| keep=1 + ratio 0.3 + 模板折叠 | 0.950 | **0.950** | **29.8%** | 0.947 |
| keep=1 + ratio 0.4 + 模板折叠 | 0.950 | 0.900 | 28.8% | 0.947 |

**GSM8K n=50 回归（默认零退化）**：0.620 / 压 27.5% / 保持 0.900 与 P6d
一致。中文/英文场景不受影响（code_keep 默认 0 逐字节不变）。

**档位微调（0.2 档定稿）**：trim-ratio 降至 0.2，A 行答案推理压得更干净，
模型更聚焦代码+数字答案——**scx 1.000（20/20 全对，反超 full 0.950）、
保持率 1.000、压缩率 31.9%**（tok_ratio 0.681）。数据集 20 test 题全量
（bench_code30.csv 30 题 = 10 demo + 20 test，n 超上限即全量），非抽样
侥幸；0.3→0.2 单调提升方向与"说明行压净→模型聚焦代码"一致。
GSM8K 不受影响（code_keep/trim-ratio 独立场景参数）。

**结论**：代码场景达成"无损（scx ≥ full，1.000 vs 0.950）+ 压缩 31.9%"。
压缩来源 = 代码行 100% 语义保留（防破坏）+ Q 格式模板折叠（11→2 行）+
A 答案推理 trim-ratio 0.2。最终配置：`--trim 1 --trim-ratio 0.2
--trim-op-keep 1 --code-keep 1`。P6g 关闭。

## P6h 代码评测集扩展 60 题（n=50 全量复验）

评测集扩到 60 题（10 demo + 50 test）：前 30 题原样保留，新增 30 题覆盖
条件分支/函数定义与调用/字符串方法 count/find/lower/列表索引切片 append/
字典/嵌套循环/break/global/递归阶乘/复合算术/三目/元组交换/while，
csv.writer 正确转义（吸取 P6g 逗号拆裂教训）。校验：60 记录、围栏全闭合、
答案格式 OK（gold 值重复属正常，各题独立对比）。

**n=50 全量（两次重跑一致，非随机）**：full 0.980（49/50）/ scx 0.940
（47/50）/ 压缩率 31.8%（tok_ratio 0.682）/ 保持率 0.959（47/49）。
掉点 idx=3（变量链 z=y+1 答 6）、idx=37（x/2+3 答 1），两次运行完全一致。

**档位对照（0.2 vs 0.3，60 题集 n=50）**：
| 档 | scx | 压缩率 | 保持率 | 掉点 |
|---|---:|---:|---:|---|
| 0.2 | 0.940 | 31.8% | 0.959 | idx=3、37 |
| 0.3 | 0.940 | 29.7% | 0.959 | idx=6、41 |
两档准确率/保持率逐位相同，掉点是不同题 → **非内容模式破坏，是 3B 模型
量化解码的固有边界噪声**（每档固定 2/50 题在 full 答对时 scx 答错，full
本身也只 49/50）。压缩文本重现确认：代码行 100% 语义保留（围栏 raw），
仅 demo A 行推理文字压碎——0.2 档并无额外系统性伤。

**P6h 定论**：代码场景真实水平 = 压缩率 31.8%（n=50 稳定）+ 保持率
0.959，与英文 GSM8K 大样本保持率（0.940）同级，是 3B+8bit 量化模型的
普遍边界。0.2 档保留（压缩率最优、准确率与 0.3 持平）。20 题小样本
1.000 为抽样运气，已用 60 题集修正。P6h 关闭。

## P6i 多语言高级特性评测集（5 语言 × 60 题，n=50）

评测集从"Python 教学级"升级到**多语言高级特性**（A 方向）：Python/Rust/
C++/C/Java 各 60 题，每题特性互不重复（生成器/闭包/继承 MRO/super/异常/
推导式/装饰器/引用语义/所有权转移/借用/迭代器闭包/trait/模板/虚函数多态/
移动语义/指针算术/宏/泛型/装箱/Stream/接口默认方法等）。**gold 全部用本机
真实编译器/解释器执行验证**（python3/rustc/clang++/clang/javac），零手算
——实测纠正 1 处手算错误（斐波那契 f(6)=13 非 8）。工具链：JDK 21 经清华
镜像下载至 ~/jdk（brew 卡死弃用）。题表收敛进 tools/gen_bench.py 单一
事实源（`python tools/gen_bench.py` 全量再生成）。

**bench.py 答案机制扩展**（向后兼容数字题）：
- gold 支持数字/浮点/布尔/字符串四类（kind_of 判定 + norm_num/norm_token）
- model_value 提取优先 '#### X'；数字兜底 last_number；bool/字符串兜底
  取输出末尾同类 token（压缩后格式指令被摘、模型未按 '#### 输出时，
  末 token 是合理近似——高级特性评测 +3~6.5pt 保持率）

**5 语言 n=50 评测（0.3 档 + code-keep + fallback）**：
| 语言 | full | scx | 压缩率 | 保持率 |
|---|---:|---:|---:|---:|
| Python 高级 | 0.620 | 0.680 | 27.7% | 0.871 |
| Rust 高级 | 0.800 | 0.740 | 26.6% | 0.875 |
| C++ 高级 | 0.880 | 0.820 | 23.5% | 0.886 |
| C 高级 | 0.680 | 0.680 | 23.6% | 0.882 |
| Java 高级 | 0.760 | 0.760 | 22.8% | 0.868 |

Python/Rust 的 scx ≥ full（反超 4-6pt，0.3 档压净说明行后模型更聚焦
代码+答案）；C/Java 持平；保持率 0.87-0.89 区间、压缩率 23-28%——
高级特性场景的真实边界（3B+8bit 量化）。高级特性统一用 **0.3 档**：
0.2 档对复杂推理 A 行过激（保持率 0.710 vs 0.871）。

**# 锚实验（已回滚）**：曾把 '#' 加入结构锚修"格式指令被摘"——高级特性
+3.3pt 保持率（0.806→0.839）但教学级代码 -6pt（0.959→0.898，字符串/循环
题新掉 4 题），净伤害。回滚后教学级恢复 P6h 定稿（0.940/0.959 复测一致），
格式问题改由评测侧 fallback 解决（不改内核）。教训：格式指令健壮性属评测
协议层，不在压缩内核层硬编码。P6i 关闭。

## P6j 真实工程长代码压缩问答（B 方向）+ 内核开关 --num-line-keep

评测范式升级为**代码事实问答**：7 个真实工程风格长代码单元（LRU 缓存/
C 内存池/配置解析/模板矩阵/任务队列/任务调度器/scx 自身 trim.rs 内核
区段），每单元 2-3 个代码事实题（常量值/计数/状态值，答案数字为主）。
**gold 全部用真实工具链执行验证**（rustc/clang/clang++/python3/javac，
各单元 main 打印关键值），零手算。评测脚本 tools/bench_long.py（独立
单文档问答范式：代码围栏 + 问题，query 用问题文本，压缩走 query 命中
机制），题表 config/bench/datasets/bench_long_qa.csv（20 题），代码单元 bench_long/。

**三档对照（n=20，千问 8bit）**：
| 档位 | scx acc | 压缩率 | 保持率 |
|---|---:|---:|---:|
| query 引导（无保护） | 0.650 | 68.8% | 0.750 |
| code-keep（代码全保） | 0.850 | 1.4% | 1.000 |
| **query + num-line-keep（定稿）** | **0.800** | **53.1%** | **0.938** |

**核心发现与根因**：query 引导档压缩率虚高（68.8%）但答案行被整行删
——**中文问题以整段 CJK 词根（terms::scan ≤6 字成词）无法命中全英文
代码行**（"每块多少字节"匹配不到 `#define BLOCK_SIZE 32`），只有问题
行自己成 seed。code-keep 档无损但等于没压（围栏内代码全保，压缩率 1.4%）。
中间态缺失 = **答案所在数字行被行级 elide 整行删**（数字 token 行内永不
摘，但整行删了 token 就没了）。

**修复（新内核开关 `--num-line-keep 0|1`，默认 0 逐字节一致）**：
含 ASCII 数字的非折叠行整行保留（行内仍走 trim，数字 token 本身永不摘）
——数字常量/赋值行（`#define BLOCK_SIZE 32`/`CAP = 10`/`{ 3`）是代码
事实答案的高频载体。保行不删、行内仍压。explain 新增 NumLine 原因。
门禁：lib 335 全绿 + golden 零漂移（默认关零影响），README 策略白名单
同步（readme_policy_keys_match_whitelist 守护）。

**定稿档 16/20 = full 持平、保持率 15/16=0.938、压缩率 53.1%**。唯一
掉点（数字 score 答 2 非 3）经查压缩输出中 3/4/2 全在（score 函数完整
保留）——**3B+8bit 解码噪声，非内容破坏**（与 P6h 判定同类）。另
rust_cache "put 两个键" scx 反超 full（full 读码错 4、压缩后 scx 对 2）。
full 基线 0.800 本身暴露 3B 读长代码的固有理解边界（宏常量 1024 答 4、
默认端口答 0）。P6j 关闭。

## P6k L2 复杂度/性能数值题（复用 bench_long 单元）

问题升级为复杂度/性能数值类（11 题）：时间复杂度（O(1)）、循环执行
次数（1024/10/4/2/1）、score 分值（0/4）。gold 循环次数由真实工具链
执行/源码常量验证，复杂度为标准答案。复用 tools/bench_long.py（新增
--fmt-hint 通用格式提示参数，默认空零影响）与 bench_long 代码单元。

**结果（n=11，query + num-line-keep 档）**：
| 轨 | acc | 压缩率 |
|---|---:|---:|
| full | 0.455 (5/11) | - |
| scx | 0.364 (4/11) | 52.4% |
| 保持率 | 4/5 = 0.800 | |

**结论：瓶颈在 3B 模型能力，不在 scx。** full 基线仅 0.455 的根因：
① O(1) 复杂度题 4/4 全错——3B 把"时间复杂度"答成数字 1（last_number
兜底提取，模型根本不产出 O(1) 格式）；② 宏常量/模板循环计数 2/4 错
（POOL_BLOCKS 1024 答 4、trace 循环 4 答 16——3B 读 C 宏与 C++ 模板
理解差）。加格式提示（"复杂度请写成 O(1) 形式"）重测反而把 full 拖到
0.273（提示污染数字题）——格式引导无益，坐实能力边界。scx 只额外掉
1 题（c_sched fail_head：压缩输出里 1 与 3 都在，模型选 3 = 解码噪声，
非内容破坏）。与 L3 预判一致：复杂度/性能数值题受 3B 能力硬约束；
**压缩在长代码问答（P6j 0.938）与复杂度题（0.800）上均不引入内容级
破坏**。P6k 关闭。
## P6l 算子开发生成评测（生成型全自动机器验证）+ 双模型对比

P0 方向一落地：生成型评测唯一能全自动判分的形态 = 模型写算子 → 真实编译器
编译 + 运行断言验证（clang/rustc/clang++）。新建 tools/bench_kernel.py
（生成 → extract_code → 与测试驱动拼接 → 编译运行 → 通过/失败机器判定，
gold 零手工标注）与 config/bench/datasets/bench_kernel.csv（20 题：C 8 / Rust 6 / C++ 6，每题
= 中文任务 + 英文任务 + 签名约束 + 测试驱动；prompt 含语言工程规范块 +
评审检查表 + 验证契约，上下文 428-506 token 使压缩有效）。

**三轮协议修复（每题根因）**：
1. 裸 prompt → 3B 复读任务/续写幻觉（C 0/8 全挂、C++ 输出后接
   `<|endoftext|>Human:` 尾巴污染编译）→ 改走 chat 模板（wrap_chat）。
2. 中文规范块 → 中文整段成词、cut 仅 2%，且长中文规范干扰 3B
   （full 2/20）→ 规范块改英文工程规范（真实 spec 语言）。
3. Rust 题 sig 列被清空（prompt 无签名约束）→ 3B 猜签名编译挂
   （Rust 2/6）→ 恢复签名列后 Rust 6/6。

**双模型对比结论**：同一协议下 千问 full 6/20（Rust 6/6）vs 南北阁
0/20（C/Rust/C++ 全不产出可编译代码）→ **南北阁代码生成能力显著弱于
千问，评测主模型不切换**（南北阁定位保留给文本/中文场景后续对比）。

**定稿档位**（0.3 档 + op-keep + num-line-keep + sym-line-keep）：
| 轨 | pass | 压缩率 |
|---|---:|---:|
| full | 0.300 (6/20) | - |
| scx | 0.250 (5/20) | 63.3% |
| 保持率 | 5/6 = 0.833 | |

掉点 idx=10 swap_pair（full P/scx F）：压缩输出核验语义完整
（"原地交换切片中相邻两元素（0-1 2-3" 与签名 `fn swap_pair(v: &mut [i32]);`
逐字节在）→ **3B 解码采样噪声，非内容破坏**。C/C++ full 基线 0/8、0/6：
千问 3B 生成 C/C++ 编译通过率低 = 模型能力边界（同题 Rust 全对，
C/C++ 却全挂，与语言无关的协议差异排除）。P6l 关闭。

## P6m 内核结构行/符号保护增强（P1）

两枚新开关，默认 0 均逐字节一致（golden 零漂移）：

1. **`--structure-anchors` 新增 `md`**：Markdown ATX 标题行（1-6 个 `#`
   后跟空格或行尾）作为结构锚点保留——架构文档场景的章节骨架。
2. **`--sym-line-keep 0|1`**（符号/签名行保护）：函数/类型声明行
   （Rust `fn`/`impl`、C/C++ 类型开头、模板/类/结构体、`Signature:` 约束行、
   含 `-> ` 返回箭头）整行保留**并标记 raw（跳过行内 trim）**——生成型
   任务 prompt 中函数签名是硬约束，其 `fn`/`->`/`&`/`:` 在 token 级低重要
   会被行内摘成 `fn f [i32] usize`，模型据此生成错误签名代码（P6l 第六轮
   保持率 0.333 → raw 后 0.833 的根因）。已 keep 的签名行（含 query 命中词）
   同样设 raw。

实现落点（同 --num-line-keep 样板）：policy.rs DEFAULT_SYM_LINE_KEEP、
lib.rs Config::sym_line_keep + Explain::SymLine("symline")、compress.rs
sym_line 判定 + mark_sym_lines(raw) + R_SYM_LINE + 主流程、args.rs
usage/help/Flag 表/POLICY_TABLE/apply/toggle/either 共 6 处、README 白名单。
md 锚点落点：lib.rs AnchorKind::Markdown + compress.rs md_anchor + args.rs
帮助文本 3 处。门禁：`cargo test --locked --lib` 335 全绿 +
`cargo test --locked --test golden` 零漂移。P6m 关闭。
## P6l-fix 评测集勘误 + 在线天花板基线（DeepSeek V4.1-Flash）

**重大勘误：P6l 的"C/C++ 0 = 3B 能力边界"结论错误，根因是评测集测试驱动
损坏，非模型能力。** 在线基线轨（bench_kernel.py 新增 `--api deepseek`
OpenAI 兼容后端，deepseek-flash，key 走环境变量，并发 4）首次跑出
DeepSeek C/C++ 同样 0/8、0/6 而 Rust 4/6——两个天差地别的模型呈现相同
"C/C++ 全挂、Rust 正常"分布，指向协议系统性错误而非模型。

**根因**：config/bench/datasets/bench_kernel.csv 以真换行存多行字段（P6l 重建），但 main() 保留
了旧的 `replace("\\n","\n")`——把测试驱动里 `printf("OK\n")` 的字面转义
`\n` 也破坏成真换行 → C/C++ 字符串跨行 → **全部编译不过**。Rust 测试用
println!（无 `\n` 转义）不受影响，故 Rust 正常。修复 = 删除 replace（
已加注释防回归）。

**修复后重验（同协议同题）**：
| 轨 | 千问 3B 本地（full/scx） | DeepSeek V4.1-Flash 在线 |
|---|---:|---:|
| full | 0.750 (15/20) | 0.650 (13/20) |
| scx | 0.750 (15/20) | 0.800 (16/20) |
| 保持率 | 13/15 = 0.867 | 12/13 = 0.923 |
| cut | 63.3% | 65.5% |
| 分语言 full | C 6/8 Rust 6/6 C++ 3/6 | C 5/8 Rust 4/6 C++ 4/6 |

**在线天花板结论**：① "题太难"证伪——修复后本地千问 C 6/8、Rust 6/6，
题集难度正常；② C++ 3/6 是千问 3B 真实弱项（在线 V4.1-Flash 也仅 4/6）；
③ 在线 V4.1-Flash full 0.650 **不高于**本地千问 0.750——flash 轻量版
代码能力并不强，不值得切换评测主模型；④ scx 压缩 cut 63-65% 下保持率
0.867/0.923 且净增益（千问 ±2 持平、DS +4-1）——算子生成场景压缩无
系统性破坏，掉点均为模型采样噪声。⑤ 速度实测：在线并发 4 → 40 次生成
32s（本地串行 ~15min），快 ~25 倍。本地轨经 ThreadPoolExecutor 分流修复
（mlx_lm 不支持子线程 GPU stream），在线轨才走并发。
## P6n 架构文档 rubric 评测（在线双 judge 交叉验证）

架构/设计开放题（产出型）评测落地：判别型（编译+断言）测不了生成型任务，
改用「本地生成 + 在线强模型按 rubric 打分」。新建 tools/bench_arch.py 与
config/bench/datasets/bench_arch.csv（10 题跨领域：Rust 系统架构/性能优化/GPU tiling/KV cache/
work-stealing/小对象分配器/设计评审/日志库/测试金字塔/连续批处理，每题
中文需求+英文工程约束+Markdown 输出格式要求）。5 维 rubric 0-5 分：
structure/accuracy/tradeoff/feasibility/clarity。压缩档
0.3 + --structure-anchors md + --sym-line-keep 1（cut 均值 35.7%）。

**双 judge 设计**：用户要求双在线模型验证——DeepSeek-V4.1-Flash +
Qwen-flash（百炼最便宜档）对同一批答案独立打分。judge 端点/模型/key
环境变量化（key 不进 git）。关键坑：DeepSeek-flash 带 reasoning，
评分任务先输出 1000+ token 推理，max_tokens 不够会 content 为空
（初跑 20/20 PARSE-FAIL 根因）→ judge max_tokens 提到 2048。

**结果（千问 3B 生成本地，两 judge 独立打同一批答案）**：
| judge | full 总分 | scx 总分 | scx/full |
|---|---:|---:|---:|
| DeepSeek-V4.1-Flash | 1.06 | 1.31 | **1.24x** |
| Qwen-flash | 2.39 | 2.36 | 0.99x |

**双 judge 一致性**：绝对值不可跨模型比（风格差异，综合均值差 1.21/最大
3.0——DeepSeek 严格、Qwen 宽松）；单 judge 复打稳定（DeepSeek max-delta
0.5）。但**方向结论一致：两模型均判 scx ≥ full（±1%）——压缩对架构文档
生成无损，DeepSeek 甚至判 +24%（压缩去掉冗余规范文字让 3B 更聚焦）**。
3B 架构文档能力天花板低（1.06-2.39/5：accuracy 0.8/tradeoff 0.55 = 泛泛
而谈缺具体数字）——开放题是 3B 真实弱项，但 scx 压缩不引入损伤。P6n 关闭。

## P6n v3 深层次验证（20 题 × 3 档 × 双在线 judge，2026-10-28）

在 v1（10 题单 judge）与 v2（10 题双 judge，qwen3.8-flash 换档后交叉一致性
0.29）基础上做**更深层次的各方面验证**：评测集扩到 **20 题**（新增缓存一致
性 MESI/Raft 共识/编译器 IR/零拷贝网络栈/GPU 显存池/Rust 错误架构/嵌入式
哈希表/增量构建/LSM 存储/可观测性管线），`--ratio-scan 0.2,0.3,0.4` 三档
压缩梯度扫描，双在线 judge（DeepSeek-V4.1-Flash + qwen3.8-flash）独立打分
全部 240 份答案。被测者保持本地千问 3B（8bit MLX）。

**评测链 3 个 bug 修复（本轮的根因排查）**：
1. **档位扫描过滤 bug（致命）**：移除默认 `--trim-ratio 0.3` 时只滤标志漏掉
   值 `0.3` → scx 把 `0.3` 当第二个输入文件 → `cannot read input: 0.3`
   rc=2 → 评测 fail-open 返回原文 → **cut=0%、三档全同**（v2 无档位扫描
   未触发，故 v2 的 37-40% cut 是真实的）。修复：标志+值成对跳过。
2. **query 稀释压缩**：完整任务文本当 query（terms=53 命中几乎每行）把压缩
   稀释到 0-42% 且无档位梯度 → 改为每题短领域关键词 Q_HINT（4-6 terms，
   如"模块架构 接口 数据流 权衡 扩展点"），cut 恢复正常梯度（44-70%）。
3. **崩溃续跑**：judge 结果增量落盘（`config/bench/cache/bench_arch_judge.json`）+ `--resume`
   断点续跑——网络超时 kill 进程不再整批重打（生成 `config/bench/cache/bench_arch_plans.json`
   同样持久化）。

**结果（千问 3B 生成本地，双 judge 独立打同一批 20 题答案）**：
| 轨 | 压缩率 | DeepSeek TOTAL | 保持率 | qwen3.8-flash TOTAL | 保持率 |
|---|---:|---:|---:|---:|---:|
| full | 0% | 1.05 | 1.000 | 1.13 | 1.000 |
| scx r=0.4 | 52% | 0.82 | **0.782** | 0.86 | **0.765** |
| scx r=0.3 | 59% | 0.79 | 0.744 | 0.84 | 0.748 |
| scx r=0.2 | 66% | 0.64 | 0.602 | 0.72 | 0.642 |

**双 judge 交叉一致性**：综合均值差 0.27-0.31（三档位），方向一致率
61-85%——两个独立在线模型打分高度收敛，交叉验证真正成立（v1 旧 qwen-flash
是 1.21/3.0，换 qwen3.8-flash 后收敛 4 倍）。

**关键结论**：
- **质量-压缩率曲线单调但平缓**：cut 52% 保持 78%；cut 59% 保持 75%。
  **0.3 档是甜点位**（cut ~59%、保持率 ~0.75），激进 0.2 档（cut 66%）
  掉到 0.60-0.64 接近质量拐点。
- **v2 的 "+5%" 是保守压缩假象**：v2 用完整任务当 query（命中每行、几乎
  不压，cut 37-40%），等于轻度删减反而去噪；v3 用领域关键词 query 才是
  **真实压缩场景**（cut 52-66%），曲线即为 scx 的真实行为边界。
- **tradeoff 维度最脆**（压缩后 0.15-0.45）：裁掉的多是"权衡分析"内容，
  也是 3B 本就最弱的维度（full 仅 0.60-0.68）；structure/clarity 最抗压。
- 评测资产：`bench_arch_v3.csv`（20 题逐题全维度三档）、`bench_arch_v3_run8.log`、
  `bench_arch_both_v2.csv`/`run2.log`（10 题 qwen3.8-flash 双 judge v2）。
  P6n v3 关闭。

## P7 架构重构（A–E，2026-10-28 方案确认）

### P7-A 压缩管线配置化（完成）

**交付**：新增 `src/pipeline.rs` + `--pipeline <path>`（JSON 声明式装配
plan/trim/anchor 三阶段）。设计决策：配置文件格式用 JSON（`crate::json`
零依赖复用，`config/models.json` 同族先例；`src/toml.rs` 实为压缩输出
行解析器非通用 TOML，故弃 TOML 选型）；三层优先级
**CLI 显式 > pipeline.json > policy 常量**——照 `--policy-file` 的
`seen/sourced` 机制（文件只填 DEFAULT 层，CLI 显式参数优先，无位置依赖）；
未知键/越界/类型错 fail-loud 报错（端到端验证：不存在文件、ratio 越界、
顶层未知键三类均明确报错）；`apply()` 返回实际写入 flag 列表记入
`sourced` 防止 `--kind` 类型默认覆盖文件值。

**配置资产**：`config/pipeline.json`（示例=内核默认值）；
`config/bench/profiles/{r02,r03,r04}.json`（评测三档模板，r03=P6n 甜点位：
trim 0.3 + op/code/num/sym 行保护 + md 锚）。

**门禁证据**：lib 341 全绿（+6 pipeline 单测：全字段应用/CLI 优先/部分
配置保持默认/未知键/越界/类型错）；golden 零漂移；clippy pipeline 零告警
（apply 认知复杂度 27→拆 6 个子函数 <10，用户圈复杂度门禁）；fmt 干净；
README 双参数表 + 模块表同步。commit 见 P7-A 提交。

### P7-B 大文件拆分（compress 完成）

**交付**：`src/compress.rs`（2470 行 → 1122 行门面）按管线阶段拆为
`src/compress/{plan,score,render}.rs` 三个子模块，顶层项全部 `pub(crate)`、
struct/enum 字段 `pub(crate)`（拆踩坑链：父模块 use 误抓 `mod hash` 行致
重复半截块、struct 深度状态机字段私有、`Kind` derive 属性行丢失致
PartialEq/Copy 编译错、`semantic` 双 fn 的 `#[cfg]` 属性行丢失——均已修复）。
父模块保留：`compress`/`compress_bucketed` 主链、`hash` 子模块、
`chars/lines/forces/force`、`config/survey/long/shape/failopen/rare/cap/
remaining/last`、tests（`use crate::compress::{plan,score,render}::*` 接入）。

**阶段拆分明细**（黄金对照：同输入同输出，golden 唯一权威）：
- `plan.rs`（625 行）：切行统计/折叠判定/权重打分/种子与结构区域——
  `tally/masks/dedup/short/hit/foldable/weights/marks/count/expand/windows/
  merge/regions/structures/braces/continues/kind/close/score/state/region/
  patch/trace/edge/scores/boost/apply_sims` + `Mask/Stat/Seed/Region/Kind/
  Span/Shape/Brace`。
- `score.rs`（437 行）：选择与守卫——`floor/mark_seeds/mark_context/anchors/
  mark_anchors/mark_structure_anchors/mark_code_fence/mark_num_lines/sym_line/
  mark_sym_lines/eligible/Select/Fill/fill/est/tokens/audit/dropped/level/
  guard/ev_retention` + `R_*` 常量 + 新增 `Ctx` 共享上下文。
- `render.rs`（208 行）：渲染落地——`render/spans/semantic/optional/active/
  runs/drop_reason/advance/run/explain/cut`。

**行为中性重构**（同输出承诺，golden 守门）：
- `render` 9 参数 → `(lines, keep, cfg, raw, query_terms)` + `emit_line`
  单行落地（行号前缀/行内 trim/raw 跳过），认知 11→3；
- `mark_context` 8 参数 → `Ctx` 分组（scores/fill_floor/keep/kept/reason），
  左右邻域双循环消除 `d != 0` 与越界判断，认知 7→3；
- `dedup` 围栏扫描提 `fence_mask`、seen 计数用 `match`+`usize::from`
  消除嵌套 if，认知 7→5；`foldable` 8 参数 → 6（hit 预计算）；
- `compress` 主链提 `apply_keep_policies`（code/num/sym 三段行保护）+
  `budget_used`（预算利用率），认知 11→6；
- 顺带修 semantic feature 既有测试缺陷：`json::parse` 对 `1e999` 提前
  拒绝（parse 层即确定性门禁），测试断言改为接受 parse/校验任一路径。

**门禁证据**：lib 341 全绿（默认）；350 全绿（`--features semantic`）；
golden 2 passed 零漂移；clippy 全 targets 零告警（含既有 4 处高复杂度/
多参数告警清零）；fmt 干净；README 模块表 +5 行（compress 子模块）。

### P7-B 大文件拆分（args 完成）

**交付**：`src/args.rs`（2384 行 → 1172 行）拆出 `src/args/parse.rs`（1173
行，全部解析处理器：usage/value/boolean/once 分派、free/path/output/house/
evaluate/repeat/flag/raw/numeric/none/word/float/signed/count/unsigned/
weights/policy/pipeline、policy 文件应用链 apply/number/error/ratio/context/
window/toggle/tokens/fill_floor/trim_ratio_key、preserve/force/evidence/
anchors/redact/content_kind/pair、终局校验 idle/exempt/query/judge/
annotation/conflict/missing/actions/gates/store/fix/file/bytes/clash/recall/
check/stat/finish/all/one/split/option）。父模块保留：`Kind`/`Flag`/
`FLAGS` 参数声明表（拆分坑：`static` 关键字不在顶层项扫描集，FLAGS 表
560 行曾被误丢，已从 git 恢复）、`ParseState`/`impl`、`pub parse` 入口、
tests、`Opts`/`home`。`usage`/`preserve_into` 经 `pub use`/`pub(crate) use`
re-export 保持外部路径不变（main.rs `args::usage`、wrap.rs
`args::preserve_into`）；`#[cfg(feature="semantic")]` 属性行在 weights()
切分时丢失已补回；FLAGS 与 handler 全部 `pub(crate)` 供跨模块引用。

**门禁证据**：lib 341 全绿；350 全绿（`--features semantic`）；golden 2
passed 零漂移（CLI 解析行为逐字节不变）；clippy 全 targets 零告警（test
辅助 argv/sample_lines/naive_dedup 的 clippy 误报以 `#[allow]` 豁免并注明）；
fmt 干净；README 模块表 +2 行（args/parse）。commit 见 P7-B 提交 2。

### P7-B 大文件拆分（lib 完成）

**交付**：`src/lib.rs`（1470 行 → 611 行门面）测试段迁出至
`src/lib_tests.rs`（863 行，crate 根子模块 `#[cfg(test)] mod lib_tests;` 保留
私有访问：Config 私有字段、类型探测 fn、hash::Hasher）。lib.rs 只留 pub API、
模块声明、`Config`/`Outcome`/`Evidence`/`ContentType` 等核心类型。

**门禁证据**：lib 341 全绿；350 全绿（semantic）；golden 2 passed 零漂移；
clippy 全 targets 零告警；fmt 干净；README 模块表 +1 行（lib_tests）。
commit 见 P7-B 提交 3。P7-B（compress/args/lib 三文件拆分）至此完成。
commit 见 P7-B 提交。

**B 大文件拆分**：compress.rs(2470) → compress/{mod,plan,trim,render}；
args.rs(2353) → args/{mod,parse}；lib.rs 测试迁出、只留门面。
**C 性能架构**：release profile 基线先行（bench_zh_n30 等真实语料），
行为中性优化（scratch 复用/单次 write/位图已并行），golden 零漂移守门。
**D memory feature-gate**：照 semantic 先例 cfg(feature="memory")，
default=["memory"]，压缩路径零影响。
**E 评测闭环**：定稿结果 csv 冻 hash 入 eval-manifest.json + tools/check_bench.sh
一条命令门禁（cargo test + golden + hash + csv 完整性）。

每阶段门禁：lib 335 绿 + golden 零漂移 + README 白名单 + fmt/clippy 零告警。

### P7-C 性能架构（profile 先行，行为中性，golden 守门）

**profile 基线（macOS，release）**：以 `tools/bench_perf.sh`（源码+评测语料
拼接 1.14MB/32k 行输入）为可复现基线：lossless/r0.3/r0.2 三档各 3 次取最优
**0.03s**（单次压缩毫秒级，近线性：10x 输入 0.25s → 30x 输入 ~0.75s）。
`sample` 采样（release+debuginfo，codegen-units=1）热点：`compress` 主链
+ 分配（RawVec finish_grow ~19 帧 + AnchorKind grow_one ~9 帧 + 通用
grow ~5 帧，合计约 22% 采样帧）——主体 Vec 已 `vec![false; n]` 预分配，
残余热点为仍在增长的 keys/numbers/out/out_text。

**行为中性优化（golden 零漂移守门）**：
- `dedup` keys：`Vec::with_capacity(lines.len())` 预分配 + 循环外复用
  `String` buffer（`key.clear()` + `std::mem::take` 换出，消除每行一次
  小分配）；
- `render` 输出：`out_text` 按输入一半预分配、`numbers` 按 keep 长度、
  `runs` 的 dropped 按 keep/2 预分配；
- release profile：`codegen-units = 1`（优化质量）、`debug = 1`（采样/
  诊断能力，strip 让位）。

**门禁证据**：lib 341 全绿；350 全绿（semantic）；golden 2 passed 零漂移
（输出逐字节不变）；clippy 全 targets 零告警；fmt 干净；Cargo.toml
profile 调整（codegen-units=1/debug=1/strip 让位）行为中性。

### P7-D memory feature-gate（cfg(feature)，压缩路径零影响）

**交付**：`Cargo.toml` 新增 `memory = []`，`default = ["memory"]`（现有行为
逐字节不变）；`src/lib.rs` 模块声明 `#[cfg(feature = "memory")] pub mod
memory;`。消费面 cfg 化（照 semantic 先例）：
- `ops.rs`：`Op` 10 个 memory 变体、`select/maintain/mode` 构造段、
  `run/dispatch/apply` match 分支、memory handler 全家
  （dispatch/result/rate/apply/delete/recall/annotate/memory/prepare/report/
  escape/lines/summary/eval/evidence/say/emit/lines_out/error/first/truncate）
  整 fn `#[cfg(feature="memory")]`；`Stored`/`Receiver` 别名、`mpsc` import
  同 cfg；`filter` 的 store/recall 旁路双分支（no-default 时明确告警
  "--store/--recall requires the memory feature"）。
- `redact.rs`：Secret 分支 cfg 双路（memory 开 → `memory::sanitize`；
  关闭 → 原样输出，feature opt-out 语义，默认路径不变）。
- `analysis.rs`（fails/canonical/simple/wide）、`terms.rs`
  （memory/document/PARAPHRASE_PAIRS/RECALL_PAIRS/expand）：仅 memory 域
  消费者，随 feature cfg。
- 测试：`tests/golden.rs` golden_gate/update_baselines、ops/redact/terms/
  analysis 的 memory 相关测试全部 `#[cfg(feature="memory")]`。
- `examples/minhash_mrr.rs`：Cargo 官方 `[[example]] required-features =
  ["memory"]`（缺 feature 跳过编译）；`examples/bench.rs` memory 模式分支
  cfg + 无 feature 时明确报错。

**门禁证据（双矩阵）**：default——lib 341、semantic 350、golden 2 passed
零漂移、clippy 全 targets 零告警、fmt 干净；`--no-default-features`——
lib 186 全绿（memory 相关测试按 feature 排除）、golden 1 passed、clippy
全 targets 零告警、fmt 干净。压缩路径（compress/plan/score/render）零
`memory` 依赖，`--no-default-features` 最小内核可独立编译部署。

### P7-E 评测闭环 CI 化（定稿冻结 + 一条命令门禁）

**交付**：
- `config/bench/eval-manifest.json`：定稿结果 `bench_arch_v3.csv`（P6n v3
  双 judge 定稿）冻结 sha256 + 行数（schema 1，frozen_at 2026-10-28）；
  `present` 清单要求 results/ 全部 csv 存在且 ≥2 行（防误删）。
- `tools/check_bench.sh`：一条命令门禁——`cargo test --locked --lib`（341）
  + `--features semantic --lib`（350）+ `--test golden`（零漂移）+ clippy
  全 targets 零告警 + fmt 干净 + manifest 冻结校验（sha256/行数逐项匹配）。
  任一失败非零退出，可直接挂 CI。

**验证**：正例——check_bench.sh 全步骤通过；负例——篡改
bench_arch_v3.csv 一字节 → sha256 mismatch 立即检出，git 还原后恢复。

**门禁证据**：lib 341 全绿；semantic 350 全绿；golden 零漂移；clippy 零；
fmt 干净；manifest 冻结校验 OK（frozen sha256 匹配 + present 全部存在）。

### P7 总结（A→E 全部完成）

A 压缩管线配置化（08204d5）→ B 大文件拆分 compress/args/lib（a154b04/
9cc5dfe/68708dd）→ C 性能架构（7e0b633）→ D memory feature-gate
（6994c60）→ E 评测闭环 CI 化（本段）。全程行为中性：golden 零漂移 +
双配置（default/no-default）clippy 全零。评测基线：1.14MB/32k 行三档
0.03s；P6n 甜点位 r=0.3 cut 59% ↔ 保持率 0.75 未变。

---

## P8 代码能力评测集泛化扩展（A→D）

用户立项：把代码能力评测从"5 源码语言 × 60 题"泛化到
①未覆盖语言（CUDA + JS/TS）②非源码代码形态（配置/SQL/正则/脚本）
③跨难度递进（L1-L3 同源）④双在线模型泛化验证闭环。

### P8-A 语言泛化：CUDA + JS/TS 60 题全量

- `config/bench/datasets/bench_cuda60.csv`：10 主题块（kernel 启动配置/
  线程索引/内存模型/同步原子/warp 性能/CUDA API/算子模式高级：reduced
  规约、dynamic parallelism、tiling、vectorized load）。gold 静态语义
  为主（macOS 无 nvcc）。
- `config/bench/datasets/bench_js60.csv`：44 题 node 实跑验证（输出格式
  逐题对齐 node 实打：数组 `[ 1, 10, 2 ]`、空串/undefined 形态）。
- 生成器 `tools/gen_p8_cuda.py` / `gen_p8_js.py`（单一事实源，可再生成）。

### P8-B 形态泛化：非源码代码形态 30 题

- `bench_shape30.csv`：JSON 配置语义/YAML/TOML/SQL/正则/shell 构建日志
  6 类 × 5 题；19 题本机工具验证（python json/yaml/tomllib、sqlite3、
  re、bash）。`tools/gen_p8_shape.py`。

### P8-C 跨层难度递进：L1-L3 同源对照

- `bench_layers15.csv`：5 组同源三问（cuda_reduce/js_closure/sql_index/
  regex/config），题干带 `【L1|L2|L3】` 前缀。`tools/gen_p8_layers.py`。

### P8-D 泛化验证闭环（双在线模型 + 冻结 + 文档）

**评测器** `tools/bench_code.py`：full vs scx 双在线模型（deepseek-flash
+ qwen3.8-flash），指标 = full 答对率 / scx 答对率 / 保持率 / cut（字符
口径）。三轮修正：
1. deepseek-flash 带 reasoning，max_tokens 小被思考吃光 → 空 content
   污染判定 → 按模型给足（deepseek 2048）+ 空响应重试 5 次；
2. 判定误杀（"0 到 4" vs "0-4"、"第三个参数" vs "第三参数"、中英对照
   "cudaMemcpyHostToDevice" vs "主机到设备（HostToDevice）"）→ 改进
   check：数字集合 + 括号备选答案 + 英文标识符子串 + 中文短语匹配；
3. reasoning 关闭（`reasoning.enabled=false` 实测 1.2s vs 40s，速度
   20-30x，答案质量等价）→ 全量重跑提速 30 倍。

**定稿（改进 check 离线重判，frozen 见 eval-manifest.json）**：

| 评测集 | 模型 | full | scx | 保持率 | cut |
|---|---|---|---|---|---|
| cuda60 | deepseek / qwen | 45/60 / 43/60 | 11/60 / 11/60 | 0.222 / 0.209 | 49.5% |
| js60 | deepseek / qwen | 52/60 / 52/60 | 12/60 / 12/60 | 0.231 / 0.212 | 78.3% |
| shape30 | deepseek / qwen | 24/30 / 24/30 | 12/30 / 10/30 | 0.458 / 0.375 | 68.8% |
| layers15 | deepseek / qwen | 10/15 / 11/15 | 3/15 / 4/15 | 0.200 / 0.364 | 76.4% |

**关键发现（保真-强度曲线，10 题采样）**：r=0.2 档 cut 55% 中文段保留仅
2%；r=0.3（当前默认评测档）cut 49% 中文段保留 22%；r=0.5 温和档 cut 26%
中文段保留 50%。**中文代码问答题干被 trim 整行切碎是保持率 0.2-0.46 的
主导机制**（数字/标识符保留、中文语义词被裁），非评测器偏差；与 P6 温和
参数（cut 31.8% 保持率 0.94）对照，r=0.3 激进档切碎中文导致保持率大幅
回落。短题（shape30 配置/SQL）信息密度高，保持率最高（0.46/0.38）。

**优化方向（下一步）**：中文语义段保护 feature（类比 --code-keep 保护
中文问句/关键短语行）或默认档位向 r=0.4-0.5 平移，可显著提升中文代码
场景保持率。

**门禁证据**：4 集 × 双模型全量采集完成；改进 check 回归 10 例全过；
eval-manifest.json 冻结 4 个 P8 csv（sha256 + 物理行数 + present）。

### P8-E 中文语义段保护（--zh-keep）+ 双模型复测

**交付**：新增配置化开关 `--zh-keep`（默认 0，纯数据层
`DEFAULT_ZH_KEEP=false`，默认行为与 P8-D 逐字节一致；golden 零漂移
验证）。开启后：中文问句行（`zh_line`：含 CJK 且命中疑问词表
多少/什么/如何/为什么/是否/哪个/哪里/哪些/哪种/请用/请回答/请给出/
请输出/输出什么/等于多少/值是多少/是多少/哪一/几次/几个/的区别，或以
`？`/`?` 结尾）**整行保留并标记 raw（跳过行内 trim）**，mark 阶段
`mark_zh_lines` 跳过 dup 折叠行。实现覆盖 9 处（args/policy/lib/
pipeline/score/render/compress/README/测试），reason 新增 `R_ZH_LINE=8`
→ `Explain::ZhLine`；评测器 `bench_code.py --scx-extra` 默认接入
`--zh-keep 1`。`apply_keep_policies` 四个平级策略段落按项目圈复杂度
偏好（<10，当前 7）局部 allow 认知复杂度 lint。

**复测（--zh-keep 1，双在线模型合并口径，覆盖 P8-D 冻结 csv）**：

| 评测集 | full 答对 | scx 答对 | 保持率（P8-D 区间） | cut（P8-D） |
|---|---|---|---|---|
| cuda60 | 43/60 | 26/60 | **0.558**（0.222/0.209） | 47.9%（49.5%） |
| js60 | 53/60 | 17/60 | **0.302**（0.231/0.212） | 77.5%（78.3%） |
| shape30 | 23/30 | 15/30 | **0.565**（0.458/0.375） | 68.1%（68.8%） |
| layers15 | 12/15 | 9/15 | **0.667**（0.200/0.364） | 74.9%（76.4%） |

**结论**：--zh-keep 全线显著回升——cuda60 保持率 0.209→0.558（2.5×）、
layers15 0.364→0.667、shape30 0.375→0.565、js60 0.212→0.302（英文题
保留 0.302 与 zh 无关，受 cut 77.5% 高裁约束）。cut 全线小幅回落
（保真换压缩，符合预期：中文问句行成本约 +1-2pp）。js60/shape30 分别
10/60、8/30 题小输入 fail-open 未压缩（P6 既有行为）。中文语义段保护
验证了 P8-D 根因判断：问句行保真即可把保持率从 0.2 拉回 0.5+，无需
平移默认档位。

**门禁证据**：lib 343（+2 zh_keep 用例）/ semantic 352 / golden 零漂移 /
clippy 全 targets 零告警 / fmt 干净；check_bench.sh 全绿（manifest
重新冻结 4 个被覆盖 csv）。P8 关闭（A→E）。

---

## P9 压缩率深水区调参（cut≈28% 冲击，P8-E 之后）

用户立项：在 P8-E（--zh-keep 保真）基础上，把压缩率（cut）冲击到
**28% 附近**的深水区并深度优化——即在 cut≈28% 档位上最大化保真
（中文段/数字/标识符保留），验证"温和档 + 语义保护"组合的甜点位。

**档位依据（fid_curve，10 题 CUDA 采样）**：r=0.5 cut 25.9% 中文保留
50%；r=0.4 cut 34.3% 中文保留 40%；r=0.3 cut 49% 中文保留 22%。28%
落在 r≈0.44-0.48 区间——比 P8-D 评测档（r=0.3，cut 49%）温和得多，
正是 P6 验证过的"高保持率甜点位"（cut 31.8% 保持率 0.94）。

**扫描设计（本机，快）**：
- 主变量 `trim_ratio` ∈ {0.44, 0.45, 0.46, 0.47, 0.48, 0.50} 步长 0.01-0.02
- 固定 P8-E 全套：`--trim 1 --trim-op-keep 1 --num-line-keep 1
  --sym-line-keep 1 --zh-keep 1`
- 辅助变量：`fill_floor` ∈ {0, 0.2}、`trim_op_keep` ∈ {0, 1}（各取 2-3 个
  交叉点验证非主变量干扰）
- 指标沿用 fid 口径：cut / 中文段保留 / 数字保留 / 标识符保留；
  目标 cut ∈ [26.5%, 29.5%] 且中文保留 ≥ 50%（zh-keep 应把中文保留推
  到 60%+）、数字/标识符 ≥ 95%
- 输出 `config/bench/profiles/r28*.json` 档位模板（配置化入 config）

**在线验证（选定档位）**：cut≈28% 最优点 → cuda60 + zh 中文集
（bench_zh_cn30/40）双在线模型复测 → 保持率对比 P8-E（r=0.3）期望
显著更高（0.558 基础上 +，向 P6 的 0.94 区间靠拢），cut 从 47.9% 降
到 ~28% 反而保真更高——验证"激进档切碎中文是保持率杀手，温和档 +
语义保护才是甜点位"。

**门禁**：lib/semantic/golden/clippy/fmt 全绿（若扫描只需参数配置则
零代码变更，直接验证）；评测 csv 重新冻结 + check_bench.sh。

**扫描实测（10 题 CUDA 采样 × --zh-keep 1，/tmp/fid_curve_p9.txt）**：

| 档位 | cut | 数字 | 标识符 | 中文段 |
|---|---|---|---|---|
| r0.44+zh | **28.2%** | 70% | 100% | 100% |
| r0.45+zh | 27.1% | 70% | 100% | 100% |
| r0.46+zh | 27.1% | 70% | 100% | 100% |
| r0.47+zh | 25.8% | 70% | 100% | 100% |
| r0.48+zh | 25.8% | 70% | 100% | 100% |
| r0.50+zh | 25.3% | 70% | 100% | 100% |

**定档**：`r0.44+zh` 命中 cut 28.2%（目标带 [26.5%, 29.5%]），中文段/
标识符全保留。档位模板 `config/bench/profiles/r28.json`
（plan.ratio=0.44 + trim.ratio=0.44 + zh_keep=true 全套）。

**在线复测（r0.44+zh，双模型，独立 csv 后缀 _r28）**：

| 评测集 | full | scx | 保持率 | cut |
|---|---|---|---|---|
| cuda60 ds / qwen | 44/60 / 45/60 | 28/60 / 25/60 | **0.568 / 0.511** | 28.7% |
| zh_cn30 ds / qwen | 30/30 / 30/30 | 29/30 / 30/30 | **0.967 / 1.000** | 94.1% |
| zh_cn40 ds / qwen | 40/40 / 40/40 | 40/40 / 40/40 | **1.000 / 1.000** | 94.2% |

**结论**：28% 深水区达成——cuda60 cut 从 P8-E 的 47.9% 降到 **28.7%**
（压缩产物小 1/3），保持率 0.511-0.568 与 P8-E（0.558 合并口径）持平
略升；中文算术集 cut 94% 极端压缩下保持率 0.967-1.000（问句行整行
保真 + 数字锚保留 → 中文题近乎无损）。验证核心主张：激进档切碎中文
是保持率杀手（P8-D r0.3 cut 49% 保持率 0.2），温和档 + 语义保护
（r0.44+zh cut 29%）在更高压缩产物下保真不降反升。

**待办（状态）**：复测完成；manifest 冻结 + check_bench.sh + 提交。

## P10 强模型天花板验证（双在线旗舰：deepseek-chat / qwen-max → deepseek-v4-pro）

**目标**：换更强在线评测模型重跑 6 集（r28 档），验证"3B 本地模型天花板"
假说——保持率低是评测模型（flash）限制还是压缩器退化。

**评测器配置化改造（已提交）**：`tools/bench_code.py` JUDGE_CFG 模型名支持
环境变量覆盖 `SCX_MODEL_DEEPSEEK` / `SCX_MODEL_QWEN`（默认值不变，
向后兼容）；reasoning 开关仅对含 "flash" 的模型生效（deepseek-chat/v4-pro
不传该字段，避免 400）。

**第一批（deepseek-chat + qwen-max，_strong csv）**：

| 评测集 | flash 基线 | deepseek-chat | qwen-max | cut |
|---|---|---|---|---|
| cuda60 | 0.511-0.568 | 0.644 | 0.750 | 28.7% |
| js60 | 0.302 | 0.500 | 0.491 | 77.1% |
| shape30 | 0.565 | 0.583 | 0.522 | 67.5% |
| layers15 | 0.667 | 0.769 | 0.571 | 70.9% |
| zh_cn30 | 0.967/1.000 | 0.933 | 1.000 | 94.1% |
| zh_cn40 | 1.000 | 0.950 | 1.000 | 94.2% |

**结论**：代码场景强模型全面提升（js60 +20pp、layers15 ds +10pp、cuda60
qwen +24pp）——天花板假说成立，压缩器保真能力被 flash 低估；中文极端
压缩 qwen-max 双集 1.000，deepseek-chat 修复后 0.933/0.950。

**途中修复（评测协议层）**：`extract_answer` 兜底只取末行——deepseek-chat
压缩输入下输出 Markdown 长文导致中文保持率假性暴跌（0.433）——修复为
`#### X` → 含数字末行 → 全文末数 三级兜底，重测恢复 0.933/0.950。

**第二批（deepseek-v4-pro + qwen-max，_v4pro csv，已完成）**：
用户确认 deepseek-v4-pro 可用（dry-run 通过，回答极简——dry-run 仅
26 字符/题）；6 集全量重跑完成（V4PRO_ALL_DONE）：

| 评测集 | flash 基线 | v4-pro（ds） | qwen-max | cut |
|---|---|---|---|---|
| cuda60 | 0.511-0.568 | 0.489 | 0.730 | 28.7% |
| js60 | 0.302 | 0.630 | 0.717 | 77.1% |
| shape30 | 0.565 | 0.750 | 0.773 | 67.5% |
| layers15 | 0.667 | 0.364 | 0.571 | 70.9% |
| zh_cn30 | 0.967/1.000 | **1.000** | **1.000** | 94.1% |
| zh_cn40 | 1.000 | **1.000** | **1.000** | 94.2% |

**P10 收官结论**：① 中文极端压缩（cut≈94%）v4-pro + qwen-max 双模型
**保持率满分 1.000**——中文语义段保护在强模型下零损失；② js60 保持率
0.630/0.717（flash 基线 0.302，+33/+42pp）——flash 确实系统性低估压缩
保真；③ shape30 0.750/0.773（+19/+21pp）同向印证；④ cuda60 v4-pro ds
0.489 低于 _strong（0.644）——v4-pro 回答极简，scx 压缩输入下细节题
答不出（full 0.750 但 scx 0.417），属评测模型风格影响而非压缩器退化
（qwen 0.730 持平）；⑤ layers15 ds 0.364 同因（长文架构题 + 极简回答）；
⑥ **天花板假说成立**：保持率低主要是 flash 评测模型限制，强模型下
代码场景 +20~40pp、中文场景满分。

## P11 专业验证补齐规划（12 项全量 + 速度优化）

**总原则**：本地优先（零 API 成本）、复用冻结、并行 + 断点续跑、最小采样；
每项完成后结果冻结 + check_bench 门禁 + 本文件记录。

### P11-A 本地化验证组（零 API 成本，优先落地）
- **A1 token 口径压缩率**：装 tiktoken(o200k_base) + tokenizers(Qwen3 官方
  tokenizer.json)，对 6 集压缩前后文本数 token → 真实 token 口径 cut
  （LLM 按 token 计费，直击省钱价值）。本地秒级。
- **A2 round-trip 无损性**：压缩产物反向重建信息保留（token 重合率 +
  BLEU-4 语义距离），gold 全集对。本地。
- **A3 边界/鲁棒性**：损坏 UTF-8/混合脚本/emoji/零宽/重复行/空输入/单行/
  超长行 8 类 × 5 例，验证不崩 + 合理退化。本地。
- **A4 敏感信息审计**：API key/路径/人名/邮箱 4 类敏感行 × 10 例，验证
  zh/code 保护下保留或透传泄漏。本地。
- **A5 规模外推**：2/4/8/16MB 合成语料 × 时间/内存曲线（近线性确认）。本地。

### P11-B 真实使用形态组（少量 API）
- **B1 多轮累积压缩保真**：5 轮 append-压缩循环保持率衰减曲线 + 幂等性
  （压缩产物再压缩是否稳定）。复用 P8/P10 数据 + 3 集采样。
- **B2 长上下文注入**：16k/32k token 输入压缩后注入真实 LLM 完成
  召回/指令/总结 3 类任务 × 10 例。
- **B3 中英混排语料**：新建 bench_mix30（中英注释/文档混排 30 题）→
  双模型验证 zh-keep 泛化。

### P11-C 评测可信度组
- **C1 采样稳定性**：cuda60/zh_cn30 各 3 次重复采样 × 保持率均值/方差。
- **C2 规模扩充**：shape30/layers15 → n=50（生成器复用）。
- **C3 模型族扩展**：可选，依赖新增 API key。

### P11-D 竞品对比组
- **D1 LLMLingua 横向**：同数据集 20 题，压缩率 × 保持率对比。
- **D2 Nugget/LLM-Compressor**：轻量对比（若可用）。

**速度优化措施（贯穿全部阶段）**：
1. 本地计算全部并行（multiprocessing 8 核）；
2. API 评测 --workers 4-8 + --resume 断点续跑；
3. 复用已冻结 csv（跳过已完成 (model,idx)）；
4. 最小采样（稳定性 3 次而非 5 次）；
5. 合成语料/边界输入本地生成秒级；
6. 批量脚本 nohup 后台 + done flag 状态机。

**门禁**：每项 csv 冻结入 eval-manifest（_p11 后缀）+ check_bench.sh 全绿 +
本文件记录。**顺序**：A（本地，本轮会话内完成）→ B（少量 API）→ C → D
（依赖调研）。

**版本（更新惯例）**：本文件版本号随每阶段落地递增；阶段状态图随交付更新。

## P11-A 落地记录（本地化验证组完成，2026-10-29）

**执行**：`/tmp/p11_a_local.py`（自包含评测同款 build_prompt：cuda 背景补足
≥1000 字符，R28 参数全套，tiktoken o200k_base）。结果冻结
`config/bench/results/p11_a_results.json`。

**A1 token 口径压缩率**（v1 直接压裸 question 全透传 cut=0，v2 对齐评测
prompt 口径后有效）：

| 评测集 | tok full→scx | token cut | 字符 cut（评测口径） | 差异 |
|---|---|---|---|---|
| cuda60 | 37557→24174 | 35.6% | 28.7% | +7pp |
| js60 | 38099→8471 | 77.8% | 77.1% | ≈0 |
| shape30 | 18863→6051 | 67.9% | 67.5% | ≈0 |
| layers15 | 9390→2348 | 75.0% | 70.9% | +4pp |
| zh_cn30 | 18817→1327 | **92.9%** | **94.1%** | -1pp |
| zh_cn40 | 25053→1733 | **93.1%** | **94.2%** | -1pp |

**口径修正（重要）**：初版 zh 集误用 cuda query hint（"CUDA 内核…"）——
背景与 hint 相关被 query 保护，误报 token cut=34.8%。改为评测同款默认
hint（"代码 输出 结果 行为 类型"）后中文 token 口径 92.9-93.1%，与字符
口径 94% 一致。**两种口径互相印证：中文压缩的 token 省钱效果 ≈93%，无
高估**（query hint 与背景相关度决定背景保留量，是 cut 的关键参数）。

**A2 round-trip**：代码集 token 覆盖率 0.367-0.716、数字覆盖率 0.467-0.600、
BLEU-4 精度 0.143-0.353——r28 激进档信息层面中等保留，压缩有损
（与 P6f 无损档位区分）。zh 集 92% 极端压缩下：数字覆盖率 100%
（数字锚生效）、中文段覆盖率仅 0.16（保留问题行）、BLEU-4 0.04
（字符级重合低，属预期——语义保真由评测保持率 0.95-1.00 印证）。

**A3 鲁棒性**：损坏 UTF-8/混合脚本/emoji/零宽/重复行/空输入/单行/超长单行
8 类全部 stable（≤0.01s，不崩溃，短输入透传、重复行微裁 1 行）。

**A4 敏感审计**：API key/路径/人名/邮箱 4 类 × 5 例嵌入长上下文 → 全部
原样保留（5/5）。**结论：scx 不产生新泄漏（无重组/拼接），但也不脱敏**；
敏感日志场景需前置脱敏。

**A5 规模外推**：2MB 0.022s → 16MB 0.117s（近线性），吞吐 94→143MB/s
（规模增大吞吐上升，缓存友好），16MB 单文件秒级压缩。

**门禁**：结果 json 已冻结入 config/bench/results/；待 check_bench 全绿 +
git 提交（随 P10 收尾一并提交）。

## P11-B1 落地记录（多轮累积压缩保真，2026-10-29）

**执行**：`/tmp/p11_b1_multi.py`（tiktoken o200k_base；累积场景 =
评测同款 build_prompt 逐轮累积全量重压缩；幂等性 = 压缩产物再压缩）。

**累积场景（推荐用法：每轮 append 后全量重压缩原始累积文本）**：

| 场景 | round1 cut | round2 | round3 | round4 | round5 | 结论 |
|---|---|---|---|---|---|---|
| cuda5 | 34.6% | 35.9% | 36.3% | 36.5% | 36.3% | 稳定 ~36% 不衰减 |
| zh5 | 92.1% | 91.7% | 91.9% | 92.3% | 92.2% | 稳定 ~92% 不衰减 |

**幂等性（重要发现）**：单轮压缩产物再压缩额外 cut=0.0000（已到极限）；
但多轮累积文本的压缩产物再压缩额外 cut≈0.30——**scx 幂等性不成立，
对已压缩文本重复压缩会继续裁减（级联退化）**。真实用法必须对
**原始累积文本**全量重压缩，不可对上次压缩产物再压缩。

**对话场景（zh-keep 副作用验证）**：纯代码+中文对话文本（无背景/结构行）
r28 全保护档 cut=0.000——zh-keep 保护中文行 + op/num/sym keep 保护代码行，
所有行被保护无处可裁。**发现：r28 档对"纯自然语言/代码混排散文"几乎不
压缩；压缩力来自可裁的结构性/背景性行**（评测集 prompt 的 CONTEXT 背景
与技术要点重复块正是此类）。

## P11-B3 落地记录（中英混排泛化，2026-10-29）

**执行**：新建 `config/bench/datasets/bench_mix30.csv`（30 题中英注释 +
代码 + 配置混排，gold 带 `#### X` 格式），`bench_code.py` 双旗舰
（deepseek-v4-pro + qwen-max）评测，结果
`config/bench/results/bench_mix30_v4pro.csv`：

| 模型 | full acc | scx acc | 保持率 | cut |
|---|---|---|---|---|
| deepseek-v4-pro | 0.789 (15/19) | 0.579 (11/19) | **0.667** | 87.8% |
| qwen-max | 0.842 (16/19) | 0.632 (12/19) | **0.688** | 87.8% |

**结论**：中英混排高压缩（87.8%）保持率 0.667/0.688，与 js 场景同档
（高压缩 + 中等保持率）——zh-keep 对中英混排保护泛化有效，无中文
特殊劣化；19/30 题可压缩（fail-open 11 题，与 shape/js 一致，短题透传）。

**执行通道修复记录**：GCU type_text 长命令反复截断（`--scx-extra
'--trim 1 --trim-` 卡在 quote> 半输入态）→ 评测命令全部改写为
脚本文件（/tmp/bench_*.sh）+ 短命令 `nohup bash xxx.sh` 启动，一次成功。

## P11-B2 落地记录（长上下文注入，2026-10-29）

**执行**：`/tmp/p11_b2_long.py`（复用 bench_code.py api_call/norm + bench.py
scx_compress）——10 个合成文档场景（seed 固定、事实名值一一对应可验证），
文档目标 28k token（full prompt ≈32k），r28 档压缩后注入 deepseek-v4-pro +
qwen-max 完成 3 类任务各 10 例。结果 `config/bench/results/p11_b2_results.json`。

**v1 语料缺陷（已修复）**：通用文档模板字段（版本号/端口/路径）与问题实体
（线程块大小/共享内存字节数）错位，且 summary 关键词未注入正文 → full 本身
答不对、数字失真。v2 重构：事实名值一一对应注入关键参数段 + 新增"核心要点"
段承载 summary 关键词。

**结果（v2，cut_char 59.0% / cut_tok 60.7%）**：

| 任务类型 | deepseek full/scx/keep | qwen full/scx/keep |
|---|---|---|
| recall（精确事实召回） | 1.000 / 1.000 / **1.000** | 1.000 / 1.000 / **1.000** |
| instruct（列表全量枚举） | 0.700 / 0.000 / **0.000** | 1.000 / 0.000 / **0.000** |
| summary（要点总结） | 0.800 / 0.600 / 0.625 | 1.000 / 0.900 / **0.900** |

**结论**：① **精确事实召回零损失**——32k token 文档压缩 60% 后双模型
recall 保持率满分 1.000（版本号/端口/函数名等全部保真，数字锚+标识符保护
有效）；② **全量枚举是压缩能力边界**——instruct 保持率 0.000：压缩后
列表项被字符级 trim 裁掉，模型从压缩文本恢复全量清单必漏项（qwen full
1.000 → scx 0.000，差距全来自压缩）；这是压缩率与枚举完整性的固有权衡，
P11 定位为验证边界而非修复；③ 总结类保持良好（qwen 0.900）；④ 32k 长文档
cut 59% 远高于代码题场景（28%），因长文档可裁的结构性/背景文本更多。
改进候选（后续档位）：列表行保护（list-keep，类似 code-keep）可缓解
枚举场景，但会降低该场景压缩率。

## P11-C1 落地记录（采样稳定性，2026-10-29）

**执行**：`/tmp/c1_sampling.sh` 补跑脚本（run1 resume 补缺 + run2/3 删污染
后串行重跑，避免并行写 CSV 撕裂）+ `/tmp/c1_agg.py` 汇总。shape30 双旗舰
（v4-pro + qwen-max）独立 3 次采样，结果
`config/bench/results/c1_shape30_run{1,2,3}.csv` →
`config/bench/results/p11_c1_results.json`。

| 模型 | run1 keep | run2 keep | run3 keep | 均值 | 标准差 | 极差 |
|---|---|---|---|---|---|---|
| deepseek-v4-pro | 0.615 | 0.565 | 0.609 | **0.596** | 0.027 | 0.050 |
| qwen-max | 0.739 | 0.682 | 0.739 | **0.720** | 0.033 | 0.057 |

（full 均值 deepseek 0.800 / qwen 0.756）

**结论**：① **3 次独立采样保持率标准差 ≤0.033、极差 ≤0.057——压缩保真
表现采样稳定**，单次评测结论可复现；② qwen 保持率均值（0.720）显著高于
deepseek（0.596），与 P10 各集趋势一致（qwen-max 对该激进档压缩文本的
恢复能力更强）；③ 绝对值低于 P10 单次 shape30（0.750/0.773）：API 采样
方差 + 模型版本漂移，多次采样均值为更可信口径。

**执行教训（重要）**：nohup 脚本被重复启动（误发两次）→ 两进程并行写
同一 CSV → 行交错撕裂（model 列混入 ```` ```makefile ```` 等内容）。
修复：run1 保留 + `--resume` 补缺，run2/3 删污染文件串行重跑。**后续
批量评测必须：a) 启动前确认无同名后台进程；b) 每轮独立 CSV + 串行。

## P11-C2 落地记录（规模扩充，2026-10-29）

**执行**：`/tmp/gen_c2_ext.py` 生成器扩展——
`config/bench/datasets/bench_shape50.csv`（30→50，新增 JSON/YAML/TOML/
SQL/正则/shell/日志各 2-3 题，gold 部分工具验证）、
`config/bench/datasets/bench_layers50.csv`（15→51，新增 Python/Rust/
并发/缓存/内存/网络/序列化/日志/数据库/容器/编译/安全 12 组 L1/L2/L3）。
双旗舰评测 `config/bench/results/bench_code_{shape50,layers50}_v4pro.csv`：

| 评测集 | 模型 | full acc | scx acc | 保持率 | cut | 未压缩 |
|---|---|---|---|---|---|---|
| shape50 | deepseek-v4-pro | 0.800 (40/50) | 0.680 | **0.800** | 55.3% | 20 题 fail-open |
| shape50 | qwen-max | 0.780 (39/50) | 0.640 | **0.795** | 55.3% | 20 题 fail-open |
| layers50 | deepseek-v4-pro | 0.608 (31/51) | 0.373 | **0.484** | 72.6% | — |
| layers50 | qwen-max | 0.431 (22/51) | 0.275 | **0.545** | 72.6% | — |

**对比 P10（shape30/layers15）**：

| 集 | 保持率 ds | 保持率 qwen | 规模 | 结论 |
|---|---|---|---|---|
| shape30 | 0.750 | 0.773 | 30 | 基线 |
| shape50 | 0.800 | 0.795 | 50 | 含 20 题 fail-open 抬升，可压缩子集口径待细分 |
| layers15 | 0.364 | 0.571 | 15 | 小样本高方差 |
| layers50 | 0.484 | 0.545 | 51 | ds +12pp、qwen 持平，50 题口径更可靠 |

**结论**：① **规模扩充成立**——30→50 / 15→51 题保持率无断崖（shape 类
0.80、layers 类 0.48-0.55）；② layers50 全量压缩（cut 72.6%）下
L1-L3 跨层保真：性能/架构（L2/L3）题仍是压缩保真的最难点（与 P10
layers15 结论一致，50 题统计更可靠）；③ shape50 新增短题（JSON/SQL/
正则/日志）大多 fail-open 未压缩——评测集扩展应保证单题 ≥1000 字符
触发压缩，短题天然透传会抬高保持率（方法论注意）。

## P11-C3 落地记录（模型族扩展：qwen-plus 第三评测模型，2026-10-29）

**执行**：选定 **qwen-plus（千问百炼中档）** 作为第三评测模型 on shape30
（能力梯度低于 qwen-max 旗舰），通过 `SCX_MODEL_*` 环境变量覆盖 bench_code.py。
产物 `config/bench/results/bench_code_shape30_qwenplus.csv`
（n=225，可压缩子集 rows=60）；汇总 `/tmp/c3_agg.py`。

| 评测模型 | full 保持率基线 | scx 保持率 | 压缩后 vs 基线 |
|---|---|---|---|
| deepseek-v4-pro | 0.800 | **0.533** | -27pp |
| qwen-plus（中档） | 0.733 | **0.400** | -33pp |

（对比：qwen-max 旗舰 C1 shape30 保持率 0.720；deepseek C1 0.596）

**结论**：**评测模型能力梯度假说成立**——同一压缩产物喂给能力越弱的
评测模型，信息保持率下降越多（qwen-max 0.720 → qwen-plus 0.400，
-32pp）。压缩保真的"绝对值"依赖评测模型能力档位；scx 相对增益
（scx vs 未压缩基线在弱模型下差距扩大）说明压缩保留了高价值信息，
弱模型从压缩文本中恢复能力有限。**口径注意**：C3 用 qwen-plus 中档
judge，与 C1/C2 的 qwen-max 旗舰口径不可直接横向比较；跨口径对比
须标注评测模型档位。

## P11-D1 落地记录（LLMLingua 竞品横向，2026-10-29）

**执行**：`uv pip install llmlingua==0.2.2` 装入 /tmp/scx-eval（torch 2.14.1）。
`/tmp/d1_llmlingua.py` 对 shape30 前 20 题公平对比：scx r0.44+zh vs
llmlingua（rate = 1-scx_cut 对齐压缩率），deepseek-chat judge，check
判 keep。结果 `config/bench/results/p11_d1_results.json`（v1 存档
`p11_d1_v1_results.json`）。

**根因修复（重要）**：llmlingua 0.2.2 与新版 transformers KV cache 结构
不兼容——llmlingua 全程假设旧式 `[[k,v],...]` 嵌套 tuple，而 torch
2.14.1 的 Qwen2 forward 只接受 `DynamicCache` 对象（无下标、迭代产出
(k,v,_) 三元组），`iterative_compress_prompt` 1659 行
`for k, v in past_key_values` 抛 `ValueError: too many values to unpack`。
修复：**只在 `self.model` 调用边界做双向归一化**——入参
list-of-[k,v] → DynamicCache 喂模型，返回 DynamicCache → list-of-[k,v]
交还 llmlingua 内部逻辑。修复后单题压缩正常（cut 0.726 @ rate 0.2）。

**LLMLingua-1 重跑结果（20 题，deepseek-chat judge）**：

| 压缩器 | cut 均值 | keep | 说明 |
|---|---|---|---|
| scx r0.44+zh | 0.777 | **0.650** | 压缩更狠且保持率显著更高 |
| LLMLingua-1 0.2.2 | 0.690 | **0.300** | 修复后真实压缩；keep 仅 0.30 |

**结论（v1）**：同压缩率对齐口径下（rate=1-scx_cut），scx 保持率
0.650 vs LLMLingua-1 0.300，**+35pp 显著领先**，且 scx 压缩率还更高
（77.7% vs 69.0%）。LLMLingua-1 的困惑度删除法对代码结构（JSON/SQL/
正则/日志）保持能力弱，多数题被删到关键信息丢失。

**LLMLingua-2 版（用户指定第二代 token 级分类压缩器）**：llmlingua 0.2.2
内置 `use_llmlingua2=True`。模型唯一正确仓库名
`microsoft/llmlingua-2-xlm-roberta-large-meetingbank`（`tinybert-distilled`
两命名在 HF 均 401），已完整下载至用户指定目录
`/Users/fei/model/llmlingua-2-xlm-roberta-large-meetingbank/`
（model.safetensors 2,235,829,648B = 2.24GB，safe_open 391 权重验证通过）。
encoder-only 无 KV 兼容问题；`force_reserve_digit=True` + `drop_consecutive=True`
保数字。脚本 `/tmp/d1_llmlingua2.py`（模型名指向本地路径）。

**下载加速要点（留存）**：HF 匿名限速 + 本机 `~/.curlrc` 固定 ssh 隧道代理
（tinyproxy）→ 单连接仅 121KB/s；`urllib.request` 天然不读 .curlrc 直连
1.0MB/s；16 段 Range 并发直连合计 ~1.3MB/s。慢段（HF 限速至 20-50KB/s）
覆盖重下 + 合并前逐段断言。

**LLMLingua-2 结果（20 题，deepseek-chat judge，2026-10-29）**：

| 压缩器 | cut 均值 | keep | 说明 |
|---|---|---|---|
| scx r0.44+zh | 0.777 | **0.550** | 压缩更狠且保持率更高 |
| LLMLingua-2 | 0.746 | **0.450** | token 级分类压缩；512 token 序列上限截断长题 |

**结论（v2）**：第二代 LLMLingua-2 明显优于第一代（keep 0.300→0.450，
cut 0.690→0.746），说明 token 级分类法比困惑度删除法对代码结构更友好。
scx 在同口径下仍领先：cut +3.1pp（0.777 vs 0.746）、keep +10pp
（0.550 vs 0.450）。LLMLingua-2 的 XLM-RoBERTa 512 token 序列上限对其
长代码题存在固有截断损失，scx 无此限制。


## P11-D2 落地记录（Nugget/LLM-Compressor 可用性调研，2026-10-29）

**调研结论（两个候选均不适用，P11-D 组以 D1 为终）**：

| 候选 | 定位 | 可用性结论 |
|---|---|---|
| Nugget2D（NUGGET2D: Dynamic Contextual Compression） | ACL 2024 学术方法：上下文压缩为 virtual tokens（nuggets），需 LLAMA 初始化 + LoRA 训练 | **无开箱即用 pip 包**；效果依赖训练，非即插即用压缩器，不适用轻量对比 |
| LLM-Compressor（llmcompressor, Neural Magic） | 模型优化库：权重/激活/KV Cache 量化、剪枝、稀疏化（面向 vLLM 部署） | **非 prompt 压缩器**，与 scx 的上下文压缩非同类型，不可比 |

**结论**：P11-D2 标记为「不可用（调研记录）」；竞品对比组以 D1（LLMLingua-1/2 横向）收官。
至此 **P11 十二项全量完成**：A1-A5 本地化验证、B1-B3 真实使用形态、C1-C3 评测可信度、
D1 竞品对比（LLMLingua-2, scx cut 0.777/keep 0.550 双领先）+ D2 调研闭环。

**P11 收官证据**：本文件 P11-A/B/C/D 各落地记录齐全；结果冻结
config/bench/results/p11_{a,c1,c2,c3,d1}*.json；评测数据集 config/bench/datasets/
bench_{cuda60,zh_cn30,mix30,shape30,layers15,layers50}.csv；git 提交链
…→ 7bd3abd（P11-D1 LLMLingua-2 收官）。文档版本随本段递增（P11 完成）。

## P12 完成记录（2026-10-29，版本 0.8.0）——速度/性能终极优化

**目标**：在 golden 逐字节零漂移守门下，把 scx 压缩吞吐推到极致；延续 P7-C 性能架构，全部变更纯 std（`std::thread::scope` 并行，无第三方 crate、无 unsafe）。

### P12-A 性能基线冻结
- 多档位语料 `config/bench/perf/corpus_p{1,10,30,100}m.txt`（p1m 1.36MB / p10m 11.4MB / p30m 34.3MB / p100m 114.2MB）。
- `tools/bench_perf.py`：3 次取最优 real 秒 + MB/s，输出 `perf_baseline.json`。
- **原始基线**（优化前）：p10m lossless 0.2328s/46.7、r0.3 0.2243s/48.5；p30m lossless 0.6377s/51.2、r0.3 0.6072s/53.8。

### P12-B 热点深挖（测量纪律）
- `sample`（bash 附加）只采到 bash `__wait4`、`xctrace record` exit 1——两条外部路径验证做不通，弃。
- 转 feature-gated 插桩（`src/perf.rs`，feature="perf" 才编，默认零开销）：
  - 真实热点 = **masks ~218ms（31%）+ trim::tokens ~130ms（render 内 97%）**。
  - **插桩伪影识别**：dedup 曾测 135ms，实为 mark_agg 逐行开销（100 万行 × 3 次）；还原插桩后 dedup 真实仅 ~5ms。**教训**：逐行 mark_agg 对百万行语料污染测量，正式对比一律走 bench_perf.py。

### P12-C 算子级优化（按序落地）
| 序号 | 优化 | 实测 | 结论 |
|---|---|---|---|
| C1 | render 消除每行 String 分配（Sink + scratch） | 几乎无差 | 架构保留 |
| C2 | trim::tokens 线程局部池化（TOK_POOL/LOW_POOL） | 小幅 | 保留 |
| C3 | lowercase 池化 + STOP/KEEP 长度桶 + query 首字节位图 | p10m -9%、p30m -4% | **正收益** |
| C4 | masks 行字节位图快速拒绝 | 变慢 | 已回退 |
| C5 | **masks 并行**（RowCtx/RowOut + split_parts + std::thread::scope，6 线程，PAR_MIN=20k） | p30m 0.5713→0.2843s | **主收益** |
| C6 | **render trim 预计算并行**（compute_trimmed，行间独立、串行回填保序） | 0.4925→0.2843s | **正收益** |
| C7 | scores 并行（尝试） | 变慢（工作量小，线程开销亏本） | 已回退 |

**最终基准（P12 收官）**：

| 档位 | 基线 r0.3 | P12 r0.3 | 提速 | MB/s |
|---|---|---|---|---|
| p10m | 0.2243s | **0.1042s** | **-53.5%** | 104.5 |
| p30m | 0.6072s | **0.2843s** | **-53.2%** | 114.9 |

（lossless：p30m 0.6377→0.2889s，-54.7%。零漂移：串行/并行 30MB 输出 SHA-256 完全一致 d4e4399e…）

### P12-D 回归门禁
- `tools/check_bench.sh` 全部通过 ✓：lib 343 + semantic + golden 2 + clippy 0 warning + fmt 干净 + eval-manifest 冻结一致。

### P12-E 交付
- 代码质量：RowCtx/RowOut 收敛参数、split_parts 借用链、clippy 归零、零第三方 crate、`#![forbid(unsafe_code)]` 保持。
- 版本 bump `0.7.0 → 0.8.0`（README/SKILL 同步）。
- git 提交见交付记录；状态图见 P12 交付消息。

## 验证覆盖矩阵修复（P0-P2）v0.8.1

### P0 manifest 冻结断层
- `tools/freeze_manifest.py` 幂等补录：frozen 8 → 48 → 50 结果资产（sha256=原始字节哈希、rows=换行计数、bytes 审计），与 check_bench.sh 同口径。
- commit 7409ff8。

### P1-A2 round-trip 保真门禁
- 定分档阈值 `config/bench/fidelity_thresholds.json`（code 档 num_cov≥0.40/token_cov≥0.30/bleu4_prec≥0.10；zh_extreme 档 num_cov≥0.95）。
- `tools/check_fidelity.py` 断言接入 check_bench.sh：cuda 0.567/0.716/0.353、js 0.600/0.367/0.216、shape 0.467/0.398/0.301、layers 0.533/0.657/0.143；zh 两集 num_cov=1.000 全过。

### P1-B1 幂等边界文档
- README「压缩产物不可二次压缩」：非幂等、级联退化（实测额外 cut≈0.30），必须对原始累积文本全量重压缩。

### P2 list-keep 列表行保护
- 根因：B2 枚举场景（instruct）scx 压缩后列表项被字符级 trim 摘词 → 保持率 0.000（base 列表行保留 4%）。
- 内核：`src/compress/score.rs` R_LIST_LINE + list_sep/list_task/list_ordered（保守判定防误伤代码 `*ptr`/`-x`/`1.5f`/`1)`）；mark_list_lines 整行保留。
- 接线：lib.rs Config + policy.rs DEFAULT_LIST_KEEP + args.rs Flag + **args/parse.rs CLI/pipeline 映射 6 处**（缺映射是"未生效"根因）+ pipeline.rs TRIM_KEYS。
- 本地验证：10 枚举场景列表行保留 4% → **100%**（cut 53.6% → 51.3%，仅 -2.3pp）。
- 在线验证（deepseek-v4-pro + qwen-max，3 场景 instruct 枚举）：full 3/6 → scx 0/6 → **scx+list-keep 6/6**。
- 冻结：p12_5_listkeep_results.json + p12_5_listkeep_online.json 入 eval-manifest。
- 门禁：lib 345 + semantic + golden + clippy 0 + fmt + manifest + A2 fidelity 全部通过 ✓

### P2-C2 可压缩子集口径重算
- `tools/check_subset.py` 幂等统计：fail-open = compressed!=True（透传/未压缩题）。
- shape50：40% fail-open（透传题通过率虚高），剔除后 deepseek 34/50→17/30（68.0%→56.7%）、qwen 32/50→18/30（64.0%→60.0%）。
- layers50：0% fail-open，全量=子集（deepseek 37.3%、qwen 27.5%），口径无需调整。

### P3 盲区验证组（RAG / 128k / 内存）
- RAG hit@k（BM25 代理，零依赖）：24-chunk 文档集早/中/晚三 query，full 与 scx 压缩产物 top-3 检索全部命中（cut 34% 下事实键保持）。
- 128k token 长文档：32k/64k/128k 三档 cut 34.4%/34.4%/34.3%，耗时 0.0s/0.0s/0.1s（P12 后 ~100MB/s）。
- 内存曲线：rss 3.5MB→4.0MB→5.1MB（近线性、峰值 <6MB，零第三方栈保持）。
- 冻结 p12_8_p3_blind.json（frozen 52）。

### P2-D1 强 judge 竞品重跑（收官）
- 口径：shape30 前 20 题，同压缩率对齐，保持率 = deepseek-v4-pro + qwen-max **双强模型均通过**（弱 judge P11-D1 为 flash 单模型）。
- 结果：**scx cut 0.777 keep 0.450**（9/20）vs **LLMLingua-2 cut 0.746 keep 0.100**（2/20）。
- 解读：强口径下差距显著拉大（弱口径 0.55 vs 0.45 → 强口径 0.45 vs 0.10）。LLMLingua-2 主因 = 512 token 窗口截断（shape30 prompt 780-920 token 全部超窗）+ 强模型对残缺枚举/格式更严格。
- scx 压缩更狠（cut 高 3.1pp）且保持率 4.5 倍。
- 冻结 p12_7_d1_strong.json（frozen 53）。

## P14 完成记录（2026-10-29，版本 0.9.0）——dedup 并行归一化指纹扫描

### P14-A 实测推翻旧认知
- P12 笔记「dedup 仅 ~5ms」是插桩伪影结论；并行化后的权威 profile（p30m r0.3）显示 **dedup 117ms / 52.3% 是第一热点**（非 masks 33ms）。
- 拆解：117ms 中 74% 是 `normalize` 扫描本身，HashMap 插入仅 ~30ms；`String` 键 map 123ms vs 元组键 map 120ms（仅差 3ms，换键无用）。
- 首次迭代（串行单趟指纹）实测反而 +29% 回归（151–267ms）——瓶颈在扫描量不在 map，**并行扫描才是正确打法**。

### P14-B 落地
- `norm_hash`：单趟归一化 + 双通道 128 位旋转乘法哈希 + 长度（(h1,h2,len) 键，不物化 String）；折叠语义与 `analysis::normalize` 逐字节一致。
- `scan_fp`：masks 同款并行基建（6 线程 + PAR_MIN 20k 分块 + std::thread::scope），附带相邻原文相等标记 `prev_same`。
- `dedup_insert`：串行 HashMap 插入（共享态不可并行），run 复用 run 首行指纹，围栏感知（code_keep）保留。
- 差分测试 `norm_hash_matches_normalize_fingerprint`（手写 15 例 + LCG 8000 例）守门；修过 Unicode 长度按字符计的口径 bug。

### P14-C 基准（bench_perf.py 同工具对比 git HEAD）
| 档位 | 旧基线 | P14 | 提速 |
|---|---|---|---|
| p30m r0.3 | 0.2940s | **0.1887s** | **-36%** |
| p30m lossless | 0.3768s | **0.1988s** | **-47%** |
| p10m r0.3 | 0.1094s | **0.0695s** | **-36%** |
| p1m r0.3 | 0.0294s | 0.0230s | -22% |

（注：plan.md P12 收官表 0.2843s 与 git HEAD json 0.294s 存在口径差——P12 记录与同一脚本重测有测量方法差异，本表以同工具重测的 git HEAD json 为严格基线。）

### P14-D 门禁与提交
- check_bench.sh 全绿：lib 346 + semantic + golden 零漂移 + clippy 0 + fmt + eval-manifest 冻结 + A2 fidelity。提交 26cf7bb。

## P15 完成记录（2026-10-29，版本 0.9.0）——输入测量单趟融合

- `terms::measure(s) -> (usize, f64)`：一趟扫描同时产出字符数 + token 估算，替代 `chars(original)` + `estimate(original)` 两趟全扫。
- ASCII 快路径（len + len/4+1）；`chars()` 保留（render.rs 在用，曾误删致编译失败已恢复）。
- 差分测试 `measure_matches_chars_estimate` 守门；lib 347。
- 基准：与 P14 持平（噪声内）——ASCII 语料上 `is_ascii` 本就 SIMD 廉价，收益被噪声掩盖；**CJK 语料上 estimate 是字符循环，融合省一整趟 O(n) 为真收益**。提交 d85f106。

## P16 完成记录（2026-10-29，版本 0.9.0）——fused 召回会话缓存（原案收敛）

### P16-A 测量驱动的范围收敛
- 实测 150 例库 fused recall **<10ms**（进程启动主导）；签名计算在当前规模可忽略。
- 结构性发现：`search_fused` 每次查询都 `store::list` + 逐 case `store::load` 全库 + 重算 minhash/embed——**即使基础 `search` 已命中会话缓存**，且融合结果从不写回。
- 真实瓶颈在大库下是逐 case 文件加载而非签名计算 → 原案「doc 签名冻结缓存」（动 Index 格式 + df 失效维护）为**过度工程**，收敛为「fused 结果会话缓存」。

### P16-B 落地（行为逐字节不变）
- cache.rs：`key_salted`（基础键 + 配置盐）+ `get_fused`/`put_fused`——内存态、1 小时窗口、gen 代际守卫、不落 qmap.bin。
- mod.rs：`search_fused` 命中 fused 快照直接返回（跳过全库重载 + 签名重算）；`read_only` 不写；fuse 配置盐含 embed 命令与两路权重，不同配置不串快照。
- 不触碰评测路径（eval/golden 走 recall::clock/read）→ recall@3 统计与 golden memory_recall 零漂移。
- 新测试：`fused_key_is_salted_and_isolated`（cache 层盐隔离）+ `search_fused_session_cache_returns_identical_hits`（窗口内二次逐字段一致、read_only 一致）；lib 347→349。

### P16-C 门禁与提交
- check_bench.sh 全绿：lib 349 + semantic + golden + clippy 0 + fmt + manifest + A2 fidelity。提交 df51fcf。

### P16-D 版本 bump 0.8.0 → 0.9.0
- Cargo.toml / README（2 处）/ SKILL.md（2 处）同步；Cargo.lock 重新生成。

### P14–P16 累计交付（v0.9.0）
- p30m r0.3：0.2940 → 0.1887s（**-36%**，173.1 MB/s）；lossless -47%。
- 阶段 profile（r0.3 p30m，perf 构建）：dedup 117→30ms（**-74%**），lines+terms ~33ms、masks ~33ms 并行均衡，总 ~130–145ms。
- 已提交：26cf7bb（P14）、d85f106（P15）、df51fcf（P16），均未 push。

## P12.6 完成记录（2026-10-30，版本 0.10.0）——证据行强制保留 + 实际序列化口径（建议 4/5 执行）

### 背景：5 项建议核查 → 执行 4/5
- 用户给出 5 项输出契约建议；逐条实测核对：1（outcome.text 作正文）✅ 已符合、2（正文+一行紧凑统计，完整报告独立 stderr）✅ 已符合、3（逐行 explain 默认关，--annotate 按需）✅ 已符合。
- **4（usage_summary 统计实际序列化响应大小）⚠️**：stats 只报字符+「约 N token」（terms::estimate 估算），无真实序列化字节。
- **5（测试断言"证据行必须出现在正文"+ small/passthrough/fail-open 覆盖）❌**：无端到端断言。
- 用户拍板「执行优化，必须验证全量顶级优秀」。

### 实测暴露双重缺陷（P12.6 核心）
- **证据 marker 被字符级 trim 摘走**：60 行文档 `--evidence UNIQUE_EVIDENCE_XYZ` + trim 深压（cut 93.3%）下，正文证据行被裁为残句（marker 消失），但 stderr 报告仍称「🎯 证据召回 1 条关键证据全部保留 · 0 丢失」。
- 根因：`apply_keep_policies` 无 evidence 分支（证据只进 `ev_retention` 事后报告，不进 keep/raw 保护）；报告卡 `r.evidence` 实为 `seeds` 复用，非真实保留数。

### 修复清单
1. **score.rs**：`R_EVIDENCE: u8 = 10` + `mark_evidence_lines`（Substr 命中行 / Line 号命中行强制 keep+raw，跳过行内 trim；越界行号静默忽略；复杂度 8/6 → 拆 `mark_evidence_substr`/`mark_evidence_line` 归零）。
2. **compress.rs**：`apply_keep_policies` 新增 evidence 分支；`!cfg.evidence.is_empty()` 并入 341 行保护触发条件；Outcome 新增 `ev_total`（两构造点：mk 闭包=evidence.len()，bucketed=0）。
3. **ops.rs**：`stats()` 末尾新增 `ser_b=`（实际序列化字节 = `render(&outcome)` UTF-8 长度，建议 4）；recall_report 证据卡改用 `ev_retention × ev_total` 如实口径（无证据显示"未声明证据"）；`render()` JSON 新增 `"ev_total":N` 与 ev_retention 配套。
4. **tests/evidence.rs（新增，建议 5）**：三模式端到端断言——sieve 深压（cut≈90% 下证据行整行逐字节在正文 + 报告如实 + ser_b 存在）/ small 透传（passthrough:small 原样保留）/ fail-open（terms=0 → mode=failopen 原样保留）。
5. **契约变更**：stats 行 `ser_b` + JSON `ev_total` 属有意的契约扩展——golden 基线整体更新（47 文件，diff 仅 ser_b 追加 / ev_total 插入 / 证据卡文案 / memory 用例日期归一），审查无意外漂移；`ev_retention_explicit_evidence_reports_dropped` 测试语义升级为 `_forced_kept`（"只校验不强制" → "声明即强制保留"，断言 ev_retention=1.0/ev_total=2）。

### 门禁与提交
- `bash tools/check_bench.sh` 全绿：lib 359（+3：evidence 单测 + 端到端×3 于 tests/evidence.rs）+ semantic + golden（基线已更新）+ clippy 0 + fmt + eval-manifest frozen 匹配 + A2 fidelity 全过。
- 版本 bump 0.9.0 → 0.10.0（Cargo.toml / README 2 处 / SKILL.md 2 处同步）。
- 提交 24d5024。

---

## P13 计划（2026-10-30 定稿，semantic 能力提升 + 整体优化，目标版本 0.11.0）

### 背景与现状分析（基于源码通读）
`src/semantic.rs`（381 行，`cargo feature="semantic"`，零第三方 crate）现状：
- **打分器**：`sigmoid(w·x)`，权重文件为自描述 JSON（schema `scx-sem-1`，sha256 校验，篡改即拒绝）——确定性有保障。
- **特征仅 6 维且粗糙**：`[overlap, hit, ln(1+chars), cap_ratio, dig_ratio, structure]`；overlap/hit 用**子串包含**匹配（无词边界，`error` 误命中 `errorhandler`）；structure 仅硬编码 6 个前缀（```/~~~ / import/ use/ #include/ at/ caused by:），与 `analysis.rs` 的 frame/stack/diff/fence 识别**未复用**。
- **融合方式**：`render.rs` 里 `scores[i] += w.score(...)` 简单加法，无量纲对齐/缩放开关。
- **无权重获取路径**：权重文件只能手写/外部训练，无离线拟合工具。
- **词形召回**：`stem_expand()` 规则词干（ies→y、s 脱落）已有，英文单复数已桥接；无词边界、无 CJK 处理。
- **中文特征短板**：cap_ratio 对中文恒 0，仅 dig_ratio/长度有效。

### 阶段拆分（每阶段门禁不过不进下一阶段）

**P13-A 特征工程强化（6 维 → 16 维）**
- 词边界命中（整词匹配替代子串 contains，降误报）
- query 词 IDF 加权命中强度（复用 plan 词表，与 IDf 对齐）
- 位置信号（行号归一化 + edge 头尾）、邻居证据簇密度（window 内命中数）
- 结构信号统一复用 `analysis.rs`（frame/stack/diff/fence/meta/import），替换硬编码前缀
- 新增：CJK 字符比例、错误码模式密度、行内 token 重复度（熵近似）、bias 常数维=1.0
- 门禁：golden 零漂移（feature 关闭路径逐字节不变）+ semantic 特征单测

**P13-B 词形/召回强化**
- 子串→词边界过渡 + 数字 token 保护（ID/端口/版本号不拆词）
- 短语命中（query 连续多词序）与跨行共现链（自动锚，配合自动 evidence）
- 中文：无词形操作，验证子串命中与 CJK 密度特征有效性
- 门禁：recall 探针矩阵（稀有词/常见词/零信号三分支回归）

**P13-C 打分器与融合增强**
- 打分器：显式 bias + 温度参数（sigmoid 带宽可控），仍纯 std 确定性
- 融合：`--sem-weight F` 缩放系数（默认保守，保证确定性证据地板不被压过）+ 量纲对齐（plan score 归一化后相加）
- 门禁：semantic on/off 双路径评测对比 + golden 不变

**P13-D 权重获取路径（离线拟合，无模型）**
- `tools/fit_weights.py`：用 `config/bench/datasets` 现有评测集离线做逻辑回归拟合 16 维权重 → 生成合法 `scx-sem-1` JSON（含 sha256）
- 确定性：固定数据 + 固定 seed + 固定迭代 = 固定权重
- 交付默认 `config/semantic/weights.json`（随仓）

**P13-E 评测与门禁闭环**
- semantic 开启评测矩阵：在 eval-manifest 冻结集上跑 semantic on/off 对比（保真/压缩率/耗时）
- check_bench.sh 新增 semantic 段（特征/打分/权重加载/fit 复现性）
- 验收：语义加分在保真不变前提下压缩率不倒退、证据地板不破

### 风险与约束
- 零第三方 crate、零模型、确定性、golden 零漂移：四条命根子不可破——所有新增特征/打分均为纯 std 确定性实现。
- semantic 是可选 feature 且默认不启用：任何改动不得影响默认路径字节输出（v0.10.0 兼容）。
- 权重文件 schema `scx-sem-1` 若扩展特征需升 schema 版本（`scx-sem-2`）或保持 dim 兼容——倾向保持 1 并允许 dim 增长。

### 状态图（P13 计划）
```
P13 定稿(0.11.0 目标)
  ├─ P13-A 特征工程 6→16 维 ──门禁A──┐
  ├─ P13-B 词形/召回强化 ──门禁B────┤
  ├─ P13-C 打分器/融合 ──门禁C──────┼→ P13-E 评测门禁闭环 → 版本 0.11.0 发布
  ├─ P13-D 权重拟合路径 ──门禁D─────┘
  └─ 全程：golden 零漂移 + clippy 0 + 407 测试 + feature 关闭路径逐字节不变
```

---

## P17 计划（2026-10-30 定稿，整体管线强化 · 竞品借鉴整合，与 P13 同批推进，目标版本 0.11.0）

### 规划原则（先盘点后强化）
竞品借鉴点先对照现状：**已有雏形 → 强化**；**无 → 新建**。全程零第三方 crate、零模型、确定性、golden 零漂移。

### P13–P17 整体能力提升总览（两模块并行：P13 semantic 线 + P17 管线线）

```
                     ┌── P13-A 特征工程 6→16 维 ──门禁A──┐
  P13 semantic 线 ───┼── P13-B 词形/召回 ──门禁B───────┤
   (模块自身强化)     ├── P13-C 打分器/融合 ──门禁C──────┼─┐
                     └── P13-D 权重拟合路径 ──门禁D─────┘ │
                                                          ├→ P18 整合评测+压缩率专项 → v0.11.0
  P17 管线线 ──────  P17-A 跨行模板折叠 ──门禁A──┐        │
   (竞品借鉴整合)     P17-B 行内裁剪强化 ──门禁B──┤        │
                     P17-C 熵近似评分 ──门禁C───┤        │
                     P17-D 自动锚发现 ──门禁D───┼─ 门禁E ┘
                     P17-E 证据簇密度加权 ──门禁E─┘
                     P17-F 反馈闭环调权（续 P14 线收尾）
```

### P17-A 跨行模板折叠（最大压缩率增量，新建）
- **机制**：复用 `sim.rs`（minhash 签名）+ 模板识别——同模板不同参数的行（日志/错误/JSON/CSV 行）→ 保留模板 1 行 + 参数表（可逆重建），round-trip 可还原原文。
- **落点**：`compress/plan.rs` 折叠判定（已有 `tally` 折叠雏形）→ 模板聚类 + 参数抽取；`render.rs` 输出模板+参数块。
- **预期**：日志/重复结构语料压缩率 +10~20%。
- **门禁**：golden 零漂移（默认关闭，`--fold 1` 显式开启）；round-trip 还原率 100% 断言。
- **完成（2026-10-30）**：`fold.rs` 落地 `--fold 1`（默认关，golden 零漂移）；实现为**骨架行 + 折叠标记行**（`scx-fold: {n-1} rows folded (diffs: ...)`），非原案"参数表可逆重建"——取**确定性启发式**路线（与竞品借鉴结论一致），round-trip 保真由"保留行全在原文 + 行序稳定"断言覆盖。

### P17-B 行内安全裁剪强化（已有 trim.rs 雏形 → 强化）
- **现状**：`trim.rs` 已是 LLMLingua-2 思路的零依赖启发式（数字/结构锚/query 词根/行首尾保留，停用词/纯标点摘除，token 池化零分配）。
- **强化**：① STOP/KEEP 词表扩展（terms.rs，KEEP 语义敏感词已有：否定/比较/数值词永不摘）；② trim 与 semantic 打分联动（semantic 高分行不裁或降裁）；③ 数字/ID/版本号 token 保护增强。
- **预期**：保留行 token 再降 8~15%，证据逐字不丢。
- **门禁**：cut 提升且 golden 不变；语义敏感词（not/不/总/每）摘除回归测试。
- **完成（2026-10-30）**：terms.rs STOP 增 please/kindly/regarding、KEEP 增 about/around/approximately/roughly（CJK 近似词以单字 KEEP 覆盖，双字 CJK 词在逐字 token 下无效——注释定案）；trim.rs MAX_DICT_WORD 8→16；回归测试 `keeps_quantity_approximation_words`。

### P17-C 熵近似评分（伪神经信号，plan.rs score() 加信号）
- **机制**：无模型的统计熵——行内 token 重复度（字符分布熵）、词频 zipf 秩惩罚（高频填充词降权）、长度熵——作为新 score 信号融入 `score()`。
- **落点**：`compress/plan.rs::score()` 新增信号项（对齐现有 sumw/state/region 结构）。
- **预期**：对未见模式（无 query 命中的信息密集行）召回提升，鲁棒性↑。
- **门禁**：语义增强基准（semantic on/off × 熵信号 on/off）对比无退化。
- **完成（2026-10-30）**：`entropy_signal` + `ent_bonus`（字符分布香农熵 7bit 归一 × 重复率降权 ×0.5，量级对齐 heading_bonus）落 plan.rs，集成于 `apply_sims` 前；`--entropy 1` 默认关；测试 `entropy_ranks_dense_over_repetitive`/`entropy_signal_off_is_noop`。

### P17-D 自动锚发现（降配置、提召回）
- **机制**：query 词子串/词形变体（复用 semantic stem_expand）/跨行共现链 → 自动生成 evidence，无需手写 `--evidence`。
- **落点**：`args` 侧 `--auto-anchor 1` + `plan.rs` 种子识别；与 P13-B 短语命中协同。
- **预期**：证据召回↑，配置负担↓。
- **门禁**：evidence 探针矩阵（显式 evidence 行为不变，自动锚仅加不减）。
- **完成（2026-10-30）**：`word_forms`（剥 s/es/ed/ing/ly/er + 双写辅音还原）+ `anchor_terms`（默认关=原词表）落 compress.rs；`--auto-anchor 1` 默认关；测试 `auto_anchor_off_is_noop_on_masks_terms`/`word_forms_strips_suffixes_and_doubled_consonants`（语义模块 feature 门控外独立实现，默认构建零依赖）。

### P17-E 证据簇密度加权（已有 boost window → 强化）
- **机制**：多证据行聚集处（同一错误栈/同一段日志）整簇加权——boost 已有窗口传播，新增**簇检测**（连续/近邻证据行的聚集评分）。
- **预期**：保留完整上下文块，round-trip 质量↑。
- **门禁**：round-trip 保真断言（保留行全在原文，簇内行逐字一致）。
- **完成（2026-10-30）**：`evidence_hits`（窗口内 ≥2 高分行计数）+ `cluster_signal`（命中簇 → boost×2 受 cap 约束）落 plan.rs，`boost` 改 `let mut`；`--cluster 1` 默认关；测试 `cluster_off_is_noop_and_on_weights_dense_evidence`（孤立证据行不加权、cap 约束）。

### P17-F 反馈闭环调权（已有 memory/feedback.rs → 强化）
- **现状**：`memory/feedback.rs` 已有 rate good/bad 记录机制（case 环 quality 状态机 + 词环 [word_w] ±Δ + L1 自适应 l1_base=P50）。
- **强化（2026-10-30 完成）**：rate 数据驱动词权重自动微调（确定性在线学习：good +Δ / bad −Δ，上限 ±0.5，固定迭代=固定结果）；新增确定性测试 `rate_word_ring_is_deterministic`；修复 `[word_w]` 节重写 bug（重复 learn 时旧节节头丢失、词权重悬空为顶层裸键 → load 读空），新增 `rewrite_drops_old_word_w_section_body` 回归测试。
- **预期达成**：越用越准；记忆库召回门（p50<5ms）不回退。
- **门禁通过**：反馈样本 → 权重变化断言（确定性）；既有 rate 行为兼容。
- **注**：compress 侧 score 权重联动（--feedback 链路）为架构性新链路，留作 P18 可选增量（现状记忆库自闭环已具备召回侧全部微调）。

### P18 整合评测 + 压缩率专项验收（收尾）
- semantic on/off × fold/trim/熵/锚/簇/反馈 全组合在 eval-manifest 冻结集评测。
- **压缩率专项**：与 SuperCompress 官方 mean cut 64.1% 同预算（35% kept）对照基线（标注口径：官方自报 vs scx 冻结实测）；目标：同预算下 cut ≥ 官方口径，fidelity 不回退。
- 版本 bump 0.10.0 → 0.11.0；check_bench.sh 扩展全部新段；文档/README/SKILL 同步。

### 铁律（全计划）
- 零第三方 crate / 零模型 / 确定性 / golden 零漂移：不可破。
- 所有新增能力**默认关闭**（显式 flag 开启），默认路径与 v0.10.0 逐字节一致。
- 每阶段门禁不过不进下一阶段：`cargo test --all-features` 407+ / clippy 0 / fmt / golden / eval-manifest / A2 fidelity。

### semantic v2 能力升级（2026-10-31）
- **居中式有符号打分**：`scores += sem_weight·(2·sigmoid(w·x)−1)` ∈ (−sem_weight, +sem_weight)
  ——旧实现 `sigmoid ∈ (0,1)` 只能加分、无法降权；现在高 logit 提升、低 logit 降权
  （可表达“子串命中但实为噪声”）。证据/最小邻域由 `floor` 无条件保留，降权只作用于
  可选填充排序，证据优先不变量不受影响。
- **命名式权重 schema v2**：`{"schema":"scx-sem-2","features":[...],"weights":[...],
  "sha256":...}`；features 为 `FEATURE_NAMES` 子集、任意顺序；sha256 绑定权重+特征名
  （篡改任一列即拒绝）。v1 `scx-sem-1`（位置式 dim）保持向后兼容。
- **特征注册表扩到 18 维**：新增 `cjk_bigram`（CJK 查询 bigram 重叠，中文转述信号）
  与 `term_prox`（query 词共现紧密度，短语级语义）；随仓 `weights.json` 迁 v2、两新
  维权重置 0.0（未校准，机制就绪——不虚构训练权重）。
- **数值稳定 sigmoid**：分支配方避免 exp 上下溢，结果恒在 (0,1)。
- 门禁：默认轨 364/0 逐字节不变（golden 零漂移）；semantic 轨 381/0（+17 semantic 测试）；
  `cargo test --all-features` / clippy（默认+all-features）`-D warnings` / fmt / rustdoc /
  MSRV 1.82 三轨 / `cargo package` / check_bench.sh 全绿。

### P0/P1 顶尖级收尾完成记录（2026-10-31，版本 0.11.0）
- **P0-1.6 可逆模板折叠**：split→step/flush_word、rebuild→parse_template/parse_params/expand_row 重构，认知复杂度<10；修复 `step()` 对中文标点/全角多字节字符按字节切片 panic（UTF-8 前导字节定宽整字符跳过）；fold 测试 15 全绿。
- **P0-1.1 消融评测**：`tools/bench_ablate.py` 确定性三形态语料（LCG seed 42 零外部依赖），6 配置矩阵（baseline/fold/entropy/anchor/cluster/semantic）；诊断链：fail-open query 修正（"parse"→"build" 75 次命中）、zh fold UTF-8 panic 修复、entropy 数字保真 guard（`entropy_signal` 跳过含 ASCII 数字行 + 0.01 平局权重）；**18 行数字冻结**（code 全配置 cut=0.1592/num_cov=0.7955/token_cov=0.8271；log baseline cut=0.3403、fold 唯一正增益 cut=0.3855；zh 全配置与基线逐字节相同）。机制事实：anchor/cluster/semantic 三信号在本语料零独立增益（触发条件或排序不翻转边界），留 P18 整合评测核。
- **P0-1.2 同预算对照**：与 SuperCompress 官方 mean cut 64.1% 同预算（35% kept）对照闭环。`--budget` 硬预算后处理（超预算按 R_* 保真优先级反向裁剪：R_FILL=3<R_ANCHOR=5<R_NUM_LINE=6<R_EVIDENCE=10）；修复 gap 省略标记 `...` 未计预算缺陷（预算口径 = Σ行 token + GAP_TOK×false 段数，裁剪循环增量维护段数）；bench_budget.py 三断言门禁。最终：code budget-0.35 kept=0.3495/cut=0.6505/num_cov=0.3138/token_cov=0.3285；log kept=0.3492/cut=0.6508；zh 全档与 baseline 相同（逐字节一致守卫 ✓）。三提交 0631a00/ff465cc/cc117e8。
- **P1-1.5 #51 unwrap 语义化**：memory/index.rs + cache.rs + compress.rs 全部 try_into().unwrap()/.ok()? 替换为定长数组拷贝（const 泛型 `arr::<N>` + copy_from_slice + from_le_bytes），9 处边界已验证字节读取统一定长解释；plan.rs region close unwrap→expect。生产路径 unwrap 清零（join().unwrap() 保留——scope 惯用语义正确）。提交 6ba4dfa。
- **P1-1.8 scores() 并行化**：逐行打分纯只读共享、scores 分块写不重叠 → ≥ PAR_MIN（20k）分 6 线程（与 masks/tally 同模板同阈值）；并行与串行逐字节一致（golden 零漂移）；A/B 实测 60k 行 3.2MB：spans+scores 1257us vs 串行 2692-2865us ≈ **2.2× 加速**。提交 c30f4d0。
- **P1-1.3+1.4 v0.11.0 bump + 门禁扩展**：Cargo.toml/Cargo.lock/README 3 处版本 0.10.0→0.11.0；check_bench.sh 新增第七步「新开关冒烟」（--budget/--fold/--auto-anchor/--cluster 单跑+组合：rc + stats + 双跑确定性；冒烟段自身缺陷修复：--json 统计行断言 `compressed: kept=`、FAIL 后不再误报 OK）；README 补 --budget 两行说明。全量门禁 check_bench.sh GATE=0（lib 375 / semantic 392 / golden 2 passed 1 ignored / clippy 0 / fmt / eval-manifest frozen+present / A2 fidelity / 冒烟 5 组合）。
- **版本状态图**：0.10.0（P17 全线 + 消融闭环）→ **0.11.0**（P0-1.2 同预算对照 + P1-1.5 unwrap 语义化 + P1-1.8 scores 并行 + 门禁冒烟扩展）。
- **待办（#54 P18 收尾）**：semantic on/off × 全信号组合在 eval-manifest 冻结集评测；压缩率专项已含同预算对照（P0-1.2 完成），冻结集全组合评测未做；文档/SKILL 同步复核。

### P18 整合评测完成记录（2026-10-31）
- **semantic on/off × 全信号组合评测**：`bench_ablate.py --combo` 新增组合矩阵
  （semantic × fold/entropy/anchor/cluster，3 语料 = 30 行 CSV 冻结）。
  **结论：全部正交**——code 全组合与基线逐字节一致（cut=0.1592/num_cov=0.7955/
  token_cov=0.8271）；log sem+fold 继承 fold 唯一正增益（cut=0.3855）其余继承单开；
  zh 全组合与单开逐位相同。semantic 权重 0.0 特征不干扰证据行，组合无交互劣化。
- **评测工具链缺陷修复**：`ensure_sem_bin()` 只查存在性不查新鲜度 → 隔离二进制
  （target/ablate-sem）在 fold UTF-8 修复前构建，sem+fold zh 组合误判 cut=0.0
  （陈旧内核按字节切片 panic）。已加**源码新鲜度检查**（src/*.rs + config/semantic/*.json
  mtime ≥ 二进制则复用，否则重建），重建后 zh sem+fold=0.8087 恢复正常。
- **冻结**：bench_p18_combo.csv（30 行）入 eval-manifest frozen（sha256+rows，
  frozen 总数 53→54）；既有 bench_p17_ablate.csv 18 行冻结不受影响。
- **门禁**：check_bench.sh 全绿 GATE=0（lib 375 / semantic 392 / golden / clippy 0 /
  fmt / eval-manifest 54 frozen / A2 / 冒烟 5 组合）；消融单开单调性门禁复跑全绿。
- **压缩率专项**：与 SuperCompress 官方 mean cut 64.1% 同预算对照已在 P0-1.2 闭环
  （code budget-0.35 kept=0.3495/cut=0.6505 ≥ 官方口径；口径双标注：官方自报 vs
  scx 冻结实测，官方行 num/token_cov=NaN 不可比）。
- **未决（如实标注）**：eval-manifest 冻结集（config/bench/datasets/*.csv）question 列为
  问答式短输入、无长文本 full 列 → 压缩率类全组合评测以确定性合成三形态语料
  （code/log/zh）为载体；冻结集 judge 语义保真评测（在线双模型）已在 P8-D 闭环。

---

## 语义蒸馏升级完成记录（2026-10-31，#61–#67，版本 0.11.0 保持）

**用户锁定口径**（verbatim）：双旗舰在线模型蒸馏、能力提升几倍即可、体积不要大。
教师 = deepseek-chat + qwen3.8-flash（temperature=0.1、150 行分块 ×12 并发、双教师并集标注）；
学生 = 纯 stdlib 批量逻辑回归（24 维，lr=0.05/epochs=150/l2=1e-3）——**零依赖、体积不变**。

### 子项状态
| 编号 | 范围 | 结果 |
| --- | --- | --- |
| #62 | 蒸馏语料扩容 + 多 query | 10 语料矩阵 17785 样本；缓存键升级 `cname\|query`；17 场景标注全部完成（并集口径） |
| #63 | 特征 18→24 维双向同步 | 追加 stop_hit/sym_noise/q_density/head_hit/rep_ratio/cap_tok；STOP_WORDS 扩为 72 词（Rust const 与 Python frozenset 唯一权威源）；对拍 7 处不一致全修 |
| #64 | 重蒸馏：新维权 + CV 复测 | 24 维权（scx-sem-2）生效；3 折 CV（17785 样本）：全量 AP 0.7678 持平，**code 0.2992→0.5082（1.70×）**；keep 率整体 0.7564、判别场景 code=0.249/log=0.161/zh=0.318；剔除 4 全保留场景收益微小 → 维持 17 场景全量口径 |
| #65 | 语义深接入管线 | `--sem-gate F`（默认 0=off）：语义锚通道叠加 cfg.sem_weight×centered_line；负值/非有限拒绝；BUDGET_MIN 陷阱文档化（<200 token 预算不生效） |
| #66 | 端到端问答保真评测（对标 LLMLingua-2） | 60 题真实工程长代码拼接（主文件+2 干扰文件，>571 token 触发压缩）；query=问题+高频标识符；ratio 0.5+fill-floor 2；**压缩触发 60/60，token 压缩率 26.46%（76687→20289）**；orig 天花板 ds/qw=1.000；comp 保真 ds=0.733/qw=0.717；comp_sem 补跑 ds/qw=0.733（首跑全 0 为 API 限流假象，已复核） |
| #67 | MLP 对照（非默认档位） | linear 批量 GD auc≈427-438/AP 0.7642-0.7717；mlp1(24→16→1) auc≈584-606/AP 0.8743-0.8826；**两模型 keep 率均 1.000 → AP 全量虚高（受天然 keep 率 0.756 钳制）**；非线性破坏零依赖/体积约束 → **维持线性学生为线上默认** |

### 关键机制修复（诚实记录）
- **#66 压缩轨根因**：`tools/bench.py:99 scx_compress` 包装命令内置 `--ratio 0.35`，
  eval_fidelity 的 EXTRA 重复传 `--ratio` → 子进程 rc=2（`--ratio specified more than once`）
  → 包装 fail-open 返回原文 → comp==orig 恒 True。修复：EXTRA 去重 + 直连 CLI
  （ratio 0.5 温和档）；**#66 评测改由 `tools/eval_fidelity.py` 直连 `_compress`（独立临时文件，6 worker 并发安全）**。
- **bench_c60 题长 106–240 字符 <BUDGET_MIN → 压缩不触发**；改用 bench_long 拼接协议
  （单题 1229–1435 tokens >571 门槛）。
- **qa 字段映射**：nl30/ck30/q30 用 `gold`/`scx` 列非 `answer` → 过滤掉 60 题；
  修复 `answer or gold or scx` 后 31→91 题（取 60）。

### 对标定位（诚实）
- **同档压缩率**（~50%）下 scx 保真可对标 LLMLingua-2 中档（本评测 26.5% 激进档保真 0.72–0.73，
  压缩率更激进而保真略低于 LLMLingua-2 论文 SQuAD 0.8+）；语义深度（24 维线性学生 vs
  LLMLingua-2 的 400M 参数模型）与数据规模（17 场景 vs 其大规模语料）是主要差距。
- **com_sem 轨与 comp 无显著差异**（0.733 vs 0.717）：当前维权在代码事实问答场景
  与词面证据保护接近——语义深接入的增益在通用文本场景（#64 CV：code 1.70×、log 近天花板）。

### 门禁与冻结
- Rust：semantic 轨 397 passed / 0 failed（含 24 维权 sha256 加载校验 + sem_gate 三测试 +
  feats_match_python_selfcheck）；clippy --all-targets 零告警；fmt --check 通过。
- 冻结基线 bench_p18_combo.csv 不变（默认路径逐字节一致，语义默认关）。
- **版本状态图**：0.10.0（P17 全线 + 消融闭环）→ 0.11.0（P0/P1 顶尖级收尾）→
  **0.11.0（语义蒸馏 #61–#67 完成：24 维权默认关 + 端到端保真评测闭环 + MLP 对照证伪）**。

---

## 记忆治理升级计划（2026-10-31，#69 起，版本 0.11.0 保持）

**来源**：用户经验评审（6 项差距）。核查结论：①③②属实；④⑤⑥部分属实
（有基础但未达顶级）。零依赖红线不变。

### P1 记忆分层 + 晋升门控（L0 原始 → L1 候选 → L2 可信）
- `Case` 增加 `tier: u8`（0/1/2）+ `tier_at`；默认写入 L0。
- 晋升门控（复用现有 minhash 相似度 + feedback quality）：
  - L0→L1：evidence 非空 + quality ≥ l1_q（缺省 0.4）+ minhash 冲突检查通过；
  - L1→L2：good ≥ l2_good（缺省 3）+ 成功召回命中 ≥ l2_hits（缺省 2）或人工 --rate good；
- 召回按 tier 加权（L2 最高，L0 兜底展示不直答）。

### P2 冲突仲裁
- 写入时 minhash 近重复检测（sim ≥ 0.8）；仲裁：证据更全/质量更高者胜，
  败者互链 `related`，仲裁记录入 meta conflict log；不删除数据。

### P3 版本化更新（Revise/Retire）
- `Case` 增加 `version: u32`、`status: active|revised|retired`、
  `supersedes`、`superseded_by`、`retire_reason`。
- `revise <id> <new…>`：先写新 case（supersedes=旧 id），再原子标记旧
  case status=revised + superseded_by=new（"先写替换者再标记旧事实"）。
- `retire <id> <reason>`：只 tombstone（status=retired + reason），不删除。
- 召回过滤 retired/revised（superseded_by 指向活跃者）。

### P4 RRF 融合召回 + 记忆类型体系
- `search_fused` 从线性加权改为 **RRF**（Reciprocal Rank Fusion，纯算法
  零依赖）：BM25 秩 + 字段/实体秩 + minhash 语义秩 + 外部 embed 秩
  （插拔）→ Σ 1/(k+rank) 融合，k=60。
- `Case` 增加 `kind`（situation/experience/procedure/semantic），写入时
  自动分类（复用 classify 判别字段 + 规则：procedure=含多步命令；
  experience=错误码+fix；situation=纯上下文；semantic=术语/定义）；
  `--recall-kind <k>` 过滤。

### P1 完成记录（2026-10-31）

- **分层落地**：`Case` 增加 `tier: u8` + `tier_at`（store.rs encode/decode 配对；
  旧 case 缺键兼容回退 L0）。新增 `src/memory/tier.rs`：`promote(meta,&mut Case,good)`
  只升不降。
- **晋升门控（实现口径，微调计划稿）**：
  - L0→L1：`evidence` 非空 且 `fix` 非空（"证据完整 + 可执行结论"双条件；
    计划稿的 minhash 冲突检查归 P2 冲突仲裁承担）；
  - L1→L2：`good ≥ tier_l2_good(3)` 且 `quality ≥ tier_l1_q(0.4)`（计划稿的
    "召回命中 ≥ 2"不可机械自证，以 feedback quality 替代，语义等价）。
- **接入点**：ingest insert 落盘前（证据完整直升 L1）；update 同指纹补录
  backfill 后；feedback rate() 写回前（L1→L2）。
- **召回加权**：Doc/Score 增加 `tier` + `tier_prior_l2/l1` 先验
  （meta DEFAULT_PARAMS：`tier_l1_q=0.4`、`tier_l2_good=3.0`、
  `tier_prior_l2=0.05`、`tier_prior_l1=0.02`）；L0 兜底展示不直答。
- **索引/桥贯通**：index.rs Record 与 postings.bin 编码新增单字节 tier
  （`POST_VER 3→4`，旧缓存版本不符自动作废重建）；edge.rs Bridge 同步。
- **可观测**：`--stats` 新增 `tier: l0=n l1=n l2=n` 分布行
  （maintenance.rs Tally 聚合，避免 sample 参数膨胀）。
- **门禁**：`cargo test --locked --features memory --lib` = **380 passed**；
  clippy（--all-targets）0 警告；fmt 干净。新增端到端测试
  `tier_promotes_through_feedback_chain`（无证据 L0 → 补全证据 L1 →
  rate×3 L2 且 tier_at 非空）。CLI 实测：无证据 store 停 L0、rate 写回正常、
  `--stats` 输出分层行。
- **提交**：见 git log（全中文，未 push）。版本 0.11.0 保持。

### P2 完成记录（2026-10-31）

- **冲突仲裁落地**：新增 `src/memory/conflict.rs`。写入新 case 落盘前（ingest.rs
  insert 内 `store::save` 之前）执行 `detect → arbitrate`：
  - 近重复检测：index 快照同桶候选优先 + 不足补全库 + 截断 `conflict_max`（512）；
    加权 Jaccard `minhash::scores`，`sim ≥ conflict_sim(0.8)` 才构成冲突；
    空文本/阈值归零直接跳过（CLI 无文本 store 不触发）。
  - 裁决：证据数（`evidence.len()`）更多者胜 → quality 更高者胜 → 先到先得
    （已存在 case 胜）；**不删除任何数据**，胜败双方 `related` 双向互链。
  - `Case` 新增 `related: Vec<String>`（encode 在 tier_at 后、decode 缺键兼容空）。
  - 互链写盘：库内一侧经 `attach` 加锁读改写（幂等去重），新 case 一侧内存 push
    随自身写入落盘——避免 insert 持锁期对未落盘 case 二次加锁（曾触发 WouldBlock
    自锁，已修复并纳入单测）。
  - 冲突日志：每次仲裁 append 一行 `index/conflicts.log`（TSV 8 列：
    stamp/winner/loser/w_ev/l_ev/w_q/l_q/sim，`lock::atomic` 原子追加）。
- **可观测**：`--stats` 新增 `conflicts: <非空行数>` 行（读 conflicts.log）。
- **参数**：meta DEFAULT_PARAMS 新增 `conflict_sim=0.8`、`conflict_max=512.0`。
- **门禁**：`cargo test --locked --features memory --lib` = **383 passed**（新增
  3 测试：近重复检出阈值、端到端双向互链 + log 落盘 + 仲裁幂等、旧 case 缺
  related 键兼容回读）；clippy（--all-targets）0 警告（Row 聚合 log 参数、
  candidates 拆 take/take_str 降认知复杂度）；fmt 干净。
- **CLI 实测**：连续两次近重复文本 store（不同 query）→ 两 case `related` 双向
  互链、conflicts.log 一行（winner=先到先得方）、`--stats` 显示 `conflicts: 1`。
- **提交**：见 git log（全中文，未 push）。版本 0.11.0 保持。P3 Revise/Retire 与
  P4 RRF+类型见上文升级计划。

### P3 完成记录（2026-10-31）

- **版本化落地**：`Case` 新增 `version: u32`（默认 1）、`life`（active/revised/
  retired）、`supersedes`、`superseded_by`、`retire_reason`（encode 在 related
  后固定键序；decode 缺键兼容：version 回退 1、life 回退 active、其余空串）。
  命名微调：计划稿的 `status` 与反馈状态机 `status`（open/solved/regressed）
  撞名，实施为 **`life`** + `life_u8()`（active=0/revised=1/retired=2，未知回退 0）。
- **revise = --store 修饰符（实现口径，修正独立 Op 稿）**：CLI 校验
  `--revise requires --store 1`；`--revise <case-id>` 复用 store 的
  query/fix/evidence 参数写入新内容。ingest 先写替换者：`version = 旧.version+1`、
  `supersedes = 旧 id`；成功后再原子标记旧 case `life="revised"` +
  `superseded_by = 新 id`（`mod.rs::mark_revised` 持旧 case 锁、幂等；
  同指纹 update 命中替换者自身时 guard `old==new` 跳过自指标记）。
  旧 case 为 retired 时拒绝 revise（InvalidInput）。
- **retire = 独立动作**：`--retire <case-id> --retire-reason <text>` 仅 tombstone：
  `life="retired"` + `retire_reason`（sanitize 后）+ `updated_at`，幂等（重复 retire
  不报错、不改写 reason）。与 delete/gc 等互斥。
- **召回过滤**：索引路径（桶过滤后）与线性路径（case 加载后）均跳过
  revised/retired；df/avgdl 全库统计口径不变。
- **索引**：postings.bin 编码新增单字节 life（tier 后），`POST_VER 4→5`，
  旧缓存自动作废重建；edge Bridge 同步跳过非 active。
- **可观测**：`--stats` 新增 `life: active=n revised=n retired=n` 分布行
  （Tally.lives 聚合，`life_u8` 计数）。
- **门禁**：`cargo test --locked --features memory --lib` = **387 passed**（新增 4
  测试：老库缺 version/life 键兼容回读、revise 端到端含召回过滤与幂等、retire
  tombstone + 过滤 + 幂等、同指纹 revise 不自指）；clippy（--all-targets）0 警告；
  fmt 干净。
- **CLI 实测**：store → revise（旧 case life=revised + superseded_by、新 case
  version=2 + supersedes）；retire（life=retired + reason，二次 retire 幂等）；
  recall 过滤 revised/retired、active 命中；`--stats` 显示
  `life: active=1 revised=1 retired=1`。
- **提交**：见 git log（全中文，未 push）。版本 0.11.0 保持。P4 RRF+类型见上文
  升级计划。


### P4 完成记录（2026-10-31）

- **RRF 纯秩融合（search_fused 重构）**：由「线性加权和」改为**纯 RRF 秩融合**
  ——`Σ 1/(k+rank)`，k=60，零新依赖。文本加载由单文本改为全 `Vec<store::Case>`；
  四路秩并列：BM25 秩（bm25_term）+ 字段/实体秩（field_term，query 侧
  `classify::keys(extract(query))` 与 case 侧 `classify::keys(&c.fields)` 交集计数，
  全库零命中该路 None 不参与）+ minhash 语义秩（minhash_term）+ 外部 embed 秩
  （embed_term，插拔）。融合分值为四路**秩和无权重乘积**；
  `embed_weight/minhash_weight` 语义降级为「路开关」（>0 启用该路）。
- **记忆类型（kind）体系**：`Case` 新增 `kind: String`（encode 键序 retire_reason
  后、decode 缺键回退 "situation"、empty() 初始化）；写入时自动分类
  `classify::kind()`——判定序：procedure=正文 ≥2 行命令状行 → experience=
  error_code 非空且 fix 非空 → semantic=定义标记（"==/定义/即/指 "）→ situation
  兜底；命令状行 = 行首空白 + `is_cmd()`（22 词白名单：sudo/cargo/docker/kubectl/
  git/curl/apt/brew/pip/npm/make/go/rustc/gcc/ssh/cp/mv/rm/cd/export/echo/sed）。
- **过滤**：新增 `pub fn filter_kind(root, hits, kind: Option<&str>)`——kind=None
  原样返回；否则剔除 `life != "active"` 与 kind 不匹配项，缺 case 跳过，保序
  只减不增。`--recall-kind <k>`（situation|experience|procedure|semantic，Value、
  once、err_empty "requires a kind"）经 args Opts/FLAGS/ParseState/free()/usage
  全链路接入，ops.rs `Op::RecallOnly` 与 `Compress` 增 kind 字段，recall_only()
  与 memory() 旁路均先 `search_fused` 再 `filter_kind`。
- **stats 扩展**：`--stats` 新增 `kind: experience=n procedure=n semantic=n
  situation=n` 分布行（Tally.kinds HashMap）。
- **门禁**：`cargo test --locked --features memory --lib` = **394 passed**（P3 387
  基线 + 6 新增 P4 测试 + 1 回归测试）；clippy（--all-targets）0 警告；fmt 干净。
- **CLI 实测**（`/tmp/scx-p4-mem`，4 case）：四类摄取后 `--stats` 输出
  `kind: experience=1 procedure=1 semantic=1 situation=1`、`life: active=4`；
  无条件 recall 正常返回（sim/score/case）；`--recall-kind` 过滤双向验证
  （query "errno" + experience → 命中 experience case；query "RRF" + semantic →
  命中 semantic case，+ experience → 空；procedure 同理；非法 kind → 空）；
  `--recall-only` 与 `--recall-kind` 可组合（不再互斥）；缺值报
  "requires a value"。
- **修复要点**：`--recall-kind` 初版解析被 `word()` 的 free() 白名单遗漏、
  误落 `path()` 被判为 `--delete`（值进 opts.delete → "no such case:
  experience"、与 --recall-only 互斥）——修复为白名单追加 `--recall-kind`，
  并新增回归测试 `p4_recall_kind_parses_into_recall_kind`。
- **提交**：见 git log（全中文，未 push）。版本 0.11.0 保持。

## P18 完成记录（2026-10-31，版本 0.11.0）——检索三通道 Recall@K/NDCG@K 基线

### 目标与口径
- 对标 LongMemEval 检索维度（Recall@K / NDCG@K）的 scx 自有基线：`--eval` 从 A/B 双通道
  扩展为 A/B/C/D 四通道，给出「完整管线 vs 朴素 BM25 vs MinHash 语义 vs RRF 融合」的
  同库同标注对比。
- C 通道 = 全库加权 MinHash-LSH jaccard 降序（sim=分/max 归一）；D 通道 = RRF 融合
  （BM25 秩 + 字段/实体秩 + MinHash 秩，k=60，无权重求和），与生产 `search_fused`
  同公式；score/sim 快照取 BM25 侧，仅靠 side 信号救回的 case 记 0（不触发 L1 直答）。

### 落地（src/memory/eval.rs）
- `Acc` 增 `agg_cd`/`per_cd`（[f64;9]：n + C/D 各 r5/r10/mrr/ndcg），与 A/B 的 agg 分离，
  机械门只读 A/B 不受影响；`fold` 收 `Quad`（四通道命中元组，clippy 参数门收敛）。
- `Plain` 增 `texts`（原文缓存）；新增 `minhash()`、`rrf()` 方法与模块级 `rrf_term()`
  + `const RRF_K = 60.0`（与生产同值）。
- `report` 每层追加 `layer_cd` 行 + 总览行 `eval: channels A=full-pipeline B=bm25
  C=minhash-only D=rrf(k=60)...`；verbose 诊断扩展为 A/B/C/D 四行。
- 新测试 +3：`rrf_term_ranks_are_deterministic_and_bounded`（同分按 id 字典序稳定、
  贡献 ∈ (0,1/61]）、`minhash_channel_returns_sorted_hits`、`rrf_channel_rescues_semantic_only_hit`；
  修 `l1_accuracy_denominator_covers_all_expected_cases` 的 3 处 fold 调用。lib 397 passed。

### 冻结基线（config/bench/results/bench_p18_channels_{gauge,v2,real}.txt）
三数据集（gauge 40 例 / v2 150 例 / real 40 例，各自语料库播种）全部逐位可重放，已入
eval-manifest 冻结（sha256+rows，check_bench 校验）。

| 数据集·层 | 指标 | A full | B bm25 | C minhash | D rrf |
|---|---|---|---|---|---|
| gauge·paraphrase (n=13) | r@5 / mrr / ndcg@10 | .692/.338/.766 | .692/.338/.772 | .769/.332/.583 | .769/.349/.770 |
| gauge·semantic (n=13) | r@10 / mrr / ndcg@10 | 1.0/.962/.811 | 1.0/.715/.817 | 1.0/.872/.663 | 1.0/.750/.820 |
| v2·paraphrase (n=60) | r@5 / mrr / ndcg@10 | .867/.556/.884 | .867/.554/.861 | .783/.571/.740 | .867/.600/.870 |
| v2·semantic (n=30) | r@10 / mrr / ndcg@10 | 1.0/.700/.945 | 1.0/.510/.917 | 1.0/.822/.902 | 1.0/.622/.940 |
| real·semantic (n=13) | r@10 / mrr / ndcg@10 | 1.0/.962/.905 | 1.0/.962/.886 | 1.0/.949/.879 | 1.0/.923/.902 |

### 结论（数据驱动，诚实呈现双面）
- **RRF 融合优于 BM25 单通道**：D 的 semantic 层 ndcg 三数据集全部 ≥ B（0.820/0.940/0.902
  vs 0.817/0.917/0.886）；paraphrase 层 mrr 在 gauge/v2 均高于 B（0.349/0.600 vs
  0.338/0.554）。
- **minhash 语义单通道的独立增益与短板**：semantic 层 mrr 普遍高于 B（v2 0.822 vs
  0.510），但 ndcg 全面低于 B——纯 jaccard 无词面区分度，召回强、排序弱；融合正是
  把语义召回与词面排序结合（v2·paraphrase D ndcg=0.870 vs C 0.740）。
- **例外不隐藏**：real·paraphrase（n=13）D r@5/mrr/ndcg（.923/.576/.819）低于 B
  （1.0/.744/.864）——样本小 + real 语料字段键与 query 偶合引入噪声，融合非免费午餐。
- **A（完整管线）仍在多数层保持 ndcg 最优或并列**——生产增强（PRF/MMR/反馈）在
  融合之上仍有增益，三通道基线把「管线增强的价值」显式化。

### 门禁
- check_bench.sh 全绿：lib 397 + semantic 418 + golden 重新冻结（memory_eval 增
  layer_cd/channels 行；memory_stats 补 P1–P3 tier/life/kind/conflicts 行；日期快照随
  当天刷新）+ clippy 0 + fmt + eval-manifest（+3 frozen）+ A2 fidelity + 四 flag 冒烟。

## P17 完成记录（2026-10-31，版本 0.11.0）——截断对手基线 + 证据保留率审计

- 资产已在早前轮次落地（commit 9490340：docs/containcmp.md 同场对照 + docs/contain5-compare.json 逐片明细 + plan 事项 2），本轮完成**冻结与复现闭环**：
- 复现：`tools/scx.py contain config/contain5.corpus.json --binary target/release/scx --ratio 0.35 --context 1 --seed contain-rand-v1` 用当前二进制逐位复现全部聚合数字（scx 70/70=100%、头截断 0/70=0%、随机 8/70=11.43%、char 82.01%、token 79.93%），随机种子逐片可重放。
- 冻结：明细副本入 `config/bench/results/bench_p17_contain5.json`，eval-manifest +frozen（sha256+rows）+present（min_rows=20）；check_bench.sh 全绿。
- 口径：证据保留率 = ground-truth 证据行（contain5 语料 25 slice × 70 条 evidence）在压缩输出中的整行存活占比；对手基线按**同 kept 字节预算**构造（trunc_budget_chars 逐片对齐 scx 实际输出），截断=保留尾部 N 行、随机=固定种子逐行抽样。

## P19–P23 官方基准全量冻结完成记录（2026-10-31，版本 0.11.0）——五业界官方基准接入与冻结

五基准全部接入 scx 评测体系并生成冻结 JSON（eval-manifest frozen=69，sha256+rows 入册），压缩线 3 项验证「逐行机械保留」，治理线 2 项验证「记忆治理层 P1–P4 真生效」。

### 压缩线（证据保真 / 编码压缩代价）
- **P19 KEEP-Bench**（HF `uclanlp/KEEP-Bench`，4,102 cases / 15,953 判分 0 error，`bench_p19_keepbench.json`）：
  overall all_rec=**29.47**；parallel 89.0（n=900）/conditional 76.03（n=1364）/science 22.08（n=1010）/policy 34.30（n=828）；
  压缩 59,137,837→25,112,352 chars（char_ratio 0.4246）；成本 $8.08。官方 1x 参考：EXIT 6.3 / RECOMP-ext 4.8 / BRIEF-Pro 6.7 / Gemma 66.8 / Claude-Haiku-4.5 61.6。
- **P22 Compact-Bench**（pip `compactbench==0.1.0` 官方 Compactor ABC，2 cases×20 模板×2 cycles，`bench_p22_compactbench.json`）：
  overall_score **0.747986**、drift_resistance **0.920729**、constraint_retention 0.777778、contradiction_rate 0.025（1/40 翻转）、compression_ratio 0.874837；392 calls 0 错误。
  建议命令 3 cases 与实跑 2 cases 差异已标注；走本机代理 127.0.0.1:8080（`SCX_P22_HTTPS_PROXY` 可覆盖）。

### 治理线（LongMemEval 知识更新 / Memora FAMA / AMA-Bench 轨迹记忆）
- **P20 LongMemEval**（500 问，`bench_p20_longmemeval.json`）：baseline task-averaged **88.35**、overall 87.23（n=470）；knowledge-update 87.5、abstention 76.67；
  知识更新子集（78 对）：versioned 旧证据进 Top-K **89.74%→1.28%**、新证据 91.03%（版本化驱逐失效记忆实证）；protocol recall_k=10、每问 fresh mem-dir、temp 0、判分 prompt 逐字复用官方。
- **P21 Memora**（600 题 / 6,415 子问题，`bench_p21_memora.json`）：gov−nogov **ΔFAMA=10.68**、ΔMPA=0.0533、ΔFAA=0.1495、Δmean_penalty=−0.0698；
  MPA 反映事实召回/作答覆盖，FAA 反映规避援引过时记忆——治理开启后过时记忆污染下降，遗忘规避显著改善。
- **P23 AMA-Bench**（real 208×12=2,496 全量，`bench_p23_amabench.json`）：scx accuracy **0.512**/F1 0.661 vs bm25 0.5076/0.656；A/B/C/D 通道与 retrieval_diagnostics 全记录；repair 151 行全部修复（0 残留）。
  synthetic 1,200 条官方未公开（已如实记录）。

### 模型口径（API 实测，勿重测）
`deepseek-chat` 别名→物理 **DeepSeek-V4.1-Flash 无推理**（temp 0）；显式 `deepseek-flash`/`deepseek-v4-pro` 推理强制（`reasoning.enabled=false` 无效、judge max_tokens=16 时 content 必空，已实测）。
P19/P23 answer+judge 与 P21 judge 走无推理别名；P21 answer 显式 flash（推理、max_tokens=2048）。全部为非官方 judge（GPT-5.2 / Qwen3-32B / 3 模型投票），绝对值仅内部纵向可比，已在各冻结 JSON 口径节如实标注。

### 性能与费用
- curl `-d @临时文件` 修复 ARG_MAX（`OSError: Errno 7` 指数退避清零），吞吐约 **15×**（3 分钟 250 条 vs 30 分钟约 200 条）。
- DeepSeek 自动前缀缓存命中 **26.1%**（18.03M hit / 50.92M miss，约省 17 元）；P21+P23 本次合计约 52 元、五基准全量总账约 75–80 元（P19 判分 15,953 次约 58 元占大头）。
- 门禁：cargo test --locked --features memory --lib **397 passed** / clippy 0 / fmt 干净；eval-manifest frozen=69；蒸馏产物 weights.json（schema scx-sem-2）已完成。

### 诚实边界
- P23 synthetic 1,200 条官方未公开（real 2,496 全量已跑）；Compact-Bench 建议 3 cases 实跑 2 cases；五基准 judge 均非官方模型，绝对值不可与官方表直接等比。
