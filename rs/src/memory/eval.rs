//! P4 评估系统与要点摘要（P4.1/P4.2，O8/O5 后半）。
//!
//! `--eval <baseline.json>`：冻结评估输入重放——标注集（query → expected case + 分层 +
//! 相关度 0..2）上分三层（literal/paraphrase/semantic）输出 Recall@5 / MRR / NDCG@10。
//! 对拍：A = 完整统计管线（recall::recall，字段加权+PRF+衰减+MMR+反馈+桶过滤），
//! B = 朴素词面基线（`plain`：全库候选、纯词面匹配 + 长度归一化、无任何增强）。
//!
//! 口径说明（P4.4 评审处置）：打分词表两边都是去重词表（tf 恒 1），实际是
//! "二值词面匹配 + 长度归一化 + 字段加权"，不是严格 BM25（tf 项恒为 1）。
//! A/B 同词表对比仍公平，但任何结论须按此口径引用，不宣称"BM25 增益"。
//! 确定性：无随机源，同 baseline + 同库 → 同输出，可重放存档。
//!
//! `case_summary`：L1 直答要点行（error_code + file_loc + 证据块首个非空行，≤300 字符），
//! 供 CLI `--recall-detail` 展开摘要，不把整块正文塞进直答。

use crate::memory::json::Val;
use crate::memory::recall::{self, RecallHit};
use crate::memory::store;
use crate::terms::build_terms;
use std::collections::HashMap;

/// 单条评估输入（baseline.json 的一行标注）。
#[derive(Debug)]
pub(crate) struct EvalCase {
    pub query: String,
    pub layer: String,
    pub expected: Vec<String>,
    pub rel: HashMap<String, f64>,
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
        // P4.4+ 优化⑩：盲测裁决门——judge:null（未裁决）一律拒绝，防止
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
        for e in &expected {
            rel.entry(e.clone()).or_insert(1.0);
        }
        out.push(EvalCase {
            query,
            layer,
            expected,
            rel,
        });
    }
    Ok(out)
}

/// expected/rel 的 id 匹配：支持完整 id、hash 后缀（tail）或 hash 后缀的后 8 位
/// （P4.4 评审处置：case id 由 32 位升级 64 位后 tail=16 hex，标注集里的旧 8 位
/// hash 仍须匹配，golden 免日期漂移）。
fn hit_expected(exp: &[String], id: &str) -> bool {
    let tail = id.rsplit('_').next().unwrap_or(id);
    let tail8 = if tail.len() >= 8 {
        &tail[tail.len() - 8..]
    } else {
        tail
    };
    exp.iter().any(|e| e == id || e == tail || e == tail8)
}

fn rel_of(c: &EvalCase, id: &str) -> f64 {
    let tail = id.rsplit('_').next().unwrap_or(id);
    let tail8 = if tail.len() >= 8 {
        &tail[tail.len() - 8..]
    } else {
        tail
    };
    c.rel
        .get(id)
        .copied()
        .or_else(|| c.rel.get(tail).copied())
        .or_else(|| c.rel.get(tail8).copied())
        .unwrap_or(0.0)
}

/// 单例指标：Recall@5 / Recall@10（expected 命中占比）、MRR（首个 expected 的 1/rank）、
/// NDCG@10（rel 分级，DCG/IDCG）。r10 供弱相关层（semantic）口径使用（P4.4+ 优化⑦）。
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
    let mut ideal: Vec<f64> = c.expected.iter().map(|id| rel_of(c, id)).collect();
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

/// 评估入口（默认报告）：读取 baseline.json → 逐例 A/B 召回 → 分层聚合 → 报告行（确定性）。
pub(crate) fn run(root: &str, path: &str) -> std::io::Result<Vec<String>> {
    run_impl(root, path, false)
}

/// 评估入口（--eval-verbose）：聚合报告 + 逐 case 的 A/B top-10 候选诊断
/// （rank/sim/score/case 尾 8 /[hit]|[miss]）——标注者可见"召回为什么对/错"，
/// 直接服务 M4 证据门的人工标注与校准（P4.4+ 优化④）。
pub(crate) fn run_verbose(root: &str, path: &str) -> std::io::Result<Vec<String>> {
    run_impl(root, path, true)
}

fn run_impl(root: &str, path: &str, verbose: bool) -> std::io::Result<Vec<String>> {
    let txt = std::fs::read_to_string(path)?;
    let v = crate::memory::json::parse(&txt)
        .map_err(|e| std::io::Error::other(format!("eval: {e}")))?;
    let cases = parse_baseline(&v).map_err(std::io::Error::other)?;
    // 分层顺序按首次出现保持（golden 稳定）。
    let mut layers: Vec<String> = Vec::new();
    // [n, A:r5,r10,mrr,ndcg, B:r5,r10,mrr,ndcg]；semantic 层报告 r@10（弱相关口径）。
    let mut agg: HashMap<String, [f64; 9]> = HashMap::new();
    // P4.4+ 优化⑩：每层每 case 的 A/B mrr/ndcg 值（bootstrap 95% CI 重采样用）。
    let mut per: HashMap<String, [Vec<f64>; 4]> = HashMap::new();
    // verbose 时缓存每例的 A/B 命中，聚合后统一输出诊断（避免二次召回）。
    let mut diag: Vec<(String, Vec<RecallHit>, Vec<RecallHit>)> = Vec::new();
    for c in &cases {
        let a = recall::recall(root, &c.query, 10)?;
        let b = plain(root, &c.query, 10)?;
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
    out.push(format!(
        "eval: cases={} layers={}",
        cases.len(),
        layers.join(",")
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
    // P2 阶段门残余：L1 直答命中率出表（[l1] 判据落地后首次可统计）。
    let (l1_hit, l1_total) = l1_stats(root, &cases)?;
    let l1_rate = if l1_total > 0 {
        l1_hit as f64 / l1_total as f64
    } else {
        0.0
    };
    out.push(format!(
        "l1: hits={l1_hit} total={l1_total} rate={l1_rate:.3}"
    ));
    // P4.4+ 优化④：逐 case 诊断段（A/B 各 top-10，case 尾 8 位与 --eval 口径一致）。
    if verbose {
        for (i, (q, a, b)) in diag.iter().enumerate() {
            out.push(format!("case {}/{} q=\"{}\"", i + 1, diag.len(), q));
            out.push(format!("  A: {}", fmt_hits(a, &cases[i])));
            out.push(format!("  B: {}", fmt_hits(b, &cases[i])));
        }
    }
    Ok(out)
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

/// P4.4+ 优化⑩：bootstrap 95% CI（百分位法，B=1000，固定 seed 确定性可重放）。
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

/// P2 阶段门残余：L1 直答命中率——top1 命中 expected 的相关 case 中，满足 L1 置信判据
/// （top1.sim ≥ l1_base 且 top1−top2 ≥ margin，单例视为满足）的比例。
/// 判据参数来自 meta.toml [params]（feedback::l1_params，随库自适应）。
fn l1_stats(root: &str, cases: &[EvalCase]) -> std::io::Result<(usize, usize)> {
    let meta = store::load_meta(root)?;
    let (base, margin) = crate::memory::feedback::l1_params(&meta);
    let mut hit = 0usize;
    let mut total = 0usize;
    for c in cases {
        let hits = recall::recall(root, &c.query, 2)?;
        if hits.is_empty() || !hit_expected(&c.expected, &hits[0].id) {
            continue; // top1 未命中相关 case：不算 L1 可直答对象
        }
        total += 1;
        let top2_ok = hits.len() < 2 || (hits[0].score - hits[1].score) >= margin;
        if hits[0].sim >= base && top2_ok {
            hit += 1;
        }
    }
    Ok((hit, total))
}

/// P4.1 评估 B 基线：朴素 BM25。全库候选（无桶过滤）、纯词面 BM25（无字段加权/
/// 衰减/quality/词权/PRF/MMR）、sim=score/sm 归一、按 score 降序取 top-k。
pub(crate) fn plain(root: &str, query: &str, k: usize) -> std::io::Result<Vec<RecallHit>> {
    let meta = store::load_meta(root)?;
    let k1 = param(&meta, "bm25_k1", 1.2);
    let b = param(&meta, "bm25_b", 0.75);
    let qterms = build_terms(query);
    let ids = store::list_ids(root)?;
    let mut docs: Vec<(String, Vec<String>)> = Vec::with_capacity(ids.len());
    let mut df: HashMap<String, usize> = HashMap::new();
    let mut total_len = 0usize;
    for id in &ids {
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
        terms.sort();
        terms.dedup();
        total_len += terms.len();
        docs.push((id.clone(), terms));
    }
    if docs.is_empty() {
        return Ok(Vec::new());
    }
    let n = docs.len() as f64;
    let avgdl = total_len as f64 / n;
    let mut hits: Vec<RecallHit> = Vec::with_capacity(docs.len());
    for (id, terms) in &docs {
        let dl = terms.len() as f64;
        let mut score = 0.0;
        for qt in &qterms {
            let d = *df.get(qt).unwrap_or(&0) as f64;
            let idf = ((1.0 + (n - d + 0.5) / (d + 0.5)).ln()).max(0.0);
            let f = terms.iter().filter(|t| *t == qt).count() as f64;
            if f > 0.0 {
                let denom = f + k1 * (1.0 - b + b * dl / avgdl);
                score += idf * f * (k1 + 1.0) / denom;
            }
        }
        hits.push(RecallHit {
            id: id.clone(),
            score,
            sim: 0.0,
        });
    }
    let mut sm = 0.0;
    for qt in &qterms {
        let d = *df.get(qt).unwrap_or(&0) as f64;
        sm += ((1.0 + (n - d + 0.5) / (d + 0.5)).ln()).max(0.0);
    }
    for h in hits.iter_mut() {
        h.sim = if sm > 0.0 {
            (h.score / sm).clamp(0.0, 1.0)
        } else {
            0.0
        };
    }
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(k);
    Ok(hits)
}

/// P2 阶段门残余：运行期 [l1] 历史出表（--l1-hist）——top1.sim 采样分布统计。
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

/// P4.2 L1 要点行摘要：error_code + file_loc + 证据块内首个错误行（无则首非空行），
/// ≤300 字符（省略加 …）。
pub(crate) fn case_summary(root: &str, id: &str) -> std::io::Result<String> {
    let c = store::read_case(root, id)?;
    let mut parts: Vec<String> = Vec::new();
    for v in &c.fields.error_code {
        parts.push(v.clone());
    }
    for v in &c.fields.file_loc {
        parts.push(v.clone());
    }
    let is_err_line = |t: &str| {
        t.contains("error")
            || t.contains("ERROR")
            || t.contains("错误")
            || t.contains("ERR")
            || t.contains("warn")
            || t.contains("WARN")
            || t.contains("警告")
    };
    let line = c
        .text
        .lines()
        .map(|l| l.trim())
        .find(|t| !t.is_empty() && is_err_line(t))
        .or_else(|| c.text.lines().map(|l| l.trim()).find(|t| !t.is_empty()));
    if let Some(l) = line {
        parts.push(l.to_string());
    }
    let s = parts.join(" | ");
    let mut out: String = s.chars().take(300).collect();
    if s.chars().count() > 300 {
        out.push('…');
    }
    Ok(out)
}

fn param(m: &store::Meta, key: &str, dflt: f64) -> f64 {
    m.params
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| *v)
        .unwrap_or(dflt)
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
    fn metric_recall_mrr_ndcg() {
        let mut rel = HashMap::new();
        rel.insert("c1".to_string(), 2.0);
        rel.insert("c2".to_string(), 1.0);
        let c = EvalCase {
            query: "q".to_string(),
            layer: "literal".to_string(),
            expected: vec!["c1".to_string(), "c2".to_string()],
            rel,
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
    fn summary_truncates_to_300() {
        use crate::memory::ingest::ingest;
        use crate::memory::store::CaseStats;
        use std::fs;
        let r = std::env::temp_dir().join(format!("scx-eval-smry-{}", store::fnv1a("y") & 0xff));
        let root = r.to_str().unwrap().to_string();
        let _ = fs::remove_dir_all(&root);
        ingest(
            &root,
            "q alpha",
            vec!["alpha".into()],
            &format!("ERROR {}", "x".repeat(400)),
            vec!["/a.log".into()],
            &CaseStats {
                kept: 1,
                total: 1,
                cut_pct: 0.0,
                risk: "HIGH".to_string(),
            },
            vec![],
        )
        .unwrap();
        let ids = store::list_ids(&root).unwrap();
        let s = case_summary(&root, &ids[0]).unwrap();
        assert_eq!(s.chars().count(), 301, "300 字符 + 省略号");
        assert!(s.ends_with('…'));
        let _ = fs::remove_dir_all(&root);
    }

    /// P4.4+ 优化⑩：judge:null（未裁决）必须被 --eval 拒绝——防止"接受预填 rel=自证"。
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
    }

    /// P4.4+ 优化⑩：bootstrap CI 确定性（同 seed 同输出）且区间在 [min,max] 内。
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
