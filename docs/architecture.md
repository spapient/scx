# 架构

本文描述 `scx` 的高层设计：模块边界、数据流、特性矩阵与不可违背的设计约束。
面向新贡献者与安全评审者。

## 设计目标（按优先级）

1. **证据优先于压缩率**：查询命中的证据行及其最小可读邻域无条件保留，
   `--ratio` 只能约束可选内容。
2. **只删整行，不改写**：筛选模式不重写、不摘要、不截断行内文本。
3. **确定性**：相同输入、查询与配置产生逐字节相同的输出；压缩主链无随机源、无网络。
4. **零第三方依赖**：`Cargo.lock` 只有 `scx` 一个包；JSON / TOML / 正则均为仓内实现。
5. **失败可解释**：小输入、无查询信号或无可压内容时回退原文，并显式报告 `mode`。

## 模块分层

```
入口层     main.rs · lib.rs · cli.rs · args.rs · mcp.rs
             │
装配层     pipeline.rs · ops.rs · setup.rs · hook.rs · assets.rs · policy.rs
             │
分析层     input.rs · terms.rs · fold.rs · trim.rs · regex.rs · contain.rs · diff.rs
             │
打分层     compress/score.rs · gate.rs · stat.rs · sem.rs · semantic.rs · sim.rs · hash.rs
             │
选择层     compress/plan.rs
             │
渲染层     compress/render.rs · meter.rs · redact.rs · json.rs · toml.rs
             │
支撑层     memory/ · par.rs · perf.rs · tooling/（离线工具，不参与主链）
```

依赖方向单一：上层可调用下层，下层不反向依赖入口层。
`tooling/` 是离线分析与训练工具，只在 `eval` 特性下编译，不进入主链。

原生内核（`gpu/cuda.cu`、`gpu/metal.metal`、`gpu/metal.mm`）不在 Rust 源码树内，
通过可选后端接入；内核只做线性代数，特征提取逻辑保留在 Rust 单一来源，
以保证 CPU 与 GPU 两条路径的对拍一致性。

## 压缩主链数据流

```
输入（文件 / stdin / 管道 / MCP 参数）
  └─ input：读取、编码规范化、行切分
      └─ 分析：词面提取（terms）、重复折叠（fold）、行内裁剪（trim）、结构识别
          └─ 打分：词面命中分 + 可选语义分（sem_weight × 居中有符号 logit）+ 策略门（gate）
              └─ 选择（plan）：先锁定证据行与种子邻域，再在剩余预算内按分数取可选行
                  └─ 渲染（render）：按原顺序输出保留行；`--lineno` 附加原始行号
                      └─ 统计（stat）：生成效果卡（cut / kept / mode / hit_retention）
```

关键不变量：

- 证据集合在选择阶段不可被预算裁掉；预算只作用于可选集合。
- 语义分以有符号形式叠加，允许对"子串命中但实为噪声"的行降权。
- 无信号时走 `failopen:zero-signal`，原样返回并报告，不静默返回空。
- 风险回退模式（`SieveRiskFallback`）归入筛选族，效果卡与机器结果必须一致。

## 特性矩阵

| 特性 | 作用 | 是否默认 |
| --- | --- | --- |
| `memory` | 本地 case memory（`--store` / `--recall`） | 是 |
| `semantic` | 语义权重打分（权重经 `include_str!` 内嵌） | 是 |
| `perf` | 性能插桩与阶段计时 | 否 |
| `eval` | 离线评测工具链（`scx-tooling`） | 否 |

`--no-default-features` 构建出的最小内核不含记忆与语义，必须在 `-D warnings`
下同样干净编译；任何仅被某特性使用的私有函数都要加对应 `cfg` 门。

## 语义与向量：两条独立路径

- **语义打分（`semantic` 特性）**：在选行阶段参与打分，权重内嵌于二进制，
  默认 `sem_weight = 1.0`，可通过 `--sem-weight` 调整。加载失败为最佳努力，
  不影响主链可用性。
- **向量检索（`vec` / `vec_sim`）**：后端选择顺序为
  显式 provider > `SCX_EMBED_VEC` 环境变量 > 内置 `sim::hash` / `sim::cos`。
  **内置后端是字面级哈希向量，不支持跨语言语义检索。**
  需要跨语言能力时必须接入外部编码器，这属于显式配置的例外，
  不属于默认零依赖路径。

两者不可混为一谈：语义打分器救不了字面级向量后端。

## 可移植性

- MSRV 声明为 Rust 1.82，已在 1.82 工具链实测编译通过。
- 发布资产：静态链接的 linux-x64（musl static-pie）、darwin-arm64、win32-x64。
- 可复现构建：两次独立 `CARGO_TARGET_DIR` 构建产出相同二进制 SHA-256。
- 无运行时依赖、无动态库依赖、无网络调用。

## 扩展点

- 新增打分通道：在 `compress/score.rs` 叠加，须保持有符号语义并在 `stat.rs` 暴露可解释指标。
- 新增向量后端：实现 provider 接口，通过 `SCX_EMBED_VEC` 或显式参数接入，
  不得改变内置后端默认行为。
- 新增 MCP 工具：`mcp.rs` 注册，schema 变更属公开契约变更，须同步文档与变更日志。

## 测试策略

- 单元测试随模块放置；集成测试在 `tests/` 下，按能力分文件。
- golden 基线按特性双份（`golden.expected` / `golden.semantic_expected`），
  由 `cfg` 选择，避免跨特性强等。
- 门禁：`fmt --check`、`clippy --all-targets --all-features -D warnings`、
  全特性矩阵构建、`cargo test --all-features`、基准门禁（50k 行压缩 < 50ms、
  10k 文档召回 p50 < 5ms）。
- 覆盖率：行覆盖 89.55%（`cargo llvm-cov`）。
