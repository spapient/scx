# scx 架构优化路线图与 P1 交付记录

> 本文档记录整体架构优化方案的阶段化落地状态（P1–P4）与每阶段的完成证据。
> 方案正文为消息交付（不落盘）；本文档只维护可执行的路线与状态。
> 更新惯例：每阶段完成后由实施方在此记录门禁证据、改动清单与遗留项。

## 路线总览（P1–P4）

| 阶段 | 主题 | 内容 | 状态 |
|---|---|---|---|
| P1 | 内核重构 + 评测协议 | compress 阶段拆分、Outcome 枚举化、args 声明式、conf 提取、--max-tokens、Containment 协议与基准资产 | ✅ 完成（见下） |
| P2 | 结构感知与 MCP 集成层 | fence/import/at 链邻域、bin/scx-mcp（可选层，不破坏纯内核）、conf 策略文件接线 | 未开始 |
| P3 | 配置化驱动深化 | policy 表文件化（--policy-file，conf::parse 消费）、语义增强 feature-gate 可选路径 | 未开始 |
| P4 | 记忆库协议演进 | 多进程写实测、L1 直答评测协议、与压缩内核解耦收尾 | 未开始 |

红线（每阶段门禁）：`cargo fmt --check` / `cargo clippy --all-targets --locked -- -D warnings` /
`RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked` / `cargo test --locked` /
`cargo build --release --locked`；golden 逐字节零漂移（新行为走新增 golden 例）；
confirm5 不退化；每函数圈复杂度 < 10；`#![forbid(unsafe_code)]` 与零第三方 crate 保持。

## P1 交付状态（2026-10-25）

| 子项 | 状态 | 门禁结果 | 改动文件 | 新增测试 |
|---|---|---|---|---|
| 1.1 compress 阶段拆分 + policy 表 | ✅ | 全绿（golden 零漂移） | `src/policy.rs`（新）、`src/compress.rs`、`src/lib.rs` | lib 若干（继承） |
| 1.2 Outcome 枚举化 | ✅ | 全绿（golden 零漂移） | `src/lib.rs`、`src/compress.rs`、`src/main.rs` | — |
| 1.3 args 声明式参数表 | ✅ | 全绿（golden 零漂移） | `src/args.rs` | cli 8 例（继承） |
| 1.4 conf 提取 | ✅ | 全绿（golden 零漂移） | `src/conf.rs`（新）、`src/memory/meta.rs`、`src/lib.rs` | +2 |
| 1.5 --max-tokens 硬上限 | ✅ | 全绿（golden 32 例零漂移） | `src/lib.rs`、`src/compress.rs`、`src/args.rs`、`src/main.rs`、`src/policy.rs`、`tests/golden.rs` + 3 基线 | +4 单测 +3 golden 例 |
| 1.6 内核评测协议 Containment | ✅ | 全绿（golden 零漂移） | `src/contain.rs`（新）、`src/lib.rs`、`tests/contain.rs`（新）、`bin/scx-eval` | +2 单测 +4 integration |
| 1.7 公开基准资产 | ✅ | 全绿（golden 零漂移） | `eval/contain5.corpus.json`（新）、`eval/CONTAINMENT.md`（新）、`eval/README.md`、`docs/plan.md`（新） | — |

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

- 语料 `eval/contain5.corpus.json`：25 切片 × 5 类（全 synthetic），1787 行、
  70 证据行、92298 字符；SHA256 `a077c35e…b681`。
- 入口 `bin/scx-eval contain`（Python 编排 + 统计，不复刻排序）；
  Rust 侧 `scx::contain` 为判定唯一事实源（整行等值），`tests/contain.rs`
  红黄蓝断言批为可执行规格。
- 实测（当前 HEAD，ratio=0.35, context=1）：**证据保留 70/70 = 100%**、
  truncation 基线 0/70、平均字符压减 82.01%、token 估算压减 79.93%。
- **性质**：作者侧合成语料自测，**非第三方独立评测**；口径差异与复现命令
  见 `eval/CONTAINMENT.md`。

### 遗留 / 待 P2

1. MCP server 集成层（`bin/scx-mcp`）未实施——设计见架构方案 §2.4，属 P2。
2. 结构感知邻域（fence/import/at 链）未实施——属 P2，与既有
   `frame_bonus`/`diffmeta_bonus` 评分表正交，改默认表须过 golden + confirm5。
3. 语义增强 feature-gate 可选路径未实施——属 P3；默认路径承诺逐字节不变。
4. 与 SuperCompress 的同场独立对照实验未做（需真实事故语料 + 第三方盲审）。
5. `bin/scx-eval` 的 `contain` 输出 token 压减为确定性正则估算，非真实
   LLM tokenizer（文档已标注）。
