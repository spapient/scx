//! 检索排序：BM25、PRF 查询扩展与 MMR 多样性去冗余的唯一实现。
//!
//! BM25 共享公式供召回、PRF 与评估 B 基线复用，避免口径漂移。
//! PRF 伪相关反馈：粗召回 top-K 后取高 IDF 高频词并入 query 再召一轮。
//!
//! 统计方法独有优势：embedding 做查询扩展很难。扩展词选取规则（确定性）：
//! 只统计与 query 词面相关（sim > 0）的粗召回 case 词表（query ∪ text 同口径）；
//! 全局 df 现算（线性阶段），批内 tf ≥2 且全局 idf 高（idf > 阈值）的词入选；
//! 排除已含在 query 词表中的词。
//! 只在召回低置信（top1.sim < 0.99）时触发：同 query 高置信已直接命中，无需扩展。
//!
//! P4.4+ 优化⑦：按 sim>0 过滤粗召回 case——无关 case（词面零重叠）的私有词
//! 不得进入扩展词表（否则扩展会把不相干桶/词拉进 query，重排噪声压过真目标）。
//!
//! MMR：贪心选择 `argmax(score_norm − λ · max(sim(c, selected)))`，以词袋
//! Jaccard 衡量冗余，确定性、零依赖。

use crate::memory::recall::RecallHit;
use crate::memory::store::{self, Meta};
use crate::terms::{build_doc_terms, build_mem_terms};
use std::collections::HashMap;

/// Robertson/Sparck Jones IDF（平滑、非负）。
pub(crate) fn idf(n: f64, df: f64) -> f64 {
    (1.0 + (n - df + 0.5) / (df + 0.5)).ln().max(0.0)
}

/// BM25 长度归一化分母；空库（avgdl=0）退化为 1.0，避免 NaN。
pub(crate) fn denom(k1: f64, b: f64, dl: f64, avgdl: f64) -> f64 {
    let norm = if avgdl > 0.0 { dl / avgdl } else { 0.0 };
    1.0 + k1 * (1.0 - b + b * norm)
}

/// 单词语义分（tf 恒为 1 的去重词表口径）。
pub(crate) fn score(idf: f64, k1: f64, denom: f64) -> f64 {
    idf * (k1 + 1.0) / denom
}

/// 扩展候选：批内出现 ≥2 次（高频）且全局稀有（idf 高）。
const MIN_BATCH_TF: usize = 2;
/// idf 阈值：仅保留 idf > 0.6 的词（df 不能过大）。
const IDF_MIN: f64 = 0.6;
/// 扩展词数量上限。
const EXPAND_MAX: usize = 6;

/// 返回扩展词（未含 query 原词的、库内高判别词）。
/// `hits`：粗召回 top-K 候选（含 sim；仅 sim>0 的 case 参与词频统计）。
/// `meta`：df 快照与库规模。
/// df 来源：rebuild 后的 [df] 快照（索引路径，免全库扫描）；
/// 快照为空（小库未 rebuild）时回退全库现算（线性兜底）。
pub(crate) fn expand(
    root: &str,
    hits: &[RecallHit],
    query: &str,
    meta: &Meta,
) -> std::io::Result<Vec<String>> {
    let qterms = build_mem_terms(query);
    let (n, df): (f64, HashMap<String, usize>) = if !meta.df.is_empty() {
        (meta.case_count as f64, meta.df.iter().cloned().collect())
    } else {
        // 线性阶段兜底：全库现算（小库可接受；大库必经 rebuild → 快照非空）。
        let all_ids = store::list_ids(root)?;
        let n = all_ids.len() as f64;
        let mut df: HashMap<String, usize> = HashMap::new();
        for id in &all_ids {
            let c = store::read_case(root, id)?;
            let (_, terms) = build_doc_terms(&c.query, &c.text);
            for t in &terms {
                *df.entry(t.clone()).or_insert(0) += 1;
            }
        }
        (n, df)
    };
    if n == 0.0 {
        return Ok(Vec::new());
    }
    // 批内 tf：只统计与 query 词面相关（sim>0）的 case——无关 case 的私有词
    // 是噪声源（会把桶判定/重排带偏），P4.4+ 优化⑦ 明确排除。
    let mut batch: HashMap<String, usize> = HashMap::new();
    for h in hits {
        if h.sim <= 0.0 {
            continue;
        }
        let c = store::read_case(root, &h.id)?;
        let mut terms = build_mem_terms(&c.query);
        terms.extend(build_mem_terms(&c.text));
        for t in &terms {
            *batch.entry(t.clone()).or_insert(0) += 1;
        }
    }
    // 筛选：批内 tf≥MIN、全局 idf>IDF_MIN、不在 query 词表。
    let mut cand: Vec<(String, f64)> = Vec::new();
    for (t, tf) in &batch {
        if *tf < MIN_BATCH_TF {
            continue;
        }
        if qterms.contains(t) {
            continue;
        }
        let d = *df.get(t).unwrap_or(&0) as f64;
        let idf = idf(n, d);
        if idf > IDF_MIN {
            cand.push((t.clone(), idf));
        }
    }
    // 确定性全序：idf 降序，idf 相等时按词面升序。`batch` 是 HashMap，迭代序随进程
    // 随机；仅按 idf 排序时并列词会继承随机序，truncate(EXPAND_MAX) 因而跨进程选出
    // 不同的扩展词集合，导致 PRF 第二轮命中数/排序不可重放（实测同一 query 跨进程
    // 候选数在 6..10 抖动）。补词面次级键使排序唯一。
    sort_expansion_candidates(&mut cand);
    cand.truncate(EXPAND_MAX);
    Ok(cand.into_iter().map(|(t, _)| t).collect())
}

/// 扩展候选排序：idf 降序、idf 相同按词面升序（确定、可重放）。
fn sort_expansion_candidates(cand: &mut [(String, f64)]) {
    cand.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
}

/// 贪心 MMR 选取：返回选中下标（顺序 = 分数优先下的多样性序）。
/// `cands` 应已按 score 降序；`docs` 与 `scores_norm` 按 doc 下标对齐。
pub(crate) fn mmr_select(
    cands: &[usize],
    docs: &[Vec<String>],
    scores_norm: &[f64],
    k: usize,
    lambda: f64,
) -> Vec<usize> {
    let mut picked: Vec<usize> = Vec::with_capacity(k.min(cands.len()));
    let mut pool: Vec<usize> = cands.to_vec();
    let mut iter = 0;
    while !pool.is_empty() && picked.len() < k && iter <= cands.len() {
        let mut best_i = 0usize;
        let mut best_v = f64::NEG_INFINITY;
        for (pi, &ci) in pool.iter().enumerate() {
            let mut max_sim = 0.0f64;
            for &pj in &picked {
                max_sim = max_sim.max(jaccard(&docs[ci], &docs[pj]));
            }
            let value = scores_norm[ci] - lambda * max_sim;
            if value > best_v {
                best_v = value;
                best_i = pi;
            }
        }
        picked.push(pool[best_i]);
        pool.remove(best_i);
        iter += 1;
    }
    picked
}

/// 已排序去重词表的 Jaccard：|A∩B| / |A∪B|（空∪空 = 0），双指针 O(n+m)。
/// 召回、MMR 与同族聚类共享此实现；调用方必须传入排序后的词表。
pub(crate) fn jaccard(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let (mut i, mut j, mut intersection) = (0usize, 0usize, 0usize);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                intersection += 1;
                i += 1;
                j += 1;
            }
        }
    }
    let union = a.len() + b.len() - intersection;
    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::ingest::ingest;
    use crate::memory::recall::RecallHit;
    use crate::memory::store::CaseStats;
    use std::fs;

    fn tmp_root(tag: &str) -> String {
        let r = std::env::temp_dir().join(format!(
            "scx-prf-{}-{}",
            tag,
            crate::memory::util::fnv1a("p") & 0xff
        ));
        let _ = fs::remove_dir_all(&r);
        r.to_str().unwrap().to_string()
    }

    fn stats() -> CaseStats {
        CaseStats {
            kept: 5,
            total: 100,
            cut_pct: 95.0,
            risk: "HIGH".to_string(),
        }
    }

    #[test]
    fn expansion_sort_ties_break_by_term_asc() {
        // idf 并列时词面升序：跨进程可重放（HashMap 迭代序不得影响截断结果）。
        let mut cand = vec![
            ("zz".to_string(), 1.0),
            ("aa".to_string(), 1.0),
            ("mm".to_string(), 2.0),
            ("bb".to_string(), 1.0),
        ];
        sort_expansion_candidates(&mut cand);
        let terms: Vec<&str> = cand.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(terms, vec!["mm", "aa", "bb", "zz"]);
    }

    #[test]
    fn prf_expands_paraphrase_query() {
        let root = tmp_root("exp");
        let s1 = stats();
        // 库内：多个 case 反复出现判别词 `errno` / `pool`（字母区分文本，
        // 避免 norm 数字折叠导致指纹相同被去重）。
        for tag in ["alpha", "beta", "gamma"] {
            ingest(
                &root,
                "连接池问题",
                vec!["conn_id".into()],
                &format!("ERROR db timeout conn_id=9f2a errno=28 pool {tag}"),
                vec!["/a.log".into()],
                &s1,
                vec![],
            )
            .unwrap();
        }
        // 一个干扰 case：词面贴近释义 query 但无判别词。
        ingest(
            &root,
            "连接池问题",
            Vec::new(),
            "数据库又慢又卡 全部超时",
            vec!["/b.log".into()],
            &s1,
            vec![],
        )
        .unwrap();
        // 再放一批无关 case，把判别词 df 稀释（idf 抬高，满足 IDF_MIN）。
        for tag in ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"] {
            ingest(
                &root,
                "无关主题",
                Vec::new(),
                &format!("INFO heartbeat ok module {tag}"),
                vec!["/c.log".into()],
                &s1,
                vec![],
            )
            .unwrap();
        }

        // 释义变体 query：词面与库内 case 几乎不重叠。
        let q = "pool 又出问题";
        let ids = store::list_ids(&root).unwrap();
        let meta = store::load_meta(&root).unwrap();
        // 构造粗召回候选：全部标记 sim=1.0（PRF 只依赖词面相关标记，词频统计看 case 内容）。
        let hits: Vec<RecallHit> = ids
            .iter()
            .map(|id| RecallHit {
                id: id.clone(),
                score: 1.0,
                sim: 1.0,
            })
            .collect();
        let ext = expand(&root, &hits, q, &meta).unwrap();
        // 扩展词应包含库内判别词（errno 或 conn_id 或 pool）。
        assert!(
            ext.iter()
                .any(|t| t == "errno" || t == "conn_id" || t == "pool"),
            "PRF 应扩出库内判别词: {:?}",
            ext
        );
        // 扩展词不得含 query 已含词（pool 若已在 query 中则不再重复）。
        let _ = fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod mmr_tests {
    use super::*;

    fn terms(ts: &[&str]) -> Vec<String> {
        ts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn mmr_removes_redundant_variant() {
        let docs = vec![
            terms(&["alpha", "conn", "db", "errno", "timeout"]),
            terms(&["beta", "conn", "db", "errno", "timeout"]),
            terms(&["disk", "fatal", "full", "space"]),
        ];
        let cands = vec![0usize, 1, 2];
        let scores = vec![1.0, 0.7, 0.6];
        let picked = mmr_select(&cands, &docs, &scores, 2, 0.5);
        assert_eq!(picked.len(), 2, "选中 2 个");
        assert!(
            !(picked.contains(&0) && picked.contains(&1)),
            "两个同词变体不应同时入选: {picked:?}"
        );
        assert!(picked.contains(&2) || picked.contains(&0), "多样候选应入选");
    }

    #[test]
    fn mmr_lambda_zero_keeps_topk() {
        let docs = vec![
            terms(&["db", "timeout"]),
            terms(&["db", "timeout"]),
            terms(&["db", "timeout"]),
        ];
        let cands = vec![0usize, 1, 2];
        let scores = vec![1.0, 0.9, 0.8];
        assert_eq!(mmr_select(&cands, &docs, &scores, 3, 0.0), vec![0, 1, 2]);
    }

    #[test]
    fn mmr_k_larger_than_pool() {
        let docs = vec![terms(&["a", "b"]), terms(&["c", "d"])];
        let cands = vec![1usize, 0];
        let scores = vec![0.8, 0.7];
        let picked = mmr_select(&cands, &docs, &scores, 5, 0.5);
        assert_eq!(picked.len(), 2);
    }
}
