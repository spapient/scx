# scx 发布检查单

> 当前候选：`0.11.0`（未发布）。最近已知发布：`0.10.0`（2026-10-30）。
> 本清单只以当前工作树的实测输出为准；历史发布记录不构成当前证据。
> 核验日期：2026-11-02。

## 当前候选核验

| 检查项 | 结果 | 证据 |
|---|---|---|
| 版本与元数据 | ✅ | `Cargo.toml`: `version = "0.11.0"`, `edition = "2021"`, `rust-version = "1.82"`, `license = "MIT OR Apache-2.0"` |
| 零第三方 crate | ✅ | `[dependencies]` 为空 |
| `cargo package --locked --allow-dirty` | ✅ | `Packaged 215 files, 4.9MiB (1.5MiB compressed)`，`Verifying scx v0.11.0` 编译通过 |
| 包内许可证 | ✅ | `cargo package --list` 含 `LICENSE-MIT`、`LICENSE-APACHE`；不含 `LICENSE` |
| 包内目录边界 | ✅ | `cargo package --list` 不含 `examples/` |
| 归档内容 | ✅ | `tar -tzf target/package/scx-0.11.0.crate` 仅见 `LICENSE-APACHE`、`LICENSE-MIT` |
| 压缩性能门 | ✅ | `xtask bench compress`: 默认 50k 中位 `6268 µs`，`gate: PASS (50k compress median < 50ms)` |
| 记忆召回门 | ✅ | `xtask bench memory`: `recall p50=106 µs`，`gate: PASS (10k recall p50 < 5ms)` |
| `cargo fmt --all -- --check` | ✅ | 退出码 0 |
| `cargo clippy --locked --all-targets -- -D warnings` | ✅ | 退出码 0 |
| `cargo clippy --locked --all-targets --all-features -- -D warnings` | ✅ | 退出码 0 |
| `cargo test --locked` | ✅ | 退出码 0 |
| `cargo test --locked --all-features` | ✅ | 退出码 0；lib 433 + 集成/工具测试全通过，0 failed |
| `cargo check --locked --all-targets --no-default-features` | ✅ | 退出码 0 |
| `cargo build --release --locked` | ✅ | 退出码 0 |
| `RUSTDOCFLAGS='-D warnings' cargo doc --locked --no-deps` | ✅ | 退出码 0 |
| `xtask audit` | ✅ | `audit_unwrap: OK（生产裸 unwrap=0，expect/panic 消息非空）` |
| `xtask selftest` | ✅ | `selftest: PASS` |

已知构建提示：`scx` 与 `xtask` 两个 bin 目标共用 `src/main.rs`，Cargo 会输出
“file found to be present in multiple build targets”警告。这是既有入口取舍
（`93d1f9e` 合并 xtask 入口、删除 `src/bin/`），不是本次改动引入，也不影响
`cargo package` 或测试结果。

## 发布前必须复查

- [ ] **用户明确授权**：`cargo publish`、打 tag、推送远程均需单独授权；默认不执行。
- [ ] 工作树干净，且待发布 commit 已通过三平台 CI。
- [ ] `cargo package --locked` 在干净树上通过，包内容与上表一致。
- [ ] `cargo run --release --locked --features eval --bin xtask -- selftest` 通过。
- [ ] 版本号、tag、README/SKILL 版本引用一致（当前 `0.11.0`）。
- [ ] crates.io / GitHub Release / npm / Homebrew 的实际状态重新查询；不得引用
      `docs/install-plan.md` 的历史快照作为当前证据。
- [ ] 不存在的入口不再写入文档：`tools/scx`、`tools/scx.py`、`src/bin/xtask` 均已移除或禁止。

## 历史发布记录（仅追溯）

| 版本 | 日期 | 结果 |
|---|---|---|
| `0.10.0` | 2026-10-30 | crates.io / GitHub Release / npm / Homebrew / Scoop 自建 bucket 已发布；winget PR 待审核 |
| `0.9.0` | 2026-10-26 | GitHub Release / npm 已发布 |
| `0.6.x` | 2026-10-26 | MCP 单 content、效果卡与证据保留修复系列 |

历史版本的包体、测试数、渠道版本号与下载统计不再回填到本表；需要审计时以对应
tag 的 CI 产物和 registry API 为准。
