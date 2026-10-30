# scx 发布检查单（crates.io 发布前）

> 状态：**已发布**（0.5.0 → … → 0.7.0 → 0.9.0 → 0.10.0，crates.io max_version=0.10.0、num_versions=12；0.10.0 含 P12.6 --evidence 强制保留 + ser_b 序列化口径修复，全平台资产 + npm 同步发布）。
> 下节为历次发布前 dry-run 验证证据模板；本节头部已更新为当前状态。
> 检查日期：2026-10-25 初版；2026-10-26 随 0.6.x 系列更新；2026-10-30 随 0.10.0 更新。

## v0.10.0 发布记录（2026-10-30）

| 渠道 | 结果 | 证据 |
|---|---|---|
| crates.io | ✅ | `cargo publish --locked` 成功；API max_version=0.10.0 / num_versions=12 / downloads=128 |
| GitHub Release v0.10.0 | ✅ | 4 资产：darwin-arm64.tar.gz（Mach-O arm64，下载回验 `scx 0.10.0`）/ linux-x86_64-musl.tar.gz（static-pie，根=scx）/ windows-x86_64.zip（PE32+）/ SHA256SUMS-0.10.0 |
| npm @spapient/scx | ✅ | latest=0.10.0；安装冒烟 `scx --version`=0.10.0；evidence 行保留 + ser_b 字段实测 |
| Homebrew tap | ✅ | formula 0.10.0（darwin tar.gz + sha 1b63bca4…），brew info 识别 0.9.0→0.10.0 |
| Scoop 自建 bucket | ✅ | spapient/scoop-scx（0.10.0 + checkver/autoupdate，license=MIT OR Apache-2.0）；`scoop bucket add scx https://github.com/spapient/scoop-scx` 即时可用 |
| winget | ⏳ | PR #466131（0.10.0 单版本 manifest，CLA 已签 agree，旧 PR #461786 关闭）；微软验证流水线 pending 排队中 |
| 门禁 | ✅ | P12.6 提交 24d5024 在发布链上（HEAD 51c7f56 祖先）；`cargo test --all-features` 407 通过；clippy 0 告警（深度测试复验） |
| 资产校验 | ✅ | SHA256SUMS-0.10.0：darwin 1b63bca4… / linux-musl 2e69399d… / windows 7ff2d3ab… |

## v0.10.0 深度测试修复（2026-10-30）

| # | 发现 | 修复 | 验证 |
|---|---|---|---|
| 1 | Scoop manifest license=MIT，与项目 MIT OR Apache-2.0（LICENSE-MIT/LICENSE-APACHE）不一致 | bucket license 改为 `MIT OR Apache-2.0` 并推送 | GitHub API 直读确认生效 |
| 2 | `wrap_maps_signal_exit_to_128_plus_signal` 测试在 macOS 本地失败（kill -TERM $$ 返回 0） | 根因：agent shell 继承 SIG_IGN（SIGTERM 被忽略）；改用不可忽略的 SIGKILL（期望 128+9=137）；Linux release 二进制实测 `scx wrap -- sh -c 'kill -TERM $$'`=143 证明实现正确 | 本机 + Linux 双验证；全量 407 测试通过 |
| 3 | linux 归档内二进制名为 `scx-linux-x86_64-musl`，与 darwin（scx）/win（scx.exe）命名不一致 | 重打归档（根=scx），重传 release 资产 + 更新 SHA256SUMS-0.10.0（linux hash → 2e69399d…） | 线上下载核验 hash 一致、根=scx；darwin/win hash 不变（Homebrew/Scoop/winget 引用不受影响） |

## 已验证（本轮 dry-run 证据）

| 检查项 | 结果 | 证据 |
|---|---|---|
| `cargo package --list` | ✅ | 196 文件（1.9MiB，压缩 350.5KiB），含 README/LICENSE/SKILL.md/docs/config/tools/bin/Cargo.toml |
| `cargo package --locked`（dry-run verify） | ✅ | `Packaged 196 files, 1.9MiB`；`Verifying scx v0.5.0` dev profile 编译通过 |
| `[package]` 元数据 | ✅ | name/version(0.6.6)/edition(2021)/rust-version(1.82)/license(MIT)/description/repository/homepage/keywords/categories 齐全 |
| LICENSE 文件 | ✅ | 仓库根 LICENSE（MIT），已入包 |
| MSRV 一致性 | ✅ | Cargo.toml `rust-version = "1.82"` 与 README 徽章 `Rust 1.82+` 一致 |
| 文档完备 | ✅ | README（P1–P3 同步至 0.5.0）、SKILL.md、docs/（memory/locking/plan/eval 预注册/RFC） |
| 零第三方 crate | ✅ | `[dependencies]` 为空；可选 feature `mcp`/`semantic` 亦零依赖（手写 std 实现） |

## 发布前必须复查

- [ ] **用户授权**：`cargo publish` 需用户明确指示；默认**不执行**。
- [ ] 包内容体积（1.9MiB / 196 文件）在 crates.io 10MiB 上限内，但若希望收紧，
      可在 `[package]` 增加 `include` 白名单（排除 `.github/`、`config/` 大语料等），
      会改变包内容——需重新 dry-run 验证。
- [ ] 三平台 CI（`ci.yml`）在 `main` 上通过（当前 `codex/readme-polish` 未 merge）。
- [ ] `cargo test --locked`、`cargo clippy --all-targets --locked -- -D warnings`
      （默认 + semantic + mcp 三轨）、`cargo doc`、`cargo build --release --locked`
      全绿（本轮已完成，见 P4 门禁证据）。
- [ ] 版本号与发布 tag 一致（当前 `0.5.0`）；发布后同步 README/SKILL 版本引用（如 bump）。
- [ ] `tools/scx` / `tools/scx install` / `tools/scx.cmd` 入口在当前 HEAD 实测可构建可运行
      （`tools/scx install` 本轮新增，已实测 `--version` 输出）。
- [ ] 名称可用性：`scx` 在 crates.io 上未占用（发布前用 `cargo search scx` 确认；
      若占用需与用户协商更名或接受不同 crate 名）。

## 一键安装命令（三平台）

- macOS / Linux / WSL：`sh tools/scx install`（release 构建 + 软链 `~/.local/bin/scx`，
  目标目录可用 `SCX_BIN_DIR` 覆盖）
- Windows PowerShell：复制 `target\release\scx.exe` 到 `$HOME\bin`（README「一键安装」）
- 任何平台：`tools/scx`（Unix）/ `tools/scx.cmd`（Windows）自举入口，按需自动构建

## 发布后事项（已执行，2026-10-26）

- crates.io `cargo install scx` 路径可用；README 安装章节已更新（含 Homebrew/npm/winget/Scoop）。本机 `~/.cargo/bin/scx` 与 `/opt/homebrew/bin/scx` 均已 0.6.5。
- GitHub Release v0.6.1 + 三平台资产（darwin-arm64 / linux-x86_64 musl / windows-x86_64）。后续 v0.6.4 / v0.6.5 同法发布（三资产 + sha256 核对）。
- Homebrew tap `spapient/homebrew-scx`（0.6.1）发布并实测。
- npm `@spapient/scx`（0.6.2，内置 0.6.1 三平台二进制）发布并实测；0.6.5 已发布（内置 0.6.5 二进制）；`scx` 纯名回收申请中（tu123dou 2017 空壳占用）。
- winget PR #461786 待官方合并；Scoop package request #8767 + 新 PR #8768（旧 #8766 已关：not-meet-criteria）待维护者处理；合并后 manifest 需对齐 0.6.5。
- 完整矩阵与接入状态见 `docs/install-plan.md`（v3）。
