# scx 记忆知识库（RAG）实施日志

> P4.4+ 优化②起每项优化/整改的实施记录（代码笔+golden+提交 hash+E2E 验证）。
> 计划主体（目标/里程碑/阶段门/语料/验证体系/评审处置）见 `rag-implementation-plan.md`。
> 代码/文档分笔提交纪律：本条日志只记录事实，不替代 git 历史。

## 15. P4.4+ 优化②实施记录（2026-10-22，已提交）

| 项 | 内容 | 验证 |
|---|---|---|
| 主流程直答 | `write_mem_lines` 的 `fix:` 行改为**无条件输出**（case 带 fix 即出，不依赖 `--recall-detail`）——L1 北极星"本地直答"在默认路径生效 | 端到端：`--query ... --recall 1`（无 detail）输出 `mem:` + `  fix: enlarge pool to 64 conns, restart daemon`；旧 case 无 fix 不输出 fix 行（golden 零漂移） |
| `--stats` 库盘点 | 新建 `rs/src/memory/stats.rs`（144 行）：`stats: cases=N gen=M` + `bucket:`/`status:`（key 字典序）/`fix: n/N (pct)`/`quality: mean=`/`disk: X.X KiB`（case 目录递归大小）；`mod.rs` 门面 `stats_table`；`cli_args --stats` 豁免 `--query`（含单测）；`main.rs` 接线（--l1-hist 后、--rate 前） | 单测 `table_reports_counts_and_coverage`（ingest + ingest_with_fix 造 2 case）；golden `memory_stats` 第 27 例（db + disk 两桶隔离日志，fix 1/2）；端到端 3 case 实测 |
| 桶判定精度 | stats 单测暴露裸 "db" 子串误判：`Db.java:4` 文件实体被归 db 桶 → compile 场景误归类、query "compile fail" 漏召。修复：FAMILIES db 家族**移除裸 "db" 子串**；`bucket_of` 对 terms 词级 `t == "db"`、`candidate_buckets` 对 `split_whitespace` 词级 `w == "db"`（宁多勿漏但别乱扩桶——"kubectl/obd" 不误扩） | bucket.rs 单测：`bucket_of(["db.java:4"], "compile fail") == "build"`、`candidate_buckets("kubectl get pods")` 不含 db；干净 compile 场景端到端 `bucket: build=1` |

**门禁终态**：73 库 + 8 CLI + 4 集成 + golden 27 例全绿；fmt/clippy 0；行数门禁（stats.rs 144 / bucket.rs 160 行）通过。
**遗留口径**：failopen 全文保留会把无关家族词带进 case.text → 桶归属随压缩结果变化（与 §11 评审④"text 是压缩结果"边界一致，非 bug）。

## 16. P4.4+ 优化③实施记录（2026-10-22，已提交）

| 项 | 内容 | 验证 |
|---|---|---|
| stored 报告 | `ingest`/`ingest_with_fix` 返回类型 `Result<bool>` → `Result<(bool, String)>`（新增标志 + case id；去重时返回既有 id）；门面 `store_case`/`store_case_fix` 透传；同步路径（cli_mem::store）与异步路径（main.rs 后台线程 tx 类型 `io::Result<(bool, String)>`）均 stderr 报告 `scx: stored case=<完整 id> added=yes|dedup`——用户拿到完整 id 可直接 `--rate`/`--delete` 闭环 | 端到端：命中 store `added=yes`、重复 store 同 id `added=dedup`、stdout 正文/统计零污染；golden 漂移仅 `memory_store.err` +1 行（最小） |
| failopen 低信号守卫 | 主流程 store 分支：`o.mode.starts_with("failopen")` 时 stderr 警告 `note: query had no hits; --store will persist the full input (low-signal memory). inspect with --recall-only first`（不拒绝——用户可刻意沉淀原文作底料） | 端到端：60 行零命中（mode=failopen:zero-signal）出警告；小输入直通（passthrough:small）不误报；命中（sieve）不误报 |

**门禁终态**：73 库 + 8 CLI + 4 集成 + golden 27 例全绿；fmt/clippy 0；测试调用点零改动（ingest 直调点均为丢弃返回值）。

## 17. P4.4+ 优化④实施记录（--eval-verbose 候选诊断，2026-10-22，已提交）

| 项 | 内容 | 验证 |
|---|---|---|
| `--eval-verbose` | eval.rs `run` → `run_impl(root, path, verbose)`（默认路径零漂移）；`run_verbose` 追加逐 case 诊断段：`case i/N q="..."` + `A:`/`B:` 各 top-10（rank + case 尾 8 + sim + score + `[hit]/[miss]`，与 --eval 的 tail8 口径一致）；verbose 时缓存每例 A/B 命中，避免二次召回 | golden `memory_eval_verbose` 第 28 例（复用 memory_eval 的 3 case 库与 baseline）：paraphrase 层 A mrr=0.5 vs B=1.0 的退化即刻可见——A top1 是 disk case `5e316932…`（[miss]），相关 db case `123e7dec…` 掉到第 2——诊断输出让标注者直接校准 |
| CLI 接线 | `--eval-verbose` 参数（需配 --eval，否则报错 exit 2；含单测）；`main.rs` --eval 分支按开关走 `eval_cases_verbose`；USAGE 同步 | cli_args 单测：`--eval-verbose` 单独使用报错、配 --eval 通过 |

**门禁终态**：73 库 + 8 CLI + 4 集成 + golden 28 例全绿；fmt/clippy 0；默认 `--eval` 输出逐字节不变（memory_eval.out 未漂移）。
**服务目标**：M4 证据门的 30–50 组人工标注——标注者可逐例核对 A/B 排序差异并据此校准 rel 标注与调参假设。

## 18. P4.4+ 优化⑤实施记录（case schema v2 原始证据，2026-10-22，已提交）

**背景**：外部评审产品缺口 B——"`--store` 存的是查询条件压缩结果，无证据行区间/原始证据，L1 只给摘要不是可复用答案"。fix（修复结论）已在优化①补齐；本轮补齐原始证据，case 升级为**四要素：query + 证据行区间（evidence_lno）+ 原始证据（evidence）+ 修复结论（fix）**。

| 项 | 内容 | 验证 |
|---|---|---|
| 引擎暴露保留行号 | `Outcome` 增 `kept_lno: Vec<usize>`（筛选模式保留行的原始 1-based 行号，输出循环同步收集；passthrough/failopen 为空）；`mk` 闭包 + 4 处调用点接线 | 单测 `sieve_kept_lno_aligns_with_output_lines`（数量=kept、严格递增、含证据行号）、`passthrough_kept_lno_empty` |
| 提取与落盘 | main.rs `extract_evidence(input, lno)` 按行号取原文（与 compress 同一归一化口径：\r\n→\n、末行剔除、越界跳过）；store 同步/异步路径传 evidence；ingest 新入口 `ingest_with_evidence`/门面 `store_case_full`（原 ingest/ingest_with_fix 签名不变，30+ 调用点零改动） | 单测 `ingest_with_evidence_roundtrip_and_legacy_compat`（落盘/旧 case 无键读空/长度不一致拒绝）；端到端：81 行输入 store → case.json `evidence_lno=[1,2,10,11,12,13]`、`evidence`=原始行，`--recall-detail` 输出 `evidence: 11 ERROR db timeout...` 等 |
| 直答回放 | `--recall-detail` 在 summary 后输出 `evidence: <行号> <原文>` 行（读一次 case 同时取 evidence/fix；无证据跳过）；主流程 fix 行行为不变 | 端到端 recall-detail 输出证据回放 + fix |
| 旧 case 兼容 | case.json 新键 evidence_lno/evidence；`val_to_case` 缺键读空 | golden memory_summary/memory_fix（旧库预置 case 无 evidence）**零漂移**——兼容实证 |
| 存储代价 | evidence 总长由保留行决定（≤ 输入）；golden memory_stats disk 1.3→1.4 KiB（新 case 含证据字段，1 行预期漂移） | golden 重冻 28 例全绿 |

**门禁终态**：76 库 + 8 CLI + 4 集成 + golden 28 例全绿；fmt/clippy 0；release ✓。
**语义边界**：passthrough/failopen 无保留行 → kept_lno 空 → 仅存压缩快照（text），低信号记忆语义不变（§16 口径）；`evidence` 是"保留行的原始文本"，与压缩结果同一行集，非全量输入原文。

## 19. P4.4+ 优化⑥实施记录（智能标注器 --annotate，2026-10-22，已提交）

**背景**：M4 证据门（30–50 组人工标注）长期未产出，是唯一未闭环的评审项。用户要求以"顶级智能标注"替代人工——实现**零依赖智能标注器**：不凭空造标注，每条 query 变体先经召回管线自检（目标 case 必须进 top-5），证据链随标注落盘。

| 项 | 内容 | 验证 |
|---|---|---|
| 三层生成 | `annotate::annotate(root, seed, max_cases)`：literal=case.query 原样（rel=2，证据 `fingerprint`）；paraphrase=seed 驱动的 SYN 同义词表字面替换（词边界保护，~45% 概率逐词 + 强制保底替换保证 ≠ 原 query），自检放行 rel=2/1（sim≥0.5 分档），证据 `synonym`+`recall sim/score/rank`；semantic=根因行（Root cause/cause/原因）词反译构造（与 query token 几乎不共享），自检放行 rel=1 | 单测 4：三层自检证据在链上、baseline roundtrip 进 eval、同 seed 逐字可重放、词边界替换不误伤（"db"→database 不碰 database 内嵌） |
| 自检 | `self_check` 走 `recall::recall`（纯检索零写入）top-5 查目标 tail16；未放行变体显式丢弃（dropped 计数），宁缺毋滥不编造 | 端到端：2 case 库 → 6 组（2×3 层），semantic query "root cause: 连接池 排查原因" 与 literal 零共享词 |
| CLI 接线 | `--annotate`（豁免 --query）、`--annotate-out <path>`（缺省 `<mem-dir>/baseline-auto.json`）、`--seed N`（缺省 0，确定性可重放）；`--annotate` 禁与 --eval/--l1-hist 同用 | cli_args 单测沿用；端到端：annotate → baseline → `--eval baseline-auto.json` 闭环出对拍表（A/B 全 1.0） |
| 输出 | baseline.json 兼容 eval 解析（extra 键 evidence/note 被忽略）；stdout 报告层分布/rel 分布 + out 文件名（相对，防跨机漂移）；expected 用 hash16 免日期漂移 | golden `memory_annotate` 第 29 例冻结：`cases=6 rel2=2 rel1=4`、layer literal/paraphrase/semantic=2、`out: baseline-auto.json` |

**门禁终态**：80 库 + 8 CLI + 4 集成 + golden 29 例全绿；fmt/clippy 0；release ✓。
**服务目标**：M4 证据门自动产出 30–50 组（16 case × 3 层 = 48 组），每条带证据链可审计；`--seed` 变化生成不同变体池，多 seed 融合可扩充。人工标注流程（docs/eval-baseline-template.md）保留作复核/抽样。

## 20. P4.4+ 优化⑦实施记录（召回一致性修复 + M4 自动回归集结论，2026-10-22，已提交）

**背景**：正式 M4 评估（优化⑥ 标注器 + --eval-verbose）暴露 A 管线劣于朴素 B 的 3 个真实缺陷——
①桶硬截断 + PRF 扩展词带偏桶判定（真目标被滤出候选集）；②PRF 扩展词统计含词面无关 case 的私有词；
③PRF 覆盖首轮结果（实体边桥入候选被降级）。另暴露标注器 2 个工程缺陷（CJK 切片 panic、semantic 自检误杀）与 1 个规模缺口（max_cases 不可调）。

| 项 | 内容 | 验证 |
|---|---|---|
| 桶一致性 | `recall()` 对原始 query 判候选桶一次；`score_pass` 接收 `cand/use_buckets` 参数；PRF 第二轮继承同桶——扩展词不再改变过滤语义 | 端到端：case `command not found kubectl`（other 桶）A 目标回归 top-1（修复前被 network 桶误滤）；golden memory_eval/memory_eval_verbose 重冻 |
| PRF 相关度门槛 | `prf::expand(root, hits, ...)` 改收 `&[RecallHit]`，仅统计 sim>0（词面相关）的 case——无关 case 私有词（bash/deploy 等）不再入扩展表 | prf 单测改签名适配；端到端 case `tls handshake 超时` A 目标回归（修复前扩展词带出 build 桶） |
| PRF 补漏不覆盖 | 扩展轮候选并入首轮（去重）按 score 重排截断——首轮命中（含实体边桥入、字段加权命中）永不降级 | edge 单测 `edge_bridge_recovers_cross_bucket_sibling` 修复前失败、修复后绿（桥入 B 保持 top-3） |
| 标注分层放行 | `self_check(root, q, c, k)` 加 k 参数：literal/paraphrase top-5、semantic（弱相关 rel=1）top-10 | annotate 单测沿用；端到端 semantic 放行 16→3（24 case 库，无根因行/error_code 的 case 不造语义变体=诚实边界） |
| eval 弱相关口径 | `metric` 返回 (r5, r10, mrr, ndcg)；聚合 [f64;9]；报告行 semantic 层显示 r@10 | eval 单测 `metric_recall_mrr_ndcg` 更新 4 元组；golden memory_eval 重冻（semantic 行 r@10） |
| CJK 切片安全 | `replace_ci` 边界检查改 `s[..pos].chars().next_back()`（pos 是 char 边界，防中文 query 字节切片 panic） | 新增单测 `replace_ci_safe_on_cjk_boundary`（"数据库连接池耗尽 pool" 替换"连接池"不 panic） |
| 标注规模可调 | CLI `--annotate-max N`（缺省 16，0=全库；annotate 内部 0→ids.len） | 24 case 库 `--annotate-max 0` 产 36 组落 30–50 窗口 |

**M4 自动回归集结论（36 组，24-case 可复现语料）**：
| 层 | n | A | B | 判定 |
|---|---|---|---|---|
| literal | 24 | r@5=1.000 mrr=0.958 ndcg@10=0.969 | r@5=1.000 mrr=1.000 ndcg@10=1.000 | r@5 持平；**MRR/NDCG A<B** |
| paraphrase | 9 | r@5=1.000 mrr=0.778 ndcg@10=0.836 | r@5=0.889 mrr=0.778 ndcg@10=0.807 | r@5 A>B；MRR 持平，n=9 无显著性 |
| semantic | 3 | r@10=1.000 mrr=0.833 | r@10=1.000 mrr=0.833 | 持平；n=3 不作质量结论 |
| l1 | 33 | hits=29/33 rate=0.879 | — | 学习闭环可用 |

**漏斗（本次 `--annotate` 实报）**：literal 24；paraphrase 9（14 个无 SYN 可替换词、1 个自检未进 top-5）；semantic 3（21 个无根因行/error_code、0 个自检失败）。24/9/3 不满足 §8“各层 ≥10”，literal 占 66.7%。

**口径**：自动标注先经 A 管线自检放行，B 不参与；因此该表是“A 已成功子集上的回归表”，不是独立 A/B 质量证据。literal MRR/NDCG A 低于 B，不能写“A≥B 全面成立”；paraphrase 的 r@5 优势是唯一正向信号，但样本量不足且无置信区间。

**门禁终态**：89 库 + 8 CLI + 4 集成 + golden 29 例全绿；fmt/clippy 0；release ✓。
**报告**：docs/m4-evidence-report.md（含方法学限制与 `scripts/m4-eval.ps1` 复现命令）。

## 21. P4.4+ 优化⑧实施记录（并发写 / 数字身份 / 证据口径，2026-10-22）

| 项 | 内容 | 验证 |
|---|---|---|
| 跨进程原子写 | 新增 `memory/lock.rs`：唯一临时名 `<file>.tmp.<pid>.<seq>` + rename（Windows sharing violation 有限重试）；`meta.toml` 的 gen/case_count/word_w/l1/params、qmap 追加/合并、postings 构建全部加锁（`meta.lock`/`qmap.lock`/`index.lock`） | 单测 `concurrent_case_writes_are_lossless`（8 线程 × 4 case：32 case、case_count=32、gen=32）；12 进程并发 store 12/12 exit 0、case_count=12、gen=12 |
| 数字身份 | `norm_identity_into` 保留 query 数字；case 去重、qmap 精确键、会话缓存键改用该口径；text 仍用 `norm_into` 折叠 PID/时间戳/行号；旧 case 通过动态重算指纹兼容 | 单测 `fingerprint_keeps_query_numbers_but_folds_text_noise`、`ingest_keeps_numeric_query_variants_and_dedups_legacy_fp`、`qmap_keeps_numeric_query_identity`；端到端 `port 8080` vs `port 9090` → 2 case，各自 top-1 正确 |
| 标注漏斗 | `AnnotateReport` 增加 `paraphrase_no_synonym` / `semantic_no_root`；stdout 输出 `dropped:` 与 `skipped:` 两类，避免只报放行集 | 单测 `annotate_reports_dropped_variants`；24-case 实报 36 组 + 14 no-synonym + 1 self-check + 21 no-root |
| 证据口径 | M4 报告/README/SKILL 改为“自动回归集可复现，独立质量证据未通过”；新增 `scripts/m4-eval.ps1` 固定 24-case 语料；semantic 仍报 r@10 但明确标注为事后放宽 | 脚本复跑（优化⑨）：36 组、literal A mrr 0.958 < B 1.000、paraphrase r@5 1.000 vs 0.889、l1 29/33 |
| 索引一致性 | `write_case` 写入后 `index::invalidate`：case 新增/更新立即让旧 postings 失效，避免大库继续用旧索引漏掉新 case（索引仍可重建，属缓存） | 单测 `write_case_invalidates_cached_index`；原 `invalidate_removes_index_file` 仍绿 |

**残余**：历史“单文件 <500 行”门禁已被 `cli_args.rs`/`eval.rs`/`recall.rs`/`annotate.rs` 等超过（非本轮引入），CI 未强制；拆分列为后续。

**结论**：并发写与数字身份是已修复的真实缺陷；M4 的“工具/回归集”可用，但“质量证据”仍未闭环，下一步是独立人工标注/盲测，而不是继续用自动标注自证。

## 22. P4.4+ 优化⑨实施记录（损坏自愈 / 同 case 并发 / 索引一致性，2026-10-22）

| 项 | 内容 | 验证 |
|---|---|---|
| qmap 损坏自愈 | append 在同一把 qmap 锁内先解码已有文件：残尾截断到最后一个完整记录、坏 header 重建后再追加；解码所有切片先校验边界。超长 query（>u16）、hit 数（>u16）、case id（>u8）明确拒绝，不再静默截断字段长度 | 单测：1 字节残尾后 lookup 不 panic、下一 append 后两条均可读；坏 header 自动重建；超长字段拒绝且不破坏已有记录 |
| 索引健壮性 | `postings.bin` 的 `body_len` 使用 checked add，n_docs/n_terms/df/字段数均做文件上界检查；单 token 在 `build_terms` 统一限制 255B（线性/索引同口径），避免 doc-term u8 长度截断错位 | 单测 `parse_rejects_overflowing_body_len`、`runaway_token_is_dropped`；全部既有索引/golden 测试通过 |
| 同 case 并发 | 新增 `store::lock_case`；`rate`、同指纹去重刷新、`delete` 共用 case 锁。同指纹 store 另有 ingest 锁，避免跨秒重复落库；空 fix/evidence 可补录，已有结论不覆盖 | 单测 `concurrent_rate_same_case_does_not_lose_feedback`（8 线程×5 good=40）、`dedup_backfills_missing_fix_and_evidence_without_overwriting` |
| 索引一致性 | `write_case`/`delete_case` 在 index 锁内完成“资产变更 → 作废 postings → bump gen”；`index::invalidate` 锁失败上抛，不再静默让旧索引继续服务 | 单测 `write_case_invalidates_cached_index`、`delete_case_invalidates_index`；无环锁序 case→index→meta/qmap |
| L1 写回 | `adapt_l1_base` 改 meta 锁 + `atomic_write`，与 word_w/gen/case_count 共用同一写路径 | 既有 4 条 L1 自适应测试通过 |
| 跨进程回归 | CLI 集成测试并发 spawn 8 个 store 进程 | `concurrent_store_processes_keep_all_cases_and_meta_updates`：8/8 成功、case 目录=8、case_count=8、gen=8 |

**M4 复跑（2026-10-22，release）**：`literal A mrr=0.958 ndcg=0.969 / B=1.000/1.000`；`paraphrase A r@5=1.000 MRR 0.778 / B r@5=0.889 MRR 0.778`；`semantic r@10=1.000 / 1.000`；`l1=29/33`。自动标注漏斗 24/9/3，结论仍为回归集可复现、独立质量证据未通过。

**门禁终态**：96 库 + 8 CLI + 5 集成 + golden 29 例全绿；fmt/clippy/release 0。

## 23. P4.4+ 优化⑩实施记录（M4 独立证据路径：PRF 严格补漏 + 盲测模式，2026-10-22）

**背景**：外部评审二轮——①literal 层 A mrr/ndcg 仍低于 B（1 例退化未修）；②自动标注自检用 A 管线 = 用 A 自证（选择偏差），要求独立人工标注或盲测，不再继续 A 管线自证。

| 项 | 内容 | 验证 |
|---|---|---|
| literal 退化定位 | `--eval-verbose` 逐 case：退化例 = `disk io read timeout`，A rank3（1) sim=0.13 score=6.00 [miss] 2) 同 3) 目标 sim=0.95 score=4.57 [hit]）vs B rank1 | 24-case 库复现；修复前 A literal mrr=0.972 → 修复后 1.000 |
| PRF 严格补漏 | 合并后**不重排**：首轮顺序权威（含实体边桥入/字段加权/桶内候选），扩展新增去重后按 q2 顺序追加尾部再截断——扩展词只补漏、永不改变首轮排序（q2 口径 score 与首轮不可比，混排会把真目标挤出） | recall.rs；edge 桥入单测沿用；golden 零漂移 |
| 盲测模式 `--annotate-raw` | 不过自检输出全变体集（含自检会丢弃的变体 = 真阴性/假阳性对照）；每条带盲测摘要 `note: target=<hash16> hint=[原文首行] fix=[修复结论]`；rel 用层默认（paraphrase=2/semantic=1），evidence 标 `raw`；与 `--annotate` 互斥，缺省输出 `baseline-raw.json` | 单测 `annotate_raw_is_superset_with_blind_hint`（raw ⊇ 自检集 + 盲测摘要字段）；24-case 库 E2E 输出 36 组 |
| 盲测 SOP（独立证据门） | 生成 raw 集 → 人工盲视裁决 rel（2/1/0 或删除，不看 A/B 分数，不参照自检放行结果）→ `--eval baseline-raw.json --eval-verbose` → 报告留档（裁决 JSON + 输出 + 裁决人/日期/seed）；任何一层 A<B 即失败 | docs/m4-evidence-report.md §9 |

**修复后重跑（36 组，自动口径）**：literal A r@5=1.000 **mrr=1.000** **ndcg@10=1.000** = B；paraphrase A r@5=1.000（B 0.889）mrr=0.833（B 0.759）ndcg=0.877（B 0.792）；semantic r@10 双 1.000；l1 0.909。**自动口径下 A 不再劣于 B 任何一层**（替代 §22 复跑表）。

**门禁终态**：97 库 + 8 CLI + 5 集成 + golden 29 例全绿；fmt/clippy/release 0。
**状态**：工具就绪（回归集 + 盲测集）；独立人工/盲测证据未闭环前 M4 不宣称通过。



## 24. 外部评审整改单落地（2026-10-22，代码已提交）

P0/P1/P2 整改单逐条落地（评审口径：qmap 陈旧快照 / 只读污染 / rebuild 语义 / 持久化安全 / unsafe mmap / 中文召回 / 隐私保留 / 评估真实性 / 文档拆分）：

- **P0-1 盲测裁决链（d2766dc）**：`--annotate-raw` 的 paraphrase/semantic 条目 rel 不再预填（judge:null）；`--eval` 遇未裁决条目直接报错；新增 `--judge [--judge-by NAME]` 交互裁决（stdin 2/1/0/s，只给 query+目标摘要、零 A/B 分数）；报告加 bootstrap 95% CI（固定 seed、B=1000、r@5+MRR 预声明主指标）。
- **P0-2 fix 写入纪律（d2766dc）**：`--store` 不带 `--fix` 出 stderr warning；新增 `--fix-file <path>` 多行修复结论（与 --fix 互斥、必须配 --store 1）；召回 fix 行定位为候选（`fix: <内容> (candidate, sim=..)`），不冒充确定答案。
- **P1-3 中文召回（d7d0b2b）**：memory 词表切 `build_mem_terms`——CJK 段 ≤6 字块整段 + 重叠 bigram/trigram；10 组中/中英混排 paraphrase 桥接测试；`连接池满了` → 命中 `数据库连接池耗尽`（改前 sim=0.00 漏召）。
- **P1-4 隐私与保留（63f6a78）**：入库前 `sanitize()`（password/token/Authorization/Bearer/URL 用户信息 → `<redacted>`，幂等多轮收敛）；`--max-bytes N` 软配额（超限 exit 2）；gc 豁免带 fix case（regressed 仍按短 TTL 清）；recall 出屏 `escape_ctrl`（C0/\x1b/\x7f 转义可见）。E2E：落盘无 Secret123、超配额拒绝、\x1b 显示为 \\x1b。
- **P1-5 评估与基准真实性（b27ca39）**：bench 混合文本分布（短日志 ~120B / 长堆栈 ~2KB）+ p50/p95/p99 + 冷启动段；eval 报告加 multi-rel/single-rel 计数（NDCG 判别强度可见）；性能回归修复——bridge_cases 每次召回全库重建实体表（10k 库 ~0.85s）改为进程内缓存（root+meta.toml mtime 键），10k 召回 p50 回到 ~1.3ms。
- **P2-6 文档拆分（本笔）**：实施记录从 plan 拆出 → 本日志；plan 保留计划主体。

### M4 证据门状态（诚实口径）

自动口径 36 组（24/9/3）可复现，但独立人工/盲测质量证据未闭环：各层 ≥10 未满足、自检选择偏差未消除、n=9/n=3 无显著性。
评审底线：做不到独立人工裁决就把 M4 降级为“回归门”。盲测裁决链（--judge）已就绪，待真人裁决 raw 集并产出可引用数字。

## 25. P4.4+ 优化⑭实施记录（--gate 机械证据门，2026-10-22）

**背景**：M4 的“30–50 组、每层 ≥10、人工裁决、A≥B”此前只写在文档/SOP 里，`--eval` 对 5 条 demo 也照常出表，存在误把 demo/自动标注当证据的风险。

| 项 | 内容 | 验证 |
|---|---|---|
| `--gate` | `--eval --gate` 机械校验：总组数 ≥30；literal/paraphrase/semantic 各 ≥10；非 literal 必须有 `judge.by`+`judge.date`；每层 A 的 r@5（semantic r@10）与 MRR 均 ≥ B；未知层直接失败；任一失败 exit 2 | 单测 5 例（完整集 PASS / 小集 FAIL / 缺 judge 元数据 FAIL / A<B FAIL / 未知层 FAIL）；demo E2E：23 组 16/5/2 被拒；自动集另报 judge metadata FAIL |
| 解析 | `EvalCase` 增加 `judged_by`/`judged_date`，从 `judge:{by,date}` 读取；自动标注集无 judge 对象 → gate 拒绝，`--judge` 裁决后方可过门 | `parse_baseline_rejects_unjudged` 扩展断言 by/date 读取 |
| 兼容 | 默认 `--eval`/`--eval-verbose` 输出零漂移；gate 行仅在 `--gate` 时追加 | golden 29 例未漂移 |

**门禁终态**：109 库 + 8 CLI + 5 集成 + golden 29 例全绿；fmt/clippy 0；release ✓。
**状态**：M4 仍为“未通过”；`--gate` 只负责阻止假通过，真人裁决与真实冻结语料仍是前置条件。
