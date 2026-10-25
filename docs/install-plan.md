# scx 发布与接入总览（v3 · 全渠道）

> 版本 v3 · 2026-10-26 · 状态：**发布链全部落地**，接入矩阵按实测更新
> 变更：千问办公（QwenWorkCN）因平台侧硬限制放弃；豆包（Doubao.app）连接器注册成功；
> 新增 Homebrew tap / npm scoped 包 / winget / Scoop 四个分发渠道；npm 纯名回收申请中。

---

## 0. 一句话总结

- **安装**：`cargo install scx`（或 Homebrew / npm / winget / Scoop，见 §2）
- **接入开发工具**：`scx setup` 一键注册到 Codex / Claude Code / Cursor / Qwen Code（全自动，实测通过）
- **接入豆包**：GUI 连接器一次填写（`npx -y @spapient/scx mcp`），已注册成功
- **千问办公**：平台侧限制本地 STDIO 连接器（`LOCAL_IDENTITY_PHASE_RESTRICTED`），已放弃

---

## 1. 发布渠道矩阵（当前全部事实）

| 渠道 | 命名 | 版本 | 状态 | 关键证据 |
|---|---|---|---|---|
| crates.io | `scx`（账户 `apex`） | **0.6.1**（4 版，未 yanked） | ✅ 已发布 | 邮箱已验证（haofei.ni@outlook.com）；`cargo install scx` 可用 |
| GitHub Release | `spapient/scx` | v0.6.1 | ✅ 已发布 | 三平台资产：darwin-arm64 / linux-x86_64（musl 静态）/ windows-x86_64 |
| Homebrew | tap `spapient/homebrew-scx` | 0.6.1 | ✅ 已发布并实测 | `brew tap spapient/scx && brew install scx` → `/opt/homebrew/bin/scx` 0.6.1 可运行 |
| npm | `@spapient/scx`（scoped） | **0.6.2** | ✅ 已发布并实测 | `npx -y @spapient/scx mcp` 通过；包内含三平台二进制（1.8MB） |
| npm 纯名 | `scx` | — | ⏳ 回收申请中 | 2017 年空壳包（tu123dou）占用；申请已发 npm 官方（落款 spapient / haofei.ni@outlook.com） |
| winget | `Spapient.Scx` | 0.6.1 | ⏳ PR 审核中 | PR **#461786**（microsoft/winget-pkgs，portable 三 yaml） |
| Scoop | `scx` | 0.6.1 | ⏳ PR 审核中 | PR **#8766**（ScoopInstaller/Main，bucket/scx.json） |
| PyPI | `scx` | — | ✗ 被占 | 现存 v1.3.0 "MIT Supply Chain Python Package"，未注册 |
| AUR | `scx` | — | ✗ 放弃 | 官网被反爬（Anubis）拦截；按用户指示放弃 |

**版本一致性**：crates 0.6.1 = GitHub Release 0.6.1 = Homebrew 0.6.1 = winget/Scoop 0.6.1 = npm 内置二进制 0.6.1（npm 包自身版本 0.6.2 仅用于 npm 发版流水号）。

---

## 2. 安装方式（按平台，全部实测或已验证）

| 平台 | 命令 | 状态 |
|---|---|---|
| Rust / Cargo（通用） | `cargo install scx` | ✅ 可用（crates.io 0.6.1） |
| macOS（Homebrew） | `brew tap spapient/scx && brew install scx` | ✅ 本机实测 |
| 任意（Node ≥18） | `npx -y @spapient/scx` | ✅ 实测（MCP 入口 `npx -y @spapient/scx mcp`） |
| Windows（winget） | `winget install Spapient.Scx` | ⏳ 待 PR 合并后可用 |
| Windows（Scoop） | `scoop install scx` | ⏳ 待 PR 合并后可用 |
| 从源码 | `cargo build --release`（零第三方 crate，离线可构建） | ✅ |
| 本地软链 | `tools/scx install`（构建 + 链接到 `~/.local/bin/scx`） | ✅ 仓库自带 |

> 升级：`cargo install scx --force` 或 `brew upgrade scx`。注意本机 `~/.cargo/bin/scx` 若停留 0.6.0，执行 `cargo install scx --force` 重装到 0.6.1。

---

## 3. Agent / App 接入矩阵（实测终版）

| 平台 | 接入机制 | scx 注册方式 | 状态 | 证据 |
|---|---|---|---|---|
| Claude Code | `claude mcp add` CLI | `scx setup` 自动 | ✅ 全自动 | `✔ Connected` + `--allowedTools mcp__scx__compress_context` 端到端成功 |
| Codex | `codex mcp add` CLI（config.toml env_key） | `scx setup` 自动 | ✅ 全自动 | `--approve-for-dev` 类放行后 MCP 通道调用成功 |
| Cursor | `~/.cursor/mcp.json` 直写 | `scx setup` 自动 | ✅ 全自动 | 配置落盘，与用户原有连接器并存 |
| Qwen Code | `qwen mcp add` CLI | `scx setup` 自动 | ✅ 全自动 | 官方 mcpServers/stdio 格式 |
| **豆包（Doubao.app）** | GUI 连接器表单（STDIO） | 手动一次填写 | ✅ **已注册成功** | 管理 → 连接器 → 个人板块显示 `scx`，开关开启 |
| **千问办公（QwenWorkCN）** | GUI JSON 导入 / 手动填写 | — | ✗ **放弃** | 平台侧 `LOCAL_IDENTITY_PHASE_RESTRICTED`：个人免费版禁止本地 STDIO 连接器 |

### 3.1 豆包接入（本次新增，已注册成功）

1. 豆包 → **插件·技能·伙伴** → 右上 `+ 添加` → **新建自定义连接器**
2. 传输类型选 **STDIO**（豆包默认显示 HTTP，需切换；表单为命令/参数/环境变量）
3. 填写（实测值）：

| 字段 | 值 |
|---|---|
| 服务器名称 | `scx` |
| 命令 | `npx` |
| 参数 | `-y` · `@spapient/scx` · `mcp`（三行） |
| 环境变量 | 留空 |

4. **保存** → 管理页「连接器」出现 `scx` 并默认启用。

> 豆包 STDIO 支持任意可执行命令（npx 或 `./script.sh` 均可），无千问办公那种"仅限 npx"限制；
> 无出网依赖问题——`@spapient/scx` 包内含本地二进制，首次启动由 npx 拉包后即本地运行。

**待办**：豆包内实际调用 `compress_context` 的端到端验证（输入框发送测试消息，观察是否返回压缩结果）。此前 computer-use 服务故障中断了自动点击；人工在豆包输入框发送即可完成。

### 3.2 千问办公（放弃结论，留档）

- 连接器存储为加密结构（IdentityConnectorConfigStore + Keychain 派生加密），外部直写文件必失败（decryptString 校验）。
- 本地 MCP runtime（127.0.0.1:54365）与内置 CLI 均需 App 内部认证，无外部脚本路径。
- **最终定性**：GUI 粘贴 JSON 在个人免费版账号被 toast `LOCAL_IDENTITY_PHASE_RESTRICTED` 拒绝——**平台按账号阶段禁止本地 STDIO 连接器**，与配置无关，脚本无法也不应绕过。
- 结论：放弃该平台接入；如后续账号升级解锁，可复用 §3.1 豆包同款配置（命令改为 `npx -y @spapient/scx mcp`）。

---

## 4. 命令速查

```bash
# 安装（任选其一）
cargo install scx                                    # Rust 通用（推荐）
brew tap spapient/scx && brew install scx            # macOS Homebrew
npx -y @spapient/scx                                 # Node 环境临时用
winget install Spapient.Scx                          # Windows（PR 合并后）
scoop install scx                                    # Windows（PR 合并后）

# 一键接入开发工具（全自动）
scx setup --all          # Codex / Claude / Cursor / Qwen Code 注册 + 握手自检
scx setup                # 交互菜单
scx setup --claude --codex   # 指定平台
scx setup --list         # 各平台检测状态
scx setup --verify       # 会话级握手自检
scx setup --uninstall    # 可逆卸载

# MCP 协议入口（供手工配置使用）
scx mcp                  # stdio JSON-RPC（serverInfo.name=scx）
```

---

## 5. 已知问题与待办清单

| # | 事项 | 状态 | 影响 |
|---|---|---|---|
| 1 | winget PR #461786 审核 | ⏳ 官方审核中 | 全球 Windows 用户 `winget install` 暂不可用 |
| 2 | Scoop PR #8766 审核 | ⏳ 官方审核中 | `scoop install scx` 暂不可用 |
| 3 | npm 纯名 `scx` 回收 | ⏳ 官方处理中 | 命名统一为 `scx` 的最终形态；现 scoped 包 `@spapient/scx` 已可用 |
| 4 | 豆包端到端调用验证 | ⏳ 待人工/重试 | 连接器已注册；验证压缩工具实际被豆包调用 |
| 5 | 本机 `~/.cargo/bin/scx` = 0.6.0 | ⚠️ 需重装 | `cargo install scx --force` 升到 0.6.1，与发布版本对齐 |
| 6 | 千问办公接入 | ✗ 平台限制 | 无解；账号升级后可复用配置 |

---

## 6. 安全与隐私说明

- scx 本体：Rust 零第三方 crate、零网络、零权重供应链；MCP 通道为本地 stdio（不进云端）。
- 豆包连接器经 npx 拉取 `@spapient/scx` 后本地运行，不传原始上下文出网（与云 API 形态不同）。
- 分发链供应链：crates.io / npm 发布均经账号令牌；GitHub Release 资产 sha256 已核对。

---

## 7. 相关文档

- 发布流程：`docs/release-checklist.md`
- 版本与阶段记录：`docs/plan.md`（P1–P4 + 0.5.0→0.6.1 发布记录）
- 能力对照：`docs/containcmp.md` / `docs/contain5-compare.json`（证据保留 70/70 vs 头截断 0/70）
- 竞品深度对比：`docs/comparison-v051.md`（本地草稿，不入库）
