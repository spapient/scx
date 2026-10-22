//! P4.4+ 优化⑭：M4 证据门的机械校验（`--eval --gate`）。
//!
//! 目标：把“30–50 组、每层 ≥10、非 literal 必须人工裁决、A 不劣于 B”
//! 从口头约定变成可执行门禁，防止用 5 条 demo 或自动标注集冒充 M4 证据。
//!
//! 纯函数：只读解析后的 EvalCase 与已聚合指标，不触碰库、不写文件。

use crate::memory::eval::EvalCase;
use std::collections::HashMap;

/// M4 证据门最少总组数（计划 §7：30–50 组；超过 50 组不惩罚）。
pub(crate) const MIN_TOTAL: usize = 30;
/// 每层最少组数（literal / paraphrase / semantic）。
pub(crate) const MIN_LAYER: usize = 10;
/// 浮点比较容差：指标是整数计数除以 n，1e-9 足以吸收除法误差。
const EPS: f64 = 1e-9;

pub(crate) struct GateResult {
    pub lines: Vec<String>,
    pub failed: bool,
}

/// 机械校验 M4 证据门。`layers` 为报告层顺序，`agg` 为 eval 的 [n, A(4), B(4)] 聚合。
pub(crate) fn check(
    cases: &[EvalCase],
    layers: &[String],
    agg: &HashMap<String, [f64; 9]>,
) -> GateResult {
    let mut lines = Vec::new();
    let mut failed = false;

    lines.push(format!(
        "gate: total={} need>={MIN_TOTAL} (target 30-50) {}",
        cases.len(),
        verdict(cases.len() >= MIN_TOTAL)
    ));
    failed |= cases.len() < MIN_TOTAL;

    for layer in ["literal", "paraphrase", "semantic"] {
        let n = cases.iter().filter(|c| c.layer == layer).count();
        let ok = n >= MIN_LAYER;
        lines.push(format!(
            "gate: layer {layer} n={n} need>={MIN_LAYER} {}",
            verdict(ok)
        ));
        failed |= !ok;
    }

    // 未知层不允许用来凑总数：M4 只认三层。
    for layer in layers {
        if !matches!(layer.as_str(), "literal" | "paraphrase" | "semantic") {
            lines.push(format!("gate: unknown layer {layer} FAIL"));
            failed = true;
        }
    }

    // literal 是 query 原样/指纹机械判定，不要求 judge 元数据；
    // paraphrase/semantic 必须由 --judge 写入 by+date，自动标注集不得过门。
    let non_literal = cases.iter().filter(|c| c.layer != "literal").count();
    let meta_ok = cases
        .iter()
        .filter(|c| c.layer != "literal")
        .all(|c| non_empty(c.judged_by.as_deref()) && non_empty(c.judged_date.as_deref()));
    lines.push(format!(
        "gate: judge metadata nonliteral={non_literal} (need by+date) {}",
        verdict(meta_ok)
    ));
    failed |= !meta_ok;

    // 主指标预声明：literal/paraphrase 用 r@5，semantic 用 r@10；另加 MRR。
    // NDCG 只报告不过门（单相关时接近二值，判别力弱）。
    for layer in layers {
        let Some(e) = agg.get(layer) else {
            continue;
        };
        let n = e[0];
        if n <= 0.0 {
            continue;
        }
        let (a_r, b_r, metric) = if layer == "semantic" {
            (e[2] / n, e[6] / n, "r@10")
        } else {
            (e[1] / n, e[5] / n, "r@5")
        };
        let a_mrr = e[3] / n;
        let b_mrr = e[7] / n;
        let r_ok = a_r + EPS >= b_r;
        let m_ok = a_mrr + EPS >= b_mrr;
        lines.push(format!(
            "gate: layer {layer} A {metric}={a_r:.3} vs B={b_r:.3} {}; A mrr={a_mrr:.3} vs B={b_mrr:.3} {}",
            verdict(r_ok),
            verdict(m_ok)
        ));
        failed |= !r_ok || !m_ok;
    }

    lines.push(format!("gate: {}", if failed { "FAIL" } else { "PASS" }));
    GateResult { lines, failed }
}

fn verdict(ok: bool) -> &'static str {
    if ok {
        "PASS"
    } else {
        "FAIL"
    }
}

fn non_empty(s: Option<&str>) -> bool {
    s.is_some_and(|v| !v.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(layer: &str, judged: bool) -> EvalCase {
        EvalCase {
            query: format!("q-{layer}"),
            layer: layer.to_string(),
            expected: vec!["a".to_string()],
            rel: HashMap::new(),
            judged_by: judged.then(|| "fei".to_string()),
            judged_date: judged.then(|| "2026-10-22T00:00:00Z".to_string()),
        }
    }

    fn corpus() -> Vec<EvalCase> {
        let mut cases = Vec::new();
        for layer in ["literal", "paraphrase", "semantic"] {
            for _ in 0..MIN_LAYER {
                cases.push(case(layer, layer != "literal"));
            }
        }
        cases
    }

    fn perfect_agg() -> HashMap<String, [f64; 9]> {
        let mut agg = HashMap::new();
        for layer in ["literal", "paraphrase", "semantic"] {
            // n=10，A/B 全部命中且排第 1：r=1、mrr=1、ndcg=1。
            agg.insert(layer.to_string(), [10.0; 9]);
        }
        agg
    }

    #[test]
    fn gate_passes_complete_judged_corpus_with_a_not_worse_than_b() {
        let cases = corpus();
        let layers = vec![
            "literal".to_string(),
            "paraphrase".to_string(),
            "semantic".to_string(),
        ];
        let r = check(&cases, &layers, &perfect_agg());
        assert!(!r.failed, "{}", r.lines.join("\n"));
        assert!(r.lines.last().unwrap().contains("gate: PASS"));
    }

    #[test]
    fn gate_rejects_small_or_unbalanced_corpus() {
        let mut cases = corpus();
        cases.truncate(23);
        let layers = vec!["literal".to_string(), "paraphrase".to_string()];
        let r = check(&cases, &layers, &perfect_agg());
        assert!(r.failed);
        let joined = r.lines.join("\n");
        assert!(joined.contains("total=23 need>=30"), "{joined}");
        assert!(
            joined.contains("layer semantic n=3 need>=10 FAIL"),
            "{joined}"
        );
    }

    #[test]
    fn gate_rejects_nonliteral_without_judge_metadata() {
        let mut cases = corpus();
        cases[10].judged_by = None; // 第一条 paraphrase 失去裁决人
        let layers = vec![
            "literal".to_string(),
            "paraphrase".to_string(),
            "semantic".to_string(),
        ];
        let r = check(&cases, &layers, &perfect_agg());
        assert!(r.failed);
        assert!(
            r.lines.join("\n").contains("judge metadata nonliteral=20"),
            "{}",
            r.lines.join("\n")
        );
    }

    #[test]
    fn gate_rejects_a_below_b_on_primary_metric() {
        let cases = corpus();
        let layers = vec![
            "literal".to_string(),
            "paraphrase".to_string(),
            "semantic".to_string(),
        ];
        let mut agg = perfect_agg();
        // paraphrase：A r@5=0.9，B=1.0；MRR 仍持平 → 必须失败。
        agg.get_mut("paraphrase").unwrap()[1] = 9.0;
        let r = check(&cases, &layers, &agg);
        assert!(r.failed);
        assert!(
            r.lines.join("\n").contains("A r@5=0.900 vs B=1.000 FAIL"),
            "{}",
            r.lines.join("\n")
        );
    }

    #[test]
    fn gate_rejects_unknown_layer() {
        let mut cases = corpus();
        cases.push(case("other", false));
        let mut layers = vec![
            "literal".to_string(),
            "paraphrase".to_string(),
            "semantic".to_string(),
        ];
        layers.push("other".to_string());
        let mut agg = perfect_agg();
        agg.insert("other".to_string(), [1.0; 9]);
        let r = check(&cases, &layers, &agg);
        assert!(r.failed);
        assert!(r.lines.join("\n").contains("unknown layer other FAIL"));
    }
}
