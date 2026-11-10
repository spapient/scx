# scx 发布检查单

> 当前候选：`0.12.0`（未发布）。最近已知发布：`0.10.0`（2026-10-30）。
> 本清单只以当前工作树的实测输出为准；历史发布记录不构成当前证据。
> 下表核验快照日期：2026-11-04（针对 `0.11.0` 候选；`0.12.0` 需在版本 bump 后重跑核验）。

## 当前候选核验

| 检查项 | 结果 | 证据 |
|---|---|---|
| 版本与元数据 | ✅ | `Cargo.toml`: `version = "0.11.0"`, `edition = "2021"`, `rust-version = "1.82"`, `license = "MIT OR Apache-2.0"` |
| 零第三方 crate | ✅ | `[dependencies]` 为空 |
| `cargo package --locked` | ✅ | 干净临时树（工作树快照）执行：`Packaged 112 files, 5.4MiB (1.6MiB compressed)`，`Verifying scx v0.11.0` 编译通过；测试资产已纳入包内 |
| 打包后测试 | ✅ | 解包 `.crate` 后 `cargo build --release --locked` + `cargo test --locked --workspace --all-features`：606 passed / 0 failed / 1 ignored |
| 包内许可证 | ✅ | `cargo package --list` 含 `LICENSE-MIT`、`LICENSE-APACHE`；不含 `LICENSE` |
| 包内目录边界 | ✅ | `cargo package --list` 不含 `examples/` |
| 开发工具边界 | ✅ | `scx-tooling` 由 `src/tooling/` 提供且 `required-features = ["eval"]`；发布二进制仅含 `scx` |
| 归档内容 | ✅ | `tar -tzf target/package/scx-0.11.0.crate` 仅见 `LICENSE-APACHE`、`LICENSE-MIT` |
| 压缩性能门 | ✅ | `scx-tooling bench compress`: 默认 50k 中位 `3545 µs`，`gate: PASS (50k compress median < 50ms)` |
| 记忆召回门 | ✅ | `scx-tooling bench memory`: `recall p50=77 µs`，`gate: PASS (10k recall p50 < 5ms)` |
| `cargo fmt --all -- --check` | ✅ | 退出码 0 |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | ✅ | 退出码 0 |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | ✅ | 退出码 0 |
| `cargo test --locked --workspace` | ✅ | 退出码 0；456 passed / 0 failed / 1 ignored |
| `cargo test --locked --workspace --all-features` | ✅ | 退出码 0；606 passed / 0 failed / 1 ignored |
| `cargo check --locked --all-targets --no-default-features` | ✅ | 退出码 0 |
| `cargo build --release --locked` | ✅ | 退出码 0 |
| `RUSTDOCFLAGS='-D warnings' cargo doc --locked --no-deps` | ✅ | 退出码 0 |
| `scx-tooling audit` | ✅ | `audit_unwrap: OK（生产裸 unwrap=0，expect/panic 消息非空）` |
| `scx-tooling selftest` | ✅ | `selftest: PASS` |

## 发布前必须复查

- [ ] **用户明确授权**：`cargo publish`、打 tag、推送远程均需单独授权；默认不执行。
- [ ] 工作树干净，且待发布 commit 已通过三平台 CI。
- [ ] `cargo package --locked` 在干净树上通过，包内容与上表一致。
- [ ] `cargo run --release --locked --features eval --bin scx-tooling -- selftest` 通过。
- [ ] 版本号、tag、README/SKILL 版本引用一致（当前 `0.12.0`）。
- [ ] crates.io / GitHub Release / npm / Homebrew 的实际状态重新查询；不得引用
      `docs/install-plan.md` 的历史快照作为当前证据。
- [ ] 不存在的入口不再写入文档：`tools/scx`、`tools/scx.py`、`src/bin` 均已移除或禁止。

## 历史发布记录（仅追溯）

| 版本 | 日期 | 结果 |
|---|---|---|
| `0.10.0` | 2026-10-30 | crates.io / GitHub Release / npm / Homebrew / Scoop 自建 bucket 已发布；winget PR 待审核 |
| `0.9.0` | 2026-10-26 | GitHub Release / npm 已发布 |
| `0.6.x` | 2026-10-26 | MCP 单 content、效果卡与证据保留修复系列 |

历史版本的包体、测试数、渠道版本号与下载统计不再回填到本表；需要审计时以对应
tag 的 CI 产物和 registry API 为准。
