# scx 发布检查单（crates.io 发布前）

> 状态：**未发布**。`cargo publish` 是外部动作，需用户明确授权后由用户在
> 本仓库执行；本文件记录 dry-run 验证证据与发布前必须复查的条目。
> 检查日期：2026-10-25，包版本 `0.5.0`。

## 已验证（本轮 dry-run 证据）

| 检查项 | 结果 | 证据 |
|---|---|---|
| `cargo package --list` | ✅ | 196 文件（1.9MiB，压缩 350.5KiB），含 README/LICENSE/SKILL.md/docs/eval/bin/Cargo.toml |
| `cargo package --locked`（dry-run verify） | ✅ | `Packaged 196 files, 1.9MiB`；`Verifying scx v0.5.0` dev profile 编译通过 |
| `[package]` 元数据 | ✅ | name/version(0.5.0)/edition(2021)/rust-version(1.82)/license(MIT)/description/repository/homepage/keywords/categories 齐全 |
| LICENSE 文件 | ✅ | 仓库根 LICENSE（MIT），已入包 |
| MSRV 一致性 | ✅ | Cargo.toml `rust-version = "1.82"` 与 README 徽章 `Rust 1.82+` 一致 |
| 文档完备 | ✅ | README（P1–P3 同步至 0.5.0）、SKILL.md、docs/（memory/locking/plan/eval 预注册/RFC） |
| 零第三方 crate | ✅ | `[dependencies]` 为空；可选 feature `mcp`/`semantic` 亦零依赖（手写 std 实现） |

## 发布前必须复查

- [ ] **用户授权**：`cargo publish` 需用户明确指示；默认**不执行**。
- [ ] 包内容体积（1.9MiB / 196 文件）在 crates.io 10MiB 上限内，但若希望收紧，
      可在 `[package]` 增加 `include` 白名单（排除 `.github/`、`eval/` 大语料等），
      会改变包内容——需重新 dry-run 验证。
- [ ] 三平台 CI（`ci.yml`）在 `main` 上通过（当前 `codex/readme-polish` 未 merge）。
- [ ] `cargo test --locked`、`cargo clippy --all-targets --locked -- -D warnings`
      （默认 + semantic + mcp 三轨）、`cargo doc`、`cargo build --release --locked`
      全绿（本轮已完成，见 P4 门禁证据）。
- [ ] 版本号与发布 tag 一致（当前 `0.5.0`）；发布后同步 README/SKILL 版本引用（如 bump）。
- [ ] `bin/run` / `bin/install` / `bin/run.cmd` 入口在当前 HEAD 实测可构建可运行
      （`bin/install` 本轮新增，已实测 `--version` 输出）。
- [ ] 名称可用性：`scx` 在 crates.io 上未占用（发布前用 `cargo search scx` 确认；
      若占用需与用户协商更名或接受不同 crate 名）。

## 一键安装命令（三平台）

- macOS / Linux / WSL：`sh bin/install`（release 构建 + 软链 `~/.local/bin/scx`，
  目标目录可用 `SCX_BIN_DIR` 覆盖）
- Windows PowerShell：复制 `target\release\scx.exe` 到 `$HOME\bin`（README「一键安装」）
- 任何平台：`bin/run`（Unix）/ `bin/run.cmd`（Windows）自举入口，按需自动构建

## 发布后事项（不在本轮范围）

- 若发布成功：README 增加 `cargo install scx` 安装路径并重新验证；确认 tag 与
  版本号；观察 crates.io 元数据页（description/keywords/categories 渲染）。
