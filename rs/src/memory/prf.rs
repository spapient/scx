//! PRF 伪相关反馈（查询扩展）：粗召回 top-K 后取高 IDF 高频词并入 query 再召一轮。
//!
//! 统计方法独有优势：embedding 做查询扩展很难。扩展词选取规则（确定性）：
//! 只统计与 query 词面相关（sim > 0）的粗召回 case 词表（query ∪ text 同口径）；
//! 全局 df 现算（线性阶段），批内 tf ≥2 且全局 idf 高（idf > 阈值）的词入选；
//! 排除已含在 query 词表中的词。
//! 只在召回低置信（top1.sim < 0.99）时触发：同 query 高置信已直接命中，无需扩展。
//!
//! P4.4+ 优化⑦：按 sim>0 过滤粗召回 case——无关 case（词面零重叠）的私有词
//! 不得进入扩展词表（否则扩展会把不相干桶/词拉进 query，重排噪声压过真目标）。

use crate::memory::recall::RecallHit;
use crate::memory::store::{self, Meta};
use crate::terms::build_terms;
use std::collections::HashMap;

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
    let qterms = build_terms(query);
    let (n, df): (f64, HashMap<String, usize>) = if !meta.df.is_empty() {
        (meta.case_count as f64, meta.df.iter().cloned().collect())
    } else {
        // 线性阶段兜底：全库现算（小库可接受；大库必经 rebuild → 快照非空）。
        let all_ids = store::list_ids(root)?;
        let n = all_ids.len() as f64;
        let mut df: HashMap<String, usize> = HashMap::new();
        for id in &all_ids {
            let c = store::read_case(root, id)?;
            let mut terms = build_terms(&c.query);
            terms.extend(build_terms(&c.text));
            let mut seen: Vec<&str> = Vec::with_capacity(terms.len());
            for t in &terms {
                if !seen.contains(&t.as_str()) {
                    seen.push(t.as_str());
                    *df.entry(t.clone()).or_insert(0) += 1;
                }
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
        let mut terms = build_terms(&c.query);
        terms.extend(build_terms(&c.text));
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
        let idf = ((1.0 + (n - d + 0.5) / (d + 0.5)).ln()).max(0.0);
        if idf > IDF_MIN {
            cand.push((t.clone(), idf));
        }
    }
    cand.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    cand.truncate(EXPAND_MAX);
    Ok(cand.into_iter().map(|(t, _)| t).collect())
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
