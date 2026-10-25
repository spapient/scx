# scx 性能基准表（跨机参考）

> 本表只做量级参考与回归对照，不是跨机器性能承诺。数据来自本机 release 构建，
> 每行标注机器型号、日期、编译 profile 与测量方法；不同行**口径不同，不可直接
> 跨行比较**（例如结构语料与默认语料内容不同，max-tokens 路径受证据地板交互影响）。
> 日期：2026-10-25（P4 新增路径首次记录）。

## 环境与方法（2026-10-25）

- 机器：Apple M5 Pro（Mac17,9），macOS 27.0.1，arm64
- 编译：`cargo build --release --locked`（profile.release opt-level=3 / lto / strip）
- 方法：预热 1 次后采样 N 次取中位；N 默认 9（压缩）/ 21（召回），可用
  `SCX_BENCH_REPEAT` 覆盖；与同一会话中历史基线交错测量避免漂移。
- 命令：`cargo run --release --locked --example bench`（压缩四路径）/
  `-- memory`（记忆库）

## 压缩路径（50k 行）

| 路径 | 语料 | 中位 | 样本（µs） | kept | 备注 |
|---|---:|---|---:|---:|---|
| 默认 | 心跳+每百行 ERROR（2,338,389 字符） | 14,757 µs | 13190–22113 | 2498 | 本轮单轮采样负载偏高；历史快照 11.7–12.0 ms（三轮交错中位）为基准值 |
| `--structure 1` 关 | 结构富语料（import/stack/fence+噪声，1,438,623 字符） | 7,888 µs | 7750–8539 | 3002 | 与开启同语料对比用 |
| `--structure 1` 开 | 同上 | 8,813 µs | 8654–12581 | 3002 | 相对关闭 +12% 增量开销；kept 不变（结构并入不改变证据地板） |
| `--max-tokens 2000` | 默认语料 | 12,029 µs | 11655–12181 | 1499 | `mode=overflow:evidence`（证据地板 > 2000，全部证据保留并如实报告） |
| `--policy-file` 字段覆盖 | 默认语料 | 12,955 µs（压缩） | 12868–13147 | 1499 | TOML 解析一次性 13,257 µs（per-invocation，与 CLI 层等价）；稳态压缩延迟与字段覆盖等价 |

口径说明：
- `--policy-file` 的 TOML 解析是 **CLI 一次性开销**（读文件 + TOML 子集解析），
  不进入每次压缩的热路径；完整 CLI 映射在 `args::parse_policy_file`，本表为
  字段级近似（ratio/context/max_tokens/structure 代表字段）。
- `--max-tokens` 路径报告的是**加了上限检查的压缩延迟**；overflow 分支保留
  全部证据行（证据地板最高优先），不做静默裁剪。
- 结构语料行更短（1.44M vs 2.34M 字符），绝对值不与默认语料横向比较；两行
  structure 之间同语料可直接比较增量。

## 记忆库（10k case）

| 工作负载 | 本轮 | 历史快照（README） | 条件 |
|---|---:|---:|---|
| store（200 seeds） | 297 ms | — | 一次性 |
| replicate 200→10000 | 2,700 ms | — | 一次性 |
| 冷启动线性首查（no index） | p50 1.021 s | 0.95–0.97 s | 受系统负载与页缓存影响 |
| 全量重建索引 | 385 ms | 373–388 ms | 三轮 |
| 索引首次加载 | 10,721 µs | 11.6–11.8 ms | 每轮单独预热 |
| 稳态倒排召回（稀疏） | p50 63 µs | 71–73 µs | 21 采样 |
| 全候选宽查询 | p50 1.118 ms | 1.11–1.15 ms | 7 采样 |
| 性能门 | PASS（p50 < 5 ms） | — | 10k recall |

## 历史快照（README「性能快照」，Apple M5 Pro，三轮 × 21 采样交错）

- 50k 行压缩中位 11.7–12.0 ms（与 HEAD 基线交错，约 6–8% 提升后数值）。
- 稀疏召回 p50 71–73 µs（先部分选择 top-k、只排序候选窗口）。
- 冷启动线性首查 0.95–0.97 s；全量重建 373–388 ms。

## 复现

```bash
cargo run --release --locked --example bench            # 压缩四路径
cargo run --release --locked --example bench -- memory  # 记忆库（含性能门）
export SCX_BENCH_REPEAT=21                              # 自定义采样数
```
