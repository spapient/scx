# scx 发布与接入总览（v8 · 全渠道）

> 版本 v8 · 2026-10-26 · 状态：**v0.6.6 全渠道发布完成，MCP 单 content 修复豆包渲染**
> 变更（v8）：**v0.6.6 MCP 单 content 合并**——`compress_context` 返回「效果卡+JSON」
> **一块** text（效果卡在上、机器 JSON 在下）。实测豆包对多 content（0.6.5 双块）仅渲染
> JSON 转述、效果卡不可见；合并为单块后任意客户端对话均原样显示效果卡。0.6.5 双 content 行为废弃。
> 变更（v7）：**v0.6.5 效果卡进 MCP content**——`compress_context` 返回 `[效果卡, JSON]` 双块，
> 豆包/千问对话调用 compress_context 直接展示「SCX 压缩召回」效果卡（此前仅终端 stderr 可见）；
> 豆包连接器已实证拉到 0.6.5（npx 缓存清理后重启，vendor 二进制 0.6.5）。
> 变更（v6）：**v0.6.4 压缩召回效果报告**——每次压缩输出「SCX 压缩召回」效果卡（stderr），
> 原始/压缩后/省下的行·字符·token 三口径 + 证据召回/去重/裁剪/风险；JSON 与 MCP 同步携带
> `tokens_in`/`tokens_out`/`token_cut_pct`。全渠道升级 0.6.4（crates/GitHub/Homebrew/npm）。
> 变更（v5）：**千问办公最终方案定为命令 `/opt/homebrew/bin/scx mcp`**（千问 STDIO 不传环境变量，
> `SCX_MCP=1` 方案在千问侧失效，已实测留档）；scx 0.6.2 全渠道发布（crates/GitHub/Homebrew）。
> 变更（v4）：千问办公（QwenWorkCN）手动配置注册成功（此前误判为平台放弃）。
> 新增 Homebrew tap / npm scoped 包 / winget / Scoop 四个分发渠道；npm 纯名回收申请中。

---

## 0. 一句话总结

- **安装**：`cargo install scx`（或 Homebrew / npm / winget / Scoop，见 §2）
- **接入开发工具**：`scx setup` 一键注册到 Codex / Claude Code / Cursor / Qwen Code（全自动，实测通过）
- **接入豆包**：GUI 连接器一次填写（`npx -y @spapient/scx mcp`），已注册成功
- **接入千问办公**：GUI 手动配置（命令 `/opt/homebrew/bin/scx mcp`，**带参数，无需环境变量**），**已注册成功并启用**（详见 §3.2）
- **效果可见**：v0.6.4 起每次压缩自动输出「SCX 压缩召回」效果卡（省了多少 token / 召回多少证据 / 做了什么 / 风险如何），豆包/千问调用 compress_context 的返回 JSON 也带 token 效果字段

---

## 1. 发布渠道矩阵（当前全部事实）

| 渠道 | 命名 | 版本 | 状态 | 关键证据 |
|---|---|---|---|---|
| crates.io | `scx`（账户 `apex`） | **0.6.6**（9 版，未 yanked） | ✅ 已发布 | 邮箱已验证（haofei.ni@outlook.com）；`cargo install scx` 可用，本机已实测 0.6.6 |
| GitHub Release | `spapient/scx` | v0.6.6 | ✅ 已发布 | 三平台资产（tar.gz/zip + sha256 已核对）：darwin-arm64 / linux-x86_64（musl 静态）/ windows-x86_64 |
| Homebrew | tap `spapient/homebrew-scx` | 0.6.6 | ✅ 已发布并实测 | `brew tap spapient/scx && brew install scx` → `/opt/homebrew/bin/scx` 0.6.6 可运行 |
| npm | `@spapient/scx`（scoped） | **0.6.6** | ✅ 已发布并实测 | `npx -y @spapient/scx mcp` 通过（serverInfo=0.6.6）；包内置 0.6.6 三平台二进制。历史：0.6.2 包误内置 0.6.1 二进制，已重发修复 |
| npm 纯名 | `scx` | — | ⏳ 回收申请中 | 2017 年空壳包（tu123dou）占用；申请已发 npm 官方（落款 spapient / haofei.ni@outlook.com） |
| winget | `Spapient.Scx` | 0.6.1 | ⏳ PR 审核中 | PR **#461786**（microsoft/winget-pkgs，portable 三 yaml） |
| Scoop | `scx` | 0.6.4 | ⏳ PR 审核中 | package request **#8767** 已提；PR **#8768**（ScoopInstaller/Main，manifest 0.6.4，bot 检查中）。注：旧 PR #8766 因缺 package request 被官方关闭（`not-meet-criteria`），已按流程重提 |
| PyPI | `scx` | — | ✗ 被占 | 现存 v1.3.0 "MIT Supply Chain Python Package"，未注册 |
| AUR | `scx` | — | ✗ 放弃 | 官网被反爬（Anubis）拦截；按用户指示放弃 |

**版本一致性**：crates 0.6.6 = GitHub Release v0.6.6 = Homebrew 0.6.6 = npm 内置二进制 0.6.6 = 豆包/千问连接器 0.6.6（winget/Scoop PR 仍为 0.6.1，待合并时对齐）。

---

## 2. 安装方式（按平台，全部实测或已验证）

| 平台 | 命令 | 状态 |
|---|---|---|
| Rust / Cargo（通用） | `cargo install scx` | ✅ 可用（crates.io 0.6.6） |
| macOS（Homebrew） | `brew tap spapient/scx && brew install scx` | ✅ 本机实测（0.6.6） |
| 任意（Node ≥18） | `npx -y @spapient/scx` | ✅ 实测（MCP 入口 `npx -y @spapient/scx mcp`，latest=0.6.6 内置 0.6.6 二进制） |
| Windows（winget） | `winget install Spapient.Scx` | ⏳ 待 PR 合并后可用 |
| Windows（Scoop） | `scoop install scx` | ⏳ 待 PR 合并后可用 |
| 从源码 | `cargo build --release`（零第三方 crate，离线可构建） | ✅ |
| 本地软链 | `tools/scx install`（构建 + 链接到 `~/.local/bin/scx`） | ✅ 仓库自带 |

> 升级：`cargo install scx --force` 或 `brew upgrade scx`。本机 `~/.cargo/bin/scx` 与 `/opt/homebrew/bin/scx` 均已对齐 0.6.6。

---

## 3. Agent / App 接入矩阵（实测终版）

| 平台 | 接入机制 | scx 注册方式 | 状态 | 证据 |
|---|---|---|---|---|
| Claude Code | `claude mcp add` CLI | `scx setup` 自动 | ✅ 全自动 | `✔ Connected` + `--allowedTools mcp__scx__compress_context` 端到端成功 |
| Codex | `codex mcp add` CLI（config.toml env_key） | `scx setup` 自动 | ✅ 全自动 | `--approve-for-dev` 类放行后 MCP 通道调用成功 |
| Cursor | `~/.cursor/mcp.json` 直写 | `scx setup` 自动 | ✅ 全自动 | 配置落盘，与用户原有连接器并存 |
| Qwen Code | `qwen mcp add` CLI | `scx setup` 自动 | ✅ 全自动 | 官方 mcpServers/stdio 格式 |
| **豆包（Doubao.app）** | GUI 连接器表单（STDIO） | 手动一次填写 | ✅ **已注册成功** | 管理 → 连接器 → 个人板块显示 `scx`，开关开启 |
| **千问办公（QwenWorkCN）** | GUI 手动填写（STDIO，命令带参数） | 手动一次填写 | ✅ **已注册成功** | 连接器 → 已安装 → `scx` 开关开启，无失败标记；命令 `/opt/homebrew/bin/scx mcp`（详见 §3.2） |

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

### 3.2 千问办公（已注册成功，2026-10-26 手动配置 · 终版方案）

**结论：手动配置路径可用，已注册成功并启用。** 此前"平台禁止本地 STDIO"（`LOCAL_IDENTITY_PHASE_RESTRICTED`）仅针对 **JSON 导入**流程；**手动填写表单**不受该限制（用户主张正确）。

**最终配置值（实测，GUI 连接器 → 已安装 → scx → 编辑）**：

| 字段 | 值 |
|---|---|
| 服务器类型 | STDIO |
| 服务器名称 | `scx` |
| 命令 | `/opt/homebrew/bin/scx mcp`（**带 `mcp` 子命令参数**） |
| 环境变量 | 留空（**千问不传 env，见坑 3**） |
| 超时时间 | 默认 60 |

**三个关键坑（务必记录）**：
1. **千问命令字段对 `npx -y @spapient/scx mcp` 只保留第一个 token**：`npx` 类命令保存后实际存为 `command: "npx"`（参数被丢弃）。**但 `/opt/homebrew/bin/scx mcp` 这类绝对路径 + 参数会被完整保存**（JSON 预览确认 `command` 完整、含空格），因此显式 `mcp` 子命令是千问侧最稳的启动方式。
2. **本机无系统级 node/npx**：千问 App 启动子进程的 PATH 不含 Node 目录 → 旧 npx 配置 `process_not_found`。直接使用 brew 二进制 `/opt/homebrew/bin/scx` 规避。
3. **千问 STDIO 不传递环境变量给子进程**：`SCX_MCP=1` 虽在 JSON 预览中正确生成（`env.SCX_MCP: "1"`、configRevision 递增），但 spawn 时 env 丢失——scx 以无参数方式启动、打印 usage 退出，诊断 `code=unknown`、`Registered 0 tools`（千问 main.log 实锤）。→ 不要依赖环境变量，用显式 `mcp` 子命令。

**注**：scx 0.6.4 仍保留 `SCX_MCP=1` 环境变量开关（无参数时进 MCP 模式），对支持 env 的 MCP 客户端（如 Cursor/其他 harness）有效；千问侧不可用。

**保存后验证**：已安装列表 `scx` 开关置 1，`IDENTITY_CONNECTOR_CONNECTION_FAILED` 消失、无「诊断」按钮；端到端调用 `compress_context` 见 §5 待办（千问对话中实测）。

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
| 4 | ~~豆包端到端调用验证~~ | ✅ 已实证 | 连接器进程链实证；豆包已拉 0.6.6，MCP 返回「效果卡+JSON」单 content（豆包对话直接可见压缩效果） |
| 5 | ~~本机版本对齐~~ | ✅ 已完成 | `~/.cargo/bin/scx` 与 `/opt/homebrew/bin/scx` 均已升 0.6.6 |
| 6 | 千问办公端到端调用验证 | ⏳ 待测试 | 连接器已注册并启用（命令 `/opt/homebrew/bin/scx mcp`）；在千问对话中调用 `compress_context` 确认 |
| 7 | ~~`/opt/homebrew/bin/scx-mcp` 旧包装脚本~~ | ✅ 已删除 | 已被「命令带 `mcp` 参数」方案取代，2026-10-26 删除 |

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
