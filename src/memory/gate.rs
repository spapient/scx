//! 证据门：机械、逐项与人工三种强度的可执行校验。
//!
//! `--gate` 只校验机械资格（样本量/分层/裁决元数据/A 不劣于 B）；
//! `--gate-human` 额外要求全量人工裁决、弱相关与负例控制，并校验 NDCG。
//! 后者才是“人工弱相关/负例闭环”的证据门，机械 target 标签不能替代它。
//!
//! 目标：把“30–50 组、每层 ≥10、非 literal 必须人工裁决、A 不劣于 B”
//! 从口头约定变成可执行门禁，防止用 5 条 demo 或自动标注集冒充人工证据。
//!
//! 纯函数：只读解析后的 EvalCase 与已聚合指标，不触碰库、不写文件。

use crate::memory::eval::EvalCase;
use std::collections::HashMap;

/// 证据门最少总组数（30–50 组；超过 50 组不惩罚）。
pub(crate) const MIN_TOTAL: usize = 30;
/// 每层最少组数（literal / paraphrase / semantic）。
pub(crate) const MIN_LAYER: usize = 10;
/// 浮点比较容差：指标是整数计数除以 n，1e-9 足以吸收除法误差。
const EPS: f64 = 1e-9;
/// 绝对地板：拒绝“全部 miss / 基线退化”仍因相对非劣而 PASS。
/// 取值低于 confirm5 各层实测最差值（paraphrase r@5 0.567、MRR 0.242），
/// 只拦截退化到不可用区间的结果，不替代相对非劣判定。
pub(crate) const MIN_RECALL: f64 = 0.20;
pub(crate) const MIN_MRR: f64 = 0.10;
/// 人工闭环最少弱相关标签数：10 与单层样本下限一致，避免单个边界例冒充控制集。
pub(crate) const MIN_WEAK: usize = 10;
/// 人工闭环最少负例标签数：同样取 10（理由同上）。
pub(crate) const MIN_NEGATIVE: usize = 10;
/// 至少两层同时含弱相关与负例：防止全部控制集中在一层。
pub(crate) const MIN_CONTROL_LAYERS: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GateMode {
    Mechanical,
    Human,
    Items,
}

impl GateMode {
    fn name(self) -> &'static str {
        match self {
            GateMode::Mechanical => "mechanical",
            GateMode::Human => "human",
            GateMode::Items => "items",
        }
    }
}

pub(crate) struct GateResult {
    pub lines: Vec<String>,
    pub failed: bool,
}

/// 机械校验证据门。`layers` 为报告层顺序，`agg` 为 eval 的 [n, A(4), B(4)] 聚合。
pub(crate) fn check(
    cases: &[EvalCase],
    layers: &[String],
    agg: &HashMap<String, [f64; 9]>,
) -> GateResult {
    check_mode(cases, layers, agg, GateMode::Mechanical)
}

/// 人工弱相关/负例闭环门：在机械门之上增加全量人工元数据、控制标签与 NDCG 校验。
pub(crate) fn check_human(
    cases: &[EvalCase],
    layers: &[String],
    agg: &HashMap<String, [f64; 9]>,
) -> GateResult {
    check_mode(cases, layers, agg, GateMode::Human)
}

/// 逐候选裁决门（允许非人类裁决人）：证据等级低于 `--gate-human`，
/// 只证明“每一条候选都有独立裁决记录”，不证明人工闭环。
pub(crate) fn check_items(
    cases: &[EvalCase],
    layers: &[String],
    agg: &HashMap<String, [f64; 9]>,
) -> GateResult {
    check_mode(cases, layers, agg, GateMode::Items)
}

fn check_mode(
    cases: &[EvalCase],
    layers: &[String],
    agg: &HashMap<String, [f64; 9]>,
    mode: GateMode,
) -> GateResult {
    let mut lines = Vec::new();
    let mut failed = false;

    lines.push(format!("gate: mode={}", mode.name()));
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

    // 未知层不允许用来凑总数：证据门只认三层。
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
        .all(|c| valid_judge(c.judged_by.as_deref()) && non_empty(c.judged_date.as_deref()));
    lines.push(format!(
        "gate: judge metadata nonliteral={non_literal} (need real by+date) {}",
        verdict(meta_ok)
    ));
    failed |= !meta_ok;
    let mut placeholders: Vec<String> = cases
        .iter()
        .filter(|c| c.layer != "literal" && !valid_judge(c.judged_by.as_deref()))
        .filter_map(|c| c.judged_by.clone())
        .filter(|s| !s.trim().is_empty())
        .collect();
    placeholders.sort();
    placeholders.dedup();
    if !placeholders.is_empty() {
        lines.push(format!(
            "gate: placeholder judge rejected: {}",
            placeholders.join(", ")
        ));
    }
    if mode != GateMode::Mechanical {
        adjudication_checks(
            cases,
            layers,
            agg,
            &mut lines,
            &mut failed,
            mode == GateMode::Human,
        );
    }

    // 主指标预声明：literal/paraphrase 用 r@5，semantic 用 r@10；另加 MRR。
    // NDCG 只报告不过门（单相关时接近二值，判别力弱）。
    // 同时校验 A/B 的绝对地板：B 全 miss 时 A 的“相对优势”不是有效证据。
    let mut diff_layers = 0usize;
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
        let r_floor_ok = a_r >= MIN_RECALL && b_r >= MIN_RECALL;
        let m_floor_ok = a_mrr >= MIN_MRR && b_mrr >= MIN_MRR;
        lines.push(format!(
            "gate: layer {layer} A {metric}={a_r:.3} vs B={b_r:.3} {}; A mrr={a_mrr:.3} vs B={b_mrr:.3} {}",
            verdict(r_ok),
            verdict(m_ok)
        ));
        failed |= !r_ok || !m_ok;
        lines.push(format!(
            "gate: layer {layer} absolute A/B {metric}={a_r:.3}/{b_r:.3}>={MIN_RECALL:.2} {}; mrr={a_mrr:.3}/{b_mrr:.3}>={MIN_MRR:.2} {}",
            verdict(r_floor_ok),
            verdict(m_floor_ok)
        ));
        failed |= !r_floor_ok || !m_floor_ok;
        if (a_r - b_r).abs() > EPS || (a_mrr - b_mrr).abs() > EPS {
            diff_layers += 1;
        }
    }

    // 标签分布与判别力只告警、不单独判失败：全 2 的语料可以证明“能召回”，
    // 但证明不了 A 相对 B 的优势；报告必须带着这两行引用。
    let mut rel2 = 0usize;
    let mut rel1 = 0usize;
    let mut rel0 = 0usize;
    for c in cases {
        for v in c.rel.values() {
            if *v >= 2.0 {
                rel2 += 1;
            } else if *v >= 1.0 {
                rel1 += 1;
            } else {
                rel0 += 1;
            }
        }
    }
    lines.push(format!("gate: rel distribution 2={rel2} 1={rel1} 0={rel0}"));
    if rel2 > 0 && rel1 == 0 && rel0 == 0 {
        lines.push("gate: warning all labels are 2 (no weak/negative controls)".to_string());
    }

    lines.push(format!(
        "gate: discrimination A!=B layers={diff_layers}/{}",
        layers.len()
    ));
    if diff_layers == 0 {
        lines.push("gate: warning A=B on all layers (no measurable advantage)".to_string());
    }

    if mode == GateMode::Mechanical {
        lines.push(
            "gate: note mechanical target labels are not human weak/negative closure; use --gate-human"
                .to_string(),
        );
    }
    lines.push(format!("gate: {}", if failed { "FAIL" } else { "PASS" }));
    GateResult { lines, failed }
}

fn adjudication_checks(
    cases: &[EvalCase],
    layers: &[String],
    agg: &HashMap<String, [f64; 9]>,
    lines: &mut Vec<String>,
    failed: &mut bool,
    human: bool,
) {
    let judge_ok = cases.iter().all(|c| {
        let by_ok = if human {
            human_judge(c)
        } else {
            valid_judge(c.judged_by.as_deref()) && !policy_materialized(c)
        };
        by_ok && non_empty(c.judged_date.as_deref())
    });
    let mut judges: Vec<String> = cases
        .iter()
        .filter_map(|c| c.judged_by.as_deref())
        .map(|s| s.trim().to_lowercase())
        .collect();
    judges.sort();
    judges.dedup();
    lines.push(format!(
        "gate: {} judge metadata all={} judges={} {}",
        if human { "human" } else { "items" },
        cases.len(),
        judges.join(","),
        verdict(judge_ok)
    ));
    *failed |= !judge_ok;
    // 规则物化标签可以证明“按人工规则排序”，但不是逐条人工裁决。human
    // 门必须失败：只告警会让“policy control set PASS”被误读成人工闭环。
    let policy = cases.iter().filter(|c| policy_materialized(c)).count();
    if policy > 0 {
        lines.push(format!(
            "gate: human provenance policy-materialized labels={policy} (not per-item review) FAIL"
        ));
        *failed = true;
    }
    // 逐候选溯源：整包签收（judge.by 写真人）不算逐条裁决。`rel` 的每个键
    // 都必须有 judge_items 条目，且条目裁决人是真人；方法必须是 per-item-*。
    // 这条让“规则生成 rel + 真人署名”无法再冒充人工弱相关/负例闭环。
    let mut required = 0usize;
    let mut covered = 0usize;
    let mut bad_by = 0usize;
    let mut complete_cases = 0usize;
    for c in cases {
        let mut complete = !c.rel.is_empty();
        for k in c.rel.keys() {
            required += 1;
            match c.judge_items.get(k) {
                Some(by)
                    if (if human {
                        human_judge_name(by)
                    } else {
                        valid_judge(Some(by))
                    }) =>
                {
                    covered += 1
                }
                Some(_) => {
                    bad_by += 1;
                    complete = false;
                }
                None => complete = false,
            }
        }
        if complete {
            complete_cases += 1;
        }
    }
    let method_ok = cases.iter().all(|c| {
        c.judged_method
            .as_deref()
            .is_some_and(|m| m.to_lowercase().starts_with("per-item-"))
    });
    let items_ok = required > 0 && covered == required && bad_by == 0 && method_ok;
    lines.push(format!(
        "gate: {} per-item provenance items={covered}/{required} cases={complete_cases}/{} bad={bad_by} method={} {}",
        if human { "human" } else { "items" },
        cases.len(),
        if method_ok { "per-item" } else { "missing/other" },
        verdict(items_ok)
    ));
    *failed |= !items_ok;
    if !human {
        let mut names: Vec<String> = cases.iter().filter_map(|c| c.judged_by.clone()).collect();
        names.sort();
        names.dedup();
        lines.push(format!(
            "gate: adjudicator={} non-human (evidence: per-item AI, not human closure)",
            names.join(",")
        ));
    }
    // literal 层 query 与目标 case 原文逐字相同：目标必须是 2。出现 0/1
    // 说明分数与行错位（或整包机械填写），此时任何 NDCG/弱相关统计都无效。
    let literal: Vec<&EvalCase> = cases
        .iter()
        .filter(|c| c.layer == "literal" && !c.rel.is_empty())
        .collect();
    let literal_bad = literal
        .iter()
        .filter(|c| c.expected_rel() != Some(2.0))
        .count();
    let literal_ok = literal_bad == 0;
    lines.push(format!(
        "gate: {} literal self-match rel=2 bad={literal_bad}/{} {}",
        if human { "human" } else { "items" },
        literal.len(),
        verdict(literal_ok)
    ));
    *failed |= !literal_ok;

    let mut rel1 = 0usize;
    let mut rel0 = 0usize;
    let mut control_layers = 0usize;
    for layer in layers {
        let mut weak = 0usize;
        let mut negative = 0usize;
        for c in cases.iter().filter(|c| &c.layer == layer) {
            for v in c.rel.values() {
                if *v >= 2.0 {
                    continue;
                }
                if *v >= 1.0 {
                    rel1 += 1;
                    weak += 1;
                } else {
                    rel0 += 1;
                    negative += 1;
                }
            }
        }
        if weak > 0 && negative > 0 {
            control_layers += 1;
        }
    }
    let control_ok =
        rel1 >= MIN_WEAK && rel0 >= MIN_NEGATIVE && control_layers >= MIN_CONTROL_LAYERS;
    lines.push(format!(
        "gate: human controls weak={rel1}>={MIN_WEAK} negative={rel0}>={MIN_NEGATIVE} layers={control_layers}>={MIN_CONTROL_LAYERS} {}",
        verdict(control_ok)
    ));
    *failed |= !control_ok;

    // NDCG 使用 rel 分级；人工控制集必须证明弱相关排序不劣于 B。
    // 主指标 A=B 时 NDCG 仍可区分“召回目标”与“把弱相关/负例排对”。
    for layer in layers {
        let Some(e) = agg.get(layer) else {
            continue;
        };
        let n = e[0];
        if n <= 0.0 {
            continue;
        }
        let a_ndcg = e[4] / n;
        let b_ndcg = e[8] / n;
        let ok = a_ndcg + EPS >= b_ndcg;
        lines.push(format!(
            "gate: layer {layer} ndcg@10 A={a_ndcg:.3} vs B={b_ndcg:.3} {}",
            verdict(ok)
        ));
        *failed |= !ok;
    }
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

/// 真实裁决人标识：拒绝空值、尖括号占位符和常见匿名/模板名。
fn valid_judge(s: Option<&str>) -> bool {
    let Some(s) = s else {
        return false;
    };
    let t = s.trim().to_lowercase();
    if t.is_empty() || t.contains('<') || t.contains('>') {
        return false;
    }
    !matches!(
        t.as_str(),
        "anonymous" | "todo" | "placeholder" | "your name" | "你的名字" | "name"
    )
}

/// 人工裁决人标识：机械 target 规则、AI 草案和自动标注不得冒充人工闭环。
fn human_judge(c: &EvalCase) -> bool {
    let Some(s) = c.judged_by.as_deref() else {
        return false;
    };
    human_judge_name(s) && !policy_materialized(c)
}

/// 逐候选裁决人标识：同一套真人判定，不含整包 policy 方法判断。
fn human_judge_name(s: &str) -> bool {
    if !valid_judge(Some(s)) {
        return false;
    }
    let t = s.trim().to_lowercase();
    ![
        "mechanical",
        "machine",
        "auto",
        "draft",
        "rule",
        "policy",
        "materialized",
        "rubric",
        "target-only",
        "codex",
        "ai:",
    ]
    .iter()
    .any(|w| t.contains(w))
}

/// 规则/评分卡物化标签：即使由真人签署，也不是逐条候选裁决。
fn policy_materialized(c: &EvalCase) -> bool {
    c.judged_method.as_deref().is_some_and(|m| {
        let m = m.to_lowercase();
        ["policy", "materialized", "rubric", "mechanical", "auto"]
            .iter()
            .any(|w| m.contains(w))
    })
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
            judged_method: None,
            judge_items: HashMap::new(),
        }
    }

    /// 把当前 rel 的每个键都标成逐条人工裁决（测试用）。
    fn stamp_items(c: &mut EvalCase, by: &str) {
        for k in c.rel.keys() {
            c.judge_items.insert(k.clone(), by.to_string());
        }
        c.judged_method = Some("per-item-blind-v1".to_string());
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
    fn gate_rejects_degenerate_absolute_recall() {
        let cases = corpus();
        let layers = vec![
            "literal".to_string(),
            "paraphrase".to_string(),
            "semantic".to_string(),
        ];
        let mut all_miss = HashMap::new();
        for layer in &layers {
            let mut e = [0.0; 9];
            e[0] = MIN_LAYER as f64;
            all_miss.insert(layer.clone(), e);
        }
        let r = check(&cases, &layers, &all_miss);
        assert!(r.failed);
        assert!(
            r.lines
                .join("\n")
                .contains("absolute A/B r@5=0.000/0.000>=0.20 FAIL"),
            "{}",
            r.lines.join("\n")
        );

        // A 满分但 B 全 miss：相对非劣成立，也不能作为有效比较证据。
        let mut broken_b = perfect_agg();
        broken_b.get_mut("paraphrase").unwrap()[5] = 0.0;
        let r = check(&cases, &layers, &broken_b);
        assert!(r.failed);
        assert!(
            r.lines
                .join("\n")
                .contains("absolute A/B r@5=1.000/0.000>=0.20 FAIL"),
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

    #[test]
    fn gate_rejects_placeholder_judge_name() {
        let mut cases = corpus();
        cases[10].judged_by = Some("<你的名字>".to_string());
        let layers = vec![
            "literal".to_string(),
            "paraphrase".to_string(),
            "semantic".to_string(),
        ];
        let r = check(&cases, &layers, &perfect_agg());
        assert!(r.failed);
        let joined = r.lines.join("\n");
        assert!(joined.contains("placeholder judge rejected"), "{joined}");
        assert!(joined.contains("<你的名字>"), "{joined}");
    }

    #[test]
    fn gate_warns_when_all_labels_are_two_and_a_equals_b() {
        let mut cases = corpus();
        for c in &mut cases {
            c.rel.insert("a".to_string(), 2.0);
        }
        let layers = vec![
            "literal".to_string(),
            "paraphrase".to_string(),
            "semantic".to_string(),
        ];
        let r = check(&cases, &layers, &perfect_agg());
        assert!(!r.failed, "{}", r.lines.join("\n"));
        let joined = r.lines.join("\n");
        assert!(joined.contains("warning all labels are 2"), "{joined}");
        assert!(joined.contains("warning A=B on all layers"), "{joined}");
        assert!(
            joined.contains("not human weak/negative closure"),
            "{joined}"
        );
    }

    #[test]
    fn human_gate_rejects_mechanical_target_labels() {
        let mut cases = corpus();
        for c in &mut cases {
            c.judged_by = Some("mechanical-target-confirm5-v1".to_string());
            c.rel.insert("a".to_string(), 2.0);
        }
        let layers = vec![
            "literal".to_string(),
            "paraphrase".to_string(),
            "semantic".to_string(),
        ];
        let r = check_human(&cases, &layers, &perfect_agg());
        assert!(r.failed);
        let joined = r.lines.join("\n");
        assert!(joined.contains("human judge metadata"), "{joined}");
        assert!(joined.contains("human controls weak=0"), "{joined}");
    }

    #[test]
    fn human_gate_rejects_policy_materialized_labels() {
        let mut cases = corpus();
        for c in &mut cases {
            c.judged_by = Some("fei+hao-policy-v1".to_string());
            c.judged_method = Some("policy-materialized-v1".to_string());
            c.rel.insert("a".to_string(), 2.0);
            c.rel.insert("weak".to_string(), 1.0);
            c.rel.insert("negative".to_string(), 0.0);
        }
        let layers = vec![
            "literal".to_string(),
            "paraphrase".to_string(),
            "semantic".to_string(),
        ];
        let r = check_human(&cases, &layers, &perfect_agg());
        assert!(r.failed);
        let joined = r.lines.join("\n");
        assert!(
            joined.contains("human provenance policy-materialized labels=30"),
            "{joined}"
        );
        assert!(joined.contains("gate: FAIL"), "{joined}");
    }

    #[test]
    fn human_gate_requires_weak_and_negative_controls() {
        let mut cases = corpus();
        for c in &mut cases {
            c.judged_by = Some("fei+hao".to_string());
            c.rel.insert("a".to_string(), 2.0);
            c.rel.insert("weak".to_string(), 1.0);
        }
        let layers = vec![
            "literal".to_string(),
            "paraphrase".to_string(),
            "semantic".to_string(),
        ];
        let r = check_human(&cases, &layers, &perfect_agg());
        assert!(r.failed);
        assert!(
            r.lines
                .join("\n")
                .contains("human controls weak=30>=10 negative=0>=10"),
            "{}",
            r.lines.join("\n")
        );
    }

    #[test]
    fn human_gate_passes_controls_with_human_judges() {
        let mut cases = corpus();
        for c in &mut cases {
            c.judged_by = Some("fei+hao".to_string());
            c.judged_date = Some("2026-10-22T00:00:00Z".to_string());
            c.rel.insert("a".to_string(), 2.0);
            c.rel.insert("weak".to_string(), 1.0);
            c.rel.insert("negative".to_string(), 0.0);
            stamp_items(c, "fei+hao");
        }
        let layers = vec![
            "literal".to_string(),
            "paraphrase".to_string(),
            "semantic".to_string(),
        ];
        let r = check_human(&cases, &layers, &perfect_agg());
        assert!(!r.failed, "{}", r.lines.join("\n"));
        let joined = r.lines.join("\n");
        assert!(joined.contains("gate: mode=human"), "{joined}");
        assert!(
            joined.contains("human per-item provenance items=90/90"),
            "{joined}"
        );
        assert!(joined.contains("ndcg@10 A=1.000 vs B=1.000"), "{joined}");
        assert!(joined.contains("gate: PASS"), "{joined}");
    }

    #[test]
    fn human_gate_rejects_package_level_signoff() {
        let mut cases = corpus();
        for c in &mut cases {
            c.judged_by = Some("fei".to_string());
            c.judged_date = Some("2026-10-24T00:00:00Z".to_string());
            c.judged_method = Some("per-item-blind-v1".to_string());
            c.rel.insert("a".to_string(), 2.0);
            c.rel.insert("weak".to_string(), 1.0);
            c.rel.insert("negative".to_string(), 0.0);
        }
        let layers = vec![
            "literal".to_string(),
            "paraphrase".to_string(),
            "semantic".to_string(),
        ];
        let r = check_human(&cases, &layers, &perfect_agg());
        assert!(r.failed);
        let joined = r.lines.join("\n");
        assert!(
            joined.contains("human per-item provenance items=0/90"),
            "{joined}"
        );
    }

    #[test]
    fn human_gate_rejects_literal_self_match_below_two() {
        let mut cases = corpus();
        for c in &mut cases {
            c.judged_by = Some("fei".to_string());
            c.judged_date = Some("2026-10-24T00:00:00Z".to_string());
            c.rel.insert("a".to_string(), 2.0);
            c.rel.insert("weak".to_string(), 1.0);
            c.rel.insert("negative".to_string(), 0.0);
            stamp_items(c, "fei");
        }
        // literal 的 query 就是目标原文，判 1 说明行与分数错位。
        cases[0].rel.insert("a".to_string(), 1.0);
        let layers = vec![
            "literal".to_string(),
            "paraphrase".to_string(),
            "semantic".to_string(),
        ];
        let r = check_human(&cases, &layers, &perfect_agg());
        assert!(r.failed);
        let joined = r.lines.join("\n");
        assert!(
            joined.contains("human literal self-match rel=2 bad=1/10 FAIL"),
            "{joined}"
        );
    }

    #[test]
    fn items_gate_accepts_ai_but_human_gate_rejects_it() {
        let mut cases = corpus();
        for c in &mut cases {
            c.judged_by = Some("codex".to_string());
            c.judged_date = Some("2026-10-24T00:00:00Z".to_string());
            c.judged_method = Some("per-item-ai-v1".to_string());
            c.rel.insert("a".to_string(), 2.0);
            c.rel.insert("weak".to_string(), 1.0);
            c.rel.insert("negative".to_string(), 0.0);
            for k in c.rel.keys() {
                c.judge_items.insert(k.clone(), "codex".to_string());
            }
        }
        let layers = vec![
            "literal".to_string(),
            "paraphrase".to_string(),
            "semantic".to_string(),
        ];
        let items = check_items(&cases, &layers, &perfect_agg());
        assert!(!items.failed, "{}", items.lines.join("\n"));
        let joined = items.lines.join("\n");
        assert!(joined.contains("gate: mode=items"), "{joined}");
        assert!(
            joined.contains("non-human (evidence: per-item AI"),
            "{joined}"
        );
        let human = check_human(&cases, &layers, &perfect_agg());
        assert!(human.failed);
        assert!(
            human
                .lines
                .join("\n")
                .contains("human judge metadata all=30 judges=codex FAIL"),
            "{}",
            human.lines.join("\n")
        );
    }

    #[test]
    fn human_gate_rejects_ndcg_below_baseline() {
        let mut cases = corpus();
        for c in &mut cases {
            c.judged_by = Some("fei+hao".to_string());
            c.judged_date = Some("2026-10-22T00:00:00Z".to_string());
            c.rel.insert("a".to_string(), 2.0);
            c.rel.insert("weak".to_string(), 1.0);
            c.rel.insert("negative".to_string(), 0.0);
            stamp_items(c, "fei+hao");
        }
        let layers = vec![
            "literal".to_string(),
            "paraphrase".to_string(),
            "semantic".to_string(),
        ];
        let mut agg = perfect_agg();
        agg.get_mut("paraphrase").unwrap()[4] = 9.0; // A NDCG 0.9 < B 1.0
        let r = check_human(&cases, &layers, &agg);
        assert!(r.failed);
        assert!(
            r.lines
                .join("\n")
                .contains("layer paraphrase ndcg@10 A=0.900 vs B=1.000 FAIL"),
            "{}",
            r.lines.join("\n")
        );
    }
}
