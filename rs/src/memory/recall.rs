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
use crate::memory::synonyms;
use crate::terms::{build_doc_terms, build_mem_terms};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub struct RecallHit {
    pub id: String,
    pub score: f64,
    pub sim: f64,
}

/// 候选 case 的召回上下文：一次读取，打分零二次 IO（P3 性能门：1 万条库 <5ms）。
struct DocCtx {
    terms: Arc<[String]>,
    /// 原始 case query 词表（线性/索引/实体桥路径同口径；query-field 重排）。
    query_terms: Arc<[String]>,
    fields: Arc<super::store::Fields>,
    status: Arc<str>,
    quality: f64,
    updated_at: Arc<str>,
    /// P4.3 实体边：与 query 共享的实体键数（0 = 词面候选，>0 = 桥入兄弟 case）。
    edge_shared: usize,
}

/// 打分中间结构：候选 (id, 上下文) 列表 + query 词 df 表。
type Docs = Vec<(String, DocCtx)>;

/// 单轮打分的 query 词快照：每个词只算一次 df/idf/权重。
struct QueryTerm<'a> {
    text: &'a String,
    idf: f64,
    weight: f64,
    word_w: f64,
    original: bool,
}

/// 单轮打分的只读上下文；集中传参避免函数参数膨胀。
struct PassCtx<'a> {
    k: usize,
    meta: &'a Meta,
    /// 冻结的评估时钟（Unix 秒）；线上入口传当前时间。
    now_s: u64,
    idx: Option<&'a Index>,
    cand: &'a [String],
    use_buckets: bool,
    qf_weight: f64,
    qf_exact_only: bool,
}

/// 归一化得分 → 排序；sim 独立供 L1 置信判定。
pub(crate) fn recall(root: &str, query: &str, k: usize) -> std::io::Result<Vec<RecallHit>> {
    let now_s = crate::memory::util::utc_now_secs();
    recall_at(root, query, k, now_s)
}

/// 指定时间戳的召回：供评估/自检冻结衰减时钟，同库快照跨天可重放。
pub(crate) fn recall_at(
    root: &str,
    query: &str,
    k: usize,
    now_s: u64,
) -> std::io::Result<Vec<RecallHit>> {
    let meta = store::load_meta(root)?;
    // O2 query 记忆图：精确同指纹直接返回历史 top-k（跳过整个 BM25 管线）。
    // 代际失效：条目 gen < 当前 meta.gen 视为过期（库已变化，走 BM25 重算）。
    if let Some((_, qhits, 0)) = crate::memory::query_cache::lookup(root, query, 2, meta.gen)? {
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
    // O6 场景桶：候选桶由原始 query 判定一次，首轮与 PRF 第二轮共用（防扩展词带偏过滤）。
    let cand = candidate_buckets(query);
    // 消融开关（默认全开）：bucket_enable/prf_enable 只服务可复现的 A−组件对拍，
    // 不改变默认检索行为。
    let mut use_buckets = !cand.is_empty() && meta.param("bucket_enable") > 0.0;
    let qf_weight = meta.param("query_field_weight");
    let qf_trigger = meta.param("query_field_trigger_sim");
    let high_sim = meta.param("confidence_high_sim");
    let mut hits = score_pass(
        root,
        query,
        &PassCtx {
            k,
            meta: &meta,
            now_s,
            idx: idx.as_deref(),
            cand: &cand,
            use_buckets,
            qf_weight: 0.0,
            qf_exact_only: false,
        },
    )?;
    // 低置信查询不能继续用场景桶硬筛：词面弱时，query 的桶证据可能只来自
    // 一个通用词（例如 thread→runtime），而目标 case 的主桶是 db。硬筛会
    // 在排序前直接删掉目标，任何字段重排都救不回来。此时退回无桶候选，
    // 保留 BM25 全量排序；高置信查询仍走桶过滤以控制扫描范围。
    let first_sim = hits.first().map(|h| h.sim).unwrap_or(0.0);
    let bucket_bypass_sim = meta.param("bucket_bypass_sim");
    if use_buckets && first_sim < bucket_bypass_sim {
        if let Ok(full_hits) = score_pass(
            root,
            query,
            &PassCtx {
                k,
                meta: &meta,
                now_s,
                idx: idx.as_deref(),
                cand: &cand,
                use_buckets: false,
                qf_weight: 0.0,
                qf_exact_only: false,
            },
        ) {
            hits = full_hits;
            use_buckets = false;
        }
    }
    // case-query 字段加权只在两种情况下启用：①低/中置信释义查询
    // （top1.sim<query_field_trigger_sim，默认 0.30）；
    // ②top1/top2 都是满分、但排序按正文长度/字段打破时（同事故兄弟 case 竞争）。
    // 高置信且无并列的 query 不改排序，避免把同域候选抬上来。
    let exact_tie = hits.len() >= 2 && hits[0].sim >= high_sim && hits[1].sim >= high_sim;
    let low_semantic = hits.first().is_some_and(|h| h.sim < qf_trigger);
    // qf 强度随词面置信度下降而平滑增大：just-below-trigger 的查询只得到
    // 很小的重排推力，避免一个宽泛的 case-query 词组（如 “retry storm”）
    // 盖过正文里更强的完整证据；真正的低置信释义仍获得接近全额的 qf。
    //
    // 低置信分支只对含 CJK 的 query 生效：中文 query 与英文 case title 的
    // 词面 sim 天然偏低，不能把“低 sim”直接当成“需要 title 重排”。英文
    // literal 也可能低于阈值（例如 confirm2 case 136），此时 title 中的
    // 2 个常见词（retry/storm）会把错误兄弟 case 抬到目标之前。英文的
    // title 重排只保留在 exact-tie 分支（top1/top2 同为满分的兄弟 case）。
    let query_has_cjk = query.chars().any(crate::terms::is_cjk);
    let top_sim = hits.first().map(|h| h.sim).unwrap_or(0.0);
    let qf_strength = query_field_strength(
        qf_weight,
        qf_trigger,
        top_sim,
        exact_tie,
        low_semantic,
        query_has_cjk,
    );
    let use_qf = qf_strength > 1e-9;
    // 并列满分时只奖原始 query 词，不奖同义词扩展：否则 `config` 与
    // `configuration` 会同时得分，兄弟 case 仍无法区分。
    let qf_exact_only = exact_tie && !low_semantic;
    if use_qf {
        if let Ok(qf_hits) = score_pass(
            root,
            query,
            &PassCtx {
                k,
                meta: &meta,
                now_s,
                idx: idx.as_deref(),
                cand: &cand,
                use_buckets,
                qf_weight: qf_strength,
                qf_exact_only,
            },
        ) {
            if !qf_hits.is_empty() {
                hits = qf_hits;
            }
        }
    }
    // P2.3 PRF：低置信（top1.sim < confidence_high_sim）时粗召回 top-K 扩展查询再召一轮。
    // 编辑距离 ≤2 变体命中也在本轮覆盖（轻量重排 = 词面重新打分）。
    let low_conf = meta.param("prf_enable") > 0.0 && hits.first().is_some_and(|h| h.sim < high_sim);
    if low_conf {
        if let Ok(ext) = crate::memory::ranking::expand(root, &hits, query, &meta) {
            if !ext.is_empty() {
                let q2 = format!("{query} {}", ext.join(" "));
                // P4.4+ 优化⑦：第二轮继承首轮候选桶（桶由原始 query 判定），
                // 扩展词不得改变过滤语义——否则跨语言变体/无关词会把真目标桶误滤（A/B 不可比）。
                if let Ok(hits2) = score_pass(
                    root,
                    &q2,
                    &PassCtx {
                        k,
                        meta: &meta,
                        now_s,
                        idx: idx.as_deref(),
                        cand: &cand,
                        use_buckets,
                        qf_weight: if use_qf { qf_strength } else { 0.0 },
                        qf_exact_only,
                    },
                ) {
                    if !hits2.is_empty() {
                        // PRF 严格补漏（P4.4+ 优化⑦）：首轮顺序权威——首轮命中
                        // （含实体边桥入、字段加权、桶内候选）位置不变；扩展轮只把
                        // 新命中的 case 按 q2 排序追加在尾部。绝不与首轮 score 混排
                        // （q2 口径 score 会被扩展词放大，混排会把真目标挤出——literal
                        // 层 mrr 退化实证：case 'disk io read timeout' A rank3 vs B rank1）。
                        let mut merged: Vec<RecallHit> = hits.clone();
                        for h in hits2 {
                            if !merged.iter().any(|m| m.id == h.id) {
                                merged.push(h);
                            }
                        }
                        merged.truncate(k);
                        return Ok(merged);
                    }
                }
            }
        }
    }
    Ok(hits)
}

/// case-query 字段重排强度：exact tie 全量；CJK 低置信按 sim 线性衰减；
/// 其余为 0。抽出纯函数使“英文低置信不得触发 title 重排”可单测。
fn query_field_strength(
    qf_weight: f64,
    trigger: f64,
    top_sim: f64,
    exact_tie: bool,
    low_semantic: bool,
    query_has_cjk: bool,
) -> f64 {
    if exact_tie && !low_semantic {
        return qf_weight;
    }
    if low_semantic && query_has_cjk {
        let denom = trigger.max(1e-6);
        return qf_weight * ((denom - top_sim) / denom).clamp(0.0, 1.0);
    }
    0.0
}

/// 桶过滤：候选桶与 case 多标签桶有交集即保留；无桶标签的 case 按 `other`
/// 处理（`bucket_allow_other=1` 时放行）。空候选桶表示全量扫。
fn bucket_allowed(buckets: &[String], cand: &[String], allow_other: bool) -> bool {
    if cand.is_empty() {
        return true;
    }
    if buckets.iter().any(|b| cand.iter().any(|c| c == b)) {
        return true;
    }
    allow_other && (buckets.is_empty() || buckets.iter().any(|b| b == "other"))
}

/// 单轮打分：BM25 + 字段加权 + 时间衰减 + 桶过滤。
/// `idx` 为 Some 时候选集来自倒排（全库 df/avgdl），None 时线性扫描现算。
/// `cand`/`use_buckets` 由调用方统一判定（首轮=原始 query 桶；PRF 第二轮继承同一桶）。
fn score_pass(root: &str, query: &str, ctx: &PassCtx<'_>) -> std::io::Result<Vec<RecallHit>> {
    let k = ctx.k;
    let meta = ctx.meta;
    let idx = ctx.idx;
    let cand = ctx.cand;
    let use_buckets = ctx.use_buckets;
    let qf_weight = ctx.qf_weight;
    let qf_exact_only = ctx.qf_exact_only;
    let k1 = meta.param("bm25_k1");
    let b = meta.param("bm25_b");
    let tau = meta.param("decay_tau_days");
    let tau_solved = meta.param("decay_tau_solved");
    let tau_regressed = meta.param("decay_tau_regressed");
    let fw = [
        meta.param("field_weight_error_code"),
        meta.param("field_weight_file_loc"),
        meta.param("field_weight_symbol"),
        meta.param("field_weight_num_fp"),
    ];
    // P4.3 实体边：共享实体加权（0 则关，兼容老 meta 缺省开 1.0）。
    let edge_w = meta.param("edge_weight");
    let orig_qterms = build_mem_terms(query);
    let qterms = if meta.param("synonyms_enable") > 0.0 {
        synonyms::expand_terms(&orig_qterms)
    } else {
        orig_qterms.clone()
    };
    // 同义扩展是补召回机制，不应与用户原词等权：等权时一个宽泛的扩展词
    // 可以把同域兄弟 case 抬到精确目标之前（confirm3 paraphrase 回归）。
    // 半权是“保留补召回、限制误抬升”的默认折中；置 1 可恢复旧行为。
    let syn_w = meta.param("synonym_weight").clamp(0.0, 1.0);
    // `other` = 分类器没有强家族证据；默认放行（宁多勿漏）。
    // 置 0 可恢复旧的硬筛行为（仅用于对拍）。
    let allow_other = meta.param("bucket_allow_other") > 0.0;
    let qfields = field::extract(query);
    let qf = [
        &qfields.error_code,
        &qfields.file_loc,
        &qfields.symbol,
        &qfields.num_fp,
    ];
    // P4.3 实体边一跳：query 实体键集合（去重在 edge 侧）。
    let q_ent_list: Vec<String> = qf.iter().flat_map(|f| f.iter().cloned()).collect();

    // O6 场景桶：query 判候选桶，case 主桶不在候选桶集内则跳过（由调用方统一传入）。

    // 候选 docs + df + 统计量：倒排路径（全库口径）或线性路径（候选集口径）。
    let (docs, df, n, avgdl): (Docs, Vec<usize>, f64, f64) = if let Some(idx) = idx {
        // 索引路径：doc 打分快照自包含在 postings 内，候选打分零 case.json IO。
        let cand_di = index::candidates(idx, &qterms);
        let mut chosen: Vec<(usize, usize)> = Vec::with_capacity(cand_di.len());
        for di in cand_di {
            let rec = &idx.docs[di];
            // `other` = 分类器没有强家族证据；硬筛会把它误当成“跨域噪声”。
            // 默认允许 other 通过（宁多勿漏），可用 bucket_allow_other=0 关闭。
            if use_buckets && !bucket_allowed(&rec.buckets, cand, allow_other) {
                continue;
            }
            chosen.push((di, 0));
        }
        // P4.3 实体边一跳：实体倒排由 postings 的 doc 快照派生，桥入无需重读
        // 全库 case.json（旧实现每次冷启动重扫 1 万条，单次约 0.2s）。
        if meta.param("edge_enable") > 0.0 && !q_ent_list.is_empty() {
            let mut seen = vec![false; idx.n_docs];
            for &(di, _) in &chosen {
                seen[di] = true;
            }
            for (di, shared) in index::entity_neighbors(idx, &q_ent_list) {
                if !seen[di] {
                    seen[di] = true;
                    chosen.push((di, shared));
                }
            }
        }
        let docs: Vec<(String, DocCtx)> = chosen
            .into_iter()
            .map(|(di, edge_shared)| {
                let rec = &idx.docs[di];
                (
                    idx.ids[di].clone(),
                    DocCtx {
                        terms: rec.terms.clone(),
                        query_terms: rec.query_terms.clone(),
                        fields: rec.fields.clone(),
                        status: rec.status.clone(),
                        quality: rec.quality,
                        updated_at: rec.updated_at.clone(),
                        edge_shared,
                    },
                )
            })
            .collect();
        if docs.is_empty() {
            return Ok(Vec::new());
        }
        let n = idx.n_docs as f64;
        let avgdl = idx.total_len as f64 / n;
        let df: Vec<usize> = qterms.iter().map(|t| index::term_df(idx, t)).collect();
        (docs, df, n, avgdl)
    } else {
        let ids = store::list_ids(root)?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut docs: Vec<(String, DocCtx)> = Vec::with_capacity(ids.len());
        let mut df = vec![0usize; qterms.len()];
        let mut total_len = 0usize;
        for id in &ids {
            let c = store::read_case(root, id)?;
            let (query_terms, terms) = build_doc_terms(&c.query, &c.text);
            let buckets = crate::memory::bucket::case_buckets(&[], &query_terms, &c.text);
            total_len += terms.len();
            for (qi, qt) in qterms.iter().enumerate() {
                if terms.binary_search(qt).is_ok() {
                    df[qi] += 1;
                }
            }
            // BM25 的 df/avgdl 必须保持全库口径；桶过滤只裁候选，不能改统计量。
            // 否则同一查询在“线性+桶”与“索引+桶”下会得到不同的长度归一化，
            // 也会让 A/B 在候选子集上比较不同的 BM25（confirm3 paraphrase 回归）。
            if use_buckets && !bucket_allowed(&buckets, cand, allow_other) {
                continue;
            }
            docs.push((
                id.clone(),
                DocCtx {
                    terms: terms.into(),
                    query_terms: query_terms.into(),
                    fields: Arc::new(c.fields),
                    status: c.status.into(),
                    quality: c.quality,
                    updated_at: c.updated_at.into(),
                    edge_shared: 0,
                },
            ));
        }
        // P4.3 实体边一跳：桥 case 并入后 avgdl 重算（df 保持词面候选口径）。
        let exist_ids: Vec<String> = docs.iter().map(|(id, _)| id.clone()).collect();
        let (bridges, _) = crate::memory::edge::bridge_cases(root, meta, &q_ent_list, &exist_ids)?;
        for b in bridges {
            docs.push((
                b.id,
                DocCtx {
                    terms: b.terms,
                    query_terms: b.query_terms,
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
        // 与索引路径一致：n/avgdl 取全库口径，桥入 case 不改变平均文档长度。
        let n = ids.len() as f64;
        let avgdl = total_len as f64 / n;
        (docs, df, n, avgdl)
    };

    let now_s = ctx.now_s;
    // O5 词级调权表：query 词命中时按词权重加成（§3.9，无词条=0）。
    let ww: HashMap<&str, f64> = meta.word_w.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    let qinfo: Vec<QueryTerm<'_>> = qterms
        .iter()
        .zip(df.iter())
        .map(|(text, &df)| {
            let original = orig_qterms.iter().any(|t| t == text);
            QueryTerm {
                text,
                idf: crate::memory::ranking::idf(n, df as f64),
                weight: if original { 1.0 } else { syn_w },
                word_w: ww.get(text.as_str()).copied().unwrap_or(0.0),
                original,
            }
        })
        .collect();
    // O5 quality 排序先验（§4.1）：solved 且质量高的 case 提分。
    let q_prior = meta.param("quality_prior");
    // score_max：原始 query 词 idf 加字段全命中，作 sim 归一化分母。
    // 同义扩展只增加命中机会，不得抬高分母把原始 query 的 sim 稀释。
    // 直觉：query 每个词在 case 中都命中一次 ≈ sim=1（tf>1 或字段命中可再溢出）。
    let mut sm: f64 = qinfo.iter().filter(|q| q.original).map(|q| q.idf).sum();
    for (f_list, w) in qf.iter().zip(fw.iter()) {
        sm += f_list.len() as f64 * w;
    }

    let mut hits: Vec<RecallHit> = Vec::with_capacity(docs.len());
    for (id, dctx) in &docs {
        let dl = dctx.terms.len() as f64;
        // terms 在建库/桥接/线性路径均已 sort+dedup，tf 恒为 1；直接二分查存在性，
        // 避免每个候选文档分配一张 tf HashMap。
        let denom = crate::memory::ranking::denom(k1, b, dl, avgdl);
        let mut score = 0.0;
        for q in &qinfo {
            if dctx.terms.binary_search(q.text).is_ok() {
                score += q.weight * crate::memory::ranking::score(q.idf, k1, denom);
                // 词级调权：命中词权重叠加（可正可负）。
                score += q.weight * q.idf * q.word_w;
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
        // 可选 BM25F 风格的 case-query 字段加权：目标 case 的 query 字段通常
        // 比正文更短、更接近用户的问题描述。默认关闭（0），只用显式参数开启，
        // 避免在未完成 held-out 验证前改变默认检索语义。索引路径暂不携带
        // query_terms，因此该特征只在 150 条小库的线性路径生效。
        let qf_w = qf_weight;
        let mut qf_add = 0.0;
        if qf_w > 0.0 {
            // 至少 2 个原始 query 词落在 case query 字段才加权：单个常见词
            //（failed/source 等）不足以证明“问的是这条 case”，否则会把
            // 同域但不同事故的 case 抬上来。
            let need = orig_qterms.len().min(2);
            let support = orig_qterms
                .iter()
                .filter(|t| dctx.query_terms.iter().any(|x| x == *t))
                .count();
            if support >= need {
                let qf_terms = if qf_exact_only { &orig_qterms } else { &qterms };
                for qt in qf_terms {
                    if dctx.query_terms.iter().any(|t| t == qt) {
                        if let Some(q) = qinfo.iter().find(|q| q.text == qt) {
                            let add = q.weight * q.idf * qf_w;
                            score += add;
                            qf_add += add;
                        }
                    }
                }
            }
        }
        // P4.3 实体边加权：共享实体数 × edge_weight（词面弱但实体强相关的兄弟 case 越线）。
        if dctx.edge_shared > 0 {
            fb += dctx.edge_shared as f64 * edge_w;
        }
        // 纯词面/字段/实体支持度：quality/decay 只能给已有证据的 case 加权，
        // 不能让零匹配 case 仅凭质量先验进入 top-k（否则任意候选会以 0 分返回，
        // 评估会把“按库顺序碰巧命中”误算成召回）。
        let support = score + fb;
        // O4 时间衰减：age 用 updated_at（去重/反馈会刷新它），τ 随状态动态。
        let tau_eff = match &*dctx.status {
            "solved" => tau_solved,
            "regressed" => tau_regressed,
            _ => tau,
        };
        let upd_s = crate::memory::util::parse_utc_secs(&dctx.updated_at);
        let age_days = (now_s.saturating_sub(upd_s)) as f64 / 86_400.0;
        let decay = 1.0 / (1.0 + age_days / tau_eff);
        // O5 quality 排序先验（§4.1）：solved/质量高的 case 提分，regressed 压分。
        let qsign = if &*dctx.status == "regressed" {
            -1.0
        } else {
            1.0
        };
        fb += dctx.quality * q_prior * qsign;
        if support <= 0.0 {
            continue;
        }
        let mut ranked = (score + fb) * decay;
        // query-field 只改排序，不伪造词面置信：sim 用不含 qf 的词面/字段分，
        // 否则同 query 的 sim 会因重排项而低于 1，误触发 PRF。
        let mut lexical = (score + fb - qf_add) * decay;
        // 词面证据过弱时不允许 qf 重排“救活”候选：否则一个泛化的 CJK
        // bigram 命中就能把跨事故 case 抬进 top-10（Flowdock 语义查询实测
        // sim=0.04 的 Tarsnap case 被抬到第 7）。阈值只作用于重排项，
        // 不影响纯词面/字段分，也不改变目标 case 的召回资格。
        let qf_min_sim = meta.param("query_field_min_sim");
        if qf_add > 0.0 && sm > 0.0 && lexical / sm < qf_min_sim {
            score -= qf_add;
            lexical = (score + fb) * decay;
            ranked = (score + fb) * decay;
        }
        hits.push(RecallHit {
            id: id.clone(),
            score: ranked,
            sim: if sm > 0.0 {
                (lexical / sm).clamp(0.0, 1.0)
            } else {
                0.0
            },
        });
    }

    // P2.5 MMR：得分排序后按多样性去冗余（λ 从 meta 读）。
    let mut order = order_by_score(&hits);
    // 同事故兄弟 case 往往共享完整根因段，词面高度重合。首轮用整篇词表做
    // Jaccard 聚类，把 top1 的同族 case 收紧到 top1 之后：同一事故的兄弟
    // 在人工口径里通常同为 rel=2，散落在弱相关候选之后会拉低 NDCG。
    // 只重排 top1 同族，且要求分数不低于 top1 的 40%，避免把“同根因但明显
    // 更弱”的候选抬上来；不改变 top1 自身位置。
    // 同族收紧只在 top1 有足够词面证据时启用。弱词面锚点通常意味着查询与
    // 候选只是共享少量泛化词；此时把整族候选前移会压过更强的跨族结果。
    let sibling_anchor_ok = sibling_anchor_ok(&hits, &order, meta.param("sibling_min_sim"));
    if meta.param("sibling_coherence") > 0.0 && sibling_anchor_ok && hits.len() > 1 {
        let terms_by_id: HashMap<&str, &[String]> = docs
            .iter()
            .map(|(id, d)| (id.as_str(), d.terms.as_ref()))
            .collect();
        order = group_siblings(order, &hits, &terms_by_id, k);
    }
    let lambda = meta.param("mmr_lambda");
    // MMR 默认关闭：v2 对拍显示 λ=0.5 会把同事故的精确目标挤出 top-5
    // （paraphrase r@5 0.783→0.867 当关闭）。需要多样性时置 mmr_enable=1。
    let picked = if meta.param("mmr_enable") > 0.0 {
        let scores_norm: Vec<f64> = hits.iter().map(|h| h.sim).collect();
        let doc_terms: Vec<Vec<String>> = docs.iter().map(|(_, d)| d.terms.to_vec()).collect();
        crate::memory::ranking::mmr_select(&order, &doc_terms, &scores_norm, k, lambda)
    } else {
        order.into_iter().take(k).collect()
    };
    let mut out: Vec<RecallHit> = Vec::with_capacity(picked.len());
    for &i in &picked {
        out.push(hits[i].clone());
    }
    Ok(out)
}

/// 确定性排序：分数降序，同分按 case id 升序。
///
/// `sort_by` 稳定，但输入 `docs` 的顺序来自倒排/哈希遍历，跨进程不保证；
/// 不设次级键会让同分兄弟 case 的 NDCG 每次运行不同（MRR 因目标不并列而看似稳定）。
fn order_by_score(hits: &[RecallHit]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..hits.len()).collect();
    order.sort_by(|&a, &b| {
        hits[b]
            .score
            .partial_cmp(&hits[a].score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| hits[a].id.cmp(&hits[b].id))
    });
    order
}

fn sibling_anchor_ok(hits: &[RecallHit], order: &[usize], min_sim: f64) -> bool {
    order.first().is_some_and(|&i| hits[i].sim >= min_sim)
}

/// 同族阈值：confirm5 同事故兄弟整篇词表 Jaccard 为 0.64–0.67，跨事故为
/// 0.06–0.07；0.35 位于两者之间，留足跨语料余量。
const SIBLING_JACCARD: f64 = 0.35;
/// 同族候选的最低相对分：低于 top1 的 40% 视为“同根因但更弱”，不抬升。
const SIBLING_SCORE_RATIO: f64 = 0.40;

/// 把 top1 的同族候选稳定地移到 top1 之后（保持各自原有先后），其余顺序不变。
/// 只看前 `3*k` 个候选：同事故兄弟在 confirm5 均落在 top-10 内，避免全库两两比较。
fn group_siblings(
    order: Vec<usize>,
    hits: &[RecallHit],
    terms_by_id: &HashMap<&str, &[String]>,
    k: usize,
) -> Vec<usize> {
    if order.len() < 2 {
        return order;
    }
    let top = order[0];
    let Some(top_terms) = terms_by_id.get(hits[top].id.as_str()) else {
        return order;
    };
    if top_terms.is_empty() {
        return order;
    }
    let window = (3 * k.max(1)).min(order.len());
    let top_score = hits[top].score.max(1e-9);
    let (mut family, mut rest) = (Vec::new(), Vec::new());
    for &i in &order[1..window] {
        let sibling = terms_by_id
            .get(hits[i].id.as_str())
            .is_some_and(|t| crate::memory::ranking::jaccard(top_terms, t) >= SIBLING_JACCARD)
            && hits[i].score >= top_score * SIBLING_SCORE_RATIO;
        if sibling {
            family.push(i);
        } else {
            rest.push(i);
        }
    }
    let mut out = Vec::with_capacity(order.len());
    out.push(top);
    out.extend(family);
    out.extend(rest);
    out.extend_from_slice(&order[window..]);
    out
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
    fn query_field_strength_gates_low_confidence_by_script() {
        // 英文低置信 literal 不触发 title 重排（confirm2 case 136 回归）。
        assert_eq!(
            query_field_strength(1.0, 0.30, 0.28, false, true, false),
            0.0
        );
        // 中文低置信释义保留平滑重排。
        let cjk = query_field_strength(1.0, 0.30, 0.15, false, true, true);
        assert!((cjk - 0.5).abs() < 1e-9);
        // 兄弟 case 满分并列时，英文也允许全量 title 重排。
        assert_eq!(
            query_field_strength(1.0, 0.30, 1.0, true, false, false),
            1.0
        );
        // 高置信且无并列：不改排序。
        assert_eq!(
            query_field_strength(1.0, 0.30, 0.80, false, false, false),
            0.0
        );
    }

    #[test]
    fn equal_scores_order_by_case_id() {
        let hit = |id: &str, score: f64| RecallHit {
            id: id.to_string(),
            score,
            sim: 1.0,
        };
        let hits = vec![hit("d", 2.0), hit("b", 2.0), hit("c", 2.0), hit("a", 1.0)];
        let ids: Vec<&str> = order_by_score(&hits)
            .into_iter()
            .map(|i| hits[i].id.as_str())
            .collect();
        assert_eq!(ids, vec!["b", "c", "d", "a"]);
    }

    #[test]
    fn sibling_gate_requires_confident_anchor() {
        let hit = |id: &str, sim: f64| RecallHit {
            id: id.into(),
            score: 1.0,
            sim,
        };
        let hits = vec![hit("low", 0.29), hit("high", 0.30)];
        let order = order_by_score(&hits);
        assert!(sibling_anchor_ok(&hits, &order, 0.30));
        assert!(!sibling_anchor_ok(&hits, &order, 0.31));
        assert!(!sibling_anchor_ok(&[], &[], 0.30));
    }

    #[test]
    fn jaccard_is_symmetric_and_bounded() {
        let a = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let b = vec!["b".to_string(), "c".to_string(), "d".to_string()];
        assert!((crate::memory::ranking::jaccard(&a, &b) - 0.5).abs() < 1e-9);
        assert!((crate::memory::ranking::jaccard(&b, &a) - 0.5).abs() < 1e-9);
        assert_eq!(crate::memory::ranking::jaccard(&[], &b), 0.0);
    }

    #[test]
    fn siblings_group_after_top1_and_weak_stays_below() {
        let hit = |id: &str, score: f64| RecallHit {
            id: id.to_string(),
            score,
            sim: score / 10.0,
        };
        let hits = vec![
            hit("top", 10.0),
            hit("sib1", 8.0),
            hit("weak", 9.0),
            hit("sib2", 7.0),
            hit("other", 6.0),
        ];
        let terms = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let top = terms(&["a", "b", "c", "d"]);
        let sib = terms(&["a", "b", "c", "d"]);
        let weak = terms(&["a", "e"]);
        let other = terms(&["z"]);
        let map: HashMap<&str, &[String]> = HashMap::from([
            ("top", top.as_slice()),
            ("sib1", sib.as_slice()),
            ("weak", weak.as_slice()),
            ("sib2", sib.as_slice()),
            ("other", other.as_slice()),
        ]);
        let out = group_siblings(vec![0, 2, 1, 3, 4], &hits, &map, 5);
        assert_eq!(out, vec![0, 1, 3, 2, 4]);
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
    fn recall_at_uses_explicit_clock() {
        let root = tmp_root("clock");
        let s1 = stats();
        for tail in ["alpha", "bravo"] {
            ingest(
                &root,
                "数据库连接超时 conn_id",
                Vec::new(),
                &format!("ERROR db timeout conn_id=9f2a {tail}"),
                vec![],
                &s1,
                vec![],
            )
            .unwrap();
        }
        let ids = store::list_ids(&root).unwrap();
        let base = crate::memory::util::parse_utc_secs(
            &store::read_case(&root, &ids[0]).unwrap().updated_at,
        );
        let query = "数据库连接超时 conn_id";
        let at_update = recall_at(&root, query, 2, base).unwrap();
        let one_day_later = recall_at(&root, query, 2, base + 86_400).unwrap();
        assert_eq!(at_update[0].id, one_day_later[0].id);
        assert!(
            at_update[0].score > one_day_later[0].score,
            "显式时钟必须驱动衰减，而不是读取墙钟: {} vs {}",
            at_update[0].score,
            one_day_later[0].score
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
        assert_eq!(all.len(), 1, "零词面支持的 case 不得以 0 分进入结果");
        let hello_case = store::read_case(&root, &all[0].id).unwrap();
        assert_eq!(hello_case.bucket, "build");
        assert!(
            recall(&root, "zzz-no-match", 5).unwrap().is_empty(),
            "无任何支持度的 query 必须返回空结果，而不是按库顺序兜底"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cross_domain_case_survives_bucket_filter_linear_and_index() {
        for force_index in [0.0, 1.0] {
            let root = tmp_root(if force_index > 0.0 {
                "xd-idx"
            } else {
                "xd-lin"
            });
            let s1 = stats();
            let (_, target) = ingest(
                &root,
                "数据库权限变更",
                vec![],
                "oversized file propagated across the network; 机器人检测数据库权限变更生成超大文件，文件传播全网导致宕机",
                vec!["/a.log".into()],
                &s1,
                vec![],
            )
            .unwrap();
            ingest(
                &root,
                "network incident",
                vec![],
                "network incident caused packet loss",
                vec!["/b.log".into()],
                &s1,
                vec![],
            )
            .unwrap();
            crate::memory::meta::set_param(&root, "index_force", force_index).unwrap();
            let hits = recall(&root, "oversized file propagated network outage", 5).unwrap();
            assert!(
                hits.iter().any(|h| h.id == target),
                "跨域 case 的多标签桶必须与 network 候选桶相交 (index_force={force_index})"
            );
            let _ = fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn low_confidence_query_bypasses_bucket_filter_linear_and_index() {
        for force_index in [0.0, 1.0] {
            let root = tmp_root(if force_index > 0.0 {
                "bp-idx"
            } else {
                "bp-lin"
            });
            let s1 = stats();
            let (_, target) = ingest(
                &root,
                "MongoDB slow queries exhausted JDBC connection pools",
                vec![],
                "MongoDB replica slow queries shrink JDBC connection pools",
                vec!["/a.log".into()],
                &s1,
                vec![],
            )
            .unwrap();
            ingest(
                &root,
                "NullPointerException stack trace",
                vec![],
                "runtime panic stack trace",
                vec!["/b.log".into()],
                &s1,
                vec![],
            )
            .unwrap();
            crate::memory::meta::set_param(&root, "index_force", force_index).unwrap();
            let hits = recall(
                &root,
                "JVM upgrade changed Docker defaults and thread connection pools",
                5,
            )
            .unwrap();
            assert!(
                hits.iter().any(|h| h.id == target),
                "低置信 query 的词面 runtime 桶不得硬筛掉 db 目标 (index_force={force_index})"
            );
            let _ = fs::remove_dir_all(&root);
        }
    }
}
