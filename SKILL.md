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

- `--store 1`：把本次压缩的证据块沉淀为记忆条目（默认不存，需显式开启；`--mem-dir DIR` 指定库目录，缺省 `~/.scx-memory`）。可配 `--fix "修复结论"`：把"这次怎么解决"随 case 入库；**不带 --fix 时 stderr 警告**（L1 无法回答"上次怎么解决"）；`--fix-file <path>` 读多行修复结论（与 --fix 互斥、必须配 --store 1）。之后任何命中召回（主流程 --recall 与 --recall-detail 均**无条件**输出 `fix:` 行，仅需 case 带结论；fix 定位为**候选答案**：`fix: <内容> (candidate, sim=..)`——相似症状不同根因时不得无条件直答）即 L1 本地直答，不再问模型。**证据行区间 + 原始证据自动入库**（优化⑤：压缩保留行的原始行号 evidence_lno + 原文 evidence，无省略标记/行号前缀；passthrough/failopen 无保留行则仅存压缩快照）——case 四要素：query + 证据行区间 + 原始证据 + 修复结论。存储结果 stderr 报告 `scx: stored case=<完整 id> added=yes|dedup`（可直接 --rate/--delete 闭环）；query 零命中（failopen，>小输入直通阈值时）会警告 `note: ... low-signal memory`（不拒绝，原文可作底料刻意沉淀）。**入库前统一脱敏**（P1-4：password/passwd/token/Authorization/Bearer/URL 用户信息等 → `<redacted>`，幂等多轮收敛；fingerprint 用脱敏后 query）。`--max-bytes N` 软配额：库超 N 字节时 store 拒绝（exit 2，提示先 --gc/--delete）。
- `--recall K`：旁路召回最近 K 条相关经验（`mem:` 段，绝不进入正文）；`--recall-only` 只召回（**查看模式：只读——不写 qmap/L1 历史/缓存，不改变库状态**；输出真实 top-k，不做 L1 过滤，方便调参/标注/演示）；`--recall-detail` 追加 `summary:` 要点行（error_code + file_loc + 错误行）、`evidence: <行号> <原文>` 原始证据回放（有证据的 case；旧 case 无键跳过）与 `fix:` 修复结论。主流程 `--recall` 的 mem 段仍按 L1 置信判据直答输出（学习路径：写入 qmap/L1 历史）。
- `--rate good|bad <case-id>`：反馈标注（状态机 solved/regressed + 质量分 + 词级调权 + L1 基线自适应；bump 库代际 → 同 query 后续召回立即看到新排序）。
- `--rebuild` / `--gc` / `--delete <case-id>`：库维护（重建索引缓存（不再改召回语义）/ 过期清理 / 删除条目，id 白名单校验）；`--stats`：库盘点（case 数/桶分布/状态分布/fix 覆盖率/质量均值/磁盘 KiB，豁免 --query）。case id 为 `<日期>_<16 位 hex>`。
- `--eval <baseline.json>`：冻结标注集重放，输出三层（literal/paraphrase/semantic）A/B 对拍表（A=完整统计管线 vs B=朴素 BM25）+ `l1:` 直答命中率；报告含 **bootstrap 95% CI**（固定 seed 可重放，r@5+MRR 预声明主指标）+ multi-rel/single-rel 计数（NDCG 判别强度可见：单相关条目 NDCG 接近二值）；`--eval` 遇未裁决条目（judge:null）直接报错。`--eval-verbose`（需配 --eval）追加逐 case 诊断：A/B 各 top-10 候选的 rank/sim/score/case 尾 8 位 + `[hit]/[miss]`——标注者可见"召回为什么对/错"，直接服务 M4 证据门校准；`--l1-hist`：运行期 [l1] 命中分布出表。标注采集见 `docs/eval-baseline-template.md`。
- `--eval --gate`：机械资格门——总组数 ≥30、literal/paraphrase/semantic 各 ≥10、非 literal 必须有 `--judge` 写入的 by+date、每层主指标与 MRR 不低于 B；全 2 标签仍会 PASS（只证明目标排序资格，不代表人工闭环）。`--eval --gate-human`：人工弱相关/负例闭环门——每条 case 都必须有非机械 by+date，全语料至少 10 条 `rel=1` + 10 条 `rel=0`，至少两层同时含两类控制，每层 NDCG@10 A≥B，provenance 不得为 `policy/materialized/rubric`，且 **`rel` 的每个键都必须在 `judge_items` 有独立真人条目、`judge.method` 为 `per-item-*`**（整包签名不算），且 literal 目标必须为 2（防分数与行错位）；任一失败 `gate: FAIL` exit 2。实测：confirm5 机械 target `--gate` PASS、`--gate-human` FAIL；policy 控制集 FAIL（per-item 0/1430）；real（0/344）、v2（0/821）在新门下也 FAIL——它们只有案例级签名或未入库的裁决日志，不能写成独立盲审。confirm5 已冻结 40/150 逐候选盲审包（384 项）。`--gate-items` 允许非人类裁决人（AI 记 `judge.by=codex`、`method=per-item-ai-v1`），PASS 仅代表 E3 逐条裁决；实测 confirm5 AI 裁决 384/384、`--gate-items` PASS；人工 hao 384/384 的 `--gate-human` 因 paraphrase/semantic NDCG A<B FAIL（0.779/0.785、0.819/0.825）。
- `--annotate`（智能标注器，P4.4+ 优化⑥）：**生成 M4 回归标注，不替代独立人工质量证据**——从库中真实 case 生成三层标注（literal 原样 / paraphrase 确定性同义词改写 / semantic 根因行词构造），**每条变体先用 A 管线自检（目标 case 必须进 top-k）才放行**，证据链（fingerprint / synonym / root-cause / recall sim-score-rank）写入 baseline.json（`--annotate-out` 指定路径，缺省 `<mem-dir>/baseline-auto.json`）；`--seed N` 固定确定性可重放；`--annotate-max N` 控制覆盖 case 数（缺省 16，0=全库）。stdout 同时输出 `dropped`（自检未放行）与 `skipped`（无同义词/无根因）漏斗，避免只报放行集。生成后直接 `--eval` 该 baseline 出回归表；它不能单独证明 A>B（自检使用 A，存在选择偏差）。
- 召回一致性（P4.4+ 优化⑦）：候选桶由原始 query 判定、PRF 第二轮继承同桶（扩展词不得改变过滤语义）；PRF 只统计词面相关（sim>0）case 的词、且**补漏不覆盖**（两轮候选合并去重重排）；标注自检按相关度分层（semantic 弱相关放宽 top-10）；评估报告 semantic 层用 r@10 口径。**M4 口径修正**：自动回归集可复现（24-case → 36 组：literal 24 / paraphrase 9 / semantic 3），但独立质量证据未通过——24/9/3 不满足各层 ≥10；自检只放行 A 能召回的变体；literal MRR/NDCG A 低于 B；paraphrase r@5 领先但 n=9 无显著性。见 `docs/m4-evidence-report.md`，复现脚本 `scripts/m4-eval.ps1`。
- M4 独立证据路径（P4.4+ 优化⑩，2026-10-22）：①**literal 退化修复**——`--eval-verbose` 定位退化例 `disk io read timeout`（A rank3 vs B rank1：PRF 扩展轮 q2 口径 score 与首轮混排，扩展词把无关 case 抬到 6.00 压过目标 4.57）；修复 = 合并后**不重排**：首轮顺序权威、扩展新增按 q2 顺序追加尾部（扩展词只补漏、永不改变首轮排序）。修复后自动口径 literal A=B=1.000、paraphrase A 反超（r@5 1.0 vs 0.889、mrr 0.833 vs 0.759、ndcg 0.877 vs 0.792）。②**`--annotate-raw` 盲测模式**（与 `--annotate` 互斥）：**不过自检**输出全变体集（含自检本会丢弃的变体 = 真阴性/假阳性对照），每条带盲测摘要 `note: target=<hash16> hint=[原文首行] fix=[修复结论]`，供人工**盲视裁决** rel（不看任何 A/B 分数）→ 改后 `--eval` 即独立证据（A/B 平等面对全集，选择偏差消除）。③**`--judge [--judge-by NAME]` 交互裁决链**（P0-1，与 --annotate/--annotate-raw/--eval 互斥）：逐条打印 `[i/n] layer=.. query=".."` + 盲测摘要（**零 A/B 分数**），stdin `2/1/0/s`（s=跳过）裁决；EOF 中止不写回；跳过条目保持 judge:null，`--eval` 仍拒绝。**2026-10-23 real-eval 机械门实测 PASS（40 组 14/13/13，judge=fei，exit 0）；但 40/40 rel=2、三层 A=B，只能写“不劣于 B”，不能写“A 优于 B”或“质量优势通过”**；SOP 见 `docs/m4-evidence-report.md` §9/§11。
- 2026-10-24 人工门复跑：confirm5 机械 target `--gate` PASS、`--gate-human` FAIL（全 2 / 机械 judge）；`real-baseline-arb`（151/46，主指标 A=B，NDCG A>B）与 `v2-baseline-arb`（312/77，paraphrase r@5 0.917 vs 0.867、semantic MRR 0.696 vs 0.484、hard ΔMRR +0.0772，p=0.0022）旧门 PASS，但逐候选门禁收紧后均 FAIL（缺 `judge_items`）；`confirm5-baseline-human` FAIL（115/665，policy 物化 provenance）。confirm5 的 policy 控制集只作替代敏感性分析，不冒充逐条盲审；独立人工弱相关/负例真实事故闭环仍未建立，盲审包见 `eval-real/confirm5-blind40-preregistration.md`。
- 并发、损坏自愈与身份（P4.4+ 优化⑧⑨）：`meta.toml`/qmap/index 的跨进程写用锁文件串行，原子写使用唯一临时名；同指纹 store、同 case 的 rate/去重/delete 也有互斥，8 进程并发 store 实测 8/8 且 case_count/gen 不丢。qmap 半写残尾会在下一次 append 前截断修复，坏 header 自动重建；倒排解析校验长度上界，超长 token（>255B）在词表层统一丢弃。case 去重、qmap、会话缓存对 query 保留数字（8080/9090 不互串），text 仍折叠 PID/时间戳噪声。多进程共享 `--mem-dir` 是受支持场景；空 fix/evidence 可由后续 store 补录，已有结论不覆盖。
- 隐私与保留（P4.4+ 优化⑫，P1-4）：入库统一脱敏（见 --store）；`--gc` 过期清理**豁免带 fix 的 case**（已沉淀"怎么解决"的经验价值最高，regressed 仍按短 TTL 清）；recall 出屏（summary/evidence/fix）控制字符转义（`\x1b`/C0 → 可见 `\xNN`，防终端注入）；`--delete` id 白名单（禁路径穿越）。
- 召回路线：自研统计向量（二值词面匹配 + 长度归一化 + 结构化字段加权 + 时间衰减 + MMR + 反馈调权 + 实体边一跳；打分词表去重、tf 恒 1，A/B 评估按此口径引用，不称 BM25），非 embedding；USearch/embedding 按触发条件留作可选 feature 默认关。**中文召回词表**（P4.4+ 优化⑪，P1-3）：memory 侧统一 `build_mem_terms`——CJK 段 ≤6 字块整段 + 重叠 bigram/trigram（>6 字切块后逐块 n-gram），中文释义变体可桥接（`连接池满了` → 命中 `数据库连接池耗尽`）；实体边表进程内缓存（root+meta.toml mtime 键，10k 库召回 p50 ~1.3ms）。设计依据 `docs/memory-rag-design.md`，计划 `docs/rag-implementation-plan.md`，实施记录 `docs/rag-implementation-log.md`。

统计行字段：`kept/total`、`chars`、`dup`、`seeds/evidence`（恒等，可机械断言）、`risk`、`guarded`、`regions`（种子区域数，0 表示回退全图排序）、`window`（规范化后的值）、`terms`、`forced`、`mode`。

## 纪律（HARD）
- 只读压缩结果；不要为了确认把原文再全量读一遍。需要更多细节时：提高 `--ratio` 或用 `--force` 重跑。
- 筛选模式保留行是原文逐字，只删不写；passthrough/fail-open 返回输入原文；`...` 单行仅表示省略区间（含尾部省略）。
- 末尾 `compressed: ...` 统计行仅作核对，不是正文，不要转述给用户。
- 查询无命中会原样保留（`mode=failopen:zero-signal`），按原文正常处理即可。
- 记忆写入纪律（P4.4+ 优化⑫，P0-2）：**已验证修复后必须 `--store 1 --fix "..."` 回写**——L1 直答的北极星依赖 fix 覆盖；`--store` 不带 --fix 会有 warning，不要静默忽略。

## 构建与来源（自举，零同步）
Rust 零依赖、离线构建、无平台分支；已复检 macOS arm64 / macOS x86_64 / Linux x86_64 三目标 cargo check。
- 架构：筛选引擎是纯函数库（`scx::compress`，零 I/O），CLI 仅是外壳；任何程序可直接复用库；记忆库独立为 `scx::memory` 模块（store/recall/feedback/eval/edge/stats 等 20+ 文件；历史门禁为单文件 <500 行，当前 `cli_args.rs`/`eval.rs`/`recall.rs`/`annotate.rs` 已超，拆分列为待办）；`cargo test` 当前 140 库 + 9 CLI + 5 集成 + 2 golden 活跃（1 个重冻 ignored），覆盖不变量、fail-open、空结果回退、`--force` 重复行、配置边界、CJK、ANSI/CR 分析、输入上限边界、CRLF、确定性/同分 id 排序/同族收紧、行号前缀、记忆往返/去重/衰减/桶过滤/坏段自愈/评估重放/rel 键去重/库盘点/并发读写/qmap 残尾修复/索引溢出/数字身份、脱敏幂等/控制字符转义/gc fix 豁免、中文词表桥接、盲测裁决链/bootstrap CI/`--gate` 机械资格门 + `--gate-human` 人工闭环门 + 逐候选 `judge_items` 溯源（含绝对地板/占位裁决/控制标签/NDCG/provenance 硬失败）与 29 用例逐字节回归门（有意变更语义时重冻：`cargo test --test golden update_baselines -- --ignored`）。
- 入口脚本自动检测：二进制缺失或源码更新时自动构建；跨机器只需携带技能目录文本与源码，二进制永远各平台本机构建（需要 Rust；macOS 用 brew/rustup，Windows 用 rustup）。详细命令见 README.md。
