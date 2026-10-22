# scx 构建说明（跨平台自举：Windows / macOS / Linux）

## 语义要点（v0.4.1）

- **结构性种子不变量**：证据行（seed）及其 ±context（默认 1）最小可读邻域先无条件标记为保留，再对剩余内容使用预算；`--ratio` 只约束可选内容，不挤占种子邻域。证据判定只来自查询证据（IDF 加权命中）与 `--force`，结构加分不制造种子。
- **`--lineno 1` 行号溯源**：保留行输出附加原文行号前缀（grep -n 风格 `N: `）；passthrough / fail-open 返回原文时不附加，保持"原样保留"语义。
- **无可用内容回退**：若预算或 `--context 0` 导致没有任何行可保留，返回 `mode=passthrough:no-kept` 和原文，不返回静默空正文。
- **针尖档 `--context 0`**：完全跳过可选填充，只保留证据行与省略标记（风险 HIGH 且 risk-gate 开时另含失败级错误行与事故标记行）。无证据时走上述回退。实测削减 95.9% / 82.0% / 93.9%（三语料），金标 9/9、9/9、27/27 全保；这是"证据全保"语义下的物理地板。
- **种子区域门控**：可选预算只填充在种子行的邻域区域（±window 扩展，间隔不超过 window*3 合并；`regions=` 为区域数，0 表示种子为零、回退全图排序）。区域外素材不参与预算（首两行身份行除外）。
- **邻域权重只排序**：零分行获得上下文排序权重，但独立入选必须来自真实得分。
- **IDF 词权重**、**无信号 fail-open**（`mode=failopen:zero-signal`）、**guarded**（风险 HIGH 时保护失败级错误行与事故标记行；warn 级噪声不成批保留）。
- 筛选模式只删不改，保留行原文逐字；passthrough/fail-open 返回输入原文；`...` 仅表示省略区间，尾部省略也会标记；确定性（无随机源）。
- 分析缓冲会剥离 ANSI 转义并把裸 `\r` 视为空白，避免彩色日志/进度条污染词面；筛选输出仍写原始行。
- 单次输入上限 64 MiB（CLI 层，含多文件合计）；超出时明确报错，需先分段，避免一次性读入和筛选向量挤爆内存。
- 统计行字段：`kept/total`、`chars`、`dup`、`seeds/evidence`（恒等，可断言）、`risk`、`guarded`、`regions`、`window`、`terms`、`forced`、`mode`。
- **记忆知识库（RAG，P1–P4.3 增量 + P4.4 评审处置 + P4.4+ --fix 本地直答与 --stats 库盘点，默认关）**：`--store 1` 沉淀证据块（≤1 MiB/case，可配 `--fix "修复结论"` 记录"上次怎么解决"）→ `--recall K` 旁路召回（mem: 段不进正文；case 带 fix 时**主流程即无条件输出 `fix:` 行**，`--recall-detail` 再加 `summary:` 错误摘要，L1 本地直答；`--recall-only` 只读查看、不污染库状态）→ `--rate good|bad <case-id>` 反馈闭环（代际失效：改动立即影响同 query 召回排序）；`--stats` 库盘点（case 数/桶分布/状态分布/fix 覆盖率/质量均值/磁盘 KiB）；召回 = 自研统计向量（词面匹配 + 结构化字段加权 + 时间衰减 + MMR + 词级调权 + 场景桶 + 实体边一跳，tf 恒 1 去重词表口径），非 embedding；`--eval <baseline.json>` 冻结标注集出 A/B 对拍表 + `l1:` 直答命中率（工具就绪、M4 证据门待 30–50 组人工标注），`--l1-hist` 出运行期命中分布；`--rebuild/--gc/--delete` 维护（id 白名单防路径穿越）；全部零依赖、离线、确定性。设计 `docs/memory-rag-design.md`，计划 `docs/rag-implementation-plan.md`（含 §11 评审处置记录、§14 优化②），标注采集 `docs/eval-baseline-template.md`。

## 削减率的物理极限（98.99% 目标核算，2026-10-14）

在"保留行原文逐字、不截行、不改写"的语义下，98.99% 削减意味着输出不超过输入字符数的 1.01%。逐字保留金标行本身就需要字符，核算如下（当月快照）：

| 语料 | 1.01% 预算（字符） | 金标行原文总字符 | 结论 |
|---|---|---|---|
| trion diff | 1,359 | 446 | 数学可容纳，但需丢弃金标以外的查询证据 |
| D7 提交 | 305 | 705 | **不可能**（仅金标行就超预算） |
| trion readme | 401 | 2,158 | **不可能**（仅金标行就超预算） |

因此"全面 98.99% 削减且证据全保"是矛盾目标；本工具在该语义下的极限就是各语料的证据地板（针尖档实测值）。**证据保留率维度已全面超过 98.99%（各档 100%）**。若要逼近 98.99% 削减，唯一路径是放弃"保留全部查询证据"（退化为仅保留 `--force` 匹配行），那属于另一种工具语义，未实现也不默认。

## 架构与复用

- **库 + CLI 分离**：筛选引擎是纯函数库 `src/lib.rs` 的 `scx::compress(&text, &query, &Config) -> Outcome`（零文件、环境与进程 I/O，无随机源）；`src/main.rs` 仅是外壳（参数、输入、统计与 JSON 格式化）。
- **模块划分**：`src/terms.rs`（词面/切词/估算）、`src/classify.rs`（错误/标记/结构分类）、`src/norm.rs`（归一化/ANSI 剥离）是独立纯函数层（`pub(crate)`），可供记忆库等外部复用；`src/lib.rs` 保留 Config/Outcome/compress 与测试，单文件 <500 行；`src/memory/` 为记忆知识库（store/recall/feedback/eval/edge/stats 等 20+ 文件，各 <500 行）。
- **复用**：任何程序或脚本可直接 `use scx::compress`；`cargo test` 当前含 73 个库测试 + 8 个 CLI 单元测试 + 4 个 CLI 集成测试 + 1 个 golden 差分门（27 例冻结基线、81 个 expected 文件），覆盖种子不变量、空结果回退、`--force` 重复行、配置边界、CJK 词面、ANSI/CR 分析、diff 正文识别、输入上限边界、CRLF 与确定性、行号前缀、记忆往返/去重/衰减/桶过滤/坏段自愈/评估重放/库盘点与字节级回归。
- **golden 差分门**：`tests/golden.rs` 将 25 个代表性用例的 stdout/stderr/退出码与冻结基线（`tests/golden/expected/`）逐字节对比；有意变更语义时重冻基线：`cargo test --test golden update_baselines -- --ignored --nocapture`，并人工审查基线明细。CI 当前执行格式、clippy、单测、golden 和 release 构建。
- **零依赖性能基准**：`examples/bench.rs`（50k 行约 2.5MB 全流程中位耗时，`std::time::Instant`，5 次采样取中位），防无声性能退化。运行：`cargo run --release --example bench --locked --target-dir "$TEMP/scx-cargo-target"`。

## 快速体验（记忆库端到端演示）

一键跑通 store → recall（literal/变体/实体）→ rate 反馈 → eval 对拍 → l1-hist → rebuild（Windows PowerShell）：

```powershell
powershell -ExecutionPolicy Bypass -File scripts\demo.ps1
```

脚本用临时库（`$TEMP\scx-demo-mem`）沉淀 16 个真实感问题 case 并跑全链路；演示 case 同时是人工标注（`docs/eval-baseline-template.md`）的现成素材。手动体验的最短路径：

```bash
# 1) 沉淀经验（日志/报错先经 scx 压缩再入库；--fix 记录"上次怎么解决"）
scx --query "db timeout" --store 1 --fix "enlarge pool, restart daemon" --mem-dir D:\scx-mem err.log
# 2) 同类问题二次遇到：本地召回直答（主流程直接出 fix: 行；--recall-detail 加 summary:）
scx --query "数据库连接池超时" --recall 3 --mem-dir D:\scx-mem
# 3) 库盘点：case 数 / 场景桶 / 状态 / fix 覆盖率 / 质量均值 / 磁盘占用
scx --stats --mem-dir D:\scx-mem
```

## 版本变动

- **0.4.1 + P4.4+ 优化（--fix 本地直答 / --stats 库盘点 / 桶判定精度，2026-10-22）**：主流程召回命中即无条件输出 `fix:` 修复结论（L1 直答不再依赖 --recall-detail）；新增 `--stats` 库盘点（case 数/场景桶/状态分布/fix 覆盖率/质量均值/磁盘 KiB，豁免 --query，维护命令线性全扫）；裸 "db" 场景桶判定改词级精确匹配（修 query "db timeout" 漏召，同时避免 "db.java:4" 文件实体把编译场景误归 db 桶——stats 单测暴露的桶误判）；门禁 73 库 + 8 CLI + 4 集成 + golden 27 例全绿，fmt/clippy 0。
- **0.4.1 + 记忆库（P1–P4.3，2026-10-22）**：`scx::memory` 知识库落地——存储/摄取/召回（自研统计向量：BM25+字段加权+时间衰减+MMR+词级调权+场景桶+实体边一跳）、qmap 记忆图、反馈双环、倒排索引（mmap+坏段自愈）、库维护（rebuild/gc/delete）、评估系统（--eval A/B 对拍 + l1 命中率 + --l1-hist）、要点摘要（--recall-detail）；门禁终态 71 库 + 8 CLI + 4 集成 + golden 25 例全绿，1 万条库召回中位 2.1ms。设计/计划/标注模板见 docs/。
- **0.4.1**：新增 `--lineno 1` 保留行行号溯源；lib.rs 模块化拆分（terms/classify/norm，过 500 行门禁，行为逐字节不变，由 golden 门证明）；新增 golden 差分门（16 用例冻结基线）与零依赖性能基准（50k 行中位 44 ms）；SKILL/README 补 0.4.1 用法与验证记录。
- **0.4.0**：空结果回退 `passthrough:no-kept`；`--force` 命中行不再被重复行折叠；配置在库入口规范化（负数/超范围 `window`、`context`、`ratio` 不再触发溢出或超大循环）；预算只约束可选内容；小输入阈值改为“行数少且 token 少”；CJK 应用 STOP、补齐尾词并扩展字符范围；`is_frame` 扩展名扫描设上限；分析缓冲剥离 ANSI 转义并折叠裸 CR（输出仍为原文）；统一 diff 的 `+code`/`-code` 正文行参与排序；CLI 严格拒绝未知选项/非法值，新增 `--no-stats`、`--flag=value` 和 64 MiB 输入上限；入口脚本检查全部源码与清单；新增三平台 CI 与 MIT 许可证。
- **0.3 内追加**：`--context N` 邻域半径档位（默认 1；0 为针尖模式，完全跳过可选填充）。刀口数据：commit 语料差距从 13.2 点收窄到 0.9 点，readme 反超同类参照 2.8 点。
- 0.3.9（CJK 错误词面）：错误/失败/报错/异常/超时/崩溃 入错误判据与风险守卫；警告按 warn 级处理（只入判据、不入守卫）。差分门 23 例有意差异逐行审查通过——新增保留行全为中文错误/失败语义行，零误留、零移除；代价：diff 削减 94.9→94.0、commit 69.7→69.3、e2e 97.9→97.3；噪声档与 0.3.8 字节全同。参照重冻 v037→v039，三平台实机同哈希复验通过。
- 0.3.8（工程化，行为字节全同）：零拷贝行切片（无 CR 输入借用原文，消掉整文复制与逐行分配）、复用缓冲的按行大小写折叠与归一化、无分配 is_lineref、ASCII 快路径 token 估算、mk 参数去重。证明：对 v0.3.7 的 40 例差分门逐字节零差异，属性门 72/72，单测 9/9；50k 行基准中位 95.1 → 61.6 ms（约 35%）；三平台实机复验通过。
- 0.3.6 到 0.3.7：事故标记行守卫（root cause / fix hint / mitigation / recommendation / action，risk HIGH 且 gate 开时与错误行同受保护）+ 守卫收窄（warn 级不再成批保留）。缘起：与 SuperCompress 本地对拍发现针尖档在噪声转储漏掉诊断行（pool 被高频行稀释 IDF）。修复后噪声语料针尖档 1,908→6 行（行削减 99.69%，字符 99.60%），证据全保；门禁 40/40 逐字节不变，属性门 72/72，三平台实机复验通过。
- 0.3.1 到 0.3.2：库化重构（lib.rs 引擎 + main.rs CLI），3 个单元测试；行为零变化。
- 0.2 到 0.3：种子区域门控；命中判定单趟位掩码；输出零克隆直装；`--stat` 与 `regions`。
- 0.1 到 0.2：种子不变量、IDF 权重、fail-open、`--window`/`--json`/`--version`；`guarded` 取代整体降级。

## 入口与自举语义（推荐用法）

- Windows：`bin\run.cmd <scx 参数...>`；macOS / Linux：`bin/run <scx 参数...>`（必要时 `sh bin/run` 或先 `chmod +x`）。
- 自举行为：入口自动检测本平台状态；Unix 在二进制缺失或 `rs/src`、`Cargo.toml`、`Cargo.lock` 更新时自动构建；Windows 在 cargo 可用时每次执行增量 `cargo build`，然后安装并转发参数。构建失败不会继续运行旧二进制。
- 跨机器策略：只需携带技能目录的文本与源码；不需要同步任何平台的二进制。需要目标机有 Rust。

## 手动构建（等价于自举，用于排查）

Windows：

```powershell
$scxHome = "$env:USERPROFILE\.codex\skills\scx"   # 装于 Claude 时把 .codex 换成 .claude
cargo build --release --locked --manifest-path "$scxHome/rs/Cargo.toml" --target-dir "$env:TEMP/scx-rs-target"
Copy-Item "$env:TEMP/scx-rs-target/release/scx.exe" "$scxHome/bin/scx.exe" -Force
```

macOS / Linux：

```bash
sh build.sh          # cargo build --release --locked 并安装到 bin/scx（构建缓存放库外）
```

## 事实与验证记录

- 二进制命名：Windows 为 `bin/scx.exe`；macOS / Linux 为 `bin/scx`（均由本机自举生成，来源同一份零依赖源码）。
- 档位对比（2026-10-14，同语料同查询同 ratio 0.35，读数为当时快照）：默认档 94.9% / 69.7% / 89.7%，针尖档 95.9% / 82.0% / 93.9%，金标全保；同类参照同尺为 98.2% / 82.9% / 91.1%（针尖档在 readme 上反超 2.8 点，commit 差 0.9 点）。
- 当前回归（0.4.1）：`cargo test` 33/33（含 golden 门 1 个 + 1 个 ignored 重冻）；`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings` 与 `cargo build --release --locked` 通过；golden 差分门对 0.4.0→0.4.1 拆分前后逐字节一致；50k 行基准中位 44 ms（0.3.8 为 61.6 ms）。CI 覆盖 Linux/macOS/Windows。
- 历史差分等价（0.3.9）：40 用例对 v0.3.9 参照逐字节一致；CJK 词面变更已经逐行审查并重冻参照（v037→v039）；当时 `cargo test` 10/10。
- 噪声转储与守卫（0.3.7）：针尖档 1,908 行→6 行（行削减 99.69%，字符 85,024→342 即 99.60%），默认档→20 行（98.9%）；4 条查询种子与“Root cause:”标记、真 ERROR 全保；Mac arm64 与 Alpine musl 实机复验通过且与基线期望逐字节一致。
- 工程化基准（0.3.8）：50k 行（2.5MB）全流程中位 95.1 → 61.6 ms；输出与 0.3.7 逐字节一致（差分门 40/40、噪声档 A/B、三平台期望文件）；单测 9/9。
- CJK 词面复验（0.3.9）：单测 10/10；属性门 72/72；diff 语料输出在三平台逐字节同哈希（e136263a…f2920c）；噪声档与 0.3.8 逐字节全同；差异逐行审查记录见版本史。
- 跨平台验证（E1）：同一源码 `cargo check --release` 通过 `aarch64-apple-darwin`、`x86_64-apple-darwin`、`x86_64-unknown-linux-gnu` 三个目标；无平台分支代码。
- 真 BPE token 计量未内置（token 估算按 1 token 约 4 字符 / 1 中文约 0.6 token，属估算口径）。

## 许可证

MIT，见 `LICENSE`。
