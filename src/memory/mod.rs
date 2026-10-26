//! memory 模块门面：记忆知识库的公开接口与顶层封装。
//!
//! 与 `compress` 完全隔离：compress 是纯函数（零 I/O），memory 是 I/O 侧模块，
//! 只做旁路召回与评分先验，绝不注入压缩正文。
//!
//! 子模块按职责分为存储（`store`/`meta`/`index`/`lock`）、摄取与分类
//! （`ingest`/`classify`）、召回排序（`recall`/`ranking`/`edge`/`cache`）、
//! 反馈维护（`feedback`/`maintenance`）以及评估（`eval`/`gate`/`annotate`）；
//! 具体文件职责见 `docs/memory.md`。

mod annotate;
mod cache;
mod classify;
mod edge;
mod eval;
mod feedback;
pub(crate) mod gate;
mod index;
mod ingest;
mod json;
pub(crate) mod lock;
mod maintenance;
mod meta;
pub(crate) mod ranking;
mod recall;
mod store;
mod util;

pub use recall::Hit;
pub use store::{Case, Feedback, Fields, Stats};

use std::collections::HashMap;

/// `--fix` / `--fix-file` 的字节上限；CLI 读取与存储校验共享同一约束。
pub const MAX_FIX_BYTES: usize = ingest::MAX_FIX_LEN;

/// 一次写入所需的完整输入。可选字段默认空，调用方只填充实际拥有的信息；
/// 新增存储能力不应再产生 `save_*` 函数族。
pub struct Input<'a> {
    pub root: &'a str,
    pub query: &'a str,
    pub text: &'a str,
    pub source: &'a [String],
    pub stats: &'a Stats,
    pub forces: &'a [String],
    pub fix: &'a str,
    pub evidence_lno: &'a [usize],
    pub evidence: &'a [String],
    /// 写入前允许的库总字节数上限；`None` 表示不限制。
    pub max_bytes: Option<u64>,
}

impl<'a> Input<'a> {
    /// 最小输入：仅 query、证据块与统计；其余字段为空。
    pub fn new(root: &'a str, query: &'a str, text: &'a str, stats: &'a Stats) -> Self {
        Self {
            root,
            query,
            text,
            source: &[],
            stats,
            forces: &[],
            fix: "",
            evidence_lno: &[],
            evidence: &[],
            max_bytes: None,
        }
    }
}

/// 摄取一个压缩结果 → 记忆 case。
///
/// 所有文本在同一存储边界脱敏；query 词表由门面生成，保证指纹、索引和召回
/// 使用同一口径。返回 `(added, case_id)`。
pub fn save(input: Input<'_>) -> std::io::Result<(bool, String)> {
    let query = crate::memory::util::sanitize(input.query);
    let text = crate::memory::util::sanitize(input.text);
    let fix = crate::memory::util::sanitize(input.fix);
    let evidence: Vec<String> = input
        .evidence
        .iter()
        .map(|e| crate::memory::util::sanitize(e))
        .collect();
    let terms = crate::terms::memory(&query);
    ingest::persist(ingest::Payload {
        root: input.root,
        query: &query,
        terms,
        text: &text,
        source: input.source,
        stats: input.stats,
        forces: input.forces,
        fix: &fix,
        evidence_lno: input.evidence_lno,
        evidence: &evidence,
        max_bytes: input.max_bytes,
    })
}

/// 门面：读取完整 case，包括可回放的原始证据行。
pub fn load(root: &str, id: &str) -> std::io::Result<Case> {
    store::load(root, id)
}

/// 转义从记忆库回放到终端的控制字符，防止日志注入 ANSI 序列。
pub fn escape(s: &str) -> String {
    util::escape(s)
}

/// 门面：按 query 召回 top-k case（会话缓存 → qmap 直达 → BM25+字段加权+衰减+桶过滤）。
///
/// 会话缓存：进程内存可配置窗口（默认 30 分钟），窗口内同 query 直接返回快照；
/// 快照记录写入时的库代际，store/rate/delete 后自动失效。快照还带结果条数：
/// 请求的 `k` 超过快照条数时回退完整召回，避免小 k 查询污染后续大 k 查询。
/// query 记忆图：recall 成功后把 (query → top-k id/score/sim) 回流写入 qmap.bin
/// （append-only，带代际），二次同指纹 query 直接命中（跳过 BM25）。变体 query
/// 不复用快照：缓存只含 top-k，无法在不重扫全库的前提下保证变体排序正确。
/// 快照只复用 1 小时，避免时间衰减排序长期停留在旧状态。
/// 回流失败静默（旁路原则：记忆图只是加速，不影响召回正确性）。
///
/// `read_only`：`--recall-only` 不写 qmap、会话缓存、L1 历史或索引缓存；
/// 索引缺失/损坏时直接回退线性扫描，保证只读查看零磁盘副作用。
pub fn search(root: &str, query: &str, k: usize, read_only: bool) -> std::io::Result<Vec<Hit>> {
    if k == 0 {
        return Ok(Vec::new());
    }
    let meta = crate::memory::meta::load(root)?;
    // 会话缓存命中直接返回快照（含 score/sim，L1 过滤可复用）。
    let cache_window = (meta.param("session_cache_minutes").max(0.0) * 60.0) as u64;
    if let Some(out) = session(root, query, &meta, cache_window, k) {
        return Ok(out);
    }
    let hits = if read_only {
        recall::read(root, query, k, &meta)?
    } else {
        recall::recall(root, query, k, &meta)?
    };
    writeback(root, query, &hits, &meta, read_only);
    Ok(hits)
}

/// RRF 融合常数：与 `tools/embed_baseline.py` 的混合检索口径一致。
const RRF_K: f64 = 60.0;

/// 融合召回：BM25 top-k 与 embedding 余弦排名的 RRF 融合（可选，`--embed-cmd`）。
///
/// `embed_cmd: None` 或 `embed_weight <= 0.0` 时直接返回 `search` 结果，行为不变。
/// 否则枚举全库 case 文本，计算 query×文本 embedding 相似度排名，与 BM25 排名做
/// RRF 融合后取 top-k。BM25 命中保留原 `score`/`sim`；仅靠 embedding 救回的 case
/// （不在 BM25 top-k）`score`/`sim` 记 0——不触发 L1 直答，符合低词面置信语义。
/// 本函数不触碰评测路径（`--eval` 走 `recall::clock`/`read`），召回评测门不回归。
pub fn search_fused(
    root: &str,
    query: &str,
    k: usize,
    read_only: bool,
    embed_cmd: Option<&str>,
    embed_weight: f64,
) -> std::io::Result<Vec<Hit>> {
    let bm25 = search(root, query, k, read_only)?;
    if embed_cmd.is_none() || embed_weight <= 0.0 || k == 0 {
        return Ok(bm25);
    }
    let ids = store::list(root)?;
    if ids.is_empty() {
        return Ok(bm25);
    }
    let mut texts: Vec<String> = Vec::with_capacity(ids.len());
    for id in &ids {
        texts.push(store::load(root, id)?.text);
    }
    let refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
    let sims = crate::sim::compute(embed_cmd, query, &refs).map_err(std::io::Error::other)?;

    // embedding 排名：sim 降序，同分按 id 升序（确定性）。
    let mut order: Vec<usize> = (0..ids.len()).collect();
    order.sort_by(|&a, &b| {
        sims[b]
            .partial_cmp(&sims[a])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| ids[a].cmp(&ids[b]))
    });
    let embed_term: HashMap<String, f64> = order
        .iter()
        .enumerate()
        .map(|(r, &i)| (ids[i].clone(), 1.0 / (RRF_K + r as f64 + 1.0)))
        .collect();

    // BM25 排名项与 score/sim 快照。
    let bm25_term: HashMap<String, f64> = bm25
        .iter()
        .enumerate()
        .map(|(r, h)| (h.id.clone(), 1.0 / (RRF_K + r as f64 + 1.0)))
        .collect();
    let bm25_meta: HashMap<String, (f64, f64)> = bm25
        .iter()
        .map(|h| (h.id.clone(), (h.score, h.sim)))
        .collect();

    let mut fused: Vec<(f64, String)> = ids
        .iter()
        .map(|id| {
            let b = bm25_term.get(id).copied().unwrap_or(0.0);
            let e = embed_term.get(id).copied().unwrap_or(0.0);
            (b + embed_weight * e, id.clone())
        })
        .collect();
    fused.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.cmp(&b.1))
    });
    fused.truncate(k);

    Ok(fused
        .into_iter()
        .map(|(_, id)| {
            let (score, sim) = bm25_meta.get(&id).copied().unwrap_or((0.0, 0.0));
            Hit { id, score, sim }
        })
        .collect())
}

fn session(
    root: &str,
    query: &str,
    meta: &store::Meta,
    cache_window: u64,
    k: usize,
) -> Option<Vec<Hit>> {
    let hits = cache::get(root, query, meta.gen, cache_window, k)?;
    let out: Vec<Hit> = hits
        .into_iter()
        .map(|(id, score, sim)| Hit { id, score, sim })
        .collect();
    debug_assert!(out.len() <= k, "session snapshot must respect k");
    Some(out)
}

fn writeback(root: &str, query: &str, hits: &[Hit], meta: &store::Meta, read_only: bool) {
    if hits.is_empty() || read_only {
        return;
    }
    let snap: Vec<(String, f64, f64)> = hits
        .iter()
        .map(|h| (h.id.clone(), h.score, h.sim))
        .collect();
    let _ = cache::append(root, query, &snap, meta.gen); // 回流失败忽略
    cache::put(root, query, snap, meta.gen);
    meta::record(root, hits[0].sim);
}

/// 门面：反馈标注（状态机 + 质量分 + 词级调权 + L1 自适应基线）。
/// 返回更新后的 case，供 CLI 输出核对。
pub fn rate(root: &str, id: &str, good: bool) -> std::io::Result<store::Case> {
    feedback::rate(root, id, good)
}

/// 门面：meta 中 L1 参数；缺省值由 `meta::DEFAULT_PARAMS` 单一来源提供。
pub fn params(root: &str) -> (f64, f64) {
    match crate::memory::meta::load(root) {
        Ok(m) => feedback::params(&m),
        Err(_) => (
            crate::memory::meta::fallback("l1_base"),
            crate::memory::meta::fallback("l1_margin_min"),
        ),
    }
}

/// 只保留达到 L1 直答条件的召回命中（top1 基线 + top1/top2 间隔）。
pub fn hits(root: &str, hits: Vec<Hit>) -> Vec<Hit> {
    let (base, margin) = params(root);
    let top1 = hits.first().map(|h| h.sim).unwrap_or(0.0);
    let top2 = hits.get(1).map(|h| h.sim).unwrap_or(0.0);
    if feedback::qualified(top1, top2, base, margin) {
        hits
    } else {
        Vec::new()
    }
}

/// 门面：重建索引（postings 全量重建 + qmap 合并 + df 快照）。
/// 返回 (n_docs, n_terms)。索引是可丢弃缓存，任何时刻可安全重建。
pub fn rebuild(root: &str) -> std::io::Result<(usize, usize)> {
    maintenance::rebuild(root)
}

/// 门面：过期清理（regressed>180d 或从未反馈且 stale>365d）。返回删除数。
pub fn gc(root: &str) -> std::io::Result<usize> {
    maintenance::gc(root)
}

/// 门面：删除 case（物理删除 + 惰性作废索引）。返回是否存在。
pub fn delete(root: &str, id: &str) -> std::io::Result<bool> {
    maintenance::delete(root, id)
}

/// 评估/门禁模式。报告模式返回的 gate 标志恒为 false。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eval {
    Report,
    Mechanical,
    Human,
    Items,
}

/// 单一评估门面：冻结 baseline 重放并按模式选择报告或门禁。
pub fn evaluate(
    root: &str,
    path: &str,
    mode: Eval,
    verbose: bool,
) -> std::io::Result<(Vec<String>, bool)> {
    eval::run(root, path, mode, verbose)
}

/// 门面：输出运行期 `[l1]` 直答命中分布（`--l1-hist`）。
pub fn history(root: &str) -> std::io::Result<Vec<String>> {
    eval::history(root)
}

/// 门面：库盘点（`--stats`）：case、桶、状态、fix 覆盖率、质量与磁盘。
pub fn stats(root: &str) -> std::io::Result<Vec<String>> {
    maintenance::stats(root)
}

/// 生成标注集、原子写入目标文件，并返回 CLI 报告行。
/// `raw` 跳过自检以生成盲测材料。
pub fn annotate(
    root: &str,
    seed: u64,
    max_cases: usize,
    raw: bool,
    path: &str,
) -> std::io::Result<Vec<String>> {
    let report = annotate::report(root, seed, max_cases, raw)?;
    let body = annotate::baseline(&report.cases);
    lock::atomic(std::path::Path::new(path), body.as_bytes())?;
    Ok(annotate::summary(&report))
}

/// 交互式逐候选裁决入口（`--judge`）。
pub fn judge(path: &str, by: &str) -> std::io::Result<Vec<String>> {
    annotate::judge(path, by)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> String {
        let root = std::env::temp_dir().join(format!("scx-fused-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root.to_str().unwrap().to_string()
    }

    fn seed(root: &str, query: &str, text: &str) -> String {
        let stats = Stats {
            kept: 1,
            total: 1,
            cut: 0.0,
            risk: "LOW".to_string(),
        };
        let (added, id) = save(Input::new(root, query, text, &stats)).unwrap();
        assert!(added, "seed must add a case");
        id
    }

    #[test]
    fn search_fused_none_equals_search() {
        let root = tmp("none");
        seed(
            &root,
            "db timeout",
            "db timeout conn pool\nroot cause: pool exhausted",
        );
        seed(&root, "disk full", "disk full errno=28\nfix: clean up");
        let plain = search(&root, "db timeout", 2, true).unwrap();
        let fused = search_fused(&root, "db timeout", 2, true, None, 1.0).unwrap();
        assert_eq!(plain, fused, "embed_cmd=None 必须与 search 逐字节一致");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn search_fused_with_provider_returns_valid_hits() {
        let root = tmp("prov");
        seed(&root, "db timeout", "db timeout conn pool");
        seed(&root, "disk full", "disk full errno=28");
        let cmd = "printf '0.5\\n0.5\\n'";
        let hits = search_fused(&root, "db timeout", 2, true, Some(cmd), 1.0).unwrap();
        assert!(!hits.is_empty(), "融合召回不应为空");
        assert!(hits.len() <= 2, "融合召回须受 k 约束");
        assert!(
            hits.iter().all(|h| h.id.contains('_')),
            "case id 形如 YYYY-MM-DD_hex"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
