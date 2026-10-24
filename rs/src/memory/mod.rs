//! memory 模块门面：记忆知识库的公开接口与顶层封装。
//!
//! 与 `compress` 完全隔离：compress 是纯函数（零 I/O），memory 是 I/O 侧模块，
//! 只做"旁路召回 / 评分先验"，绝不注入压缩正文（红线 N1/N2/N3）。
//!
//! 子模块：`json`（零依赖 JSON 子集）、`store`（case/vec 读写）、`meta`（meta.toml
//! 与库根）、`model`（case 数据模型）、`util`（FNV/UTC/编辑距离）、`bucket`（场景桶）、
//! `qmap`（query 记忆图）、`ingest`（压缩产物 → case，指纹去重）、`field`（结构化字段
//! 抽取）、`recall`（BM25+字段加权+衰减+桶过滤召回）。
//! 本阶段（P1.1-P1.5 + P2.1/P2.4/P2.7）落地 json/store/meta/model/util/bucket/qmap/
//! ingest/field/recall 与 CLI 门面。

pub mod annotate;
mod bm25;
pub mod bucket;
pub mod cache;
pub mod edge;
pub mod eval;
pub mod feedback;
pub mod field;
pub(crate) mod gate;
pub mod housekeep;
pub mod index;
pub mod ingest;
pub mod json;
pub mod judge;
pub(crate) mod lock;
pub mod meta;
pub mod mmr;
pub mod model;
pub mod postings;
pub mod prf;
pub mod qmap;
pub mod recall;
pub mod stats;
pub mod store;
pub(crate) mod synonyms;
pub mod util;

pub use recall::RecallHit;
pub use store::CaseStats;

/// 记忆库顶层句柄：路径 + 元信息。后续阶段（索引/召回）在此扩展。
pub struct Memory {
    /// 库根目录（~/.scx-memory/ 或 --mem-dir 指定）。
    pub root: String,
    /// 当前 case 数（从 meta 加载，写入时维护）。
    pub case_count: usize,
}

impl Memory {
    /// 打开（或初始化）一个记忆库；根目录不存在时创建。
    /// 纯文件操作，与 compress 无交集。
    pub fn open(root: &str) -> std::io::Result<Memory> {
        store::ensure_root(root)?;
        let meta = store::load_meta(root)?;
        Ok(Memory {
            root: root.to_string(),
            case_count: meta.case_count,
        })
    }
}

/// 门面：摄取一个压缩结果（query + 证据块）→ 记忆 case。
/// terms 由门面内部从 query 生成（与压缩路径同一词表，保证确定性）。
/// 返回 `(added, case_id)`（P4.4+ 优化③：id 供 CLI 报告 stored: 行）。
/// P4.4+ 优化⑫（P1-4）：query/text/fix/evidence 统一经 sanitize 脱敏后才落盘
/// （token/密码/连接串不进入记忆库；指纹基于脱敏后 query，幂等）。
pub fn store_case(
    root: &str,
    query: &str,
    text: &str,
    source: Vec<String>,
    stats: &store::CaseStats,
    forces: Vec<String>,
) -> std::io::Result<(bool, String)> {
    let query = crate::memory::util::sanitize(query);
    let text = crate::memory::util::sanitize(text);
    let terms = crate::terms::build_mem_terms(&query);
    ingest::ingest(root, &query, terms, &text, source, stats, forces)
}

/// 带修复结论的摄取（P4.4+ 优化：--store 配 --fix，L1 本地直答的"上次怎么解决"）。
pub fn store_case_fix(
    root: &str,
    query: &str,
    text: &str,
    source: Vec<String>,
    stats: &store::CaseStats,
    forces: Vec<String>,
    fix: &str,
) -> std::io::Result<(bool, String)> {
    let query = crate::memory::util::sanitize(query);
    let text = crate::memory::util::sanitize(text);
    let fix = crate::memory::util::sanitize(fix);
    let terms = crate::terms::build_mem_terms(&query);
    ingest::ingest_with_fix(root, &query, terms, &text, source, stats, forces, &fix)
}

/// P4.4+ 优化⑤：带原始证据的摄取（评审缺口 B——case 四要素：query + 证据行区间 +
/// 原始证据 + 修复结论）。`evidence_lno`/`evidence` 由调用方从压缩结果 kept_lno +
/// 输入提取；旧 case/无证据调用等价 store_case_fix。
#[allow(clippy::too_many_arguments)]
pub fn store_case_full(
    root: &str,
    query: &str,
    text: &str,
    source: Vec<String>,
    stats: &store::CaseStats,
    forces: Vec<String>,
    fix: &str,
    evidence_lno: &[usize],
    evidence: &[String],
) -> std::io::Result<(bool, String)> {
    let query = crate::memory::util::sanitize(query);
    let text = crate::memory::util::sanitize(text);
    let fix = crate::memory::util::sanitize(fix);
    let evidence: Vec<String> = evidence
        .iter()
        .map(|e| crate::memory::util::sanitize(e))
        .collect();
    let terms = crate::terms::build_mem_terms(&query);
    ingest::ingest_with_evidence(
        root,
        &query,
        terms,
        &text,
        source,
        stats,
        forces,
        &fix,
        evidence_lno,
        &evidence,
    )
}

/// 读取 case 的修复结论（空串 = 未记录；缺 case 返回 Err）。
pub fn case_fix(root: &str, id: &str) -> std::io::Result<String> {
    store::read_case(root, id).map(|c| c.fix)
}

/// 门面：读取完整 case（P4.4+ 优化⑤：原始证据回放——evidence_lno/evidence 字段）。
pub fn read_case(root: &str, id: &str) -> std::io::Result<model::Case> {
    store::read_case(root, id)
}

/// 门面：按 query 召回 top-k case（会话缓存 → qmap 直达 → BM25+字段加权+衰减+桶过滤）。
///
/// O3 会话缓存：进程内存可配置窗口（默认 30 分钟），窗口内同 query 直接返回快照（零磁盘），
/// 快照记录写入时的库代际，store/rate/delete 后自动失效（P4.4 评审处置）；
/// O2 记忆图：recall 成功后把 (query → top-k id/score/sim) 回流写入 qmap.bin
/// （append-only，带代际），二次同指纹 query 直接命中（跳过 BM25），编辑距离 ≤2
/// 变体走轻量重排。回流失败静默（旁路原则：记忆图只是加速，不影响召回正确性）。
///
/// `read_only`：查看模式（--recall-only / 评估）不写任何库状态
/// （qmap / 会话缓存 / L1 历史），保证查看、标注、演示零副作用。
pub fn recall_cases(
    root: &str,
    query: &str,
    k: usize,
    read_only: bool,
) -> std::io::Result<Vec<RecallHit>> {
    let meta = store::load_meta(root)?;
    // O3：会话缓存命中直接返回快照（含 score/sim，L1 过滤可复用）。
    let cache_window = (meta.param("session_cache_minutes").max(0.0) * 60.0) as u64;
    if let Some(hits) = cache::get(query, meta.gen, cache_window) {
        let mut out: Vec<RecallHit> = hits
            .into_iter()
            .map(|(id, score, sim)| RecallHit { id, score, sim })
            .collect();
        out.truncate(k);
        return Ok(out);
    }
    let hits = recall::recall(root, query, k)?;
    if !hits.is_empty() && !read_only {
        let snap: Vec<(String, f64, f64)> = hits
            .iter()
            .map(|h| (h.id.clone(), h.score, h.sim))
            .collect();
        cache::put(query, snap.clone(), meta.gen);
        let _ = qmap::append(root, query, &snap, meta.gen); // 回流失败忽略
                                                            // O5 L1 自适应：记录 top1.sim 到滑动历史（旁路，失败静默）。
        meta::push_l1_hist(root, hits[0].sim);
    }
    Ok(hits)
}

/// 门面：反馈标注（状态机 + 质量分 + 词级调权 + L1 自适应基线）。
/// 返回更新后的 case，供 CLI 输出核对。
pub fn rate_case(root: &str, id: &str, good: bool) -> std::io::Result<crate::memory::model::Case> {
    feedback::rate(root, id, good)
}

/// 门面：L1 自适应置信判据（§5.1）。
pub fn l1_qualified(top1_sim: f64, top2_sim: f64, l1_base: f64, margin: f64, ext_hit: f64) -> bool {
    feedback::l1_qualified(top1_sim, top2_sim, l1_base, margin, ext_hit)
}

/// 门面：meta 中 L1 参数；缺省值由 `meta::DEFAULT_PARAMS` 单一来源提供。
pub fn l1_params(root: &str) -> (f64, f64) {
    match store::load_meta(root) {
        Ok(m) => feedback::l1_params(&m),
        Err(_) => (
            crate::memory::meta::default_param("l1_base"),
            crate::memory::meta::default_param("l1_margin_min"),
        ),
    }
}

/// 门面：列出全部 case id（确定性排序）。
pub fn list_cases(root: &str) -> std::io::Result<Vec<String>> {
    store::list_ids(root)
}

/// 门面：重建索引（postings 全量重建 + qmap 合并 + df 快照）。
/// 返回 (n_docs, n_terms)。索引是可丢弃缓存，任何时刻可安全重建。
pub fn rebuild(root: &str) -> std::io::Result<(usize, usize)> {
    housekeep::rebuild(root)
}

/// 门面：过期清理（regressed>180d 或从未反馈且 stale>365d）。返回删除数。
pub fn gc(root: &str) -> std::io::Result<usize> {
    housekeep::gc(root)
}

/// 门面：删除 case（物理删除 + 惰性作废索引）。返回是否存在。
pub fn delete_case(root: &str, id: &str) -> std::io::Result<bool> {
    housekeep::delete_case(root, id)
}

/// 门面：评估系统（P4.1，O8）。冻结 baseline.json 重放 → 分层指标报告行（确定性）。
pub fn eval_cases(root: &str, path: &str) -> std::io::Result<Vec<String>> {
    eval::run(root, path)
}

/// 门面：--eval-verbose 逐候选诊断（P4.4+ 优化④，服务 M4 人工标注校准）。
pub fn eval_cases_verbose(root: &str, path: &str) -> std::io::Result<Vec<String>> {
    eval::run_verbose(root, path)
}

/// 门面：`--eval --gate` 的 M4 机械门——返回报告行与是否通过。
pub fn eval_cases_gate(
    root: &str,
    path: &str,
    verbose: bool,
) -> std::io::Result<(Vec<String>, bool)> {
    if verbose {
        eval::run_gate_verbose(root, path)
    } else {
        eval::run_gate(root, path)
    }
}

/// 门面：`--eval --gate-human` 的人工弱相关/负例闭环门。
pub fn eval_cases_gate_human(
    root: &str,
    path: &str,
    verbose: bool,
) -> std::io::Result<(Vec<String>, bool)> {
    if verbose {
        eval::run_gate_human_verbose(root, path)
    } else {
        eval::run_gate_human(root, path)
    }
}

/// 门面：`--eval --gate-items` 的逐候选裁决门（允许非人类裁决人）。
pub fn eval_cases_gate_items(
    root: &str,
    path: &str,
    verbose: bool,
) -> std::io::Result<(Vec<String>, bool)> {
    if verbose {
        eval::run_gate_items_verbose(root, path)
    } else {
        eval::run_gate_items(root, path)
    }
}

/// 门面：L1 要点行摘要（P4.2，O5 后半），≤300 字符。
pub fn case_summary(root: &str, id: &str) -> std::io::Result<String> {
    eval::case_summary(root, id)
}

/// 门面：运行期 [l1] 直答命中分布出表（P2 阶段门残余，--l1-hist）。
pub fn l1_hist_table(root: &str) -> std::io::Result<Vec<String>> {
    eval::l1_hist_table(root)
}

/// 门面：库盘点（P4.4+ 优化，--stats）：case/桶/状态/fix 覆盖率/质量/磁盘。
pub fn stats_table(root: &str) -> std::io::Result<Vec<String>> {
    stats::table(root)
}

/// 门面：智能标注（P4.4+ 优化⑥，--annotate）：替代人工采集 M4 证据门标注。
/// 生成三层标注（literal/paraphrase/semantic），每条经召回自检验证并带证据链；
/// 返回标注集（调用方负责序列化/落盘/报告）。
pub fn annotate_cases(
    root: &str,
    seed: u64,
    max_cases: usize,
) -> std::io::Result<Vec<annotate::AnnotatedCase>> {
    annotate::annotate(root, seed, max_cases)
}

/// 门面：标注 + 自检丢弃统计（CLI 报告用）。
pub fn annotate_report(
    root: &str,
    seed: u64,
    max_cases: usize,
) -> std::io::Result<annotate::AnnotateReport> {
    annotate::annotate_report(root, seed, max_cases, false)
}

/// 门面：raw 模式（不过自检的全变体集，人工盲测裁决用）。
pub fn annotate_raw_report(
    root: &str,
    seed: u64,
    max_cases: usize,
) -> std::io::Result<annotate::AnnotateReport> {
    annotate::annotate_report(root, seed, max_cases, true)
}
