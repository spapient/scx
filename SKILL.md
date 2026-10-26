---
name: scx
description: 本地查询感知上下文压缩器（Rust，跨平台自举，v0.6.0+ 记忆库；内置 MCP stdio server 与一键注册）。对长日志、命令输出、diff、错误堆栈、大文件按当前问题逐行筛选证据；证据行永不丢弃，可选内容只在种子邻域区域内填充，查询无命中时原样保留；保留行可加原文行号前缀；附带本地记忆知识库（--store/--recall/--eval，零依赖离线，L1 直答优先）；输出 compressed: 统计行与风险审计。凡预期输出很大的命令、读大文件、长粘贴、检索结果，先经它再进上下文。
---

# scx — 本地证据筛选压缩（Rust v0.6.0）

## 何时用（触发条件）
- 命令输出预计超过约 200 行：构建、测试、跑批、抓包转储。
- 文件超过约 500 行，而问题只关心其中相关片段。
- 错误堆栈、git diff、检索结果、外部粘贴进入上下文之前。

## 怎么用（两步）
1. 大输出先落盘，不要让整段进入上下文：`<命令> > /tmp/out.txt 2>&1`
2. 按当前问题关键词压缩，只把压缩结果读进来。入口为技能目录 `tools/scx`（Windows 为 `tools/scx.cmd`）；首次调用自动识别本机并构建，权限不足时用 `sh <技能目录>/tools/scx`。路径按安装位置替换：Claude 时为 `~/.claude/skills/scx`。

```bash
tools/scx --query "问题关键词" --ratio 0.35 /tmp/out.txt
```

高频参数（完整参数与默认值以 README「CLI 参考」与 `tools/scx --help` 为准，不在本文档重复定义）：
- `--ratio`：可选内容预算（默认 0.35，范围 0.02..=1.0）；证据行与最小邻域不受预算限制。
- `--context 0`：针尖模式，完全跳过可选填充，只保留证据行与省略标记；token 极紧时使用，日常默认 1。
- `--force "子串A|子串B"`：强制保留匹配行（视为证据行），大小写不敏感，可重复；`re:` 前缀进入有限正则子集（交替/字符类/量词），例如 `--force "re:ERROR|FATAL"`。
- `--max-tokens N`：输出 token 硬上限；证据行及其最小邻域优先（证据地板最高优先），超限如实报告 `overflow:evidence`/`overflow:fill`，绝不静默丢证据。
- `--structure 1`：结构感知邻域——fence 代码块、import/use 块、堆栈 `at` 链整段并入；默认关（关闭时与 v0.4.1 逐字节一致）。
- `--policy-file <path>`：策略文本覆盖默认值（ratio/context/window/elide/risk_gate/lineno/max_tokens/structure）；优先级 CLI 显式 > 文件 > DEFAULT。
- `--lineno 1`：筛选输出附原文行号前缀（`N: `）；passthrough/fail-open 返回原文时不附加。
- `--risk-gate`（默认开）：风险审计为 HIGH 时额外保护失败级错误行与事故标记行（统计行 `guarded=1`）。
- `--json` / `--stat` / `--no-stats`：统计输出形态（`--json` 含 dropped 区间明细与 max_tokens/tok_est 字段）。
- `scx mcp`：进入 MCP stdio server 模式（initialize/tools/list/tools/call；`serverInfo.name=scx`，工具 `compress_context`/`connect_account`/`usage_summary`）——供 Claude/Codex/Cursor 以子进程拉起。
- `scx setup [--claude|--codex|--cursor] [--project-cursor]`：一键注册 MCP 配置到三种 agent（注册名统一小写 `scx`；幂等、写前备份 `<path>.scx.bak`、写后 tools/list 自检；Claude/Codex 优先官方 CLI、兜底写配置文件）。

约束：单次输入上限 64 MiB（多文件合计）；未提供 `--query` 时必须至少一个非空 `--force`；小输入（<40 行且 <600 token 估算）自动 passthrough。

## 行为保证（不变量）
- 只删不改：筛选保留行是原文逐字，`...` 仅表示省略区间；passthrough/fail-open 返回输入原文。
- 证据优先：证据行及其最小邻域先于预算确定，`--ratio` 不挤证据。
- 确定性：无随机源、无网络；相同输入、查询与配置产出相同结果。
- 显式回退：无命中或无可用内容时返回原文并报告 `mode`，不静默返回空。
- `compressed:` 统计行仅作核对，不是正文，不转述给用户；`seeds` 与 `evidence` 恒等，可机械断言。
- v0.6.4+ 每次压缩在 stderr 输出「SCX 压缩召回」效果卡（原始/压缩后/省下的行·字符·token、证据召回、去重裁剪明细、风险；小输入透传与零命中各有结论卡）；`--json` 与 MCP 返回同步携带 `tokens_in`/`tokens_out`/`token_cut_pct`（估算口径，数字带"约"）。

## 记忆知识库（可选，默认关闭）
- 沉淀：`--store 1 --fix "修复结论"` 把证据块与修复结论入库（入库前统一脱敏已知敏感模式，非通用 DLP）；不带 `--fix` 会警告。库目录 `--mem-dir DIR`（默认 `$SCX_MEM_DIR` 或 `~/.scx-memory`）；case id 形如 `<日期>_<16位hex>`。
- 召回：`--recall K` 旁路输出 `mem:` 段；`--recall-detail` 附 `summary:`/`evidence:`/`fix:`；`--recall-only` 为只读模式，不写 qmap、会话缓存、索引或 L1 历史，也不改 case/meta。`fix:` 行是**候选答案**（`candidate, sim=..`）——相似症状不同根因时不得无条件直答。
- 反馈与维护：`--rate good|bad <case-id>`；`--stats` / `--gc` / `--delete` / `--rebuild`。多进程共享同一 `--mem-dir` 受支持（写入有互斥与自愈；`locks` 固定锁序契约见 `docs/locking.md`）。
- 评估：`--eval <baseline.json>`、`--gate`、`--gate-human`、`--annotate`、`--judge` 等协议见 README「评估与维护」与 `docs/eval.md`；召回路线、中文词表、存储格式与实现记录见 `docs/memory.md`。

## 纪律（HARD）
- 只读压缩结果；需要更多细节时提高 `--ratio` 或用 `--force` 重跑，不要为确认把原文再全量读一遍。
- 筛选结果只删不写；`...` 仅表示省略区间。
- 已验证修复后必须 `--store 1 --fix "..."` 回写——L1 直答依赖 fix 覆盖；不带 `--fix` 的 warning 不得静默忽略。
- 查询无命中时按原文处理（`mode=failopen:zero-signal`），不自行改写。

## 构建与来源（自举，零同步）
Rust 零依赖、离线构建、无平台分支；已复检 macOS arm64 / macOS x86_64 / Linux x86_64。架构：筛选引擎是纯函数库（`scx::compress`，零 I/O），CLI 仅是外壳（`scx::ops` 编排），记忆库独立为 `scx::memory` 模块；MCP server 无条件编译进主二进制（`scx mcp`），可选 feature 仅 `semantic`（手写打分器 + SHA256 权重校验），均不引入第三方 crate；任何程序可直接复用库。详细命令见 README.md。
