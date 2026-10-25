# scx

[![CI](https://github.com/spapient/scx/actions/workflows/ci.yml/badge.svg)](https://github.com/spapient/scx/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust 1.82+](https://img.shields.io/badge/rust-1.82%2B-orange.svg)](Cargo.toml)
[![PRs welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](CONTRIBUTING.md)

**面向 AI 上下文的本地、确定性证据筛选器。**
从日志、命令输出、diff 和错误堆栈中，按当前问题逐行保留相关证据，删除低价值噪声。保留内容不重写，运行过程不联网，第三方 crate 依赖为零。

> Local, deterministic, query-aware context compression for logs and command output.
>
> Keep the lines that matter; leave the rest behind. Verbatim output, offline by default, zero dependencies.

[快速开始](#快速开始) · [工作原理](#工作原理) · [CLI 参考](#cli-参考) · [本地记忆库](#本地记忆库) · [质量与证据](#质量与证据) · [贡献与安全](#贡献与安全)

## 为什么选择 scx

把一大段日志直接塞进模型上下文，既昂贵又容易淹没关键行。通用摘要器会改写内容，抽样器可能恰好丢掉根因，向量检索则不适合逐行保留原始证据。

`scx` 选择另一种约束：**先确定证据，再分配预算；只删除整行，不改写原文。**

- **证据优先**：查询命中的证据行及其最小可读邻域无条件保留，`--ratio` 只能约束可选内容，不能挤掉证据。
- **原文可追溯**：筛选模式只删整行；`--lineno 1` 为保留行附加原始行号，`--force` 可锁定必须保留的片段。
- **本地且确定**：无网络请求、无随机源、无第三方运行时依赖；相同输入、查询和配置产生相同输出。
- **面向 Agent 工作流**：同时提供 CLI、Rust 库、JSON 统计和跨平台自举入口，便于接到脚本、Agent 或本地工具链。
- **失效可解释**：小输入、无查询信号或无可用内容时回退原文，并明确报告 `mode`，不会静默返回空结果。
- **可选经验复用**：内置可关闭的本地 case memory，通过 `--store`、`--recall`、`--rate` 沉淀并召回排障经验。

`scx` 不是通用摘要器、日志平台、向量数据库或 DLP 系统。它解决的是更窄但高频的问题：**在有限上下文中，尽可能完整地保留与当前问题相关的原始行。**

## 快速开始

### 环境要求

- Rust 1.82 或更高版本（含 Cargo）
- macOS、Linux 或 Windows
- 无外部 crate；若尚未安装 Rust，需要先联网获取工具链；`scx` 本身不联网

### 从源码运行

```bash
git clone https://github.com/spapient/scx.git
cd scx
./tools/scx --version
```

Unix 入口会在二进制缺失或源码更新时自动执行 release 构建，然后转发参数。若文件系统丢失了执行位，使用 `sh tools/scx --version`。

Windows PowerShell：

```powershell
git clone https://github.com/spapient/scx.git
Set-Location scx
.\tools\scx.cmd --version
```

也可以直接构建独立二进制：

```bash
# macOS / Linux / WSL
cargo build --release --locked
./target/release/scx --version
```

```powershell
# Windows
cargo build --release --locked
.\target\release\scx.exe --version
```

### 作为 Codex 技能安装

仓库同时包含 [SKILL.md](SKILL.md)。将仓库放入技能目录后，Codex 可以在处理长日志或大文件时自动发现并使用它：

```bash
git clone https://github.com/spapient/scx.git \
  "${CODEX_HOME:-$HOME/.codex}/skills/scx"
```

### 一键安装

`tools/scx install` 构建 release 二进制并软链到 `~/.local/bin/scx`（零第三方 crate，
`--locked` 离线可构建；目标目录可用 `SCX_BIN_DIR` 覆盖）：

```bash
# macOS / Linux / WSL
sh tools/scx install                 # 构建 + 安装到 ~/.local/bin/scx
export PATH="$HOME/.local/bin:$PATH"
```

```powershell
# Windows PowerShell：复制 release 二进制到用户工具目录
New-Item -ItemType Directory -Force "$HOME\bin" | Out-Null
Copy-Item target\release\scx.exe "$HOME\bin\scx.exe"
$env:Path = "$HOME\bin;" + $env:Path
```

或直接使用自举入口（无需安装，按需自动构建）：

```bash
./tools/scx --version            # macOS / Linux / WSL
.\tools\scx.cmd --version       # Windows
```

### 安装自检

Codex、Claude Code 与 PATH 入口应指向同一版本；副本漂移是“技能存在但实际不可用”的常见原因：

```bash
"${CODEX_HOME:-$HOME/.codex}/skills/scx/tools/scx" --version
"$HOME/.claude/skills/scx/tools/scx" --version   # 安装到 Claude Code 时
scx --version                                  # 已将入口加入 PATH 时
```

三条命令都应输出版本号（当前为 `scx 0.5.1`）。若 Claude Code 入口报 `permission denied`，说明副本过旧或执行位丢失，请更新该副本后重试；版本相同但行为不一致时，用 `git -C <技能目录> rev-parse HEAD` 检查两份副本是否在同一提交。

只对实际安装过的入口执行上述检查；未安装的路径报“不存在”不代表工具本身异常。

### 第一次压缩

```bash
some-noisy-command > /tmp/scx-input.log 2>&1
./tools/scx \
  --query "database timeout connection pool" \
  --ratio 0.35 \
  --lineno 1 \
  /tmp/scx-input.log
```

stdout 是筛选后的正文，末尾附一行统计。典型输出形状如下：

```text
42: 2026-10-24T10:31:08Z ERROR db timeout waiting for connection
43:   at Pool.acquire(Pool.java:214)
...
compressed: kept=13/98 lines chars=4950->802 (83.8% cut) dup=84 seeds=4 evidence=4 risk=LOW guarded=0 regions=2 window=2 terms=3 forced=0 mode=sieve
```

- `kept/total` 是保留行数与输入行数。
- `seeds` 与 `evidence` 是结构性不变量，二者始终相等。
- `mode=sieve` 表示实际执行了筛选；passthrough 或 fail-open 模式返回原文。
- 统计行属于 stdout；管道消费正文时加 `--no-stats`，需要机器可读统计时加 `--json`。

常用调节方式：

```bash
# 更紧的可选预算；证据及其 ±1 行邻域仍不会被挤掉
./tools/scx --query "panic overflow index" --ratio 0.15 app.log

# 针尖模式：不填充可选内容
./tools/scx --query "panic overflow index" --context 0 app.log

# 强制保留指定子串（可重复或使用 | 分隔；大小写不敏感）
./tools/scx --query "deploy failed" --force "rollback" --force "Region=eu-west-1" app.log

# 正则子集：re: 前缀进入有限正则（交替/字符类/量词，Thompson NFA，无回溯灾难）
./tools/scx --query "oom crash" --force "re:ERROR|FATAL" --force "re:(panic|assert) in" app.log

# 硬 token 上限：证据地板优先，超限如实报告 overflow:evidence/fill，绝不静默丢证据
./tools/scx --query "db timeout" --max-tokens 2000 app.log

# 结构感知邻域：fence 代码块 / import/use 块 / 堆栈 at 链整体并入（默认关，显式开启）
./tools/scx --query "conn broken" --structure 1 app.log

# 策略文件覆盖默认值：CLI 显式参数 > 文件 > DEFAULT
./tools/scx --query "oom crash" --policy-file policy.toml app.log
```

## 工作原理

```text
query + input
      │
      ├─ 1. 计算 IDF 加权词面命中，确定证据行
      ├─ 2. 无条件保留证据行及其 ±context 邻域
      ├─ 3. 可选：--structure 1 把 fence 代码块 / import/use 块 / 堆栈 at 链
      │        作为结构区间整段并入邻域（默认关，关闭时与 v0.4.1 逐字节一致）
      ├─ 4. 只在种子邻域内，用剩余预算填充可选行
      ├─ 5. --max-tokens N 硬上限：证据地板优先，超限报 overflow:evidence/fill
      ├─ 6. HIGH 风险时启用错误行 / 事故标记守卫
      └─ 7. 输出原文行子集与统计，不重写、不截断保留行
```

核心参数：

| 参数 | 默认值 | 语义 |
|---|---:|---|
| `--ratio` | `0.35` | 可选内容的 token 估算预算比例，范围 `0.02..=1.0` |
| `--context` | `1` | 证据行两侧最小保留邻域，范围 `0..=4`；`0` 为针尖模式 |
| `--window` | `2` | 种子区域扩展与邻域排序半径，范围 `0..=8`；`0` 规范化为 `1` |
| `--risk-gate` | `1` | 风险为 HIGH 时保护失败级错误行和事故标记行 |
| `--lineno` | `0` | 为筛选输出添加 `N: ` 原文行号前缀 |
| `--force` | 无 | 强制保留匹配行；大小写不敏感的字面子串；`re:` 前缀进入有限正则子集 |
| `--max-tokens` | 无 | 输出 token 硬上限；证据行及其最小邻域优先，超限报 `overflow:evidence`/`overflow:fill`，绝不静默丢弃证据 |
| `--structure` | `0` | `1` 时启用结构感知邻域：fence 代码块、`import`/`use`/`#include` 块、堆栈 `at` 链整体并入 |
| `--policy-file` | 无 | 策略文本覆盖默认值（`ratio`/`context`/`window`/`elide`/`risk_gate`/`lineno`/`max_tokens`/`structure`）；优先级 CLI 显式 > 文件 > DEFAULT |

### 四条行为保证

1. **只删不改**：筛选结果中的保留行来自输入原文；`...` 仅表示被省略的区间。
2. **证据不变量**：证据行及其最小邻域先于预算确定，不会被 `--ratio` 淘汰。
3. **确定性**：不使用随机数；排序平局也有稳定规则。
4. **显式回退**：无内容可保留或无查询信号时返回原文，不返回静默空正文。

### 输出模式

| `mode` | 含义 |
|---|---|
| `sieve` | 已执行证据筛选 |
| `passthrough:small` | 输入同时少于 40 行且少于约 1200 token，直接返回原文 |
| `passthrough:longline` | 输入包含超长行（> 约 5000 字符）无法安全切块，直接返回原文 |
| `passthrough:no-kept` | 预算或 `--context 0` 下没有可保留行，返回原文 |
| `failopen:zero-signal` | 查询没有命中，返回原文 |
| `overflow:evidence` | `--max-tokens` 下限：证据行 + 最小邻域本身已超限，保留全部证据并如实报告 |
| `overflow:fill` | `--max-tokens` 上限：预算填充后仍超限，按硬上限裁剪并报告 |

查询词与输入存在词面重叠时效果最佳。`scx` 不生成 embedding，也不承诺解决完全无共享词的语义检索问题。

## CLI 参考

完整入口：

```text
scx --query "..." [OPTIONS] [file...]
```

### 输入与输出

| 选项 | 说明 |
|---|---|
| `--query <TEXT>` | 当前问题或检索意图；未提供时必须至少给出一个非空 `--force` |
| `file...` | 一个或多个输入文件；省略时读取 stdin；单次输入总计上限 64 MiB |
| `... --stat` | 只输出统计行 |
| `... --no-stats` | 只输出正文，不输出统计行 |
| `... --json` | 向 stderr 追加 `scxjson: {...}` 机器可读统计；含 `dropped` 区间明细（`--max-tokens`/预算裁剪）与 `max_tokens`/`tok_est` 字段 |
| `--max-tokens <N>` | 输出 token 硬上限；证据地板最高优先，超限报 `overflow:evidence`/`overflow:fill` |
| `--structure <0\|1>` | `1` 时启用结构感知邻域（fence/import/stack）；默认 `0`，关闭时输出与 v0.4.1 逐字节一致 |
| `--policy-file <PATH>` | 策略文本覆盖默认值；键白名单 `ratio`/`context`/`window`/`elide`/`risk_gate`/`lineno`/`max_tokens`/`structure`；非法文件报错含行号；优先级 CLI 显式 > 文件 > DEFAULT |
| `--force <P>` | 强制保留匹配行；字面子串（大小写不敏感）；`re:` 前缀进入有限正则子集（`\|` 交替、`[...]` 字符类、`*` `+` `?` 量词，Thompson NFA，非法模式报错并提示支持子集） |
| `--semantic <WEIGHTS>` | 可选神经重打分（需 `cargo build --features semantic` 构建）；权重文件自描述 JSON `{schema,dim,weights,sha256}`，SHA256 校验失败即拒绝运行；不启用时默认路径逐字节不变 |
| `--help`, `--version` | 显示用法或版本 |

### 本地记忆库

| 选项 | 说明 |
|---|---|
| `--mem-dir <DIR>` | 记忆库目录；默认 `$SCX_MEM_DIR`，否则 `~/.scx-memory`（Windows 为 `%USERPROFILE%\.scx-memory`） |
| `--store 1` | 将本次证据块写入记忆库；默认关闭 |
| `--fix <TEXT>` | 随 case 保存已验证的修复结论 |
| `--fix-file <PATH>` | 从文件读取多行修复结论，与 `--fix` 互斥 |
| `--max-bytes <N>` | 记忆库总容量软配额；超限时拒绝写入 |
| `--recall <K>` | 在正文旁路召回最多 K 条相关 case |
| `--recall-detail` | 召回时附带摘要、原始证据行和修复结论 |
| `--recall-only` | 只读召回，不写 query 图、会话缓存、索引或 L1 历史 |
| `--rate good\|bad <CASE_ID>` | 对一次召回结果给出反馈 |
| `--stats` | 显示 case 数、状态、fix 覆盖率、质量均值和磁盘占用 |
| `--rebuild`, `--gc`, `--delete <CASE_ID>` | 重建索引、清理过期条目、删除指定 case |

### 评估与维护

| 选项 | 说明 |
|---|---|
| `--eval <BASELINE.json>` | 对冻结标注集执行 A/B 重放，比较完整管线与词面基线 |
| `--eval-verbose` | 追加逐 case 候选诊断 |
| `--gate` | 机械资格门：样本规模、分层、judge 元数据和基础指标 |
| `--gate-human` | 人工弱相关/负例闭环门，要求逐候选真人 provenance |
| `--gate-items` | 允许非人类逐候选裁决，并明确标注非人工证据 |
| `--annotate`, `--annotate-raw` | 从本地库生成回归标注或盲审材料 |
| `--judge <BASELINE.json>` | 交互式逐候选裁决 |
| `--l1-hist` | 查看本地直答命中分布 |

运行 `./tools/scx --help` 可查看当前构建支持的完整参数。

## 本地记忆库

记忆库用于保存“哪几行是证据、当时如何修复”，适合重复出现的构建、运维和故障排查问题。它默认关闭、完全本地，并使用无随机源的词面/字段统计召回，而不是 embedding。

```bash
# 1. 在问题解决后沉淀证据与修复结论
./tools/scx \
  --query "db timeout connection pool" \
  --store 1 \
  --fix "increase pool size and restart the service" \
  --mem-dir .scx-memory \
  app.log

# 2. 再次遇到相似问题时只读召回
./tools/scx \
  --query "database connection pool exhausted" \
  --recall-only --recall 3 --recall-detail \
  --mem-dir .scx-memory

# 3. 查看库状态
./tools/scx --stats --mem-dir .scx-memory
```

默认工作流会在召回结果前加 `mem:` 前缀，使记忆内容与当前输入明确分离。每条 case 包含查询、证据行区间、原始证据和可选修复结论；`--fix` 内容是**候选答案**，相似症状不代表同一根因，使用前仍需验证。

`--recall K` 的快照缓存也按 `K` 校验：先用小 `K` 暖缓存不会让后续大 `K`
少返回；条数不足时自动回退完整 BM25 召回。持久化 qmap 快照仅复用 1 小时内
的结果，避免时间衰减排序长期停留在旧状态。

入库前会对已知的 password、token、Authorization、Bearer 和 URL 用户信息模式做脱敏，但这只是尽力而为的已知模式匹配，**不是通用 DLP**。分享日志或记忆库前请人工复核。

多进程共享同一 `--mem-dir` 受支持：所有跨文件操作通过 `locks` 按固定锁序
（`cases/{id} → index → meta → qmap → fingerprints`）一次取多锁，避免死锁；
持锁后的写路径使用 `*_locked` 变体，不自相争锁。并发压力测试覆盖
store/rate/delete/rebuild 组合。契约全文见 [锁序契约](docs/locking.md)。

设计、存储格式、运维与实现记录统一见 [本地记忆库](docs/memory.md)。

## Rust 库

压缩引擎是纯函数，不访问文件、环境变量或网络。本仓库不提供 crates.io 发布，可从源码作为 path dependency 使用：

```toml
[dependencies]
scx = { path = "../scx" }
```

```rust
use scx::{compress, Config};

fn main() {
    let log = std::fs::read_to_string("app.log").expect("read app.log");
    let cfg = Config {
        ratio: 0.25,
        lineno: true,
        ..Config::default()
    };

    let out = compress(&log, "database timeout", &cfg);
    print!("{}", out.text);

    assert_eq!(out.seeds, out.evidence);
    eprintln!(
        "kept={} evidence={} mode={}",
        out.kept, out.evidence, out.mode
    );
}
```

公开类型定义在 [`src/lib.rs`](src/lib.rs)，记忆库入口为 `scx::memory`。筛选引擎与记忆库、CLI I/O 相互隔离。

## 架构

| 路径 | 职责 |
|---|---|
| `src/lib.rs` | 公共 API、`Config`、`Outcome`、`Mode`/`Risk` 枚举 |
| `src/compress.rs` | 纯函数筛选与预算分配（sieve 内核，零 I/O） |
| `src/analysis.rs`, `src/terms.rs` | 归一化、结构分类（含 fence/import/stack 判定）、切词与 token 估算 |
| `src/policy.rs` | 默认策略数据表（`ScoreTable`/`RiskTable`/`RegionCfg`/硬常量），不落独立硬编码文件 |
| `src/toml.rs` | TOML 子集解析（`Entry`/`parse`/`float`/`size`），供 `--policy-file` 与记忆库 `meta.toml` 复用 |
| `src/ops.rs` | 操作编排层：`Op` 枚举 + `run` + 纯渲染（统计/JSON），`main.rs` 仅薄壳 |
| `src/regex.rs` | 手写有限正则子集（交替/字符类/量词，Thompson NFA，零第三方 crate） |
| `src/semantic.rs` | 可选神经重打分（cargo feature=semantic）：手写 SHA-256、权重加载校验、确定性 sigmoid |
| `src/json.rs` | 手写 JSON parser（`mcp` 与 `--json` 渲染共用） |
| `src/contain.rs` | 证据保留（Containment）评测协议实现 |
| `src/memory/` | 本地 case 存储、召回、反馈、索引与评估（`locks` 固定锁序） |
| `src/main.rs`, `src/args.rs`, `src/input.rs` | CLI 入口、声明式参数表（`FLAGS`）、文件 I/O |
| `src/mcp.rs` | stdio MCP server（cargo feature=mcp，零第三方 crate）；`tools/scx setup` 构建入口 |
| `tools/scx`, `tools/scx.cmd` | Unix / Windows 自举构建入口 |
| `tools/scx.py` | 评测语料回放、盲审与配对统计（仅标准库；含 `contain` 证据保留子命令） |
| `examples/bench.rs` | 压缩与记忆库的统一性能基准入口 |
| `tests/golden.rs` | stdout、stderr、退出码的逐字节差分门（37 例） |
| `config/` | 可重建语料、冻结标签、盲审包与语义权重（`confirm5.*`/`contain5.*`/`real.*`/`v2.*`） |
| `docs/` | 设计、评测协议、锁序契约与证据报告 |

`src/memory/` 保持扁平、单一职责的布局：`mod.rs` 是唯一公开门面，存储
（`store`/`meta`/`ingest`/`lock`/`json`/`util`）、召回
（`recall`/`ranking`/`index`/`cache`/`edge`/`classify`）、学习与维护
（`feedback`/`maintenance`）和评估（`eval`/`gate`/`annotate`）各自成组；
不设跨层子目录，也不把内部文件格式或缓存提升为外部契约。

有意修改 CLI 语义时，需要运行冻结基线更新并逐项审查差异：

```bash
cargo test \
  --test golden update_baselines -- --ignored --nocapture
```

## 质量与证据

每次变更通过以下门禁（P1–P3 起为「默认 + semantic + mcp」三轨，可选 feature 均需编译与测试全绿）：

- `cargo fmt -- --check`
- `cargo clippy --locked --all-targets -- -D warnings`（另跑 `--features semantic` 与 `--features mcp` 双轨）
- `RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps`
- 单元、CLI 集成、并发、金标与评测协议测试（`golden` 37 例逐字节零漂移为硬红线；新行为只走新测试例）
- `cargo test --locked --features mcp`（含 MCP stdio 冒烟）与 `--features semantic`（含权重校验/确定性）
- `cargo build --release --locked`
- Rust 1.82 声明 MSRV 实编译
- 50k 行压缩基准
- 10k case 倒排召回 p50 < 5 ms 性能门
- Ubuntu、macOS、Windows 三平台 CI
- confirm5 冻结语料上的质量回归门，以及“policy/AI 标签不得冒充人工闭环”的负向测试
- `contain5` 证据保留协议（自测、非第三方独立评测；见 [证据保留报告](docs/containment.md)）

### 压缩质量

confirm5 使用 30 起公开事故和 150 条查询，预注册主指标为 hard 层 MRR@10。在机械目标标签上，完整管线的 hard 层 MRR@10 相对朴素词面基线提升 **+0.0669**，paired bootstrap 95% CI **[0.0238, 0.1176]**，permutation `p=0.0020`，literal 层没有退化。

当前默认配置经过后续校准，在同一语料上为 **ΔMRR +0.0608**，95% CI **[0.0202, 0.1095]**，`p=0.0029`。这证明的是目标排序优于该词面基线，不是对 embedding、混合检索或任意 RAG 系统的普遍优势。

逐候选人工门在 hao 的 384 项标签上通过点估计，但这些标签也参与了配置校准，因此它是**事后验收，不是新的独立 holdout**；NDCG 差值的配对 bootstrap CI 仍跨 0，不能表述为“NDCG 显著优于基线”。

**证据保留（contain5，P1 新增协议）**：25 个合成噪音切片（build/stack/diff/log/test 各 5，
1787 行、70 条必需证据行，含日志/堆栈/diff 混合），在 `ratio=0.35` 默认配置下
**scx 证据保留率 70/70 = 100%**（truncation 头部截断基线 0/70），平均字符压减
**82.01%**、token 估算压减 79.93%。这是作者侧合成语料**自测，非第三方独立评测**，
口径仿 SuperCompress evidence-containment；冻结语料 SHA256 见
[docs/containment.md](docs/containment.md)。

完整协议、结果与限制见：

- [评估结果与证据边界](docs/evaluation-results.md)
- [盲审预注册](docs/eval-blind40-preregistration.md)
- [证据保留协议](docs/containment.md)
- [评估工具与复现命令](docs/eval.md)

### 性能快照

以下数据来自本机 release 构建，用于量级参考，不是跨机器性能承诺。P1–P3 默认路径
逐字节不变（golden 37 例零漂移），上述快照仍有效；可选 feature（semantic 重打分、
structure 结构并入）未在此快照内。

| 工作负载 | 结果 | 条件 |
|---|---|---|
| 50k 行 / 2,338,389 字符压缩 | 中位 **11.7–12.0 ms** | Apple M5 Pro，三轮 × 21 次采样 |
| 10k case 稀疏查询召回 | p50 **71–73 µs** | 三轮 × 21 次采样 |
| 10k case 全候选宽查询 | p50 **1.11–1.15 ms** | 三轮 × 7 次采样 |
| 10k case 首次索引加载 | **11.6–11.8 ms** | 每轮单独预热，不计入稳态 p99 |
| 10k case 冷启动线性首查 | p50 **0.95–0.97 s** | 三轮；受系统负载与页缓存影响 |
| 10k case 全量重建索引 | **373–388 ms** | 三轮 |

基准使用合成负载：压缩输入为 50k 行混合日志，记忆库为 1 万条短日志/长堆栈
混合 case。与同一会话中的 HEAD 基线交错测量：压缩中位 12.7–12.8 ms → 11.7–12.0 ms
（约 6–8%），稀疏召回 p50 163–177 µs → 71–73 µs（约 −57%），宽查询
1.17–1.39 ms → 1.11–1.15 ms（差异受负载影响），冷启动线性首查 1.01–1.03 s → 0.95–0.97 s。
稀疏提速来自“先部分选择 top-k、只排序候选窗口”，不再为 5 条结果排序全部候选；
冷启动收益来自读取 case 时不再克隆整棵 JSON；宽查询仍主要受逐候选打分约束。
它衡量实现开销与回归，不代表真实事故分布或跨机器性能。

复现：

```bash
export CARGO_TARGET_DIR="${TMPDIR:-/tmp}/scx-cargo-target"
cargo test --locked
cargo run --release --locked --example bench
cargo run --release --locked --example bench -- memory
```

### 同类工具与可比性

不同项目解决的任务不同，不能用单一压缩比或单条延迟做跨任务排名。这里只说明定位与可借鉴之处：

| 项目 | 主要任务 | 与 scx 的边界 |
|---|---|---|
| [ripgrep](https://github.com/BurntSushi/ripgrep) | 行式正则搜索 | 不做上下文预算与证据压缩；其清晰的 CLI/core 分层与流式扫描是架构参照 |
| [LLMLingua](https://github.com/microsoft/LLMLingua) | 模型式提示压缩 | 依赖模型且可能改写/丢弃 token；scx 零模型、离线、只删整行，保留原文 |
| [logreduce](https://github.com/logreduce/logreduce) | 基于成功日志模型高亮异常 | 需要成功/失败基线学习 novelty，不按当前 query 选择原始证据行 |
| [logparser](https://github.com/logpai/logparser) | Drain/SLCT 等日志模板抽取 | 面向模板聚类与结构化分析，不提供查询相关的上下文预算契约 |
| [loglizer](https://github.com/logpai/loglizer) | 机器学习日志异常检测 | 做异常判定/分类，不按当前查询逐行筛证据；任务与输出契约不同 |

scx 的可测目标是冻结语料上的检索 MRR、证据行保留、确定性与延迟，不是通用摘要质量或
跨语言模型压缩比。上述项目在各自任务上更成熟；本次横向审计只比较输入、模型依赖、
输出可追溯性和确定性契约，不把不同任务的压缩比或延迟做单排名。scx 只优化
“查询相关的原始行选择”。

## 边界与限制

- **词面相关优先**：查询没有词面命中时会 fail-open；完全释义、无共享词的查询不在保证范围内。
- **压缩率不固定**：保留全部证据的语义决定了输出存在“证据地板”；`--ratio` 不是硬性字符上限。
- **`--max-tokens` 是上限不是下限**：证据地板最高优先；下限（`overflow:evidence`）保留全部证据并如实报告，上限（`overflow:fill`）按硬上限裁剪——两者都绝不静默丢弃证据行。
- **结构感知是叠加不是替代**：`--structure 1` 的结构区间并入默认邻域；默认关闭时输出与 v0.4.1 逐字节一致。
- **正则子集有限**：`re:` 支持交替/字符类/量词，不支持 `.`、捕获组、锚点与简写类；非法模式报错并提示支持子集。
- **`--semantic` 可选且默认关闭**：需 `--features semantic` 构建；权重文件 SHA256 校验失败即拒绝运行，不启用时默认路径逐字节不变。
- **MCP 是可选层**：`--features mcp` 构建的 `scx-mcp` 走 stdio，只调库内核与 JSON 渲染；默认构建不含 MCP 代码。
- **token 为估算值**：按字符与 CJK 启发式估算，不是特定模型的 BPE 计数。
- **单机本地工具**：记忆库是本地目录，不是多租户服务或分布式数据库。
- **非安全边界**：`--store` 的脱敏不能替代 DLP、凭据扫描或人工审查。
- **输入有上限**：CLI 单次输入总计 64 MiB；更大的输入需要分段。
- **pre-1.0 状态**：当前版本 `0.5.1`，CLI、case schema 和评估接口仍可能演进；生产集成应固定 commit。

## 贡献与安全

欢迎聚焦的 bug 修复、测试、文档和可复现的性能改进。提交前请阅读 [CONTRIBUTING.md](CONTRIBUTING.md)。

```bash
export CARGO_TARGET_DIR="${TMPDIR:-/tmp}/scx-cargo-target"
cargo fmt -- --check
cargo clippy --locked --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps
cargo test --locked
cargo build --release --locked
```

`main` 受分支保护：常规变更需要 PR、通过的 CI 和维护者评审，直接推送会被拒绝；管理员绕过只用于仓库恢复。不要把真实凭据、客户数据或未公开事故材料放入 issue、日志或测试夹具。

安全漏洞请使用 [GitHub 私密漏洞报告](https://github.com/spapient/scx/security/advisories/new)，不要先开公开 issue。边界说明见 [SECURITY.md](SECURITY.md)。

## 许可证

[MIT](LICENSE) © scx contributors
