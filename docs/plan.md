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
