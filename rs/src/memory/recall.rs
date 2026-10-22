//! 召回：线性扫描 BM25 + 结构化字段加权（O1）+ 时间衰减（O4）+ 场景桶过滤（O6）。
//!
//! P1 阶段规模决策：库 <1 万 case 时线性扫描（每次召回现算 df/avgdl），
//! P3.1 起：库 ≥ 1 万（或 --rebuild 强制，meta index_force=1）自动启用倒排索引
//! （index/postings.bin），候选集来自 term→doc 倒排，df/avgdl 为全库口径。
//! 零依赖、无随机源、确定性：同 query 同库同输出。
//!
//! 公式：bm25(t,d)=idf·tf·(k1+1)/(tf+k1·(1−b+b·dl/avgdl))；idf=ln(1+(N−df+0.5)/(df+0.5))
//! score=Σ bm25+Σ 字段命中×权重；decay=1/(1+age/τ)（τ:90/solved 180/regressed 30）；sim=score·decay/score_max
//! 场景桶（O6）：query 判候选桶，case 主桶不在候选桶集则跳过（候选桶空=全量扫防漏召）。

// 非 test 构建暂放宽 dead_code（CLI 接线在 P1.5 前无调用点）。#![cfg_attr(not(test), allow(dead_code))]

use crate::memory::bucket::candidate_buckets;
use crate::memory::field;
use crate::memory::index::{self, Index};
use crate::memory::store::{self, Meta};
use crate::terms::build_terms;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct RecallHit {
    pub id: String,
    pub score: f64,
    pub sim: f64,
}

/// 候选 case 的召回上下文：一次读取，打分零二次 IO（P3 性能门：1 万条库 <5ms）。
struct DocCtx {
    terms: Vec<String>,
    fields: super::store::Fields,
    status: String,
    quality: f64,
    updated_at: String,
    /// P4.3 实体边：与 query 共享的实体键数（0 = 词面候选，>0 = 桥入兄弟 case）。
    edge_shared: usize,
}

/// 打分中间结构：候选 (id, 上下文) 列表 + query 词 df 表。
type Docs = Vec<(String, DocCtx)>;
type Df = HashMap<String, usize>;

/// 归一化得分 → 排序；sim 独立供 L1 置信判定。
pub(crate) fn recall(root: &str, query: &str, k: usize) -> std::io::Result<Vec<RecallHit>> {
    let meta = store::load_meta(root)?;
    // O2 query 记忆图：精确同指纹直接返回历史 top-k（跳过整个 BM25 管线）。
    // 代际失效：条目 gen < 当前 meta.gen 视为过期（库已变化，走 BM25 重算）。
    if let Some((_, qhits, 0)) = crate::memory::qmap::lookup(root, query, 2, meta.gen)? {
        let mut hits: Vec<RecallHit> = qhits
            .into_iter()
            .map(|h| RecallHit {
                id: h.id,
                score: h.score as f64,
                sim: h.sim as f64,
            })
            .collect();
        hits.truncate(k);
        return Ok(hits);
    }
    // P3.1 倒排路径：规模达标或 --rebuild 强制（index_force=1）时启用。
    // 索引缺失/坏段（P3.2）→ 自动重建覆盖（build 确定性），重建失败回退线性扫描。
    let idx = if index::use_index(&meta) {
        match index::load(root) {
            Ok(Some(i)) => Some(i),
            _ => {
                let _ = index::build(root);
                index::load(root).ok().flatten()
            }
        }
    } else {
        None
    };
    let hits = score_pass(root, query, k, &meta, idx.as_deref())?;
    // P2.3 PRF：低置信（top1.sim < 0.99）时粗召回 top-K 扩展查询再召一轮。
    // 编辑距离 ≤2 变体命中也在本轮覆盖（轻量重排 = 词面重新打分）。
    let low_conf = hits.first().is_some_and(|h| h.sim < 0.99);
    if low_conf {
        let top_ids: Vec<String> = hits.iter().map(|h| h.id.clone()).collect();
        if let Ok(ext) = crate::memory::prf::expand(root, &top_ids, query, &meta) {
            if !ext.is_empty() {
                let q2 = format!("{query} {}", ext.join(" "));
                if let Ok(hits2) = score_pass(root, &q2, k, &meta, idx.as_deref()) {
                    if !hits2.is_empty() {
                        return Ok(hits2);
                    }
                }
            }
        }
    }
    Ok(hits)
}

/// 单轮打分：BM25 + 字段加权 + 时间衰减 + 桶过滤。
/// `idx` 为 Some 时候选集来自倒排（全库 df/avgdl），None 时线性扫描现算。
fn score_pass(
    root: &str,
    query: &str,
    k: usize,
    meta: &Meta,
    idx: Option<&Index>,
) -> std::io::Result<Vec<RecallHit>> {
    let k1 = param(meta, "bm25_k1", 1.2);
    let b = param(meta, "bm25_b", 0.75);
    let tau = param(meta, "decay_tau_days", 90.0);
    let fw = [
        param(meta, "field_weight_error_code", 3.0),
        param(meta, "field_weight_file_loc", 2.5),
        param(meta, "field_weight_symbol", 2.0),
        param(meta, "field_weight_num_fp", 2.0),
    ];
    // P4.3 实体边：共享实体加权（0 则关，兼容老 meta 缺省开 1.0）。
    let edge_w = param(meta, "edge_weight", 1.0);
    let qterms = build_terms(query);
    let qfields = field::extract(query);
    let qf = [
        &qfields.error_code,
        &qfields.file_loc,
        &qfields.symbol,
        &qfields.num_fp,
    ];
    // P4.3 实体边一跳：query 实体键集合（去重在 edge 侧）。
    let q_ent_list: Vec<String> = qf.iter().flat_map(|f| f.iter().cloned()).collect();

    // O6 场景桶：query 判候选桶，case 主桶不在候选桶集内则跳过。
    let cand = candidate_buckets(query);
    let use_buckets = !cand.is_empty();

    // 候选 docs + df + 统计量：倒排路径（全库口径）或线性路径（候选集口径）。
    let (docs, df, n, avgdl): (Docs, Df, f64, f64) = if let Some(idx) = idx {
        // 索引路径：doc 打分快照自包含在 postings 内，候选打分零 case.json IO。
        let cand_di = index::candidates(idx, &qterms);
        let mut docs: Vec<(String, DocCtx)> = Vec::with_capacity(cand_di.len());
        for di in cand_di {
            let rec = &idx.docs[di];
            if use_buckets && !cand.iter().any(|cb| cb == &rec.bucket) {
                continue;
            }
            docs.push((
                idx.ids[di].clone(),
                DocCtx {
                    terms: rec.terms.clone(),
                    fields: rec.fields.clone(),
                    status: rec.status.clone(),
                    quality: rec.quality,
                    updated_at: rec.updated_at.clone(),
                    edge_shared: 0,
                },
            ));
        }
        // P4.3 实体边一跳：query 实体 → 兄弟 case 兜底并入（df/avgdl 保持 idx 全库口径）。
        let exist_ids: Vec<String> = docs.iter().map(|(id, _)| id.clone()).collect();
        let (bridges, _) =
            crate::memory::edge::bridge_cases(root, &meta.params, &q_ent_list, &exist_ids)?;
        for b in bridges {
            docs.push((
                b.id,
                DocCtx {
                    terms: b.terms,
                    fields: b.fields,
                    status: b.status,
                    quality: b.quality,
                    updated_at: b.updated_at,
                    edge_shared: b.edge_shared,
                },
            ));
        }
        if docs.is_empty() {
            return Ok(Vec::new());
        }
        let n = idx.n_docs as f64;
        let avgdl = idx.total_len as f64 / n;
        let df: HashMap<String, usize> = qterms
            .iter()
            .map(|t| (t.clone(), index::term_df(idx, t)))
            .collect();
        (docs, df, n, avgdl)
    } else {
        let ids = store::list_ids(root)?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut docs: Vec<(String, DocCtx)> = Vec::with_capacity(ids.len());
        let mut df: HashMap<String, usize> = HashMap::new();
        let mut total_len = 0usize;
        for id in &ids {
            let c = store::read_case(root, id)?;
            if use_buckets && !cand.iter().any(|cb| cb == &c.bucket) {
                continue;
            }
            let mut terms = build_terms(&c.query);
            terms.extend(build_terms(&c.text));
            let mut seen: Vec<&str> = Vec::with_capacity(terms.len());
            for t in &terms {
                if !seen.contains(&t.as_str()) {
                    seen.push(t.as_str());
                    *df.entry(t.clone()).or_insert(0) += 1;
                }
            }
            terms.sort();
            terms.dedup(); // 与索引路径 DocRec.terms 同口径（去重词表，tf 每词=1）
            total_len += terms.len();
            docs.push((
                id.clone(),
                DocCtx {
                    terms,
                    fields: c.fields,
                    status: c.status,
                    quality: c.quality,
                    updated_at: c.updated_at,
                    edge_shared: 0,
                },
            ));
        }
        // P4.3 实体边一跳：桥 case 并入后 avgdl 重算（df 保持词面候选口径）。
        let exist_ids: Vec<String> = docs.iter().map(|(id, _)| id.clone()).collect();
        let (bridges, bridge_len) =
            crate::memory::edge::bridge_cases(root, &meta.params, &q_ent_list, &exist_ids)?;
        for b in bridges {
            docs.push((
                b.id,
                DocCtx {
                    terms: b.terms,
                    fields: b.fields,
                    status: b.status,
                    quality: b.quality,
                    updated_at: b.updated_at,
                    edge_shared: b.edge_shared,
                },
            ));
        }
        if docs.is_empty() {
            return Ok(Vec::new());
        }
        let n = docs.len() as f64;
        let avgdl = (total_len + bridge_len) as f64 / n;
        (docs, df, n, avgdl)
    };

    let now_s = crate::memory::util::parse_utc_secs(&crate::memory::util::utc_now());
    // O5 词级调权表：query 词命中时按词权重加成（§3.9，无词条=0）。
    let ww: HashMap<&str, f64> = meta.word_w.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    // O5 quality 排序先验（§4.1）：solved 且质量高的 case 提分。
    let q_prior = param(meta, "quality_prior", 0.5);

    let mut hits: Vec<RecallHit> = Vec::with_capacity(docs.len());
    for (id, dctx) in &docs {
        let dl = dctx.terms.len() as f64;
        let mut tf: HashMap<&str, usize> = HashMap::new();
        for t in &dctx.terms {
            *tf.entry(t.as_str()).or_insert(0) += 1;
        }
        let mut score = 0.0;
        for qt in &qterms {
            let d = *df.get(qt).unwrap_or(&0) as f64;
            let idf = ((1.0 + (n - d + 0.5) / (d + 0.5)).ln()).max(0.0);
            let f = *tf.get(qt.as_str()).unwrap_or(&0) as f64;
            if f > 0.0 {
                let denom = f + k1 * (1.0 - b + b * dl / avgdl);
                score += idf * f * (k1 + 1.0) / denom;
                // 词级调权：命中词权重叠加（可正可负）。
                if let Some(w) = ww.get(qt.as_str()) {
                    score += idf * w;
                }
            }
        }
        let mut fb = 0.0;
        let cfields = [
            &dctx.fields.error_code,
            &dctx.fields.file_loc,
            &dctx.fields.symbol,
            &dctx.fields.num_fp,
        ];
        for ((qf, cf), w) in qf.iter().zip(cfields.iter()).zip(fw.iter()) {
            for v in qf.iter() {
                if cf.iter().any(|c| c == v) {
                    fb += w;
                }
            }
        }
        // O4 时间衰减：age 用 updated_at（去重/反馈会刷新它），τ 随状态动态。
        let tau_eff = match dctx.status.as_str() {
            "solved" => param(meta, "decay_tau_solved", 180.0),
            "regressed" => param(meta, "decay_tau_regressed", 30.0),
            _ => tau,
        };
        let upd_s = crate::memory::util::parse_utc_secs(&dctx.updated_at);
        let age_days = (now_s.saturating_sub(upd_s)) as f64 / 86_400.0;
        let decay = 1.0 / (1.0 + age_days / tau_eff);
        // O5 quality 排序先验（§4.1）：solved/质量高的 case 提分，regressed 压分。
        let qsign = if dctx.status == "regressed" {
            -1.0
        } else {
            1.0
        };
        fb += dctx.quality * q_prior * qsign;
        // P4.3 实体边加权：共享实体数 × edge_weight（词面弱但实体强相关的兄弟 case 越线）。
        if dctx.edge_shared > 0 {
            fb += dctx.edge_shared as f64 * edge_w;
        }
        hits.push(RecallHit {
            id: id.clone(),
            score: (score + fb) * decay,
            sim: 0.0,
        });
    }

    // score_max：所有 query 词 idf 加字段全命中，作 sim 归一化分母。
    // 直觉：query 每个词在 case 中都命中一次 ≈ sim=1（tf>1 或字段命中可再溢出）。
    let mut sm = 0.0;
    for qt in &qterms {
        let d = *df.get(qt).unwrap_or(&0) as f64;
        sm += ((1.0 + (n - d + 0.5) / (d + 0.5)).ln()).max(0.0);
    }
    for (f_list, w) in qf.iter().zip(fw.iter()) {
        sm += f_list.len() as f64 * w;
    }
    for h in hits.iter_mut() {
        h.sim = if sm > 0.0 {
            (h.score / sm).clamp(0.0, 1.0)
        } else {
            0.0
        };
    }

    // P2.5 MMR：得分排序后按多样性去冗余（λ 从 meta 读）。
    let mut order: Vec<usize> = (0..hits.len()).collect();
    order.sort_by(|&a, &b| {
        hits[b]
            .score
            .partial_cmp(&hits[a].score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let lambda = param(meta, "mmr_lambda", 0.5);
    let scores_norm: Vec<f64> = hits.iter().map(|h| h.sim).collect();
    let doc_terms: Vec<Vec<String>> = docs.iter().map(|(_, d)| d.terms.clone()).collect();
    let picked = crate::memory::mmr::select(&order, &doc_terms, &scores_norm, k, lambda);
    let mut out: Vec<RecallHit> = Vec::with_capacity(picked.len());
    for &i in &picked {
        out.push(hits[i].clone());
    }
    Ok(out)
}

fn param(m: &Meta, key: &str, dflt: f64) -> f64 {
    m.params
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| *v)
        .unwrap_or(dflt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::ingest::ingest;

    const L1_SIM: f64 = 0.55;
    use crate::memory::store::{self, CaseStats};
    use std::fs;

    fn tmp_root(tag: &str) -> String {
        let r =
            std::env::temp_dir().join(format!("scx-recall-{}-{}", tag, store::fnv1a("y") & 0xff));
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
    fn recall_roundtrip_same_query() {
        let root = tmp_root("rt");
        ingest(
            &root,
            "数据库连接超时 conn_id",
            vec!["conn_id".into()],
            "ERROR db timeout conn_id=9f2a errno=28\nat Pool.java:214",
            vec!["/x.log".into()],
            &stats(),
            vec![],
        )
        .unwrap();
        let hits = recall(&root, "数据库连接超时 conn_id", 3).unwrap();
        assert!(!hits.is_empty(), "同 query 应召回原 case");
        assert!(
            hits[0].sim > L1_SIM,
            "同 query 置信应过 L1 阈值：sim={}",
            hits[0].sim
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn recall_field_priority_over_word_overlap() {
        let root = tmp_root("fp");
        // A：含目标 error_code 的 case
        ingest(
            &root,
            "查询连接池",
            vec!["conn_id".into()],
            "ERROR db timeout conn_id=9f2a\nat Pool.java:214",
            vec!["/a.log".into()],
            &stats(),
            vec![],
        )
        .unwrap();
        // B：词面更贴近 query 但没有该 error_code
        ingest(
            &root,
            "查询连接池",
            vec!["查询".into(), "连接池".into()],
            "查询连接池 状态 ok 全部在线",
            vec!["/b.log".into()],
            &stats(),
            vec![],
        )
        .unwrap();
        // 字段变体 query：含同 error_code，词面与 A 不同
        let hits = recall(&root, "conn_id=9f2a 又超时了", 2).unwrap();
        assert!(!hits.is_empty());
        let top = &hits[0];
        let top_case = store::read_case(&root, &top.id).unwrap();
        assert!(
            top_case
                .fields
                .error_code
                .iter()
                .any(|v| v == "conn_id=9f2a"),
            "字段精确命中应压过纯词面：top={}",
            top.id
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn time_decay_prefers_fresh_case() {
        let root = tmp_root("td");
        let s1 = stats();
        // A 新鲜、B 老旧：query/字段词面相同但证据文本不同（指纹不同，不会去重）。
        ingest(
            &root,
            "数据库连接超时",
            vec!["conn_id".into()],
            "ERROR db timeout conn_id=9f2a errno=28",
            vec!["/a.log".into()],
            &s1,
            vec![],
        )
        .unwrap();
        ingest(
            &root,
            "数据库连接超时",
            vec!["conn_id".into()],
            "ERROR db timeout conn_id=9f2a errno=28 socket",
            vec!["/b.log".into()],
            &s1,
            vec![],
        )
        .unwrap();
        let ids = store::list_ids(&root).unwrap();
        assert_eq!(ids.len(), 2, "证据文本不同不应去重");
        // 人为把 B 的 updated_at 拨老 90 天。
        let mut c = store::read_case(&root, &ids[0]).unwrap();
        let now = store::utc_now();
        let older = format!("2024-07-24{}", &now[10..]);
        c.updated_at = older;
        store::write_case(&root, &c).unwrap();

        let hits = recall(&root, "数据库连接超时", 2).unwrap();
        assert_eq!(hits.len(), 2);
        let fresh_id = ids.iter().find(|i| **i != ids[0]).unwrap();
        assert_eq!(
            hits[0].id,
            *fresh_id,
            "新鲜 case 应排最前（decay 生效）: {:?}",
            hits.iter().map(|h| (&h.id, h.score)).collect::<Vec<_>>()
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn bucket_filter_reduces_scan() {
        let root = tmp_root("bf");
        let s1 = stats();
        // A：db 桶；B：build 桶。
        ingest(
            &root,
            "数据库超时",
            vec!["conn_id".into()],
            "ERROR db timeout conn_id=9f2a",
            vec!["/a.log".into()],
            &s1,
            vec![],
        )
        .unwrap();
        ingest(
            &root,
            "cargo build",
            vec!["cargo".into()],
            "error: could not compile hello",
            vec!["/b.log".into()],
            &s1,
            vec![],
        )
        .unwrap();
        // db query 只能召到 A（B 被桶过滤）。
        let hits = recall(&root, "数据库超时 conn_id", 5).unwrap();
        let top_case = store::read_case(&root, &hits[0].id).unwrap();
        assert_eq!(top_case.bucket, "db", "桶过滤后 top 必须是 db 桶");
        assert_eq!(hits.len(), 1, "build 桶 case 不应参与打分");
        // 无家族命中的 query：候选桶空 → 全量扫（防漏召）。
        let all = recall(&root, "hello", 5).unwrap();
        assert_eq!(all.len(), 2);
        let _ = fs::remove_dir_all(&root);
    }
}
