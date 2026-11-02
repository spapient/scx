//! 评估核心：baseline/verbose 解析、A/B 指标、配对 bootstrap/permutation、
//! 分层与 leave-one-incident-out。语义与 `tools/scx.py` 的 baseline/stats
//! 逐项对齐（含 CPython RNG 与输出格式），迁移完成后由 Rust 成为唯一真源。
//!
//! 解析与格式对齐函数分支多但都是线性映射；认知复杂度门禁针对压缩内核，
//! 不适用于迁移工具，故本模块局部关闭。
#![allow(clippy::cognitive_complexity)]

use super::io;
use super::rng::PyRng;
use crate::json::Json;
use std::collections::{HashMap, HashSet};
use std::fs;

pub(crate) const LAYERS: [&str; 3] = ["literal", "paraphrase", "semantic"];
const STOP: [&str; 29] = [
    "the", "a", "an", "of", "to", "and", "in", "on", "for", "with", "by", "from", "after",
    "before", "was", "were", "is", "are", "be", "that", "this", "it", "as", "at", "into", "during",
    "caused", "cause", "causing",
];

#[derive(Debug, Clone)]
pub(crate) struct Case {
    pub query: String,
    pub layer: String,
    pub expected: Vec<String>,
    pub rel: Vec<(String, f64)>,
    pub case_index: Option<i64>,
}

pub(crate) fn load_baseline(path: &str) -> Result<Vec<Case>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let value = crate::json::parse(&text).map_err(|e| format!("{path}: {e}"))?;
    let items = match value.get("cases") {
        Some(Json::Arr(items)) => items,
        _ => return Err("baseline: missing array \"cases\"".to_string()),
    };
    if items.is_empty() {
        return Err("stats: baseline has no cases".to_string());
    }
    let mut cases = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        cases.push(parse_case(item, index)?);
    }
    Ok(cases)
}

fn parse_case(value: &Json, index: usize) -> Result<Case, String> {
    let layer = value
        .get("layer")
        .and_then(Json::str)
        .ok_or_else(|| format!("stats: case {index} missing layer"))?
        .to_string();
    let expected = match value.get("expected") {
        Some(Json::Arr(items)) => items
            .iter()
            .filter_map(Json::str)
            .map(str::to_string)
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    if expected.is_empty() {
        return Err(format!("stats: case {index} has no expected target"));
    }
    let query = value
        .get("query")
        .and_then(Json::str)
        .unwrap_or_default()
        .to_string();
    let rel = match value.get("rel") {
        Some(Json::Obj(entries)) => parse_rel(entries),
        _ => Vec::new(),
    };
    let case_index = value.get("case_index").and_then(as_i64);
    Ok(Case {
        query,
        layer,
        expected,
        rel,
        case_index,
    })
}

fn parse_rel(entries: &[(String, Json)]) -> Vec<(String, f64)> {
    entries
        .iter()
        .filter_map(|(key, value)| {
            let rel = match value {
                Json::Num(n) => Some(*n),
                Json::Str(s) => s.parse().ok(),
                _ => None,
            };
            rel.map(|n| (key.clone(), n))
        })
        .collect()
}

fn as_i64(value: &Json) -> Option<i64> {
    match value {
        Json::Num(n) if n.fract() == 0.0 => Some(*n as i64),
        Json::Str(s) => s.parse().ok(),
        _ => None,
    }
}

#[derive(Debug, Default)]
pub(crate) struct Hits {
    pub a: Option<Vec<String>>,
    pub b: Option<Vec<String>>,
}

pub(crate) fn parse_verbose(path: &str, expected: usize) -> Result<Vec<Hits>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let mut rows: Vec<Hits> = Vec::new();
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if is_case_line(line) {
            rows.push(Hits::default());
            continue;
        }
        if let Some((side, rest)) = side_line(line) {
            let Some(current) = rows.last_mut() else {
                return Err(malformed(path, line));
            };
            let slot = if side == 'A' {
                &mut current.a
            } else {
                &mut current.b
            };
            if slot.is_some() {
                return Err(format!("stats: duplicate {side} line in {path}"));
            }
            *slot = Some(scan_ids(rest));
            continue;
        }
        if line.starts_with("case ") || line.starts_with("  A: ") || line.starts_with("  B: ") {
            return Err(malformed(path, line));
        }
    }
    if rows.len() != expected {
        return Err(format!(
            "stats: verbose cases={} baseline cases={expected}",
            rows.len()
        ));
    }
    for (index, row) in rows.iter_mut().enumerate() {
        for (side, slot) in [('A', &mut row.a), ('B', &mut row.b)] {
            if slot.is_none() {
                return Err(format!(
                    "stats: case {} missing {side} line in {path}",
                    index + 1
                ));
            }
        }
    }
    Ok(rows)
}

fn malformed(path: &str, line: &str) -> String {
    format!("stats: malformed verbose line in {path}: {line:?}")
}

fn is_case_line(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("case ") else {
        return false;
    };
    let Some((number, rest)) = rest.split_once('/') else {
        return false;
    };
    let Some((total, rest)) = rest.split_once(' ') else {
        return false;
    };
    number.chars().all(|c| c.is_ascii_digit())
        && !number.is_empty()
        && total.chars().all(|c| c.is_ascii_digit())
        && !total.is_empty()
        && rest.starts_with("q=\"")
        && rest.ends_with('"')
}

fn side_line(line: &str) -> Option<(char, &str)> {
    line.strip_prefix("  A: ")
        .map(|rest| ('A', rest))
        .or_else(|| line.strip_prefix("  B: ").map(|rest| ('B', rest)))
}

fn scan_ids(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(pos) = rest.find(") ") {
        let after = &rest[pos + 2..];
        if let Some(end) = after.find(" sim=") {
            let id = &after[..end];
            if !id.is_empty() && !id.chars().any(char::is_whitespace) {
                out.push(id.to_string());
            }
        }
        rest = after;
    }
    out
}

/// 分层：优先读 `--strata`，否则从 memory cases 按 query Jaccard 计算。
pub(crate) fn strata(
    cases: &[Case],
    mem_dir: Option<&str>,
    strata_csv: Option<&str>,
) -> Result<Vec<String>, String> {
    if let Some(path) = strata_csv {
        return strata_from_csv(path, cases.len());
    }
    let Some(root) = mem_dir else {
        return Err("stats: --mem-dir or --strata is required".to_string());
    };
    strata_from_memory(root, cases)
}

fn strata_from_csv(path: &str, total: usize) -> Result<Vec<String>, String> {
    let (_, rows) = io::read_csv(path)?;
    let mut tiers: Vec<Option<String>> = vec![None; total];
    for row in rows {
        let raw = row.get("case_index").cloned().unwrap_or_default();
        let index: i64 = raw
            .trim()
            .parse()
            .map_err(|_| format!("stats: bad strata case_index: {raw:?}"))?;
        if index < 0 || index as usize >= total {
            return Err(format!("stats: strata case_index out of range: {index}"));
        }
        let slot = &mut tiers[index as usize];
        if slot.is_some() {
            return Err(format!("stats: duplicate strata case_index: {index}"));
        }
        let tier = row.get("tier").map(|v| v.trim()).unwrap_or("");
        if !matches!(tier, "easy" | "medium" | "hard") {
            return Err(format!("stats: bad tier for case {index}: {tier:?}"));
        }
        *slot = Some(tier.to_string());
    }
    let missing: Vec<usize> = tiers
        .iter()
        .enumerate()
        .filter(|(_, t)| t.is_none())
        .map(|(i, _)| i)
        .collect();
    if !missing.is_empty() {
        let head = &missing[..missing.len().min(8)];
        return Err(format!("stats: strata missing case indexes: {head:?}"));
    }
    Ok(tiers.into_iter().map(|t| t.unwrap_or_default()).collect())
}

fn strata_from_memory(root: &str, cases: &[Case]) -> Result<Vec<String>, String> {
    let dir = format!("{root}/cases");
    if !std::path::Path::new(&dir).is_dir() {
        return Err(format!("stats: missing memory cases dir: {dir}"));
    }
    let mut entries: Vec<_> = fs::read_dir(&dir)
        .map_err(|e| format!("{dir}: {e}"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    let mut by_full: HashMap<String, (String, String)> = HashMap::new();
    let mut by_tail: HashMap<String, (String, String)> = HashMap::new();
    for entry in entries {
        let path = entry.join("case.json");
        if !path.is_file() {
            continue;
        }
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let value = crate::json::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let fallback = entry
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let id = value
            .get("id")
            .and_then(Json::str)
            .filter(|s| !s.is_empty())
            .unwrap_or(fallback.as_str())
            .to_string();
        let query = value
            .get("query")
            .and_then(Json::str)
            .unwrap_or_default()
            .to_string();
        let record = (id.clone(), query);
        by_tail.insert(tail_of(&id).to_string(), record.clone());
        by_full.insert(id, record);
    }
    let mut tiers = Vec::with_capacity(cases.len());
    for case in cases {
        let target = &case.expected[0];
        let hit = by_full
            .get(target)
            .or_else(|| by_tail.get(tail_of(target)))
            .ok_or_else(|| format!("stats: target {target} not found in memory dir {root}"))?;
        tiers.push(tier_of(jaccard(&case.query, &hit.1)).to_string());
    }
    Ok(tiers)
}

fn tail_of(key: &str) -> &str {
    key.rsplit('_').next().unwrap_or(key)
}

fn last8(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    chars[chars.len().saturating_sub(8)..].iter().collect()
}

pub(crate) fn jaccard(left: &str, right: &str) -> f64 {
    let a = tokens(left);
    let b = tokens(right);
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let common = a.intersection(&b).count();
    let union = a.len() + b.len() - common;
    if union == 0 {
        0.0
    } else {
        common as f64 / union as f64
    }
}

fn tokens(text: &str) -> HashSet<String> {
    let lower = text.to_lowercase();
    let chars: Vec<char> = lower.chars().collect();
    let mut out = HashSet::new();
    let mut i = 0;
    while i < chars.len() {
        if word_start(chars[i]) {
            let start = i;
            while i < chars.len() && word_part(chars[i]) {
                i += 1;
            }
            let token: String = chars[start..i].iter().collect();
            let token = token.trim_end_matches(['.', '-', '_']);
            if token.chars().count() >= 2 && !STOP.contains(&token) {
                out.insert(token.to_string());
            }
        } else {
            i += 1;
        }
    }
    let mut i = 0;
    while i < chars.len() {
        if cjk(chars[i]) {
            let start = i;
            while i < chars.len() && cjk(chars[i]) {
                i += 1;
            }
            for window in 1..=2 {
                for j in start..i {
                    if j + window <= i {
                        out.insert(chars[j..j + window].iter().collect());
                    }
                }
            }
        } else {
            i += 1;
        }
    }
    out
}

fn word_start(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit()
}

fn word_part(c: char) -> bool {
    word_start(c) || c == '_' || c == '-' || c == '.'
}

fn cjk(c: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&c)
}

pub(crate) fn tier_of(value: f64) -> &'static str {
    if value > 0.5 {
        "easy"
    } else if value >= 0.2 {
        "medium"
    } else {
        "hard"
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Row {
    pub i: i64,
    pub layer: String,
    pub tier: String,
    pub a: f64,
    pub b: f64,
}

#[derive(Debug, Clone)]
pub(crate) struct ResultRow {
    pub group: String,
    pub n: usize,
    pub a: f64,
    pub b: f64,
    pub delta: f64,
    pub ci_lo: f64,
    pub ci_hi: f64,
    pub perm_p: f64,
    pub mde: f64,
    pub advantage: bool,
    pub n_clusters: Option<usize>,
}

/// 统计参数：bootstrap/permutation 次数、随机种子与最小可检测效应。
#[derive(Clone, Copy)]
pub(crate) struct Plan {
    pub boot: usize,
    pub perm: usize,
    pub seed: i64,
    pub mde: f64,
}

pub(crate) fn analyze(name: &str, rows: &[&Row], plan: Plan) -> Option<ResultRow> {
    let Plan {
        boot,
        perm,
        seed,
        mde,
    } = plan;
    if rows.is_empty() {
        return None;
    }
    let deltas: Vec<f64> = rows.iter().map(|row| row.a - row.b).collect();
    let n = deltas.len();
    let observed = deltas.iter().sum::<f64>() / n as f64;
    let mut rng = PyRng::new(seed);
    let mut means = Vec::with_capacity(boot);
    for _ in 0..boot {
        let mut sum = 0.0;
        for _ in 0..n {
            sum += deltas[rng.randrange(n)];
        }
        means.push(sum / n as f64);
    }
    means.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let lo = means[(0.025 * (boot as f64 - 1.0)) as usize];
    let hi = means[(0.975 * (boot as f64 - 1.0)) as usize];
    let mut hits = 0usize;
    for _ in 0..perm {
        let mut sum = 0.0;
        for &delta in &deltas {
            sum += if rng.random() >= 0.5 { delta } else { -delta };
        }
        if (sum / n as f64).abs() >= observed.abs() {
            hits += 1;
        }
    }
    let p_value = (hits as f64 + 1.0) / (perm as f64 + 1.0);
    Some(result_row(
        name,
        rows,
        observed,
        (lo, hi),
        p_value,
        mde,
        None,
    ))
}

pub(crate) fn analyze_cluster(rows: &[&Row], cluster_size: usize, plan: Plan) -> Option<ResultRow> {
    let Plan {
        boot,
        perm,
        seed,
        mde,
    } = plan;
    if rows.is_empty() || cluster_size == 0 {
        return None;
    }
    let mut clusters: Vec<Vec<&Row>> = Vec::new();
    let mut index: HashMap<i64, usize> = HashMap::new();
    for row in rows {
        let key = row.i.div_euclid(cluster_size as i64);
        let slot = *index.entry(key).or_insert_with(|| {
            clusters.push(Vec::new());
            clusters.len() - 1
        });
        clusters[slot].push(row);
    }
    if clusters.len() < 2 {
        return None;
    }
    let deltas: Vec<f64> = rows.iter().map(|row| row.a - row.b).collect();
    let observed = deltas.iter().sum::<f64>() / deltas.len() as f64;
    let mut rng = PyRng::new(seed);
    let mut means = Vec::with_capacity(boot);
    for _ in 0..boot {
        let mut values = Vec::new();
        for _ in 0..clusters.len() {
            let cluster = &clusters[rng.randrange(clusters.len())];
            values.extend(cluster.iter().map(|row| row.a - row.b));
        }
        means.push(values.iter().sum::<f64>() / values.len() as f64);
    }
    means.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let lo = means[(0.025 * (boot as f64 - 1.0)) as usize];
    let hi = means[(0.975 * (boot as f64 - 1.0)) as usize];
    let mut hits = 0usize;
    for _ in 0..perm {
        let mut total = 0.0;
        let mut count = 0usize;
        for cluster in &clusters {
            let sign = if rng.random() >= 0.5 { 1.0 } else { -1.0 };
            for row in cluster {
                total += sign * (row.a - row.b);
                count += 1;
            }
        }
        if (total / count as f64).abs() >= observed.abs() {
            hits += 1;
        }
    }
    let p_value = (hits as f64 + 1.0) / (perm as f64 + 1.0);
    Some(result_row(
        "tier:hard:cluster",
        rows,
        observed,
        (lo, hi),
        p_value,
        mde,
        Some(clusters.len()),
    ))
}

fn result_row(
    name: &str,
    rows: &[&Row],
    observed: f64,
    ci: (f64, f64),
    p_value: f64,
    mde: f64,
    n_clusters: Option<usize>,
) -> ResultRow {
    let (lo, hi) = ci;
    let n = rows.len() as f64;
    let a = rows.iter().map(|row| row.a).sum::<f64>() / n;
    let b = rows.iter().map(|row| row.b).sum::<f64>() / n;
    ResultRow {
        group: name.to_string(),
        n: rows.len(),
        a: io::round4(a),
        b: io::round4(b),
        delta: io::round4(observed),
        ci_lo: io::round4(lo),
        ci_hi: io::round4(hi),
        perm_p: io::round4(p_value),
        mde,
        advantage: lo > 0.0 && observed >= mde && p_value < 0.05,
        n_clusters,
    }
}

pub(crate) struct Loo {
    pub rows: Vec<ResultRow>,
    pub summary: LooSummary,
}

pub(crate) struct LooSummary {
    pub n_runs: usize,
    pub min_delta: f64,
    pub mean_delta: f64,
    pub max_delta: f64,
    pub min_ci_lo: f64,
}

pub(crate) fn loo(
    rows: &[Row],
    group_size: usize,
    plan: Plan,
    gate_delta: f64,
    gate: bool,
) -> Result<Loo, String> {
    if group_size == 0 {
        return Err("stats: --loo-group must be positive".to_string());
    }
    let hard: Vec<&Row> = rows.iter().filter(|row| row.tier == "hard").collect();
    if hard.len() < 2 * group_size {
        return Err("stats: --loo needs hard cases spanning >= 2 incidents".to_string());
    }
    let ngroups = hard
        .iter()
        .map(|row| row.i.div_euclid(group_size as i64))
        .max()
        .unwrap_or(0)
        + 1;
    let mut out = Vec::new();
    for group in 0..ngroups {
        let rest: Vec<&Row> = hard
            .iter()
            .copied()
            .filter(|row| row.i.div_euclid(group_size as i64) != group)
            .collect();
        if rest.len() < 2 {
            continue;
        }
        if let Some(result) = analyze(&format!("loo:incident-{group}"), &rest, plan) {
            out.push(result);
        }
    }
    if out.is_empty() {
        return Err("stats: --loo produced no runs".to_string());
    }
    let deltas: Vec<f64> = out.iter().map(|row| row.delta).collect();
    let summary = LooSummary {
        n_runs: deltas.len(),
        min_delta: deltas.iter().copied().fold(f64::INFINITY, f64::min),
        mean_delta: deltas.iter().sum::<f64>() / deltas.len() as f64,
        max_delta: deltas.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        min_ci_lo: out
            .iter()
            .map(|row| row.ci_lo)
            .fold(f64::INFINITY, f64::min),
    };
    println!(
        "stats: loo incidents={} min_delta={:+.4} mean={:+.4} max={:+.4} min_ci_lo={:+.4}",
        summary.n_runs, summary.min_delta, summary.mean_delta, summary.max_delta, summary.min_ci_lo
    );
    if gate {
        let mut bad: Vec<&str> = out
            .iter()
            .filter(|row| row.delta < gate_delta || row.ci_lo <= 0.0)
            .map(|row| row.group.as_str())
            .collect();
        bad.sort_unstable();
        if !bad.is_empty() {
            let shown = bad[..bad.len().min(10)].join(", ");
            let more = if bad.len() > 10 { " ..." } else { "" };
            return Err(format!(
                "stats: LOO GATE FAILED: advantage lost after dropping {} incident(s): {shown}{more}",
                bad.len()
            ));
        }
        println!(
            "stats: loo gate PASS (every dropped incident keeps delta >= {gate_delta:.2} and CI lo>0)"
        );
    }
    Ok(Loo { rows: out, summary })
}

pub(crate) fn target_rank(ids: &[String], target: &str) -> usize {
    let tail = tail_of(target);
    let tail8 = last8(tail);
    for (index, case_id) in ids.iter().take(10).enumerate() {
        if case_id == target || case_id == tail || last8(case_id) == tail8 {
            return index + 1;
        }
    }
    0
}

pub(crate) fn mrr(rank: usize) -> f64 {
    if rank == 0 {
        0.0
    } else {
        1.0 / rank as f64
    }
}

pub(crate) fn recall_at(rank: usize, k: usize) -> f64 {
    if rank != 0 && rank <= k {
        1.0
    } else {
        0.0
    }
}

pub(crate) fn dcg(rels: &[f64]) -> f64 {
    rels.iter()
        .enumerate()
        .map(|(i, rel)| rel / (i as f64 + 2.0).log2())
        .sum()
}

pub(crate) fn ndcg(case: &Case, ids: &[String]) -> f64 {
    let observed: Vec<f64> = ids.iter().take(10).map(|id| rel_of(case, id)).collect();
    let mut ideal: Vec<f64> = case
        .rel
        .iter()
        .map(|(_, rel)| *rel)
        .filter(|rel| *rel > 0.0)
        .collect();
    ideal.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    ideal.truncate(10);
    let ideal_dcg = dcg(&ideal);
    if ideal_dcg > 0.0 {
        dcg(&observed) / ideal_dcg
    } else {
        0.0
    }
}

fn rel_of(case: &Case, case_id: &str) -> f64 {
    if let Some((_, rel)) = case.rel.iter().find(|(key, _)| key == case_id) {
        return *rel;
    }
    let tail = tail_of(case_id);
    if let Some((_, rel)) = case.rel.iter().find(|(key, _)| key == tail) {
        return *rel;
    }
    let tail8 = last8(tail);
    if let Some((_, rel)) = case.rel.iter().find(|(key, _)| key == &tail8) {
        return *rel;
    }
    let mut keys: Vec<&String> = case.rel.iter().map(|(key, _)| key).collect();
    keys.sort();
    keys.into_iter()
        .find(|key| id_matches(key, case_id))
        .map(|key| {
            case.rel
                .iter()
                .find(|(candidate, _)| candidate == key)
                .map(|(_, rel)| *rel)
                .unwrap_or(0.0)
        })
        .unwrap_or(0.0)
}

fn id_matches(expected: &str, case_id: &str) -> bool {
    let tail = tail_of(case_id);
    let tail8 = last8(tail);
    expected == case_id || expected == tail || expected == tail8 || tail_of(expected) == tail
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(i: i64, tier: &str, a: f64, b: f64) -> Row {
        Row {
            i,
            layer: "literal".to_string(),
            tier: tier.to_string(),
            a,
            b,
        }
    }

    #[test]
    fn metrics_match_reference_values() {
        let case = Case {
            query: "q".to_string(),
            layer: "literal".to_string(),
            expected: vec!["abc_9f2a".to_string()],
            rel: vec![("abc_9f2a".to_string(), 2.0)],
            case_index: None,
        };
        let ids = vec!["x".to_string(), "abc_9f2a".to_string()];
        assert_eq!(target_rank(&ids, "abc_9f2a"), 2);
        assert_eq!(mrr(2), 0.5);
        assert_eq!(recall_at(2, 1), 0.0);
        assert_eq!(recall_at(2, 5), 1.0);
        assert!((ndcg(&case, &ids) - 1.0 / 3.0f64.log2()).abs() < 1e-12);
    }

    #[test]
    fn analyze_matches_python_vector() {
        let deltas = [0.05, 0.1, -0.02, 0.0, 0.07, 0.03, 0.11, -0.01, 0.04, 0.06];
        let rows: Vec<Row> = deltas
            .iter()
            .enumerate()
            .map(|(i, d)| row(i as i64, "hard", *d, 0.0))
            .collect();
        let refs: Vec<&Row> = rows.iter().collect();
        let result = analyze(
            "all",
            &refs,
            Plan {
                boot: 1000,
                perm: 1000,
                seed: 7,
                mde: 0.04,
            },
        )
        .unwrap();
        assert_eq!(result.delta, 0.043);
        assert_eq!(result.ci_lo, 0.015);
        assert_eq!(result.ci_hi, 0.068);
        // Python `analyze` 在 bootstrap 后继续用同一 rng 做 permutation：
        // 命中 14/1000 → p=0.014985… → round4=0.015。
        assert_eq!(result.perm_p, 0.015);
        assert!(result.advantage);
    }

    #[test]
    fn strata_csv_requires_full_coverage() {
        let dir = crate::testsupport::tmpdir("strata");
        let path = format!("{dir}.csv");
        io::write_csv(
            &path,
            &["case_index".to_string(), "tier".to_string()],
            &[
                vec!["0".to_string(), "easy".to_string()],
                vec!["1".to_string(), "hard".to_string()],
            ],
        )
        .unwrap();
        let got = strata_from_csv(&path, 2).unwrap();
        assert_eq!(got, vec!["easy", "hard"]);
        assert!(strata_from_csv(&path, 3).is_err());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn verbose_parses_ranked_ids() {
        let dir = crate::testsupport::tmpdir("verbose");
        let path = format!("{dir}.txt");
        fs::write(
            &path,
            "case 1/2 q=\"q1\"\n  A: 1) aaa sim=0.9 score=2.0 [hit] | 2) bbb sim=0.1 score=1.0 [miss]\n  B: 1) bbb sim=0.8 score=2.0 [miss]\ncase 2/2 q=\"q2\"\n  A: \n  B: 1) ccc sim=0.5 score=1.0 [miss]\n",
        )
        .unwrap();
        let rows = parse_verbose(&path, 2).unwrap();
        assert_eq!(rows[0].a.as_ref().unwrap(), &["aaa", "bbb"]);
        assert_eq!(rows[0].b.as_ref().unwrap(), &["bbb"]);
        assert!(rows[1].a.as_ref().unwrap().is_empty());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn jaccard_tiers_match_reference() {
        assert_eq!(tier_of(0.6), "easy");
        assert_eq!(tier_of(0.2), "medium");
        assert_eq!(tier_of(0.1), "hard");
        assert!((jaccard("alpha beta", "alpha gamma") - 1.0 / 3.0).abs() < 1e-12);
    }
}
