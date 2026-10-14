---
name: scx
description: 本地查询感知上下文压缩器（Rust，跨平台自举，v0.3 种子区域门控）。对长日志、命令输出、diff、错误堆栈、大文件按当前问题逐行筛选证据；证据行永不丢弃，可选内容只在种子邻域区域内填充，查询无命中时原样保留；输出 compressed: 统计行与风险审计。凡预期输出很大的命令、读大文件、长粘贴、检索结果，先经它再进上下文。
---

# scx — 本地证据筛选压缩（配置加工具自研方案，Rust v0.3）

## 何时用（触发条件）
- 命令预期输出超过约 200 行：构建、测试、跑批、抓包转储。
- 要读的文件超过约 500 行，而问题只关心其中相关片段。
- 错误堆栈、git diff、检索结果、外部粘贴进入上下文之前。

## 怎么用（两步：先落盘，再压缩）
```bash
# 1) 大输出先落盘，不要让整段进入上下文
<命令> > /tmp/out.txt 2>&1
# 2) 按当前问题的关键词压缩，只把压缩结果读进来；入口是技能目录 bin 下的自举脚本
#    Windows：      "%USERPROFILE%/.claude/skills/scx/bin/run.cmd" --query "问题关键词" --ratio 0.35 /tmp/out.txt
#    macOS / Linux： ~/.claude/skills/scx/bin/run --query "问题关键词" --ratio 0.35 /tmp/out.txt
#    首次调用自动识别本机并构建；权限不足时用 sh ~/.claude/skills/scx/bin/run 或 chmod +x。
```

参数（两平台一致）：
- `--ratio`：可选内容的 token 预算占输入的比例（默认 0.35）。证据行与种子 ±context 最小可读邻域不受预算限制。
- `--context N`：证据行的最小可读邻域半径（默认 1，0 到 4）。**0 为针尖模式：完全跳过可选填充，只保留证据行与省略标记**（风险 HIGH 且 risk-gate 开时另含失败级错误行与事故标记行）；token 极紧时使用，削减率显著提升且证据 100% 保留；日常阅读用默认 1。
- `--window N`：种子区域扩展与邻域权重半径（默认 2，0 视为 1，上限 8）；邻域权重只影响排序，独立入选必须来自真实得分。
- `--force "子串A|子串B"`：强制保留匹配行（视为证据行），大小写不敏感字面子串，可重复。
- `--risk-gate 1`（默认开）：风险审计为 HIGH 时额外保护失败级错误行（中英文；warn/警告 级除外）与事故标记行（root cause / fix hint / mitigation / recommendation / action / assertionerror，行首、大小写不敏感）（统计行 `guarded=1`）。
- `--json`：stderr 追加 `scxjson: {...}`（机器可读统计）。`--stat`：只输出统计行，不输出正文。
- `--version` / `--elide 0` / 小输入自动 passthrough（< 40 行或 < 1200 token 估算）。

统计行字段：`kept/total`、`chars`、`dup`、`seeds/evidence`（恒等，可机械断言）、`risk`、`guarded`、`regions`（种子区域数，0 表示回退全图排序）、`window`、`terms`、`forced`、`mode`。

## 纪律（HARD）
- 只读压缩结果；不要为了确认把原文再全量读一遍。需要更多细节时：提高 `--ratio` 或用 `--force` 重跑。
- 保留行是原文逐字；本工具只删不写（`...` 单行仅表示省略区间）。
- 末尾 `compressed: ...` 统计行仅作核对，不是正文，不要转述给用户。
- 查询无命中会原样保留（`mode=failopen:zero-signal`），按原文正常处理即可。

## 构建与来源（自举，零同步）
Rust 零依赖、离线构建、无平台分支；已复检 macOS arm64 / macOS x86_64 / Linux x86_64 三目标 cargo check。
- 架构：筛选引擎是纯函数库（`scx::compress`，零 I/O），CLI 仅是外壳；任何程序可直接复用库；`cargo test` 含不变量、fail-open、确定性三个单测。
- 入口脚本自动检测：二进制缺失或源码更新时自动构建；跨机器只需携带技能目录文本与源码，二进制永远各平台本机构建（需要 Rust；macOS 用 brew/rustup，Windows 用 rustup）。详细命令见 README.md。
