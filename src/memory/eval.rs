//! 评估系统：冻结 baseline 重放、A/B 指标与诊断输出。
//!
//! `--eval <baseline.json>`：冻结评估输入重放——标注集（query → expected case + 分层 +
//! 相关度 0..2）上分三层（literal/paraphrase/semantic）输出 Recall@5 / MRR / NDCG@10。
//! 对拍：A = 完整统计管线（recall::recall，字段加权+PRF+衰减+MMR+反馈+桶过滤），
//! B = 朴素词面基线（`plain`：全库候选、纯词面匹配 + 长度归一化、无任何增强）。
//!
//! 口径说明：打分词表两边都是去重词表（tf 恒 1），实际是
//! "二值词面匹配 + 长度归一化 + 字段加权"，不是严格 BM25（tf 项恒为 1）。
//! A/B 同词表对比仍公平，但任何结论须按此口径引用，不宣称"BM25 增益"。
//! 确定性：无随机源，同 baseline + 同库 → 同输出，可重放存档。
//! 时间衰减时钟冻结到库的 `created_at`，避免同一快照因运行时刻不同而在近似并列处换序。
//!
use crate::memory::gate::GateMode;
use crate::memory::json::Val;
use crate::memory::recall::{self, RecallHit};
use crate::memory::store;
use crate::memory::EvalMode;
use crate::terms::{build_doc_terms, build_mem_terms};
use std::collections::HashMap;

/// 单条评估输入（baseline.json 的一行标注）。
#[derive(Debug)]
pub(crate) struct EvalCase {
    pub query: String,
    pub layer: String,
    pub expected: Vec<String>,
    pub rel: HashMap<String, f64>,
    /// `--judge` 写入的裁决人；自动标注集为空（`--gate` 据此拒绝非 literal 条目）。
    pub judged_by: Option<String>,
    /// `--judge` 写入的裁决时间（UTC）；与 judged_by 同时存在才算人工裁决。
    pub judged_date: Option<String>,
    /// `judge.method`：区分逐条盲审与“人工规则物化”，供 gate 输出证据边界。
    pub judged_method: Option<String>,
    /// 逐候选裁决溯源：候选 id → 裁决人。`rel` 里每个键都必须能在此找到
    /// 对应条目，`--gate-human` 才认可为“逐条人工”而非整包签收。
    pub judge_items: HashMap<String, String>,
}

impl EvalCase {
    /// expected 首个目标的相关度；literal 层 query 就是目标 case 原文，
    /// 该值必须为 2，否则标签与行不对应（或整包机械填写）。
    pub(crate) fn expected_rel(&self) -> Option<f64> {
        self.expected.first().map(|e| rel_of(self, e))
    }
}

/// 解析 baseline.json：`{"name":..,"cases":[{"query":..,"layer":..,"expected":[..],"rel":{..}}]}`
/// rel 缺省时 expected 内 case 视为 rel=1；容错（缺字段报错，不静默）。
pub(crate) fn parse_baseline(v: &Val) -> Result<Vec<EvalCase>, String> {
    let cases = match v {
        Val::Obj(kvs) => match kvs.iter().find(|(k, _)| k == "cases") {
            Some((_, Val::Arr(items))) => items,
            _ => return Err("baseline: missing array \"cases\"".to_string()),
        },
        _ => return Err("baseline: expected top-level object".to_string()),
    };
    let mut out = Vec::with_capacity(cases.len());
    for (i, item) in cases.iter().enumerate() {
        let kvs = match item {
            Val::Obj(kvs) => kvs,
            _ => return Err(format!("baseline: cases[{i}] not an object")),
        };
        let get_str = |key: &str| -> Result<String, String> {
            match kvs.iter().find(|(k, _)| k == key) {
                Some((_, Val::Str(s))) => Ok(s.clone()),
                _ => Err(format!("baseline: cases[{i}] missing string \"{key}\"")),
            }
        };
        let query = get_str("query")?;
        let layer = get_str("layer")?;
        // 盲测裁决门——judge:null（未裁决）一律拒绝，防止
        // "人工直接接受预填 rel = 自动标注自证"。先跑 --judge 再评估。
        if kvs
            .iter()
            .any(|(k, v)| k == "judge" && matches!(v, Val::Null))
        {
            return Err(format!(
                "baseline: cases[{i}] unjudged (judge:null) - run --judge first"
            ));
        }
        let expected = match kvs.iter().find(|(k, _)| k == "expected") {
            Some((_, Val::Arr(items))) => {
                let mut v = Vec::with_capacity(items.len());
                for e in items {
                    match e {
                        Val::Str(s) => v.push(s.clone()),
                        _ => return Err(format!("baseline: cases[{i}].expected not strings")),
                    }
                }
                v
            }
            _ => return Err(format!("baseline: cases[{i}] missing array \"expected\"")),
        };
        let mut rel = HashMap::new();
        if let Some((_, Val::Obj(rel_kvs))) = kvs.iter().find(|(k, _)| k == "rel") {
            for (rk, rv) in rel_kvs {
                let r = match rv {
                    Val::Num(n) if n.is_finite() && (0.0..=2.0).contains(n) => *n,
                    _ => return Err(format!("baseline: cases[{i}].rel.{rk} not 0..=2 number")),
                };
                rel.insert(rk.clone(), r);
            }
        }
        // expected 可能是完整 id，而 rel 字典常用 hash 后缀（跨日重建后日期前缀
        // 不同）。先按同一匹配口径找已有标签，避免为同一 case 再插入 rel=1
        // 的重复键；否则 rel_of 会因 HashMap 遍历顺序随机取到 1 或 2。
        for e in &expected {
            if !rel.keys().any(|k| id_matches(e, k) || id_matches(k, e)) {
                rel.insert(e.clone(), 1.0);
            }
        }
        // 读取 judge 元数据供 --gate 校验。自动标注集没有 judge
        // 对象（或只有 rel），因此不能冒充人工裁决证据。
        let mut judged_by = None;
        let mut judged_date = None;
        let mut judged_method = None;
        if let Some((_, Val::Obj(jkvs))) = kvs.iter().find(|(k, _)| k == "judge") {
            let get_optional = |key: &str| -> Option<String> {
                jkvs.iter()
                    .find(|(k, _)| k == key)
                    .and_then(|(_, v)| match v {
                        Val::Str(s) if !s.trim().is_empty() => Some(s.clone()),
                        _ => None,
                    })
            };
            judged_by = get_optional("by");
            judged_date = get_optional("date");
            judged_method = get_optional("method");
        }
        let mut judge_items = HashMap::new();
        if let Some((_, Val::Obj(ikvs))) = kvs.iter().find(|(k, _)| k == "judge_items") {
            for (ik, iv) in ikvs {
                let by = match iv {
                    Val::Obj(m) => m
                        .iter()
                        .find(|(k, _)| k == "by")
                        .and_then(|(_, v)| match v {
                            Val::Str(s) if !s.trim().is_empty() => Some(s.clone()),
                            _ => None,
                        }),
                    _ => None,
                };
                if let Some(by) = by {
                    judge_items.insert(ik.clone(), by);
                }
            }
        }
        out.push(EvalCase {
            query,
            layer,
            expected,
            rel,
            judged_by,
            judged_date,
            judged_method,
            judge_items,
        });
    }
    Ok(out)
}

/// expected/rel 的 id 匹配：支持完整 id、hash 后缀（tail）或 hash 后缀的后 8 位
/// （case id 为 64 位后 tail=16 hex，标注集里的旧 8 位
/// hash 仍须匹配，golden 免日期漂移）。完整 id 的日期前缀允许不同：重建库跨日
/// 运行时 hash 不变，冻结 baseline 不应因此整体 miss。
fn id_matches(exp: &str, id: &str) -> bool {
    let tail = id.rsplit('_').next().unwrap_or(id);
    let tail8 = if tail.len() >= 8 {
        &tail[tail.len() - 8..]
    } else {
        tail
    };
    exp == id
        || exp == tail
        || exp == tail8
        || exp.rsplit('_').next().is_some_and(|suffix| suffix == tail)
}

fn hit_expected(exp: &[String], id: &str) -> bool {
    exp.iter().any(|e| id_matches(e, id))
}

fn rel_of(c: &EvalCase, id: &str) -> f64 {
    if let Some(v) = c.rel.get(id) {
        return *v;
    }
    let tail = id.rsplit('_').next().unwrap_or(id);
    if let Some(v) = c.rel.get(tail) {
        return *v;
    }
    let tail8 = if tail.len() >= 8 {
        &tail[tail.len() - 8..]
    } else {
        tail
    };
    if let Some(v) = c.rel.get(tail8) {
        return *v;
    }
    // 慢路径：rel 用完整 id、命中用另一日期前缀时按后缀匹配。
    // 先按 key 排序再取，避免 HashMap 遍历顺序影响重复/近重复键的选择。
    let mut keys: Vec<&String> = c.rel.keys().collect();
    keys.sort();
    keys.into_iter()
        .find(|k| id_matches(k, id))
        .map(|k| c.rel[k])
        .unwrap_or(0.0)
}

/// 单例指标：Recall@5 / Recall@10（expected 命中占比）、MRR（首个 expected 的 1/rank）、
/// NDCG@10（rel 分级，DCG/IDCG）。r10 供弱相关 semantic 层使用。
fn metric(c: &EvalCase, hits: &[RecallHit]) -> (f64, f64, f64, f64) {
    let (r5, r10) = if c.expected.is_empty() {
        (0.0, 0.0)
    } else {
        let hit5 = hits
            .iter()
            .take(5)
            .filter(|h| hit_expected(&c.expected, &h.id))
            .count();
        let hit10 = hits
            .iter()
            .take(10)
            .filter(|h| hit_expected(&c.expected, &h.id))
            .count();
        (
            hit5 as f64 / c.expected.len() as f64,
            hit10 as f64 / c.expected.len() as f64,
        )
    };
    let mut mrr = 0.0;
    for (i, h) in hits.iter().take(10).enumerate() {
        if hit_expected(&c.expected, &h.id) {
            mrr = 1.0 / (i as f64 + 1.0);
            break;
        }
    }
    let dcg_r: Vec<f64> = hits.iter().take(10).map(|h| rel_of(c, &h.id)).collect();
    let dc = dcg(&dcg_r);
    // IDCG 必须来自 rel 字典里所有标注为相关的候选（含非 expected 的弱相关/强相关），
    // 不能只用 expected：否则实际 top-10 里的 rel=1 候选会让 DCG > IDCG，NDCG 突破 1.0。
    let mut ideal: Vec<f64> = c.rel.values().copied().filter(|v| *v > 0.0).collect();
    ideal.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    ideal.truncate(10);
    let idcg = dcg(&ideal);
    let ndcg = if idcg > 0.0 { dc / idcg } else { 0.0 };
    (r5, r10, mrr, ndcg)
}

/// DCG：Σ rel_i / log2(i+1)，i 从 0 计（log2(2)=1）。
fn dcg(rels: &[f64]) -> f64 {
    rels.iter()
        .enumerate()
        .map(|(i, r)| r / (i as f64 + 2.0).log2())
        .sum()
}

/// 单一评估入口：报告/三种门禁 × 是否逐例诊断由参数决定，不再维护函数矩阵。
pub(crate) fn run(
    root: &str,
    path: &str,
    mode: EvalMode,
    verbose: bool,
) -> std::io::Result<(Vec<String>, bool)> {
    let gate_mode = match mode {
        EvalMode::Report => None,
        EvalMode::Mechanical => Some(GateMode::Mechanical),
        EvalMode::Human => Some(GateMode::Human),
        EvalMode::Items => Some(GateMode::Items),
    };
    run_impl(root, path, verbose, gate_mode)
}

fn run_impl(
    root: &str,
    path: &str,
    verbose: bool,
    gate_mode: Option<GateMode>,
) -> std::io::Result<(Vec<String>, bool)> {
    let txt = std::fs::read_to_string(path)?;
    let v = crate::memory::json::parse(&txt)
        .map_err(|e| std::io::Error::other(format!("eval: {e}")))?;
    let cases = parse_baseline(&v).map_err(std::io::Error::other)?;
    let meta = store::load_meta(root)?;
    let eval_now_s = crate::memory::util::parse_utc_secs(&meta.created_at);
    let (l1_base, l1_margin) = crate::memory::feedback::l1_params(&meta);
    let plain = PlainScorer::load(root, &meta)?;
    let mut l1_hit = 0usize;
    let mut l1_total = 0usize;
    // 分层顺序按首次出现保持（golden 稳定）。
    let mut layers: Vec<String> = Vec::new();
    // [n, A:r5,r10,mrr,ndcg, B:r5,r10,mrr,ndcg]；semantic 层报告 r@10（弱相关口径）。
    let mut agg: HashMap<String, [f64; 9]> = HashMap::new();
    // 每层每 case 的 A/B mrr/ndcg 值（bootstrap 95% CI 重采样用）。
    let mut per: HashMap<String, [Vec<f64>; 4]> = HashMap::new();
    // verbose 时缓存每例的 A/B 命中，聚合后统一输出诊断（避免二次召回）。
    let mut diag: Vec<(String, Vec<RecallHit>, Vec<RecallHit>)> = Vec::new();
    for c in &cases {
        let a = recall::recall_at(root, &c.query, 10, eval_now_s, &meta)?;
        let b = plain.search(&c.query, 10);
        if let Some(top) = a.first() {
            if hit_expected(&c.expected, &top.id) {
                l1_total += 1;
                let margin_ok = a
                    .get(1)
                    .is_none_or(|second| top.score - second.score >= l1_margin);
                if top.sim >= l1_base && margin_ok {
                    l1_hit += 1;
                }
            }
        }
        let (ar5, ar10, amrr, andcg) = metric(c, &a);
        let (br5, br10, bmrr, bndcg) = metric(c, &b);
        let e = agg.entry(c.layer.clone()).or_insert([0.0; 9]);
        e[0] += 1.0;
        e[1] += ar5;
        e[2] += ar10;
        e[3] += amrr;
        e[4] += andcg;
        e[5] += br5;
        e[6] += br10;
        e[7] += bmrr;
        e[8] += bndcg;
        let p = per
            .entry(c.layer.clone())
            .or_insert_with(|| [Vec::new(), Vec::new(), Vec::new(), Vec::new()]);
        p[0].push(amrr);
        p[1].push(andcg);
        p[2].push(bmrr);
        p[3].push(bndcg);
        if !layers.iter().any(|l| l == &c.layer) {
            layers.push(c.layer.clone());
        }
        if verbose {
            diag.push((c.query.clone(), a, b));
        }
    }
    let mut out = Vec::with_capacity(layers.len() + 2);
    // 报告 NDCG 判别强度——multi-rel 条目（expected>1 或 rel 多值）
    // 使 NDCG 非二值；single-rel 条目 NDCG 接近二值（单相关时评估严格性有限）。
    let multi_rel = cases
        .iter()
        .filter(|c| c.expected.len() > 1 || c.rel.values().any(|r| *r > 0.0 && *r < 2.0))
        .count();
    out.push(format!(
        "eval: cases={} layers={} multi-rel={} single-rel={} (NDCG strength)",
        cases.len(),
        layers.join(","),
        multi_rel,
        cases.len().saturating_sub(multi_rel)
    ));
    for l in &layers {
        let e = agg.get(l).unwrap_or(&[0.0; 9]);
        let n = e[0];
        let p = per.get(l).unwrap();
        // 主指标（预声明）：r@5/r@10 点值 + MRR/NDCG bootstrap 95% CI（固定 seed 确定性）。
        let am_ci = boot_ci(&p[0], CI_SEED);
        let an_ci = boot_ci(&p[1], CI_SEED);
        let bm_ci = boot_ci(&p[2], CI_SEED);
        let bn_ci = boot_ci(&p[3], CI_SEED);
        // 弱相关层（semantic）用 r@10 口径；强相关层（literal/paraphrase）用 r@5。
        if l == "semantic" {
            out.push(format!(
                "layer {l}: n={n:.0} A r@10={:.3} mrr={:.3} ci95=[{:.3},{:.3}] ndcg@10={:.3} ci95=[{:.3},{:.3}] | B r@10={:.3} mrr={:.3} ci95=[{:.3},{:.3}] ndcg@10={:.3} ci95=[{:.3},{:.3}]",
                e[2] / n.max(1.0),
                e[3] / n.max(1.0),
                am_ci.0,
                am_ci.1,
                e[4] / n.max(1.0),
                an_ci.0,
                an_ci.1,
                e[6] / n.max(1.0),
                e[7] / n.max(1.0),
                bm_ci.0,
                bm_ci.1,
                e[8] / n.max(1.0),
                bn_ci.0,
                bn_ci.1,
            ));
        } else {
            out.push(format!(
                "layer {l}: n={n:.0} A r@5={:.3} mrr={:.3} ci95=[{:.3},{:.3}] ndcg@10={:.3} ci95=[{:.3},{:.3}] | B r@5={:.3} mrr={:.3} ci95=[{:.3},{:.3}] ndcg@10={:.3} ci95=[{:.3},{:.3}]",
                e[1] / n.max(1.0),
                e[3] / n.max(1.0),
                am_ci.0,
                am_ci.1,
                e[4] / n.max(1.0),
                an_ci.0,
                an_ci.1,
                e[5] / n.max(1.0),
                e[7] / n.max(1.0),
                bm_ci.0,
                bm_ci.1,
                e[8] / n.max(1.0),
                bn_ci.0,
                bn_ci.1,
            ));
        }
    }
    // L1 直答命中率出表。
    let l1_rate = if l1_total > 0 {
        l1_hit as f64 / l1_total as f64
    } else {
        0.0
    };
    out.push(format!(
        "l1: hits={l1_hit} total={l1_total} rate={l1_rate:.3}"
    ));
    // 机械门——样本量/分层/裁决元数据/主指标 A≥B。
    let mut gate_failed = false;
    if let Some(mode) = gate_mode {
        let g = match mode {
            GateMode::Mechanical => crate::memory::gate::check(&cases, &layers, &agg),
            GateMode::Human => crate::memory::gate::check_human(&cases, &layers, &agg),
            GateMode::Items => crate::memory::gate::check_items(&cases, &layers, &agg),
        };
        out.extend(g.lines);
        gate_failed = g.failed;
    }
    // 逐 case 诊断段（A/B 各 top-10，case 尾 8 位与 --eval 口径一致）。
    if verbose {
        for (i, (q, a, b)) in diag.iter().enumerate() {
            out.push(format!("case {}/{} q=\"{}\"", i + 1, diag.len(), q));
            out.push(format!("  A: {}", fmt_hits(a, &cases[i])));
            out.push(format!("  B: {}", fmt_hits(b, &cases[i])));
        }
    }
    Ok((out, gate_failed))
}

/// 单行候选诊断：`1) <tail8> sim=0.86 score=0.55 [hit] | 2) ... [miss]`。
fn fmt_hits(hits: &[RecallHit], c: &EvalCase) -> String {
    if hits.is_empty() {
        return "(no hits)".to_string();
    }
    let mut parts = Vec::with_capacity(hits.len());
    for (i, h) in hits.iter().enumerate() {
        let tail = h.id.rsplit('_').next().unwrap_or(&h.id);
        let tag = if hit_expected(&c.expected, &h.id) {
            "hit"
        } else {
            "miss"
        };
        parts.push(format!(
            "{}) {} sim={:.2} score={:.2} [{}]",
            i + 1,
            tail,
            h.sim,
            h.score,
            tag
        ));
    }
    parts.join(" | ")
}

/// bootstrap 95% CI（百分位法，固定 seed 确定性可重放）。
/// 对每层每 case 的指标值有放回重采样，取 2.5/97.5 百分位——n 很小（9/3）时
/// 置信区间如实展示不确定性，不允许"点值更高 = 更好"的结论。
const CI_SEED: u64 = 0x5DEE_CE66_D202_6102;

/// xorshift64*：零依赖确定性 PRNG（无随机源，同 seed 逐位可重放）。
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

/// 有放回重采样 B 次 → 样本均值 → 95% 分位区间。空输入返回 (0,0)。
fn boot_ci(vals: &[f64], seed: u64) -> (f64, f64) {
    const B: usize = 1000;
    let n = vals.len();
    if n == 0 {
        return (0.0, 0.0);
    }
    let mut rng = Rng(seed ^ 0x9E37_79B9_7F4A_7C15);
    let mut means = Vec::with_capacity(B);
    for _ in 0..B {
        let mut s = 0.0;
        for _ in 0..n {
            let idx = (rng.next() % n as u64) as usize;
            s += vals[idx];
        }
        means.push(s / n as f64);
    }
    means.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let lo = means[(B * 25 / 1000).min(B - 1)];
    let hi = means[(B * 975 / 1000).min(B - 1)];
    (lo, hi)
}

/// 评估 B 基线：朴素 BM25。全库候选（无桶过滤）、纯词面 BM25（无字段加权/
/// 衰减/quality/词权/PRF/MMR）、sim=score/sm 归一、按 score 降序取 top-k。
/// 语料只加载一次，150 条查询复用，避免每个 case 重复读取全库 JSON。
struct PlainScorer {
    docs: Vec<(String, Vec<String>)>,
    df: HashMap<String, usize>,
    n: f64,
    avgdl: f64,
    k1: f64,
    b: f64,
}

impl PlainScorer {
    fn load(root: &str, meta: &store::Meta) -> std::io::Result<Self> {
        let mut docs = Vec::new();
        let mut df: HashMap<String, usize> = HashMap::new();
        let mut total_len = 0usize;
        for id in store::list_ids(root)? {
            let c = store::read_case(root, &id)?;
            let (_, terms) = build_doc_terms(&c.query, &c.text);
            total_len += terms.len();
            for t in &terms {
                *df.entry(t.clone()).or_insert(0) += 1;
            }
            docs.push((id, terms));
        }
        let n = docs.len() as f64;
        Ok(Self {
            avgdl: if n > 0.0 { total_len as f64 / n } else { 0.0 },
            n,
            docs,
            df,
            k1: meta.param("bm25_k1"),
            b: meta.param("bm25_b"),
        })
    }

    fn search(&self, query: &str, k: usize) -> Vec<RecallHit> {
        if self.docs.is_empty() {
            return Vec::new();
        }
        let qterms = build_mem_terms(query);
        // query 相关 IDF 只依赖 df，与文档无关：每条 query 预计算一次，
        // 避免在 10k 候选 × 150 query 的热路径上重复做 df 查表与 ln。
        let qidf: Vec<f64> = qterms
            .iter()
            .map(|qt| {
                let d = *self.df.get(qt).unwrap_or(&0) as f64;
                crate::memory::ranking::idf(self.n, d)
            })
            .collect();
        let sm: f64 = qidf.iter().sum();
        let mut scored: Vec<(usize, f64)> = Vec::with_capacity(self.docs.len());
        for (idx, (_, terms)) in self.docs.iter().enumerate() {
            let mut score = 0.0;
            let denom =
                crate::memory::ranking::denom(self.k1, self.b, terms.len() as f64, self.avgdl);
            for (qt, idf) in qterms.iter().zip(&qidf) {
                if terms.binary_search(qt).is_err() {
                    continue;
                }
                score += crate::memory::ranking::score(*idf, self.k1, denom);
            }
            // 与 A 管线的零支持度过滤同口径：没有 query 词命中的 case 不得以
            // 0 分进入 top-k，否则 B 会靠库顺序“碰巧命中”被高估。
            if score > 0.0 {
                scored.push((idx, score));
            }
        }
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| self.docs[a.0].0.cmp(&self.docs[b.0].0))
        });
        scored.truncate(k);
        scored
            .into_iter()
            .map(|(idx, score)| RecallHit {
                id: self.docs[idx].0.clone(),
                score,
                sim: if sm > 0.0 {
                    (score / sm).clamp(0.0, 1.0)
                } else {
                    0.0
                },
            })
            .collect()
    }
}

/// 运行期 [l1] 历史出表（--l1-hist）——top1.sim 采样分布统计。
/// hist 由 recall 侧每次 push_l1_hist 落盘（meta.toml [l1] hist，CAP 上限），
/// 出表兑现"L1 命中率可统计"；over_base = 达到当前 l1_base 的采样占比。
pub(crate) fn l1_hist_table(root: &str) -> std::io::Result<Vec<String>> {
    let meta = store::load_meta(root)?;
    let h = &meta.l1_hist;
    if h.is_empty() {
        return Ok(vec!["l1hist: n=0 (no recall history yet)".to_string()]);
    }
    let mut sorted = h.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p = |q: f64| {
        let idx = ((sorted.len() as f64 - 1.0) * q).round() as usize;
        sorted[idx]
    };
    let mean = h.iter().sum::<f64>() / h.len() as f64;
    let base = crate::memory::feedback::l1_params(&meta).0;
    let over = h.iter().filter(|s| **s >= base).count();
    Ok(vec![format!(
        "l1hist: n={} mean={mean:.3} p50={:.3} p90={:.3} over_base={over} (base={base:.2})",
        h.len(),
        p(0.50),
        p(0.90),
    )])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(id: &str, score: f64) -> RecallHit {
        RecallHit {
            id: id.to_string(),
            score,
            sim: score,
        }
    }

    #[test]
    fn dcg_matches_standard() {
        // rel [2,1]：2/1 + 1/log2(3) = 2 + 0.6309
        let d = dcg(&[2.0, 1.0]);
        assert!((d - (2.0 + 1.0 / (3.0f64).log2())).abs() < 1e-9);
        assert_eq!(dcg(&[]), 0.0);
    }

    #[test]
    fn expected_does_not_override_tail_label_with_one() {
        // rel 用 hash 后缀、expected 用完整 id（跨日重建后的常见形态）：
        // 解析不得再插入一个 rel=1 的完整 id 键，否则 rel_of 随机取到 1/2。
        let v = crate::memory::json::parse(
            r#"{"cases":[{"query":"q","layer":"literal","expected":["2026-10-22_abcdef0123456789"],"rel":{"abcdef0123456789":2},"judge":{"by":"fei","date":"2026-10-24T00:00:00Z"}}]}"#,
        )
        .unwrap();
        let cases = parse_baseline(&v).unwrap();
        assert_eq!(cases[0].rel.len(), 1, "{:?}", cases[0].rel);
        assert_eq!(rel_of(&cases[0], "2026-10-24_abcdef0123456789"), 2.0);
    }

    #[test]
    fn metric_recall_mrr_ndcg() {
        let mut rel = HashMap::new();
        rel.insert("c1".to_string(), 2.0);
        rel.insert("c2".to_string(), 1.0);
        let c = EvalCase {
            query: "q".to_string(),
            layer: "literal".to_string(),
            expected: vec!["c1".to_string(), "c2".to_string()],
            rel,
            judged_by: None,
            judged_date: None,
            judged_method: None,
            judge_items: HashMap::new(),
        };
        // 命中顺序 c2 在前：recall@5=1.0，recall@10=1.0，MRR=1/1（c2 首个 expected 即第一），NDCG<1（c1 排后）。
        let hits = vec![hit("c2", 0.9), hit("c1", 0.8), hit("c3", 0.1)];
        let (r5, r10, mrr, ndcg) = metric(&c, &hits);
        assert!((r5 - 1.0).abs() < 1e-9);
        assert!((r10 - 1.0).abs() < 1e-9);
        assert!((mrr - 1.0).abs() < 1e-9);
        // DCG=1/1+2/log2(3)=2.2619，IDCG=2/1+1/log2(3)=2.6309 → NDCG≈0.8597（c1 rel=2 排在 c2 后）。
        assert!((ndcg - 0.8597).abs() < 1e-3, "ndcg={ndcg}");
        // 完全未命中：全 0。
        let (r5, r10, mrr, ndcg) = metric(&c, &[hit("c9", 0.5)]);
        assert_eq!((r5, r10, mrr, ndcg), (0.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn frozen_full_ids_match_across_date_prefix() {
        let expected = vec!["2026-10-22_337ecde86ee61f26".to_string()];
        let current = "2026-10-24_337ecde86ee61f26";
        assert!(hit_expected(&expected, current));
        let mut rel = HashMap::new();
        rel.insert(expected[0].clone(), 2.0);
        let c = EvalCase {
            query: "q".to_string(),
            layer: "literal".to_string(),
            expected,
            rel,
            judged_by: None,
            judged_date: None,
            judged_method: None,
            judge_items: HashMap::new(),
        };
        assert_eq!(rel_of(&c, current), 2.0);
        assert!(!id_matches("2026-10-22_aaaaaaaaaaaaaaaa", current));
    }

    #[test]
    fn ndcg_never_exceeds_one_with_non_expected_relevant() {
        // rel 里有非 expected 的弱相关候选：理想排序必须把它纳入 IDCG，
        // 否则 top-10 命中它时 NDCG 会超过 1.0（回归：gate 实测 literal 1.108）。
        let mut rel = HashMap::new();
        rel.insert("t".to_string(), 2.0);
        rel.insert("w".to_string(), 1.0);
        let c = EvalCase {
            query: "q".to_string(),
            layer: "literal".to_string(),
            expected: vec!["t".to_string()],
            rel,
            judged_by: None,
            judged_date: None,
            judged_method: None,
            judge_items: HashMap::new(),
        };
        let hits = vec![hit("t", 1.0), hit("w", 0.9)];
        let (_, _, _, ndcg) = metric(&c, &hits);
        // DCG = 2/1 + 1/log2(3)；IDCG 同为 [2,1] → NDCG=1.0（不越界）。
        assert!((ndcg - 1.0).abs() < 1e-9, "ndcg={ndcg}");
    }

    #[test]
    fn parse_baseline_is_strict_and_tolerant() {
        let v = crate::memory::json::parse(
            r#"{"cases":[{"query":"q1","layer":"literal","expected":["a","b"],"rel":{"a":2}}]}"#,
        )
        .unwrap();
        let cases = parse_baseline(&v).unwrap();
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].expected, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(cases[0].rel.get("a"), Some(&2.0));
        assert_eq!(cases[0].rel.get("b"), Some(&1.0), "expected 内缺省 rel=1");
        // 缺 cases → 报错。
        assert!(parse_baseline(&Val::Obj(Vec::new())).is_err());
        // rel 越界 → 报错。
        let bad = crate::memory::json::parse(
            r#"{"cases":[{"query":"q","layer":"l","expected":["a"],"rel":{"a":3}}]}"#,
        )
        .unwrap();
        assert!(parse_baseline(&bad).is_err());
    }

    #[test]
    fn parse_baseline_reads_per_item_provenance() {
        let v = crate::memory::json::parse(
            r#"{"cases":[{"query":"q","layer":"semantic","expected":["a"],
                "rel":{"a":2,"b":0},
                "judge":{"by":"fei","date":"2026-10-24T00:00:00Z","method":"per-item-blind-v1"},
                "judge_items":{"a":{"by":"fei","date":"2026-10-24T00:00:00Z"},
                               "b":{"by":"fei","date":"2026-10-24T00:00:00Z"}}}]}"#,
        )
        .unwrap();
        let cases = parse_baseline(&v).unwrap();
        assert_eq!(cases[0].judge_items.len(), 2);
        assert_eq!(
            cases[0].judge_items.get("b").map(String::as_str),
            Some("fei")
        );
        assert_eq!(cases[0].judged_method.as_deref(), Some("per-item-blind-v1"));
    }

    /// judge:null（未裁决）必须被 --eval 拒绝，防止接受预填 rel 自证。
    #[test]
    fn parse_baseline_rejects_unjudged() {
        let v = crate::memory::json::parse(
            r#"{"cases":[{"query":"q","layer":"paraphrase","expected":["a"],"judge":null}]}"#,
        )
        .unwrap();
        let e = parse_baseline(&v).unwrap_err();
        assert!(e.contains("unjudged"), "{e}");
        // 裁决后（judge 对象 + rel）→ 正常解析。
        let v2 = crate::memory::json::parse(
            r#"{"cases":[{"query":"q","layer":"paraphrase","expected":["a"],"rel":{"a":1},"judge":{"by":"tester","date":"2026-10-22T00:00:00Z"}}]}"#,
        )
        .unwrap();
        let cases = parse_baseline(&v2).unwrap();
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].rel.get("a"), Some(&1.0));
        assert_eq!(cases[0].judged_by.as_deref(), Some("tester"));
        assert_eq!(
            cases[0].judged_date.as_deref(),
            Some("2026-10-22T00:00:00Z")
        );
    }

    /// bootstrap CI 必须确定性（同 seed 同输出）且区间在 [min,max] 内。
    #[test]
    fn bootstrap_ci_deterministic_and_bounded() {
        let vals = vec![1.0, 1.0, 1.0, 1.0, 0.5, 0.0];
        let (a_lo, a_hi) = boot_ci(&vals, CI_SEED);
        let (b_lo, b_hi) = boot_ci(&vals, CI_SEED);
        assert_eq!(a_lo, b_lo, "同 seed 必须逐位可重放");
        assert_eq!(a_hi, b_hi);
        assert!(a_lo >= 0.0 && a_hi <= 1.000_000_1, "区间在指标范围内");
        assert!(a_lo <= a_hi);
        // 空输入 → (0,0) 不 panic。
        assert_eq!(boot_ci(&[], CI_SEED), (0.0, 0.0));
    }
}
