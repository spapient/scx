---
name: scx
description: 本地查询感知上下文压缩器（Rust，跨平台自举，v0.4.1+ 记忆库）。对长日志、命令输出、diff、错误堆栈、大文件按当前问题逐行筛选证据；证据行永不丢弃，可选内容只在种子邻域区域内填充，查询无命中时原样保留；保留行可加原文行号前缀；附带本地记忆知识库（--store/--recall/--eval，零依赖离线，L1 直答优先）；输出 compressed: 统计行与风险审计。凡预期输出很大的命令、读大文件、长粘贴、检索结果，先经它再进上下文。
---

# scx — 本地证据筛选压缩（配置加工具自研方案，Rust v0.4.1）

## 何时用（触发条件）
- 命令预期输出超过约 200 行：构建、测试、跑批、抓包转储。
- 要读的文件超过约 500 行，而问题只关心其中相关片段。
- 错误堆栈、git diff、检索结果、外部粘贴进入上下文之前。

## 怎么用（两步：先落盘，再压缩）
```bash
# 1) 大输出先落盘，不要让整段进入上下文
<命令> > /tmp/out.txt 2>&1
# 2) 按当前问题的关键词压缩，只把压缩结果读进来；入口是技能目录 bin 下的自举脚本
#    Windows：      "%USERPROFILE%/.codex/skills/scx/bin/run.cmd" --query "问题关键词" --ratio 0.35 /tmp/out.txt
#    macOS / Linux： ~/.codex/skills/scx/bin/run --query "问题关键词" --ratio 0.35 /tmp/out.txt
#    路径按实际安装位置替换：装于 Claude 时把 .codex 换成 .claude（Windows 为 %USERPROFILE%/.claude/skills/scx）。
#    首次调用自动识别本机并构建；权限不足时用 sh <技能目录>/bin/run 或 chmod +x。
```

参数（两平台一致）：
- `--ratio`：可选内容的 token 预算占输入的比例（默认 0.35）。证据行与种子 ±context 最小可读邻域不受预算限制。
- `--context N`：证据行的最小可读邻域半径（默认 1，0 到 4）。**0 为针尖模式：完全跳过可选填充，只保留证据行与省略标记**（风险 HIGH 且 risk-gate 开时另含失败级错误行与事故标记行）；无证据可保留时回退原文（`mode=passthrough:no-kept`）。token 极紧时使用，削减率显著提升且证据 100% 保留；日常阅读用默认 1。
- `--window N`：种子区域扩展与邻域权重半径（默认 2，0 视为 1，上限 8）；邻域权重只影响排序，独立入选必须来自真实得分。
- `--force "子串A|子串B"`：强制保留匹配行（视为证据行），大小写不敏感字面子串，可重复。
- `--lineno 1`（默认 0）：筛选模式保留行输出附加原文行号前缀（grep -n 风格 `N: `）；passthrough/fail-open 返回原文时不附加。多文件/长日志定位原行号时使用。
- `--risk-gate 1`（默认开）：风险审计为 HIGH 时额外保护失败级错误行（中英文；warn/警告 级除外）与事故标记行（root cause / fix hint / mitigation / recommendation / action / assertionerror，行首、大小写不敏感）（统计行 `guarded=1`）。
- `--json`：stderr 追加 `scxjson: {...}`（机器可读统计）。`--stat`：只输出统计行，不输出正文；`--no-stats`：只输出正文，不输出统计行（二者互斥）。
- `--version` / `--elide 0` / 小输入自动 passthrough（**同时**满足 < 40 行且 < 1200 token 估算）。`--ratio` 必须是 0.02 到 1.0 的有限数；未提供 `--query` 时必须至少提供一个非空 `--force`；单次输入上限 64 MiB（多文件合计），超出需分段。

## 记忆知识库（RAG，v0.4.1 之上的 P1–P4.3 增量；默认关闭，零依赖离线确定性）

- `--store 1`：把本次压缩的证据块沉淀为记忆条目（默认不存，需显式开启；`--mem-dir DIR` 指定库目录，缺省 `~/.scx-memory`）。可配 `--fix "修复结论"`：把"这次怎么解决"随 case 入库；之后任何命中召回（主流程 --recall 与 --recall-detail 均**无条件**输出 `fix:` 行，仅需 case 带结论）即 L1 本地直答，不再问模型。存储结果 stderr 报告 `scx: stored case=<完整 id> added=yes|dedup`（可直接 --rate/--delete 闭环）；query 零命中（failopen，>小输入直通阈值时）会警告 `note: ... low-signal memory`（不拒绝，原文可作底料刻意沉淀）。
- `--recall K`：旁路召回最近 K 条相关经验（`mem:` 段，绝不进入正文）；`--recall-only` 只召回（**查看模式：只读——不写 qmap/L1 历史/缓存，不改变库状态**；输出真实 top-k，不做 L1 过滤，方便调参/标注/演示）；`--recall-detail` 追加 `summary:` 要点行（error_code + file_loc + 错误行）。主流程 `--recall` 的 mem 段仍按 L1 置信判据直答输出（学习路径：写入 qmap/L1 历史）。
- `--rate good|bad <case-id>`：反馈标注（状态机 solved/regressed + 质量分 + 词级调权 + L1 基线自适应；bump 库代际 → 同 query 后续召回立即看到新排序）。
- `--rebuild` / `--gc` / `--delete <case-id>`：库维护（重建索引缓存（不再改召回语义）/ 过期清理 / 删除条目，id 白名单校验）；`--stats`：库盘点（case 数/桶分布/状态分布/fix 覆盖率/质量均值/磁盘 KiB，豁免 --query）。case id 为 `<日期>_<16 位 hex>`。
- `--eval <baseline.json>`：冻结标注集重放，输出三层（literal/paraphrase/semantic）A/B 对拍表（A=完整统计管线 vs B=朴素 BM25）+ `l1:` 直答命中率；`--eval-verbose`（需配 --eval）追加逐 case 诊断：A/B 各 top-10 候选的 rank/sim/score/case 尾 8 位 + `[hit]/[miss]`——标注者可见"召回为什么对/错"，直接服务 M4 证据门校准；`--l1-hist`：运行期 [l1] 命中分布出表。标注采集见 `docs/eval-baseline-template.md`。
- 召回路线：自研统计向量（二值词面匹配 + 长度归一化 + 结构化字段加权 + 时间衰减 + MMR + 反馈调权 + 实体边一跳；打分词表去重、tf 恒 1，A/B 评估按此口径引用，不称 BM25），非 embedding；USearch/embedding 按触发条件留作可选 feature 默认关。设计依据 `docs/memory-rag-design.md`，计划 `docs/rag-implementation-plan.md`。

统计行字段：`kept/total`、`chars`、`dup`、`seeds/evidence`（恒等，可机械断言）、`risk`、`guarded`、`regions`（种子区域数，0 表示回退全图排序）、`window`（规范化后的值）、`terms`、`forced`、`mode`。

## 纪律（HARD）
- 只读压缩结果；不要为了确认把原文再全量读一遍。需要更多细节时：提高 `--ratio` 或用 `--force` 重跑。
- 筛选模式保留行是原文逐字，只删不写；passthrough/fail-open 返回输入原文；`...` 单行仅表示省略区间（含尾部省略）。
- 末尾 `compressed: ...` 统计行仅作核对，不是正文，不要转述给用户。
- 查询无命中会原样保留（`mode=failopen:zero-signal`），按原文正常处理即可。

## 构建与来源（自举，零同步）
Rust 零依赖、离线构建、无平台分支；已复检 macOS arm64 / macOS x86_64 / Linux x86_64 三目标 cargo check。
- 架构：筛选引擎是纯函数库（`scx::compress`，零 I/O），CLI 仅是外壳；任何程序可直接复用库；记忆库独立为 `scx::memory` 模块（store/recall/feedback/eval/edge/stats 等 20+ 文件，各 <500 行）；`cargo test` 当前 73 库 + 8 CLI + 4 集成 + golden 27 例差分门（81 个基线文件），覆盖不变量、fail-open、空结果回退、`--force` 重复行、配置边界、CJK、ANSI/CR 分析、输入上限边界、CRLF、确定性、行号前缀、记忆往返/去重/衰减/桶过滤/坏段自愈/评估重放/库盘点与 27 用例逐字节回归门（有意变更语义时重冻：`cargo test --test golden update_baselines -- --ignored`）。
- 入口脚本自动检测：二进制缺失或源码更新时自动构建；跨机器只需携带技能目录文本与源码，二进制永远各平台本机构建（需要 Rust；macOS 用 brew/rustup，Windows 用 rustup）。详细命令见 README.md。
