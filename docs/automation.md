# scx 全事件自动化与统计体系工程方案

> 版本：v3（2026-11-10）。状态：**S0–S6 代码实施完成（全门绿）；发布待授权**。
> 本文是跨客户端事件自动化、统计重构与代码精简的总设计，实施按 §7 阶段门推进，
> 门不过不进下一阶段。历史路线见 `docs/plan.md`（归档），当前发布核验见
> `docs/release-checklist.md`。
>
> 实施回填：S0–S4 已按 §7 落地（hook 事件路由、setup 同表写 MCP+hooks、统一
> `scx-stat-1` 统计与卡片、并行度/哈希/校准/词边界性能项），S5 完成行谓词
> mark 骨架合并与文档收口；S6 已接入 Cursor 官方 hooks.json 并核实豆包无文档化
> hook 面（保留 MCP 连接器）。所有门禁双轨绿，golden 零漂移。

## 1. 目标与验收标准

1. **全事件触发**：新建会话、恢复/历史会话、窗口开启、每条用户消息（含简单的
   "你好"）、每次工具调用，scx 都先被调用；是否压缩由 scx 自己判定，任何路径
   都不静默。
2. **恒等口径可见**：不压缩时同样输出统计——原始 token、输出 token（恒等）、
   判定结果与原因；压缩时输出完整 delta。
3. **专业配置到位**：每个客户端使用其官方事件名、配置文件与 schema 名称，不
   自造字段；安装流程一次写齐 MCP 连接器与事件 hook。
4. **代码更精简**：表驱动注册、统一事件路由、合并重复统计渲染，目标净减少
   源码文件与行数（具体数字在 S0 基线测量后回填，不预先承诺）。
5. **红线不破**：零第三方 crate、`unsafe_code = "forbid"`、golden 逐字节零漂移
   （新行为走新例）、三平台 CI、MSRV 1.82。

## 2. 事实基线（研究证据）

| 事实 | 来源 |
|---|---|
| Claude Code 事件分三档：会话级 `SessionStart/SessionEnd`；每轮 `UserPromptSubmit/Stop/StopFailure`；工具级 `PreToolUse/PostToolUse/PostToolUseFailure/PostToolBatch`；另有 `Setup/PreCompact/SubagentStart` 等共 30+ 事件 | Claude Code Hooks reference, code.claude.com/docs/en/hooks |
| Claude hooks 配置在 `~/.claude/settings.json`（用户）与 `.claude/settings.json`（项目），三层嵌套：事件 → matcher group → handler；handler 类型 `command/prompt/agent`；command 经 stdin 收 JSON | 同上 |
| 启动时的 `SessionStart` 早于 MCP 可用，`mcp_tool` 类 handler 被跳过；启动期注入必须用 `type:"command"` | 同上 |
| matcher 规则：`*`/空 = 全匹配；纯字母与 `,\|` = 精确枚举；含其他字符 = JS 正则；工具事件按 tool_name 过滤，如 `Bash`、`Edit \| Write`、`mcp__.*` | 同上 |
| handler 可返回 `hookSpecificOutput.additionalContext`（注入上下文）、`updatedToolOutput`（替换工具输出）、`systemMessage`（UI 提示） | 同上 |
| Codex CLI 自 v0.114 起支持 shell hooks，现默认开启；配置在独立文件 `~/.codex/hooks.json`（`$CODEX_HOME` 可重定位），项目级 `<repo>/.codex/hooks.json`，两者叠加 | symposium Codex CLI Hooks Reference（非官方，需以官方文档复核） |
| Codex hook 按**定义哈希**建立信任：未信任的 hook 不执行，需在 `/hooks` 审核；`codex exec --dangerously-bypass-hook-trust` 可单次绕过 | 同上 |
| Codex 事件：`SessionStart/PreToolUse/PostToolUse/UserPromptSubmit/Stop`；仅 `command` handler；默认超时 600s；退出码 0=继续、2=阻断 | 同上 |
| JetBrains 2025.2 起内置 MCP Server 插件（默认启用），设置内 Clients Auto-Configuration 可自动改写 Junie/VS Code/Claude Code/Codex/Air/Copilot CLI 的配置 | jetbrains.com/help/ai-assistant/mcp.html |
| JetBrains 提供 router-only 模式：工具从直出列表隐藏，仅经统一工具 `execute_tool` 调用，减少工具描述占用的上下文；配套 `ij-debugger` 等技能 | jetbrains.com/help/idea/agentic-debugging.html |
| 本机现状：`~/.claude.json` 的 `hooks` 为空、`settings.json` 无 hooks 段；Codex `config.toml` 无 hooks；`src/setup.rs` 全文无 hook 写入；`scx hook` 二进制可用但仅处理 Bash 的 PostToolUse | 本机实测，2026-11-10 |
| 豆包/千问公开配置目录中未发现用户 hook schema（仅 MCP 连接器表单项）；不作为"无能力"结论，列入 S6 核实项 | 本机实测 |

## 3. 客户端事件配置矩阵（专业名称，不做自造字段）

### 3.1 Claude Code — `~/.claude/settings.json` 的 `hooks` 段

| 挂接事件 | matcher | 用途 |
|---|---|---|
| `SessionStart` | `startup,resume,clear,compact,fork` | 开窗/恢复即触发；初始化统计口径，可经 additionalContext 注入环境事实 |
| `UserPromptSubmit` | `*` | 每条消息必触发（含"你好"）；scx 判定后返回恒等或压缩统计 |
| `PostToolUse` | `Bash,Read,Grep,Glob` | 工具输出回填前压缩；非 Bash 工具由升级后的通用路由处理 |
| `PreToolUse`（可选） | `Bash` | 需要硬预算/拦截时启用，默认不挂 |

handler 统一形态：`{"type":"command","command":"<scx绝对路径> hook","timeout":10}`。
注意启动期 SessionStart 不能用 MCP 类 handler，command 形态是唯一合规选择。

### 3.2 Codex — `~/.codex/hooks.json`（独立文件，非 config.toml）

| 挂接事件 | matcher | 备注 |
|---|---|---|
| `SessionStart` | `""`（全匹配） | 同 Claude 语义 |
| `UserPromptSubmit` | `""` | 每条消息 |
| `PostToolUse` | `"Bash|Read|Grep"`（正则口径） | 工具输出 |
| `Stop` | 无 | 轮次结束的统计收口 |

写入后必须引导用户在 `/hooks` 完成信任审核（按哈希），否则定义不执行；安装输出
需明确打印这一人工步骤，不能给用户"已全自动"的错觉。`config.toml` 侧确认
`[features] hooks` 未被关闭。

### 3.3 Cursor — 官方 hooks.json（S6 已接入）

Cursor 已提供文档化 hook 体系（cursor.com/docs/hooks）：用户级 `~/.cursor/hooks.json`
（项目级 `<repo>/.cursor/hooks.json`），schema 为 `{"version":1,"hooks":{事件:[定义]}}`，
定义是扁平结构（`command`/`timeout`/`matcher`，matcher 为正则）。

| 挂接事件 | 官方事件名 | 回执口径 |
|---|---|---|
| 会话开启 | `sessionStart` | `additional_context` 注入恒等统计 |
| 用户提交 | `beforeSubmitPrompt` | `continue:true`（官方仅文档化阻断语义） |
| 工具回填 | `postToolUse` | `additional_context` 注入压缩文本/统计；`tool_output` 为 JSON 字符串化载荷，含 stdout 对象时解包 |
| 轮次结束 | `stop` | 空对象收口 |

已真机验证：`scx setup --cursor` 幂等写入四个事件，外部定义保留、旧 scx 定义替换；
真实事件经 `scx hook` 端到端压缩正确。

### 3.4 豆包 — S6 核实结论：无文档化 hook 面，保留 MCP 连接器

证据（2026-11-10）：

- 豆包客户端官方仅提供 GUI 管理的"自定义连接器"（MCP HTTP/STDIO），帮助中心与
  豆包开放平台文档均无用户级 hook 配置项或事件 schema；
- 本机连接器配置由账号托管/加密存储（Preferences 中 `saman.mcp` 仅含
  `encrypted_credentials_v1`），无文档化明文格式；
- 火山引擎系具备 hook 能力的是独立产品 TRAE，并非豆包客户端。

结论：按红线不写未文档化字段，豆包侧维持 MCP 连接器，统计在 MCP 工具返回内
呈现；后续官方开放 hook 面后再按官方 schema 补行。

### 3.5 千问 — Qwen Code 官方 hook 体系（待接入）

Qwen Code 已文档化 hooks（qwenlm.github.io，事件含 UserPromptSubmit 等，配置与
Claude Code 同构），具备接入条件，列入后续实施。

## 4. 统一 hook 事件架构（`scx hook` 升级）

现状 `src/hook.rs` 只识别 Bash PostToolUse，且非 sieve 即返回 None。升级为**事件
路由器**，单一二进制处理全部事件：

```text
stdin JSON
   │
   ├─ 1. 解析 hook_event_name（缺省按 PostToolUse 兼容旧宿主）
   ├─ 2. 事件取数：
   │       SessionStart       → 环境/会话事实，无正文载荷
   │       UserPromptSubmit   → prompt 文本
   │       PostToolUse        → tool_response（stdout/stderr/结构位）
   │       Stop               → last_assistant_message
   ├─ 3. 统一判定：compress 内核输出 mode（sieve/overflow/passthrough/failopen）
   ├─ 4. 装配宿主契约：
   │       updatedToolOutput（PostToolUse 替换，结构位逐字段保留）
   │       additionalContext（统计/事实注入，全部事件可用）
   │       systemMessage（UI 提示，可选）
   └─ 5. 恒等路径也返回结构化 JSON（continue + 统计），不再返回空
```

设计约束：

- 事件适配为表驱动（事件名 → 取数函数 → 输出槽位），新增客户端事件只加表行；
- 保留现有 64 MiB 上限、超限 drain 防 EPIPE、非 UTF-8 透传逻辑；
- 压缩 query 来源优先级：事件显式字段 > 工具命令文本 > 固定兜底词；
- 输出对宿主必须是其文档化契约，任何不识别的事件返回 `{"continue":true}` 并在
  统计中标记 `decision:"unknown-event"`，不报错退出。

## 5. 统计展示体系（工业级，美观与专业双口径）

### 5.1 统一数据模型（机器 JSON，schema 版本化）

所有事件恒返回以下字段，**恒等路径填恒等值而非省略**：

| 字段 | 含义 |
|---|---|
| `schema` | 统计 schema 版本，当前 `"scx-stat-1"` |
| `decision` | `compress` / `passthrough` / `failopen` / `overflow` / `unknown-event` |
| `mode` / `risk` | 内核 Mode、Risk 原值 |
| `lines_in/out`、`chars_in/out`、`tokens_in/out` | 行/字符/估算 token 三口径，恒等时 in=out |
| `delta_lines/chars/tokens`、`cut_pct` | 差值与百分比；恒等全为 0 |
| `evidence_total/retention` | 证据声明数与实际保留率 |
| `ser_b` | 实际序列化载荷 UTF-8 字节数（真实口径，非估算） |
| `reason` | 判定的稳定枚举原因码，供 UI 本地化 |
| `rewritten` | trim/fold 改写标记，默认 false |

向后兼容：现有 `--json` 字段不删不改，新字段追加；golden 旧例保持零漂移。

### 5.2 人读卡片（stderr / systemMessage / MCP 单 content）

- 统一栅格：标签列等宽、数值列右对齐、百分比统一一位小数；三口径（行/字符/
  token）分行呈现，中文场景三者独立计算；
- 恒等卡片明确写"原样保留 · 未压缩"并附原因（小输入/零命中/无信号），不出现
  空卡片；
- 视觉语言保持克制：单一分隔符、无多余装饰、等宽数字对齐；卡片样式在
  `ops::recall_report` 一处定义，hook/MCP/CLI 共用，禁止各模块自绘。

## 6. 代码精简方案

| 动作 | 现状 | 目标 |
|---|---|---|
| 注册表驱动 | `setup.rs` 中 `claude_reg/codex_reg/cursor_reg` 三个近似函数 + 各自写盘 | 客户端能力表（配置路径/段名/写入器）+ 单一注册循环；MCP 与 hook 两类条目同表声明 |
| 通用事件路由 | `hook.rs` Bash 专用 transform | §4 事件表，单函数分发，删除分支特化代码 |
| 统计收口 | `ops::recall_report`、`meter::render`、hook 内拼装三处重复渲染 | 统一 `stat` 模型 + 单一渲染器，其余模块只填数据 |
| 死代码与重复 | S0 用 `scx-tooling audit` 与调用点扫描确认 | 删除零调用项；合并重复 JSON/字符串构造 |
| 测试资产 | 事件行为分散在各 mod tests | 事件表驱动测试矩阵，减少重复用例代码 |

不做的事：不改变压缩内核算法语义、不合并 memory I/O 与纯函数层的隔离边界、
不引入任何第三方 crate、不做未文档化的客户端字段写入。

## 7. 阶段门实施计划

| 阶段 | 内容 | 门禁（不过不进） | 状态 |
|---|---|---|---|
| S0 | 修复当前编译失败（`setup.rs:479` 切片引用；测试内 unsafe set_var 改为纯函数 `home_from` 传参）；建立源码行数/审计基线 | fmt、clippy 双轨、`test` 默认+all-features、release build 全绿 | ✅ 完成 |
| S1 | `scx hook` 升级为通用事件路由器（§4），含事件表与恒等结构化输出 | 全测试绿；新增事件路由单测；golden 零漂移 | ✅ 完成 |
| S2 | `setup` 升级：同表写 MCP + hooks（Claude settings.json、Codex hooks.json），Codex 信任步骤显式提示 | 真机握手 + 触发实测；备份/幂等/自检测试齐 | ✅ 完成（3 个注册测试） |
| S3 | 统一统计模型与卡片（§5），恒等路径全量呈现 | 机器 JSON 契约测试；卡片快照测试；旧字段兼容校验 | ✅ 完成（`src/stat.rs`，4 单测） |
| S4 | 性能项：`par::count` 统一并行度、哈希 `step` 同源、`BUDGET_SAFETY_FACTOR` 校准冻结 1.787、`--word-boundary` 词边界 | 全门禁；golden 零漂移；`scx-tooling calibrate` 370 样本 | ✅ 完成（`src/par.rs`） |
| S5 | 精简收口（§6）：行谓词 mark 合并为 `mark_by_predicate`+`MarkOut`；文档与 release-checklist 口径 | 全门禁双轨；golden 零漂移 | ✅ 代码完成 |
| S6 | 客户端 hook 能力核实：Cursor 官方 hooks.json 已接入（真机验证）；豆包无文档化 hook 面，保留 MCP 连接器；千问 Qwen Code 具备同构 hook，待后续接入 | 证据留档；仅按官方 schema 写入 | ✅ Cursor/豆包完成，千问待接入 |

发布动作（crates.io/npm/brew/scoop/winget 等）属高影响外发，需用户显式授权，
且先对齐 `docs/release-checklist.md` 与 tag v0.11.0 的口径。

## 8. 风险与回滚

1. **宿主契约漂移**：Claude/Codex hook schema 随版本变化——S2/S6 以官方文档
   复核为准，渲染契约集中在适配层，变更只改表行。
2. **Codex 信任门槛**：未审核 hook 不执行属预期行为，安装输出必须显式提示。
3. **golden 红线**：任何默认路径输出变化先失败、再决定新增用例，禁止直接改基线。
4. **回滚**：所有写盘保留 `.scx.bak` 单份最新备份；代码改动按阶段独立提交，
   可逐阶段 revert；不删除、不移动用户既有配置与未跟踪文件。
