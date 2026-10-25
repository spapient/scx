//! 召回：词面打分、结构化字段加权、时间衰减与场景桶过滤。
//!
//! 库小于 1 万 case 时线性扫描并现算 df/avgdl；达到 10k 或
//! `meta [params] index_force=1` 时使用 `index/postings.bin` 倒排候选，
//! df/avgdl 为全库口径。
//! 零依赖、无随机源；固定时钟 `now_s` 时，同 query 同库同输出。
//!
//! 公式：bm25(t,d)=idf·tf·(k1+1)/(tf+k1·(1−b+b·dl/avgdl))；idf=ln(1+(N−df+0.5)/(df+0.5))
//! score=Σ bm25+Σ 字段命中×权重；decay=1/(1+age/τ)（τ:90/solved 180/regressed 30）；sim=score·decay/score_max
//! 场景桶：query 判候选桶，case 主桶不在候选桶集则跳过（候选桶空=全量扫，防漏召）。

use crate::memory::classify::{self, candidate_buckets};
use crate::memory::index::{self, Index};
use crate::memory::store::{self, Meta};
use crate::terms::{build_doc_terms, build_mem_terms};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub struct RecallHit {
    pub id: String,
    pub score: f64,
    pub sim: f64,
}

/// 候选 case 的召回上下文：一次读取，打分阶段零二次 I/O。
struct DocCtx {
    terms: Arc<[String]>,
    /// 原始 case query 词表（线性/索引/实体桥路径同口径；query-field 重排）。
    query_terms: Arc<[String]>,
    fields: Arc<super::store::Fields>,
    status: Arc<str>,
    quality: f64,
    updated_s: u64,
    /// 实体边：与 query 共享的实体键数（0 = 词面候选，>0 = 桥入兄弟 case）。
    edge_shared: usize,
}

/// 打分中间结构：候选 (id, 上下文) 列表 + query 词 df 表。
type Docs = Vec<(Arc<str>, DocCtx)>;

/// 候选打分结果。只保存文档下标，排序完成后再物化前 k 个公开 id。
struct Ranked {
    doc: usize,
    score: f64,
    sim: f64,
}

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

/// query 的只读派生数据：一次分析，首轮/桶回退/字段重排/PRF 共用。
///
/// 分散在每轮打分里重复切词、抽字段会让低置信多轮路径付 2–4 倍分析成本；
/// 同时集中词表口径，避免扩展开关在不同轮次出现分歧。
struct QueryPlan {
    terms: Vec<String>,
    original_len: usize,
    fields: super::store::Fields,
    entities: Vec<String>,
}

impl QueryPlan {
    fn new(query: &str, meta: &Meta) -> Self {
        let mut terms = build_mem_terms(query);
        let original_len = terms.len();
        if meta.param("synonyms_enable") > 0.0 {
            terms = crate::terms::expand_recall_terms(&terms);
        }
        let fields = classify::extract(query);
        let entities = [
            &fields.error_code,
            &fields.file_loc,
            &fields.symbol,
            &fields.num_fp,
        ]
        .into_iter()
        .flat_map(|values| values.iter().cloned())
        .collect();
        Self {
            terms,
            original_len,
            fields,
            entities,
        }
    }

    /// 同义扩展只追加、不重排，因此原词是计划词表的前缀，无需再复制一份。
    fn original(&self) -> &[String] {
        &self.terms[..self.original_len]
    }
}

/// 归一化得分 → 排序；sim 独立供 L1 置信判定。
pub(crate) fn recall(
    root: &str,
    query: &str,
    k: usize,
    meta: &Meta,
) -> std::io::Result<Vec<RecallHit>> {
    let now_s = crate::memory::util::utc_now_secs();
    recall_inner(root, query, k, now_s, meta, true)
}

/// 只读召回：索引缺失或损坏时回退线性扫描，绝不为修复缓存而写入磁盘。
pub(crate) fn recall_read_only(
    root: &str,
    query: &str,
    k: usize,
    meta: &Meta,
) -> std::io::Result<Vec<RecallHit>> {
    let now_s = crate::memory::util::utc_now_secs();
    recall_inner(root, query, k, now_s, meta, false)
}

/// 指定时间戳的召回：供评估/自检冻结衰减时钟，同库快照跨天可重放。
pub(crate) fn recall_at(
    root: &str,
    query: &str,
    k: usize,
    now_s: u64,
    meta: &Meta,
) -> std::io::Result<Vec<RecallHit>> {
    recall_inner(root, query, k, now_s, meta, true)
}

fn recall_inner(
    root: &str,
    query: &str,
    k: usize,
    now_s: u64,
    meta: &Meta,
    repair_index: bool,
) -> std::io::Result<Vec<RecallHit>> {
    if k == 0 {
        return Ok(Vec::new());
    }
    // query 记忆图：精确同指纹直接返回历史 top-k（跳过整个 BM25 管线）。
    // 代际失效：条目 gen < 当前 meta.gen 视为过期（库已变化，走 BM25 重算）。
    // 条数不足 k 的快照同样不可用：快照只含当时的 top-k，不能证明它覆盖本次 k。
    if let Some((_, qhits)) = crate::memory::query_cache::lookup(root, query, k, meta.gen)? {
        let hits: Vec<RecallHit> = qhits
            .into_iter()
            .map(|h| RecallHit {
                id: h.id,
                score: h.score as f64,
                sim: h.sim as f64,
            })
            .collect();
        return Ok(hits);
    }
    // 倒排路径：规模达标或 --rebuild 强制（index_force=1）时启用。
    // 索引缺失/坏段时自动重建覆盖（build 确定性），重建失败回退线性扫描。
    let idx = if index::use_index(meta) {
        match index::load(root) {
            Ok(Some(i)) => Some(i),
            _ if repair_index => {
                let _ = index::build(root);
                index::load(root).ok().flatten()
            }
            _ => None,
        }
    } else {
        None
    };
    // 场景桶：候选桶由原始 query 判定一次，首轮与 PRF 第二轮共用（防扩展词带偏过滤）。
    let cand = candidate_buckets(query);
    // 消融开关（默认全开）：bucket_enable/prf_enable 只服务可复现的 A−组件对拍，
    // 不改变默认检索行为。
    let mut use_buckets = !cand.is_empty() && meta.param("bucket_enable") > 0.0;
    let high_sim = meta.param("confidence_high_sim");
    let plan = QueryPlan::new(query, meta);
    let mut hits = score_pass(
        root,
        &plan,
        &PassCtx {
            k,
            meta,
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
    let (hits, use_buckets) = recall_bucket_bypass(
        root,
        &plan,
        k,
        now_s,
        meta,
        idx.as_deref(),
        &cand,
        use_buckets,
        hits,
    );
    // case-query 字段加权只在两种情况下启用：①低/中置信释义查询
    // （top1.sim<query_field_trigger_sim，默认 0.30）；
    // ②top1/top2 都是满分、但排序按正文长度/字段打破时（同事故兄弟 case 竞争）。
    // 高置信且无并列的 query 不改排序，避免把同域候选抬上来。
    let (hits, qf_strength, qf_exact_only, use_qf) = recall_qf_rerank(
        root,
        &plan,
        k,
        now_s,
        meta,
        idx.as_deref(),
        &cand,
        use_buckets,
        hits,
    );
    // PRF：低置信（top1.sim < confidence_high_sim）时粗召回 top-K 扩展查询再召一轮。
    // 扩展轮严格补漏，不覆盖首轮顺序（见下方合并逻辑）。
    let low_conf = meta.param("prf_enable") > 0.0 && hits.first().is_some_and(|h| h.sim < high_sim);
    if low_conf {
        if let Some(merged) = recall_prf_extend(
            root,
            query,
            &plan,
            k,
            now_s,
            meta,
            idx.as_deref(),
            &cand,
            use_buckets,
            qf_strength,
            qf_exact_only,
            &hits,
        ) {
            return Ok(merged);
        }
    }
    Ok(hits)
}

/// 低置信退桶：首轮 top1 sim 低于 `bucket_bypass_sim` 时，词面证据不足，
/// 继续用场景桶硬筛会误删目标 case——退回全量 BM25 候选（不改变全库统计口径）。
/// 返回（命中，是否仍在桶过滤）；score_pass 失败时保持原值（原语义）。
/// （recall_inner 提取段，行为不变）
fn recall_bucket_bypass(
    root: &str,
    plan: &QueryPlan,
    k: usize,
    now_s: u64,
    meta: &Meta,
    idx: Option<&Index>,
    cand: &[String],
    use_buckets: bool,
    mut hits: Vec<RecallHit>,
) -> (Vec<RecallHit>, bool) {
    let first_sim = hits.first().map(|h| h.sim).unwrap_or(0.0);
    let bucket_bypass_sim = meta.param("bucket_bypass_sim");
    if use_buckets && first_sim < bucket_bypass_sim {
        if let Ok(full_hits) = score_pass(
            root,
            plan,
            &PassCtx {
                k,
                meta,
                now_s,
                idx,
                cand,
                use_buckets: false,
                qf_weight: 0.0,
                qf_exact_only: false,
            },
        ) {
            hits = full_hits;
            return (hits, false);
        }
    }
    (hits, use_buckets)
}

/// case-query 字段重排：exact tie / 低置信 CJK 触发时按 `query_field_strength`
/// 强度重跑一轮 score_pass；未触发或重跑失败时保持原命中（原语义：失败静默）。
/// 返回（命中，qf 强度，qf 仅原始词，是否启用 qf）。（recall_inner 提取段）
fn recall_qf_rerank(
    root: &str,
    plan: &QueryPlan,
    k: usize,
    now_s: u64,
    meta: &Meta,
    idx: Option<&Index>,
    cand: &[String],
    use_buckets: bool,
    hits: Vec<RecallHit>,
) -> (Vec<RecallHit>, f64, bool, bool) {
    let qf_weight = meta.param("query_field_weight");
    let qf_trigger = meta.param("query_field_trigger_sim");
    let high_sim = meta.param("confidence_high_sim");
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
    let query_has_cjk = query_terms_has_cjk(plan);
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
    let mut out = hits;
    if use_qf {
        if let Ok(qf_hits) = score_pass(
            root,
            plan,
            &PassCtx {
                k,
                meta,
                now_s,
                idx,
                cand,
                use_buckets,
                qf_weight: qf_strength,
                qf_exact_only,
            },
        ) {
            if !qf_hits.is_empty() {
                out = qf_hits;
            }
        }
    }
    (out, qf_strength, qf_exact_only, use_qf)
}

/// query 原始词是否含 CJK（qf 低置信分支仅对中文 query 生效）。
fn query_terms_has_cjk(plan: &QueryPlan) -> bool {
    plan.original()
        .iter()
        .any(|t| t.chars().any(crate::terms::is_cjk))
}

/// PRF 扩展补漏：低置信时粗召回 top-K 扩展查询再召一轮；扩展轮严格补漏，
/// 首轮顺序权威——新命中按 q2 排序追加在尾部，绝不与首轮 score 混排
/// （q2 口径 score 会被扩展词放大，混排会把真目标挤出——literal 层 mrr
/// 退化实证：case 'disk io read timeout' A rank3 vs B rank1）。
/// 第二轮继承首轮候选桶（桶由原始 query 判定），扩展词不得改变过滤语义。
/// 返回 None = 未触发或扩展轮无新命中（原语义：静默保持首轮）。（recall_inner 提取段）
fn recall_prf_extend(
    root: &str,
    query: &str,
    plan: &QueryPlan,
    k: usize,
    now_s: u64,
    meta: &Meta,
    idx: Option<&Index>,
    cand: &[String],
    use_buckets: bool,
    qf_strength: f64,
    qf_exact_only: bool,
    hits: &[RecallHit],
) -> Option<Vec<RecallHit>> {
    if let Ok(ext) = crate::memory::ranking::expand(root, hits, query, meta, idx) {
        if !ext.is_empty() {
            let q2 = format!("{query} {}", ext.join(" "));
            let q2_plan = QueryPlan::new(&q2, meta);
            if let Ok(hits2) = score_pass(
                root,
                &q2_plan,
                &PassCtx {
                    k,
                    meta,
                    now_s,
                    idx,
                    cand,
                    use_buckets,
                    qf_weight: if qf_strength > 1e-9 { qf_strength } else { 0.0 },
                    qf_exact_only,
                },
            ) {
                if !hits2.is_empty() {
                    let mut merged: Vec<RecallHit> = hits.to_vec();
                    for h in hits2 {
                        if !merged.iter().any(|m| m.id == h.id) {
                            merged.push(h);
                        }
                    }
                    merged.truncate(k);
                    return Some(merged);
                }
            }
        }
    }
    None
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
fn score_pass(root: &str, plan: &QueryPlan, ctx: &PassCtx<'_>) -> std::io::Result<Vec<RecallHit>> {
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
    // 实体边：共享实体加权（0 则关闭，兼容老 meta 缺省开 1.0）。
    let edge_w = meta.param("edge_weight");
    let orig_qterms = plan.original();
    let qterms = plan.terms.as_slice();
    // 同义扩展是补召回机制，不应与用户原词等权：等权时一个宽泛的扩展词
    // 可以把同域兄弟 case 抬到精确目标之前（confirm3 paraphrase 回归）。
    // 半权是“保留补召回、限制误抬升”的默认折中；置 1 可恢复旧行为。
    let syn_w = meta.param("synonym_weight").clamp(0.0, 1.0);
    // `other` = 分类器没有强家族证据；默认放行（宁多勿漏）。
    // 置 0 可恢复旧的硬筛行为（仅用于对拍）。
    let allow_other = meta.param("bucket_allow_other") > 0.0;
    let qf = [
        &plan.fields.error_code,
        &plan.fields.file_loc,
        &plan.fields.symbol,
        &plan.fields.num_fp,
    ];
    let q_ent_list = &plan.entities;

    // 场景桶：query 判候选桶，case 主桶不在候选桶集内则跳过。

    // 候选 docs + df + 统计量：倒排/线性两条路径都用全库口径；桶过滤只裁候选。
    let (docs, df, n, avgdl): (Docs, Vec<usize>, f64, f64) = match idx {
        Some(i) => collect_docs_idx(i, qterms, use_buckets, cand, allow_other, q_ent_list)?,
        None => collect_docs_scan(
            root,
            plan,
            qterms,
            use_buckets,
            cand,
            allow_other,
            q_ent_list,
            meta,
        )?,
    };

    let now_s = ctx.now_s;
    // 词级调权表：query 词命中时按词权重加成（无词条=0）。
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
    // quality 排序先验：solved 且质量高的 case 提分。
    let q_prior = meta.param("quality_prior");
    let qf_min_sim = meta.param("query_field_min_sim");
    // score_max：原始 query 词 idf 加字段全命中，作 sim 归一化分母。
    // 同义扩展只增加命中机会，不得抬高分母把原始 query 的 sim 稀释。
    // 直觉：query 每个词在 case 中都命中一次 ≈ sim=1（tf>1 或字段命中可再溢出）。
    let mut sm: f64 = qinfo.iter().filter(|q| q.original).map(|q| q.idf).sum();
    for (f_list, w) in qf.iter().zip(fw.iter()) {
        sm += f_list.len() as f64 * w;
    }

    let mut hits: Vec<Ranked> = Vec::with_capacity(docs.len());
    for (doc, (_id, dctx)) in docs.iter().enumerate() {
        // terms 在建库/桥接/线性路径均已 sort+dedup，tf 恒为 1；直接二分查存在性，
        // 避免每个候选文档分配一张 tf HashMap。
        let dl = dctx.terms.len() as f64;
        let denom = crate::memory::ranking::denom(k1, b, dl, avgdl);
        let mut score = doc_lexical(dctx, &qinfo, k1, denom);
        let mut fb = doc_field_bonus(dctx, &qf, &fw, edge_w);
        let (score_qf, qf_add) = doc_qf_bonus(
            dctx,
            &qinfo,
            orig_qterms,
            qterms,
            qf_exact_only,
            qf_weight,
            score,
        );
        score = score_qf;
        // 纯词面/字段/实体支持度：quality/decay 只能给已有证据的 case 加权，
        // 不能让零匹配 case 仅凭质量先验进入 top-k（否则任意候选会以 0 分返回，
        // 评估会把“按库顺序碰巧命中”误算成召回）。
        let support = score + fb;
        // 时间衰减：age 用 updated_at 的预解析快照，τ 随状态动态。
        let (decay, qsign) = doc_decay(dctx, now_s, tau_solved, tau_regressed, tau);
        // quality 排序先验：solved/质量高的 case 提分，regressed 压分。
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
        if qf_add > 0.0 && sm > 0.0 && lexical / sm < qf_min_sim {
            score -= qf_add;
            lexical = (score + fb) * decay;
            ranked = (score + fb) * decay;
        }
        hits.push(Ranked {
            doc,
            score: ranked,
            sim: if sm > 0.0 {
                (lexical / sm).clamp(0.0, 1.0)
            } else {
                0.0
            },
        });
    }
    Ok(select_hits(&hits, &docs, k, meta))
}

/// 索引路径候选：倒排候选 + 桶过滤 + 实体边一跳（桥入无需重读全库 case.json，
/// 旧实现每次冷启动重扫 1 万条，单次约 0.2s）。返回候选 docs + 全库 df/n/avgdl。
/// （score_pass 提取段，行为不变）
fn collect_docs_idx(
    idx: &Index,
    qterms: &[String],
    use_buckets: bool,
    cand: &[String],
    allow_other: bool,
    q_ent_list: &[String],
) -> std::io::Result<(Docs, Vec<usize>, f64, f64)> {
    let cand_di = index::candidates(idx, qterms);
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
    // 实体边一跳：实体倒排由 postings 的 doc 快照派生，桥入无需重读全库 case.json。
    if !q_ent_list.is_empty() {
        extend_entity_edges(idx, q_ent_list, &mut chosen);
    }
    let docs: Docs = chosen
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
                    updated_s: rec.updated_s,
                    edge_shared,
                },
            )
        })
        .collect();
    if docs.is_empty() {
        return Ok((Vec::new(), Vec::new(), 0.0, 0.0));
    }
    let n = idx.n_docs as f64;
    let avgdl = idx.total_len as f64 / n;
    let df: Vec<usize> = qterms.iter().map(|t| index::term_df(idx, t)).collect();
    Ok((docs, df, n, avgdl))
}

/// 实体边一跳并入候选：跳过已在 chosen 中的 doc；共享实体数记录为 edge_shared。
fn extend_entity_edges(idx: &Index, q_ent_list: &[String], chosen: &mut Vec<(usize, usize)>) {
    let mut seen = vec![false; idx.n_docs];
    for &(di, _) in chosen.iter() {
        seen[di] = true;
    }
    for (di, shared) in index::entity_neighbors(idx, q_ent_list) {
        if !seen[di] {
            seen[di] = true;
            chosen.push((di, shared));
        }
    }
}

/// 线性路径候选：全库顺序扫描 + 桶过滤 + 实体边桥入。df/n/avgdl 取全库词面口径，
/// 桶过滤只裁候选不改统计量（否则“线性+桶”与“索引+桶”会得到不同的长度归一化，
/// 也会让 A/B 在候选子集上比较不同的 BM25——confirm3 paraphrase 回归）。
/// （score_pass 提取段，行为不变）
fn collect_docs_scan(
    root: &str,
    plan: &QueryPlan,
    qterms: &[String],
    use_buckets: bool,
    cand: &[String],
    allow_other: bool,
    q_ent_list: &[String],
    meta: &Meta,
) -> std::io::Result<(Docs, Vec<usize>, f64, f64)> {
    let ids = store::list_ids(root)?;
    if ids.is_empty() {
        return Ok((Vec::new(), Vec::new(), 0.0, 0.0));
    }
    let mut docs: Docs = Vec::with_capacity(ids.len());
    let mut df = vec![0usize; qterms.len()];
    let mut total_len = 0usize;
    for id in &ids {
        let c = store::read_case(root, id)?;
        let (query_terms, terms) = build_doc_terms(&c.query, &c.text);
        let buckets = classify::case_buckets(&[], &query_terms, &c.text);
        total_len += terms.len();
        for (qi, qt) in qterms.iter().enumerate() {
            if terms.binary_search(qt).is_ok() {
                df[qi] += 1;
            }
        }
        // BM25 的 df/avgdl 必须保持全库口径；桶过滤只裁候选，不能改统计量。
        if use_buckets && !bucket_allowed(&buckets, cand, allow_other) {
            continue;
        }
        docs.push((
            Arc::from(id.as_str()),
            DocCtx {
                terms: terms.into(),
                query_terms: query_terms.into(),
                fields: Arc::new(c.fields),
                status: c.status.into(),
                quality: c.quality,
                updated_s: crate::memory::util::parse_utc_secs(&c.updated_at),
                edge_shared: 0,
            },
        ));
    }
    // 实体边一跳：桥 case 只并入候选；df/n/avgdl 仍取全库词面口径。
    let exist_ids: Vec<String> = docs.iter().map(|(id, _)| id.to_string()).collect();
    let bridges = crate::memory::edge::bridge_cases(root, meta, q_ent_list, &exist_ids)?;
    for b in bridges {
        docs.push((
            b.id.into(),
            DocCtx {
                terms: b.terms,
                query_terms: b.query_terms,
                fields: b.fields,
                status: b.status,
                quality: b.quality,
                updated_s: crate::memory::util::parse_utc_secs(&b.updated_at),
                edge_shared: b.edge_shared,
            },
        ));
    }
    if docs.is_empty() {
        return Ok((Vec::new(), Vec::new(), 0.0, 0.0));
    }
    // 与索引路径一致：n/avgdl 取全库口径，桥入 case 不改变平均文档长度。
    let n = ids.len() as f64;
    let avgdl = total_len as f64 / n;
    Ok((docs, df, n, avgdl))
}

/// 词面分：query 词命中（tf 恒 1，二分查存在性）+ 词级调权（可正可负）。
/// 浮点累加顺序与原实现一致，拆分不改任何字节。
fn doc_lexical(dctx: &DocCtx, qinfo: &[QueryTerm<'_>], k1: f64, denom: f64) -> f64 {
    let mut score = 0.0;
    for q in qinfo {
        if dctx.terms.binary_search(q.text).is_ok() {
            score += q.weight * crate::memory::ranking::score(q.idf, k1, denom);
            score += q.weight * q.idf * q.word_w;
        }
    }
    score
}

/// 字段命中分 + 实体边加权（共享实体数 × edge_weight）。
fn doc_field_bonus(dctx: &DocCtx, qf: &[&Vec<String>], fw: &[f64; 4], edge_w: f64) -> f64 {
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
    if dctx.edge_shared > 0 {
        fb += dctx.edge_shared as f64 * edge_w;
    }
    fb
}

/// 可选 BM25F 风格的 case-query 字段加权：至少 2 个原始 query 词落在 case
/// query 字段才加权（单个常见词不足以证明“问的是这条 case”）；默认关闭（0），
/// 只用显式参数开启。返回（含 qf 的词面分，qf 加分）。
fn doc_qf_bonus(
    dctx: &DocCtx,
    qinfo: &[QueryTerm<'_>],
    orig_qterms: &[String],
    qterms: &[String],
    qf_exact_only: bool,
    qf_w: f64,
    mut score: f64,
) -> (f64, f64) {
    let mut qf_add = 0.0;
    if qf_w > 0.0 {
        let need = orig_qterms.len().min(2);
        let support = orig_qterms
            .iter()
            .filter(|t| dctx.query_terms.iter().any(|x| x == *t))
            .count();
        if support >= need {
            let qf_terms = if qf_exact_only { orig_qterms } else { qterms };
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
    (score, qf_add)
}

/// 时间衰减：age 用 updated_at 的预解析快照，τ 随状态动态；
/// quality 符号 solved/默认 +1，regressed -1。
fn doc_decay(
    dctx: &DocCtx,
    now_s: u64,
    tau_solved: f64,
    tau_regressed: f64,
    tau: f64,
) -> (f64, f64) {
    let tau_eff = match &*dctx.status {
        "solved" => tau_solved,
        "regressed" => tau_regressed,
        _ => tau,
    };
    let age_days = (now_s.saturating_sub(dctx.updated_s)) as f64 / 86_400.0;
    let decay = 1.0 / (1.0 + age_days / tau_eff);
    let qsign = if &*dctx.status == "regressed" {
        -1.0
    } else {
        1.0
    };
    (decay, qsign)
}

/// MMR/同族收紧/截断 + RecallHit 组装（score_pass 提取段）。
/// MMR 默认关闭：v2 对拍显示 λ=0.5 会把同事故的精确目标挤出 top-5
/// （paraphrase r@5 0.783→0.867 当关闭）。需要多样性时置 mmr_enable=1。
fn select_hits(hits: &[Ranked], docs: &Docs, k: usize, meta: &Meta) -> Vec<RecallHit> {
    // 得分排序后按多样性去冗余（λ 从 meta 读）。关闭 MMR（默认）时最终只需
    // 要 top-k；同族收紧最多查看前 3k 个候选，因此先做 O(n) 的部分选择再排序
    // 3k，避免为 5 条结果排序整个 10k 候选集。打开 MMR 时仍保留完整顺序。
    let mmr_enabled = meta.param("mmr_enable") > 0.0;
    let window = if mmr_enabled {
        hits.len()
    } else {
        k.saturating_mul(3).max(k).min(hits.len())
    };
    let mut order = order_top(hits, docs, window);
    // 同事故兄弟 case 往往共享完整根因段，词面高度重合。首轮用整篇词表做
    // Jaccard 聚类，把 top1 的同族 case 收紧到 top1 之后：同一事故的兄弟
    // 在人工口径里通常同为 rel=2，散落在弱相关候选之后会拉低 NDCG。
    // 只重排 top1 同族，且要求分数不低于 top1 的 40%，避免把“同根因但明显
    // 更弱”的候选抬上来；不改变 top1 自身位置。
    // 同族收紧只在 top1 有足够词面证据时启用。弱词面锚点通常意味着查询与
    // 候选只是共享少量泛化词；此时把整族候选前移会压过更强的跨族结果。
    let sibling_anchor_ok = sibling_anchor_ok(hits, &order, meta.param("sibling_min_sim"));
    if meta.param("sibling_coherence") > 0.0 && sibling_anchor_ok && hits.len() > 1 {
        order = group_siblings(order, hits, docs, k);
    }
    let lambda = meta.param("mmr_lambda");
    let picked = if mmr_enabled {
        let scores_norm: Vec<f64> = hits.iter().map(|h| h.sim).collect();
        let doc_terms: Vec<Vec<String>> = docs.iter().map(|(_, d)| d.terms.to_vec()).collect();
        crate::memory::ranking::mmr_select(&order, &doc_terms, &scores_norm, k, lambda)
    } else {
        order.truncate(k);
        order
    };
    let mut out: Vec<RecallHit> = Vec::with_capacity(picked.len());
    for &i in &picked {
        let hit = &hits[i];
        out.push(RecallHit {
            id: docs[hit.doc].0.to_string(),
            score: hit.score,
            sim: hit.sim,
        });
    }
    out
}

/// 确定性排序：分数降序，同分按 case id 升序；只保留前 `limit` 个。
///
/// `sort_by` 稳定，但输入 `docs` 的顺序来自倒排/哈希遍历，跨进程不保证；
/// 不设次级键会让同分兄弟 case 的 NDCG 每次运行不同（MRR 因目标不并列而看似稳定）。
/// 比较器同时包含分数与唯一 id，是全序，因此 `select_nth_unstable_by` 的
/// 非稳定性不会引入跨运行差异。
fn order_top(hits: &[Ranked], docs: &Docs, limit: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..hits.len()).collect();
    let cmp = |a: &usize, b: &usize| {
        hits[*b]
            .score
            .partial_cmp(&hits[*a].score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| docs[hits[*a].doc].0.cmp(&docs[hits[*b].doc].0))
    };
    let keep = limit.min(order.len());
    if keep == 0 {
        return Vec::new();
    }
    if keep < order.len() {
        order.select_nth_unstable_by(keep, cmp);
        order.truncate(keep);
    }
    order.sort_unstable_by(cmp);
    order
}

fn sibling_anchor_ok(hits: &[Ranked], order: &[usize], min_sim: f64) -> bool {
    order.first().is_some_and(|&i| hits[i].sim >= min_sim)
}

/// 同族阈值：confirm5 同事故兄弟整篇词表 Jaccard 为 0.64–0.67，跨事故为
/// 0.06–0.07；0.35 位于两者之间，留足跨语料余量。
const SIBLING_JACCARD: f64 = 0.35;
/// 同族候选的最低相对分：低于 top1 的 40% 视为“同根因但更弱”，不抬升。
const SIBLING_SCORE_RATIO: f64 = 0.40;

/// 把 top1 的同族候选稳定地移到 top1 之后（保持各自原有先后），其余顺序不变。
/// 只看前 `3*k` 个候选：同事故兄弟在 confirm5 均落在 top-10 内，避免全库两两比较。
fn group_siblings(order: Vec<usize>, hits: &[Ranked], docs: &Docs, k: usize) -> Vec<usize> {
    if order.len() < 2 {
        return order;
    }
    let top = order[0];
    let top_terms = &docs[hits[top].doc].1.terms;
    if top_terms.is_empty() {
        return order;
    }
    let window = (3 * k.max(1)).min(order.len());
    let top_score = hits[top].score.max(1e-9);
    let (mut family, mut rest) = (Vec::new(), Vec::new());
    for &i in &order[1..window] {
        let sibling = crate::memory::ranking::jaccard(top_terms, &docs[hits[i].doc].1.terms)
            >= SIBLING_JACCARD
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
    use crate::memory::util::tmp_root;
    use std::fs;

    fn stats() -> CaseStats {
        crate::memory::store::sample_stats()
    }

    fn recall_test(root: &str, query: &str, k: usize) -> std::io::Result<Vec<RecallHit>> {
        recall(root, query, k, &store::load_meta(root)?)
    }

    fn doc(id: &str, terms: &[&str]) -> (Arc<str>, DocCtx) {
        (
            Arc::from(id),
            DocCtx {
                terms: terms
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
                    .into(),
                query_terms: Arc::from(Vec::new()),
                fields: Arc::new(Default::default()),
                status: "open".into(),
                quality: 0.0,
                updated_s: 0,
                edge_shared: 0,
            },
        )
    }

    fn ranked(doc: usize, score: f64, sim: f64) -> Ranked {
        Ranked { doc, score, sim }
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
    fn query_plan_reuses_one_term_and_field_snapshot() {
        let meta = |synonyms| Meta {
            created_at: String::new(),
            case_count: 1,
            gen: 1,
            df: Vec::new(),
            params: vec![("synonyms_enable".to_string(), synonyms)],
            word_w: Vec::new(),
            l1_hist: Vec::new(),
        };
        let on = QueryPlan::new("ERROR conn_id=9f2a memory", &meta(1.0));
        assert!(on.original().iter().any(|t| t == "memory"));
        assert!(
            on.terms.iter().any(|t| t == "heap") || on.terms.iter().any(|t| t == "oom"),
            "启用同义词时扩展词必须进入同一份计划"
        );
        assert!(
            on.fields.error_code.iter().any(|v| v == "conn_id=9f2a"),
            "结构化字段必须只抽取一次并供所有打分轮共用"
        );
        assert!(
            on.entities.iter().any(|v| v == "conn_id=9f2a"),
            "实体边候选必须复用同一字段快照"
        );
        let off = QueryPlan::new("ERROR conn_id=9f2a memory", &meta(0.0));
        assert_eq!(
            off.terms.as_slice(),
            off.original(),
            "关闭同义词时不得扩展词表"
        );
    }

    #[test]
    fn equal_scores_order_by_case_id() {
        let docs = vec![doc("d", &[]), doc("b", &[]), doc("c", &[]), doc("a", &[])];
        let hits = vec![
            ranked(0, 2.0, 1.0),
            ranked(1, 2.0, 1.0),
            ranked(2, 2.0, 1.0),
            ranked(3, 1.0, 1.0),
        ];
        let ids: Vec<&str> = order_top(&hits, &docs, usize::MAX)
            .into_iter()
            .map(|i| &*docs[hits[i].doc].0)
            .collect();
        assert_eq!(ids, vec!["b", "c", "d", "a"]);
    }

    #[test]
    fn sibling_gate_requires_confident_anchor() {
        let docs = vec![doc("low", &[]), doc("high", &[])];
        let hits = vec![ranked(0, 1.0, 0.29), ranked(1, 1.0, 0.30)];
        let order = order_top(&hits, &docs, usize::MAX);
        assert!(sibling_anchor_ok(&hits, &order, 0.30));
        assert!(!sibling_anchor_ok(&hits, &order, 0.31));
        assert!(!sibling_anchor_ok(&[], &[], 0.30));
    }

    #[test]
    fn partial_order_matches_full_top_prefix() {
        let docs: Vec<(Arc<str>, DocCtx)> =
            (0..64).map(|i| doc(&format!("d{i:02}"), &[])).collect();
        let hits: Vec<Ranked> = (0..64)
            .map(|i| ranked(i, (i * 37 % 23) as f64, 1.0))
            .collect();
        let full = order_top(&hits, &docs, usize::MAX);
        for limit in [1usize, 5, 15, 63, 64, 100] {
            let part = order_top(&hits, &docs, limit);
            assert_eq!(
                part,
                full[..limit.min(full.len())],
                "partial top-{limit} must equal the full-order prefix"
            );
        }
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
        let docs = vec![
            doc("top", &["a", "b", "c", "d"]),
            doc("sib1", &["a", "b", "c", "d"]),
            doc("weak", &["a", "e"]),
            doc("sib2", &["a", "b", "c", "d"]),
            doc("other", &["z"]),
        ];
        let hits = vec![
            ranked(0, 10.0, 1.0),
            ranked(1, 8.0, 0.8),
            ranked(2, 9.0, 0.9),
            ranked(3, 7.0, 0.7),
            ranked(4, 6.0, 0.6),
        ];
        let out = group_siblings(vec![0, 2, 1, 3, 4], &hits, &docs, 5);
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
        let hits = recall_test(&root, "数据库连接超时 conn_id", 3).unwrap();
        assert!(!hits.is_empty(), "同 query 应召回原 case");
        assert!(
            hits[0].sim > L1_SIM,
            "同 query 置信应过 L1 阈值：sim={}",
            hits[0].sim
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn read_only_recall_does_not_repair_the_index_cache() {
        let root = tmp_root("ro");
        ingest(
            &root,
            "db timeout conn_id",
            vec!["conn_id".into()],
            "ERROR db timeout conn_id=9f2a",
            vec![],
            &stats(),
            vec![],
        )
        .unwrap();
        crate::memory::meta::set_param(&root, "index_force", 1.0).unwrap();
        let meta = store::load_meta(&root).unwrap();
        let postings = index::postings_path(&root);

        let hits = recall_read_only(&root, "db timeout conn_id", 3, &meta).unwrap();
        assert!(!hits.is_empty());
        assert!(
            !postings.exists(),
            "read-only recall must fall back to linear scan without writing an index"
        );

        assert!(!recall(&root, "db timeout conn_id", 3, &meta)
            .unwrap()
            .is_empty());
        assert!(
            postings.exists(),
            "normal recall may build the disposable cache"
        );

        fs::write(&postings, b"corrupt").unwrap();
        let hits = recall_read_only(&root, "db timeout conn_id", 3, &meta).unwrap();
        assert!(!hits.is_empty());
        assert_eq!(
            fs::read(&postings).unwrap(),
            b"corrupt",
            "read-only recall must not overwrite a corrupt index"
        );

        assert!(!recall(&root, "db timeout conn_id", 3, &meta)
            .unwrap()
            .is_empty());
        assert!(
            index::load(&root).unwrap().is_some(),
            "normal recall may repair the corrupt cache"
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
        let hits = recall_test(&root, "conn_id=9f2a 又超时了", 2).unwrap();
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

        let hits = recall_test(&root, "数据库连接超时", 2).unwrap();
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
        let meta = store::load_meta(&root).unwrap();
        let at_update = recall_at(&root, query, 2, base, &meta).unwrap();
        let one_day_later = recall_at(&root, query, 2, base + 86_400, &meta).unwrap();
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
        let hits = recall_test(&root, "数据库超时 conn_id", 5).unwrap();
        let top_case = store::read_case(&root, &hits[0].id).unwrap();
        assert_eq!(top_case.bucket, "db", "桶过滤后 top 必须是 db 桶");
        assert_eq!(hits.len(), 1, "build 桶 case 不应参与打分");
        // 无家族命中的 query：候选桶空 → 全量扫（防漏召）。
        let all = recall_test(&root, "hello", 5).unwrap();
        assert_eq!(all.len(), 1, "零词面支持的 case 不得以 0 分进入结果");
        let hello_case = store::read_case(&root, &all[0].id).unwrap();
        assert_eq!(hello_case.bucket, "build");
        assert!(
            recall_test(&root, "zzz-no-match", 5).unwrap().is_empty(),
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
            let hits = recall_test(&root, "oversized file propagated network outage", 5).unwrap();
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
            let hits = recall_test(
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
