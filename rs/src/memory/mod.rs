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

pub mod bucket;
pub mod cache;
pub mod edge;
pub mod eval;
pub mod feedback;
pub mod field;
pub mod housekeep;
pub mod index;
pub mod ingest;
pub mod json;
pub mod meta;
pub mod mmap;
pub mod mmr;
pub mod model;
pub mod postings;
pub mod prf;
pub mod qmap;
pub mod recall;
pub mod store;
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
/// 返回 true=新增，false=指纹去重（仅刷新 updated_at）。
pub fn store_case(
    root: &str,
    query: &str,
    text: &str,
    source: Vec<String>,
    stats: &store::CaseStats,
    forces: Vec<String>,
) -> std::io::Result<bool> {
    let terms = crate::terms::build_terms(query);
    ingest::ingest(root, query, terms, text, source, stats, forces)
}

/// 门面：按 query 召回 top-k case（会话缓存 → qmap 直达 → BM25+字段加权+衰减+桶过滤）。
///
/// O3 会话缓存：进程内存 30 分钟窗口，窗口内同 query 直接返回快照（零磁盘）；
/// O2 记忆图：recall 成功后把 (query → top-k id/score/sim) 回流写入 qmap.bin
/// （append-only）；二次同指纹 query 直接命中（跳过 BM25），编辑距离 ≤2 变体
/// 走轻量重排。回流失败静默（旁路原则：记忆图只是加速，不影响召回正确性）。
pub fn recall_cases(root: &str, query: &str, k: usize) -> std::io::Result<Vec<RecallHit>> {
    // O3：会话缓存命中直接返回快照（含 score/sim，L1 过滤可复用）。
    if let Some(hits) = cache::get(query) {
        let mut out: Vec<RecallHit> = hits
            .into_iter()
            .map(|(id, score, sim)| RecallHit { id, score, sim })
            .collect();
        out.truncate(k);
        return Ok(out);
    }
    let hits = recall::recall(root, query, k)?;
    if !hits.is_empty() {
        let snap: Vec<(String, f64, f64)> = hits
            .iter()
            .map(|h| (h.id.clone(), h.score, h.sim))
            .collect();
        cache::put(query, snap.clone());
        let _ = qmap::append(root, query, &snap); // 回流失败忽略
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

/// 门面：meta 中 L1 参数（base/margin，缺省 0.55/0.15）。
pub fn l1_params(root: &str) -> (f64, f64) {
    match store::load_meta(root) {
        Ok(m) => feedback::l1_params(&m),
        Err(_) => (0.55, 0.15),
    }
}

/// 门面：列出全部 case id（确定性排序）。
pub fn list_cases(root: &str) -> std::io::Result<Vec<String>> {
    store::list_ids(root)
}

/// 门面：重建索引（postings 全量重建 + qmap 合并 + df 快照 + index_force=1）。
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

/// 门面：L1 要点行摘要（P4.2，O5 后半），≤300 字符。
pub fn case_summary(root: &str, id: &str) -> std::io::Result<String> {
    eval::case_summary(root, id)
}

/// 门面：运行期 [l1] 直答命中分布出表（P2 阶段门残余，--l1-hist）。
pub fn l1_hist_table(root: &str) -> std::io::Result<Vec<String>> {
    eval::l1_hist_table(root)
}
