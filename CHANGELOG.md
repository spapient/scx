# 变更日志

本文件记录 `scx` 的对外可见变更，格式参照 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)（SemVer 2.0.0）。

更早版本（0.11.0 及以前）请见
[GitHub Releases](https://github.com/spapient/scx/releases)。

## [未发布]

### 新增

- `scripts/sbom.py`：零依赖 SPDX 2.3 生成器，命名空间由 `Cargo.lock` 内容哈希派生、
  创建时间固定，输出可复现（两次运行逐字节一致）。
- CI 新增 `security` / `coverage` / `sbom` 三个作业：库目标上强制
  `unwrap_used` / `panic` / `todo` / `unimplemented` 为零、禁止未文档化 unsafe、
  断言 `Cargo.lock` 仅 1 个 package；覆盖率门禁 80%；SBOM 随每次构建产出。
- `tests/robust.rs`：确定性伪随机（xorshift64*）驱动的健壮性测试，
  2000 例畸形输入对拍 + 极端尺寸 + 非法配置归一化 + 输出逐字节确定性。
- `.github/workflows/release.yml`：标签触发三平台构建、生成 `SHA256SUMS.txt`、
  用 cosign keyless 签名（OIDC 身份来自工作流，签名入 Rekor 透明日志），
  验证流程见 `docs/release-signing.md`。
- 治理与文档：`GOVERNANCE.md`、`CODE_OF_CONDUCT.md`、`docs/architecture.md`、
  `docs/threat-model.md`。

### 修复

- **打包漏带权重文件**：`Cargo.toml` 的 `include` 白名单缺少 `config/gate.json`
  与 `config/semantic-pw.json`，打包后的 crate 内相关测试必然失败。
  真实用户在 `cargo install` 后也无法从包内取得这两个文件。
- **原子写失败测试依赖 Unix 语义**：原实现靠"目录去掉写权限"制造失败，
  Windows 上目录只读属性不阻止创建文件，测试在 CI windows 作业恒红。
  现按平台注入真实失败（Unix 用权限、Windows 用目标文件占用）。
- **生产线程 join 不再裸 unwrap**：`compress/plan.rs` 与 `compress/render.rs`
  的 `join().unwrap()` 改为 `unwrap_or_else(resume_unwind)`，语义等价且可被
  安全 lint 约束。
- golden 基线重冻：`eval/budget-expected.csv` 更新到当前确定性输出。

## [0.12.2] - 2026-11-10

### 新增

- **语义权重随二进制内嵌**：`config/semantic.json` 通过 `include_str!` 编入二进制，
  `Weights::load_default()` 解析内嵌内容，安装到任意路径后都能加载，
  不再依赖构建机的源码目录。
- `Config::ensure_semantic()` 最佳努力加载默认权重，CLI 与 MCP 走同一口径。
- `default = ["memory", "semantic"]`：默认构建即包含记忆库与语义打分，安装后不再静默缺能力。
- 查询命中留存双口径指标 `hit_retention`、`hit_total`，配合 Python / Java 语料的证据门禁。
- golden 按特性双基线：`golden.expected` 与 `golden.semantic_expected` 各 37 个用例，
  由 `cfg` 选择基线，避免单条基线跨特性强等。

### 修复

- **效果卡风险回退误报**：`SieveRiskFallback` 此前未被 `sieve_family` 判为筛选族，
  导致风险回退时人类可读卡片显示"一字未动"而机器侧确有压缩。现纳入判定并补回归测试。
- 零词面命中时按错误行回退，跨语言查询不再直接停压。
- MCP 账本落盘、记忆落库，进程重启后统计不丢。
- `evidence_of` 补 `cfg(memory)` 门，消除无该特性构建下的 dead-code。

### 变更

- MCP `vec` / `vec_sim` 后端选择顺序显式化：
  显式 provider > `SCX_EMBED_VEC` 环境变量 > 内置 `sim::hash` / `sim::cos`。
- 发布渠道补齐：crates.io、npm `@spapient/scx`、Homebrew tap
  以及 GitHub Release 三平台预编译产物（darwin-arm64 / linux-x64 / win32-x64）。

### 已知限制

- 内置向量后端为字面级哈希向量，不支持跨语言语义检索；
  需要跨语言能力时须通过 `SCX_EMBED_VEC` 接入外部编码器。
- 默认 `sem_weight = 1.0` 在真实日志上与未开启的选行结果可能逐字节相同，
  需要更强抑制时可显式提高权重。

## [0.12.0] - 2026-10-28

### 新增

- 自研 GPU 双后端（Metal / CUDA），内核只做线性代数，特征逻辑保留在 Rust 单一来源。
- 训练链与离线工具 `scx-tooling`（`bench` / `calibrate` / `quality` / `train` / `audit`）。
- MCP 服务端：`compress_context`、`context_snapshot`、`context_diff`、`ledger_summary`、
  `usage_summary`、`vec`、`vec_sim` 等工具。
- `scx setup` 跨平台配置写入加固。

### 变更

- 在 `Cargo.toml` 层面对所有编译目标强制禁止 `unsafe`。
- 移除 10 处冗余（4 处 `unnecessary_wraps`、6 处 `redundant_clone`）。

### 修复

- `scx setup` 的跨平台 home 解析与 Windows 路径兼容问题。

## [0.11.0] - 2026-10-05

早期版本，变更详情见
[v0.11.0 Release](https://github.com/spapient/scx/releases/tag/v0.11.0)。

[未发布]: https://github.com/spapient/scx/compare/v0.12.2...HEAD
[0.12.2]: https://github.com/spapient/scx/compare/v0.12.0...v0.12.2
[0.12.0]: https://github.com/spapient/scx/releases/tag/v0.12.0
[0.11.0]: https://github.com/spapient/scx/releases/tag/v0.11.0
