//! `xtask manifest` / `xtask report`：冻结输入哈希与冻结指标文档（feature = "eval"）。
//!
//! - `manifest`：对评测输入计算 SHA-256；`--expect` 校验冻结 manifest，漂移退出 2；
//!   `--files` + `--out` 重建 manifest；`date:` 前缀会把 `YYYY-MM-DD_` 规范化为 `<date>_`。
//! - `report`：从 stats CSV / LOO JSON / fidelity JSON 组合 `docs/gauge-metrics.md`；
//!   `--check` 逐字节校验，漂移退出 2。
//!
//! 语义与 `tools/scx.py manifest` / `report` 对齐。

// 报告格式化分支线性；认知复杂度门禁针对压缩内核，不适用于迁移工具。
#![allow(clippy::cognitive_complexity)]

use super::gen::to_j;
use super::io::{self, J};
use super::{fail, Fail};
use crate::json::Json;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

// ---------------------------------------------------------------------------
// manifest
// ---------------------------------------------------------------------------

struct ManifestOpts {
    files: Vec<String>,
    out: Option<String>,
    expect: Option<String>,
}

pub(crate) fn manifest(args: &[String]) -> Result<(), Fail> {
    let opts = parse_manifest(args)?;
    if let Some(expect) = &opts.expect {
        return verify(expect);
    }
    build(&opts)
}

fn parse_manifest(args: &[String]) -> Result<ManifestOpts, Fail> {
    let mut opts = ManifestOpts {
        files: Vec::new(),
        out: None,
        expect: None,
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--files" => {
                let start = opts.files.len();
                i += 1;
                while i < args.len() && !args[i].starts_with("--") {
                    opts.files.push(args[i].clone());
                    i += 1;
                }
                if opts.files.len() == start {
                    return Err(usage("--files expects one or more paths"));
                }
                continue;
            }
            "--out" => {
                i += 1;
                opts.out = Some(
                    args.get(i)
                        .cloned()
                        .ok_or_else(|| usage("--out expects a path"))?,
                );
            }
            "--expect" => {
                i += 1;
                opts.expect = Some(
                    args.get(i)
                        .cloned()
                        .ok_or_else(|| usage("--expect expects a path"))?,
                );
            }
            "-h" | "--help" => return Err(usage(String::new())),
            other => return Err(usage(format!("unrecognized argument: {other}"))),
        }
        i += 1;
    }
    Ok(opts)
}

fn verify(path: &str) -> Result<(), Fail> {
    let value = load(Path::new(path))?;
    let files = match value.get("files") {
        Some(Json::Obj(entries)) => entries,
        _ => return Err(fail("manifest: missing files object")),
    };
    let normalize: Vec<String> = match value.get("normalize") {
        Some(Json::Arr(items)) => items
            .iter()
            .filter_map(Json::str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    };
    let mut entries: Vec<(&String, &str)> = files
        .iter()
        .map(|(path, want)| (path, want.str().unwrap_or("")))
        .collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    let mut failures = Vec::new();
    for (path, want) in &entries {
        let got = sha256_file(path, normalize.iter().any(|item| item == *path))?;
        if got != *want {
            failures.push(format!("{path}: expected {want} got {got}"));
        }
    }
    if !failures.is_empty() {
        for line in failures {
            println!("manifest: MISMATCH {line}");
        }
        return Err(Fail {
            msg: String::new(),
            code: 2,
        });
    }
    println!("manifest: PASS ({} files frozen)", entries.len());
    Ok(())
}

fn build(opts: &ManifestOpts) -> Result<(), Fail> {
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut normalize: Vec<String> = Vec::new();
    for raw in &opts.files {
        let (path, norm) = match raw.strip_prefix("date:") {
            Some(path) => (path, true),
            None => (raw.as_str(), false),
        };
        if norm && !normalize.iter().any(|item| item == path) {
            normalize.push(path.to_string());
        }
        let hash = sha256_file(path, norm)?;
        if let Some(entry) = entries.iter_mut().find(|(key, _)| key == path) {
            entry.1 = hash;
        } else {
            entries.push((path.to_string(), hash));
        }
    }
    normalize.sort();
    let value = J::Obj(vec![
        (
            "note".to_string(),
            J::Str("frozen evaluation inputs; CI asserts hashes are stable".to_string()),
        ),
        (
            "files".to_string(),
            J::Obj(
                entries
                    .iter()
                    .map(|(path, hash)| (path.clone(), J::Str(hash.clone())))
                    .collect(),
            ),
        ),
        (
            "normalize".to_string(),
            J::Arr(normalize.into_iter().map(J::Str).collect()),
        ),
    ]);
    if let Some(out) = &opts.out {
        io::write_json_indent(out, &value, 1).map_err(fail)?;
        println!("manifest: wrote {out} ({} files)", entries.len());
    } else {
        print!("{}", io::render_json(&value, 1));
    }
    Ok(())
}

pub(crate) fn sha256_file(path: &str, normalize_date: bool) -> Result<String, Fail> {
    let mut data = fs::read(path).map_err(|e| fail(format!("{path}: {e}")))?;
    if normalize_date {
        data = normalize_date_bytes(&data);
    }
    Ok(crate::hash::hex(&crate::hash::sha256(&data)))
}

fn normalize_date_bytes(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i + 11 <= data.len() {
        let date = is_digit(data[i])
            && is_digit(data[i + 1])
            && is_digit(data[i + 2])
            && is_digit(data[i + 3])
            && data[i + 4] == b'-'
            && is_digit(data[i + 5])
            && is_digit(data[i + 6])
            && data[i + 7] == b'-'
            && is_digit(data[i + 8])
            && is_digit(data[i + 9])
            && data[i + 10] == b'_';
        if date {
            out.extend_from_slice(b"<date>_");
            i += 11;
        } else {
            out.push(data[i]);
            i += 1;
        }
    }
    out.extend_from_slice(&data[i..]);
    out
}

fn is_digit(byte: u8) -> bool {
    byte.is_ascii_digit()
}

// ---------------------------------------------------------------------------
// report
// ---------------------------------------------------------------------------

struct ReportOpts {
    stats: String,
    stats_recall3: String,
    loo_json: String,
    fidelity: String,
    out: Option<String>,
    check: Option<String>,
}

pub(crate) fn report(args: &[String]) -> Result<(), Fail> {
    let opts = parse_report(args)?;
    let mrr = hard_of(&opts.stats, "mrr")?;
    let recall3 = hard_of(&opts.stats_recall3, "recall@3")?;
    let loo = load(Path::new(&opts.loo_json))?;
    let summary = loo
        .get("summary")
        .ok_or_else(|| fail("report: loo json missing summary"))?;
    let loo_min = number(summary, "min_delta")?;
    let loo_mean = number(summary, "mean_delta")?;
    let loo_max = number(summary, "max_delta")?;
    let loo_ci = number(summary, "min_ci_lo")?;

    let fid = load(Path::new(&opts.fidelity))?;
    let agg = fid
        .get("aggregate")
        .ok_or_else(|| fail("report: fidelity missing aggregate"))?;
    let n = integer(agg, "n")?;
    let ev_kept = integer(agg, "ev_kept")?;
    let ev_total = integer(agg, "ev_total")?;
    let trunc_ev = integer(agg, "trunc_ev")?;
    let ans = integer(agg, "ans")?;
    let ans_trunc = integer(agg, "ans_trunc")?;
    let tok = number(agg, "tok")?;
    let ev_rate = ratio(ev_kept, ev_total);
    let ev_trunc = ratio(trunc_ev, ev_total);
    let ans_rate = ratio(ans, n);
    let ans_trunc_rate = ratio(ans_trunc, n);

    let kinds = match agg.get("kinds") {
        Some(Json::Obj(entries)) => entries,
        _ => return Err(fail("report: fidelity missing kinds")),
    };
    let mut per_kind_bad: Vec<&str> = kinds
        .iter()
        .filter(|(_, kind)| {
            let kept = kind.get("ev_kept").and_then(Json::num).unwrap_or(0.0);
            let total = kind.get("ev_total").and_then(Json::num).unwrap_or(0.0);
            total > 0.0 && kept / total < 0.95
        })
        .map(|(name, _)| name.as_str())
        .collect();
    per_kind_bad.sort_unstable();

    let domain = agg.get("domain_boot").and_then(|value| match value {
        Json::Obj(entries) => Some(entries.as_slice()),
        _ => None,
    });
    let domain_mean = domain.and_then(|entries| {
        entries
            .iter()
            .find(|(key, _)| key == "mean_delta")
            .and_then(|(_, value)| value.num())
    });
    let domain_p = domain
        .and_then(|entries| {
            entries
                .iter()
                .find(|(key, _)| key == "p")
                .and_then(|(_, value)| value.num())
        })
        .unwrap_or(1.0);
    let positive_domains = domain
        .and_then(|entries| {
            entries
                .iter()
                .find(|(key, _)| key == "positive_domains")
                .and_then(|(_, value)| value.num())
        })
        .unwrap_or(0.0);
    let domains = domain
        .and_then(|entries| {
            entries
                .iter()
                .find(|(key, _)| key == "n_domains")
                .and_then(|(_, value)| value.num())
        })
        .unwrap_or(0.0);

    let holm = holm_bonferroni(&[mrr.p, recall3.p]);
    let mut gate_bad: Vec<String> = Vec::new();
    if !(mrr.delta >= 0.05 && mrr.ci_lo > 0.0 && mrr.p < 0.05) {
        gate_bad.push("mrr hard advantage".to_string());
    }
    if !(recall3.delta >= 0.05 && recall3.ci_lo > 0.0) {
        gate_bad.push("recall@3 hard advantage".to_string());
    }
    if !(loo_min >= 0.05 && loo_ci > 0.0) {
        gate_bad.push("leave-one-incident-out stability".to_string());
    }
    if !(ev_rate >= 0.95 && ev_rate > ev_trunc) {
        gate_bad.push("fidelity evidence retention".to_string());
    }
    if !(ans_rate >= 0.90 && ans_rate > ans_trunc_rate) {
        gate_bad.push("fidelity answerability".to_string());
    }
    if let Some(mean) = domain_mean {
        if !(mean > 0.0 && domain_p < 0.05) {
            gate_bad.push("fidelity domain cluster-bootstrap".to_string());
        }
    }
    if !per_kind_bad.is_empty() {
        gate_bad.push(format!(
            "per-kind zero-regression ({} kinds)",
            per_kind_bad.len()
        ));
    }
    if !holm.iter().all(|flag| *flag) {
        gate_bad.push("holm-bonferroni family-wise control".to_string());
    }
    let gate_ok = gate_bad.is_empty();

    let text = render(
        &mrr,
        &recall3,
        loo_min,
        loo_mean,
        loo_max,
        loo_ci,
        ev_kept,
        ev_total,
        ev_rate,
        ev_trunc,
        ans,
        n,
        ans_rate,
        ans_trunc_rate,
        tok,
        kinds.len(),
        &per_kind_bad,
        domain_mean,
        domain_p,
        positive_domains as i64,
        domains as i64,
        &holm,
        gate_ok,
        &gate_bad,
    );
    if let Some(check) = &opts.check {
        return check_report(check, &text);
    }
    if let Some(out) = &opts.out {
        fs::write(out, &text).map_err(|e| fail(format!("{out}: {e}")))?;
        println!("report: wrote {out}");
    } else {
        print!("{text}");
    }
    Ok(())
}

struct Hard {
    n: i64,
    a: f64,
    b: f64,
    delta: f64,
    ci_lo: f64,
    ci_hi: f64,
    p: f64,
}

fn hard_of(path: &str, metric: &str) -> Result<Hard, Fail> {
    let (_, rows) = io::read_csv(path).map_err(fail)?;
    let hard: Vec<&HashMap<String, String>> = rows
        .iter()
        .filter(|row| row.get("group").map(String::as_str) == Some("tier:hard"))
        .collect();
    if hard.len() != 1 {
        return Err(fail(format!("report: {path} missing tier:hard row")));
    }
    let row = hard[0];
    Ok(Hard {
        n: field(row, "n")?.parse().map_err(|_| bad(path, "n"))?,
        a: field(row, &format!("A_{metric}"))?
            .parse()
            .map_err(|_| bad(path, metric))?,
        b: field(row, &format!("B_{metric}"))?
            .parse()
            .map_err(|_| bad(path, metric))?,
        delta: field(row, "delta")?
            .parse()
            .map_err(|_| bad(path, "delta"))?,
        ci_lo: field(row, "ci_lo")?
            .parse()
            .map_err(|_| bad(path, "ci_lo"))?,
        ci_hi: field(row, "ci_hi")?
            .parse()
            .map_err(|_| bad(path, "ci_hi"))?,
        p: field(row, "perm_p")?
            .parse()
            .map_err(|_| bad(path, "perm_p"))?,
    })
}

fn field<'a>(row: &'a HashMap<String, String>, key: &str) -> Result<&'a str, Fail> {
    row.get(key)
        .map(String::as_str)
        .ok_or_else(|| fail(format!("report: missing column {key}")))
}

fn bad(path: &str, key: &str) -> Fail {
    fail(format!("report: {path} bad {key}"))
}

fn number(value: &Json, key: &str) -> Result<f64, Fail> {
    value
        .get(key)
        .and_then(Json::num)
        .ok_or_else(|| fail(format!("report: missing number {key}")))
}

fn integer(value: &Json, key: &str) -> Result<i64, Fail> {
    number(value, key).map(|number| number as i64)
}

fn ratio(part: i64, total: i64) -> f64 {
    if total == 0 {
        0.0
    } else {
        part as f64 / total as f64
    }
}

pub(crate) fn holm_bonferroni(ps: &[f64]) -> Vec<bool> {
    let m = ps.len();
    let mut order: Vec<usize> = (0..m).collect();
    order.sort_by(|a, b| {
        ps[*a]
            .partial_cmp(&ps[*b])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut decisions = vec![false; m];
    for (rank, index) in order.into_iter().enumerate() {
        if ps[index] <= 0.05 / (m - rank) as f64 {
            decisions[index] = true;
        } else {
            break;
        }
    }
    decisions
}

#[allow(clippy::too_many_arguments)]
fn render(
    mrr: &Hard,
    recall3: &Hard,
    loo_min: f64,
    loo_mean: f64,
    loo_max: f64,
    loo_ci: f64,
    ev_kept: i64,
    ev_total: i64,
    ev_rate: f64,
    ev_trunc: f64,
    ans: i64,
    n: i64,
    ans_rate: f64,
    ans_trunc: f64,
    tok: f64,
    kind_count: usize,
    per_kind_bad: &[&str],
    domain_mean: Option<f64>,
    domain_p: f64,
    positive_domains: i64,
    domains: i64,
    holm: &[bool],
    gate_ok: bool,
    gate_bad: &[String],
) -> String {
    let mut lines = vec![
        "# 评测实测指标（自动生成）".to_string(),
        String::new(),
        "> 由 `xtask report` 生成；CI 断言与本文件逐字节一致，文档数字一律以本文件为准。".to_string(),
        String::new(),
        "## Gauge（150 查询，seed 20261023，tier:hard）".to_string(),
        String::new(),
        "| 指标 | n | A | B | Δ | 95% CI | perm p |".to_string(),
        "| --- | --- | --- | --- | --- | --- | --- |".to_string(),
        format!(
            "| MRR | {} | {:.4} | {:.4} | +{:.4} | [{:+.4},{:+.4}] | {:.4} |",
            mrr.n, mrr.a, mrr.b, mrr.delta, mrr.ci_lo, mrr.ci_hi, mrr.p
        ),
        format!(
            "| recall@3 | {} | {:.4} | {:.4} | +{:.4} | [{:+.4},{:+.4}] | {:.4} |",
            recall3.n, recall3.a, recall3.b, recall3.delta, recall3.ci_lo, recall3.ci_hi, recall3.p
        ),
        String::new(),
        "## 留一事故泛化（30 incidents 逐一剔除）".to_string(),
        String::new(),
        format!(
            "- min Δ **+{loo_min:.4}** / mean +{loo_mean:.4} / max +{loo_max:.4}；min CI 下界 +{loo_ci:.4}"
        ),
        format!(
            "- 门禁：{}",
            if loo_min >= 0.05 && loo_ci > 0.0 {
                "PASS"
            } else {
                "FAIL"
            }
        ),
        String::new(),
        "## Fidelity（40 slice / 8 kinds）".to_string(),
        String::new(),
        format!(
            "- 证据保留 {ev_kept}/{ev_total} = {:.2}%（trunc {:.2}%）",
            100.0 * ev_rate,
            100.0 * ev_trunc
        ),
        format!(
            "- 可回答率 {ans}/{n} = {:.2}%（trunc {:.2}%）",
            100.0 * ans_rate,
            100.0 * ans_trunc
        ),
        format!("- token recall {:.2}%（报告-only）", 100.0 * tok),
        format!(
            "- per-kind 证据保留：{}/{} 域 ≥95%{}",
            kind_count - per_kind_bad.len(),
            kind_count,
            if per_kind_bad.is_empty() {
                String::new()
            } else {
                format!("（未过：{}）", per_kind_bad.join(", "))
            }
        ),
    ];
    match domain_mean {
        Some(mean) => lines.push(format!(
            "- 跨域优势（cluster=kind，单侧 cluster-bootstrap）：mean Δ +{:.2}pp，p={:.4}（{}/{} 域正差）",
            100.0 * mean,
            domain_p,
            positive_domains,
            domains
        )),
        None => lines.push("- 跨域优势：域不足，无法检验".to_string()),
    }
    lines.extend([
        String::new(),
        "## 多重比较（Holm-Bonferroni，Gauge 族 m=2，α=0.05）".to_string(),
        String::new(),
        "| 检验 | perm p | Holm 阈值 | 校正后 |".to_string(),
        "| --- | --- | --- | --- |".to_string(),
        format!(
            "| MRR hard | {:.4} | 0.0250 | {} |",
            mrr.p,
            if holm[0] { "显著" } else { "不显著" }
        ),
        format!(
            "| recall@3 hard | {:.4} | 0.0500 | {} |",
            recall3.p,
            if holm[1] { "显著" } else { "不显著" }
        ),
        String::new(),
        "fidelity 的 ev/ans 是阈值门禁（非显著性检验），无 p 需校正。".to_string(),
        String::new(),
        format!("## 汇总门禁：{}", if gate_ok { "PASS" } else { "FAIL" }),
    ]);
    if !gate_bad.is_empty() {
        lines.push(String::new());
        lines.push(format!("失败项：{}", gate_bad.join("；")));
    }
    lines.join("\n") + "\n"
}

fn check_report(path: &str, text: &str) -> Result<(), Fail> {
    let frozen = fs::read_to_string(path).ok();
    if frozen.as_deref() == Some(text) {
        println!("report: PASS (matches {path})");
        return Ok(());
    }
    println!("report: DRIFT in {path}");
    match &frozen {
        None => println!("report: frozen file missing"),
        Some(frozen) => {
            println!("--- frozen");
            println!("+++ live");
            for line in frozen.lines() {
                println!("-{line}");
            }
            for line in text.lines() {
                println!("+{line}");
            }
        }
    }
    Err(Fail {
        msg: String::new(),
        code: 2,
    })
}

fn parse_report(args: &[String]) -> Result<ReportOpts, Fail> {
    let mut stats = None;
    let mut stats_recall3 = None;
    let mut loo_json = None;
    let mut fidelity = None;
    let mut out = None;
    let mut check = None;
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        match flag {
            "-h" | "--help" => return Err(usage(String::new())),
            "--stats" | "--stats-recall3" | "--loo-json" | "--fidelity" | "--out" | "--check" => {
                i += 1;
                let value = args
                    .get(i)
                    .cloned()
                    .ok_or_else(|| usage(format!("{flag} expects a path")))?;
                match flag {
                    "--stats" => stats = Some(value),
                    "--stats-recall3" => stats_recall3 = Some(value),
                    "--loo-json" => loo_json = Some(value),
                    "--fidelity" => fidelity = Some(value),
                    "--out" => out = Some(value),
                    "--check" => check = Some(value),
                    _ => {}
                }
            }
            other => return Err(usage(format!("unrecognized argument: {other}"))),
        }
        i += 1;
    }
    Ok(ReportOpts {
        stats: stats.ok_or_else(|| usage("--stats is required"))?,
        stats_recall3: stats_recall3.ok_or_else(|| usage("--stats-recall3 is required"))?,
        loo_json: loo_json.ok_or_else(|| usage("--loo-json is required"))?,
        fidelity: fidelity.ok_or_else(|| usage("--fidelity is required"))?,
        out,
        check,
    })
}

fn load(path: &Path) -> Result<Json, Fail> {
    let text = fs::read_to_string(path).map_err(|e| fail(format!("{}: {e}", path.display())))?;
    crate::json::parse(&text).map_err(|e| fail(format!("{}: {e}", path.display())))
}

fn usage(message: impl Into<String>) -> Fail {
    let body = "usage: xtask manifest [--files PATH...] [--out PATH] [--expect PATH]\n       xtask report --stats CSV --stats-recall3 CSV --loo-json JSON --fidelity JSON [--out PATH | --check PATH]\n       xtask freeze [--results DIR] [--manifest FILE]\n       xtask subset [CSV...] [--dir DIR] [--out FILE]\n";
    let message = message.into();
    Fail {
        msg: if message.is_empty() {
            body.to_string()
        } else {
            format!("{message}\n\n{body}")
        },
        code: 2,
    }
}

// ---------------------------------------------------------------------------
// freeze
// ---------------------------------------------------------------------------

struct FreezeOpts {
    results: String,
    manifest: String,
}

pub(crate) fn freeze(args: &[String]) -> Result<(), Fail> {
    let opts = parse_freeze(args)?;
    let mut value = load(Path::new(&opts.manifest))?;
    let entries = match &mut value {
        Json::Obj(entries) => entries,
        _ => return Err(fail("freeze: manifest is not an object")),
    };
    let mut frozen: Vec<(String, Json)> = entries
        .iter()
        .find(|(key, _)| key == "frozen")
        .and_then(|(_, value)| match value {
            Json::Obj(items) => Some(items.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let mut files: Vec<(String, PathBuf)> = fs::read_dir(&opts.results)
        .map_err(|e| fail(format!("{}: {e}", opts.results)))?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let keep = path.is_file()
                && matches!(
                    path.extension().and_then(|ext| ext.to_str()),
                    Some("csv") | Some("json")
                );
            keep.then_some((name, path))
        })
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    if files.is_empty() {
        println!("no result files under {}", opts.results);
        return Err(Fail {
            msg: String::new(),
            code: 1,
        });
    }
    let mut added = 0usize;
    let mut updated = 0usize;
    for (name, path) in files {
        let data = fs::read(&path).map_err(|e| fail(format!("{}: {e}", path.display())))?;
        let sha = crate::hash::hex(&crate::hash::sha256(&data));
        let rows = data_rows(&data) as u64;
        let bytes = data.len() as u64;
        if let Some(slot) = frozen.iter_mut().find(|(key, _)| *key == name) {
            if !same_entry(&slot.1, &sha, bytes, rows) {
                slot.1 = entry_json(&sha, bytes, rows);
                updated += 1;
            }
        } else {
            frozen.push((name, entry_json(&sha, bytes, rows)));
            added += 1;
        }
    }
    frozen.sort_by(|a, b| a.0.cmp(&b.0));
    set_key(entries, "frozen", Json::Obj(frozen));
    set_key(entries, "schema", Json::Num(1.0));
    set_key(entries, "frozen_at", Json::Str("2026-10-29".to_string()));
    set_key(
        entries,
        "note",
        Json::Str(
            "定稿评测结果冻结（不可变基线）。修改/重跑后必须重新冻结并提交；\
             check_bench.sh 校验 sha256+行数。"
                .to_string(),
        ),
    );
    io::write_json_raw(&opts.manifest, &to_j(&value), 2).map_err(fail)?;
    let count = match value.get("frozen") {
        Some(Json::Obj(items)) => items.len(),
        _ => 0,
    };
    println!(
        "frozen={count}  added={added}  updated={updated}  ({})",
        opts.results
    );
    Ok(())
}

fn parse_freeze(args: &[String]) -> Result<FreezeOpts, Fail> {
    let mut results = format!("{ROOT}/config/bench/results");
    let mut manifest = format!("{ROOT}/config/bench/eval-manifest.json");
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--results" | "--manifest" => {
                let flag = args[i].as_str();
                i += 1;
                let value = args
                    .get(i)
                    .cloned()
                    .ok_or_else(|| usage(format!("{flag} expects a path")))?;
                if flag == "--results" {
                    results = value;
                } else {
                    manifest = value;
                }
            }
            "-h" | "--help" => return Err(usage(String::new())),
            other => return Err(usage(format!("unrecognized argument: {other}"))),
        }
        i += 1;
    }
    Ok(FreezeOpts { results, manifest })
}

fn data_rows(data: &[u8]) -> usize {
    data.iter().filter(|byte| **byte == b'\n').count() + if data.ends_with(b"\n") { 0 } else { 1 }
}

fn entry_json(sha: &str, bytes: u64, rows: u64) -> Json {
    Json::Obj(vec![
        ("sha256".to_string(), Json::Str(sha.to_string())),
        ("bytes".to_string(), Json::Num(bytes as f64)),
        ("rows".to_string(), Json::Num(rows as f64)),
    ])
}

fn same_entry(existing: &Json, sha: &str, bytes: u64, rows: u64) -> bool {
    let Json::Obj(items) = existing else {
        return false;
    };
    if items.len() != 3 {
        return false;
    }
    let get = |key: &str| {
        items
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    };
    get("sha256").and_then(Json::str) == Some(sha)
        && get("bytes").and_then(Json::num).map(|n| n as u64) == Some(bytes)
        && get("rows").and_then(Json::num).map(|n| n as u64) == Some(rows)
}

fn set_key(entries: &mut Vec<(String, Json)>, key: &str, value: Json) {
    if let Some(slot) = entries.iter_mut().find(|(name, _)| name == key) {
        slot.1 = value;
    } else {
        entries.push((key.to_string(), value));
    }
}

// ---------------------------------------------------------------------------
// subset (check subset)
// ---------------------------------------------------------------------------

const SUBSET_DEFAULT: [&str; 2] = [
    "bench_code_shape50_v4pro.csv",
    "bench_code_layers50_v4pro.csv",
];

struct SubsetOpts {
    names: Vec<String>,
    dir: String,
    out: String,
}

pub(crate) fn subset(args: &[String]) -> Result<(), Fail> {
    let opts = parse_subset(args)?;
    let names: Vec<&str> = if opts.names.is_empty() {
        SUBSET_DEFAULT.to_vec()
    } else {
        opts.names.iter().map(String::as_str).collect()
    };
    println!("== C2 可压缩子集口径重算（剔除 fail-open）==");
    let mut files = Vec::new();
    for name in names {
        let path = if Path::new(name).is_absolute() {
            PathBuf::from(name)
        } else {
            Path::new(&opts.dir).join(name)
        };
        let (_, rows) = io::read_csv(&path.to_string_lossy()).map_err(fail)?;
        let mut by_model: BTreeMap<String, Vec<&HashMap<String, String>>> = BTreeMap::new();
        for row in &rows {
            let model = row.get("model").cloned().unwrap_or_default();
            by_model.entry(model).or_default().push(row);
        }
        let mut models = Vec::new();
        for (model, rows) in &by_model {
            let n = rows.len();
            let pass = rows.iter().filter(|row| flag(row, "scx")).count();
            let cp: Vec<_> = rows.iter().filter(|row| flag(row, "compressed")).collect();
            let fo: Vec<_> = rows.iter().filter(|row| !flag(row, "compressed")).collect();
            let cp_n = cp.len();
            let pass_cp = cp.iter().filter(|row| flag(row, "scx")).count();
            let pass_fo = fo.iter().filter(|row| flag(row, "scx")).count();
            let raw_rate = if n == 0 { 0.0 } else { pass as f64 / n as f64 };
            let raw_cp = if cp_n == 0 {
                0.0
            } else {
                pass_cp as f64 / cp_n as f64
            };
            let rate = round_rate(pass, n);
            let rate_cp = round_rate(pass_cp, cp_n);
            println!(
                "  {name} {model}: 全量 {pass}/{n} ({:.1}%) | 剔除fail-open {pass_cp}/{cp_n} ({:.1}%) | fail-open {}",
                raw_rate * 100.0,
                raw_cp * 100.0,
                fo.len()
            );
            models.push((
                model.clone(),
                J::Obj(vec![
                    (
                        "all".to_string(),
                        J::Obj(vec![
                            ("pass".to_string(), J::Int(pass as i64)),
                            ("n".to_string(), J::Int(n as i64)),
                            ("rate".to_string(), J::Num(rate)),
                        ]),
                    ),
                    (
                        "subset".to_string(),
                        J::Obj(vec![
                            ("pass".to_string(), J::Int(pass_cp as i64)),
                            ("n".to_string(), J::Int(cp_n as i64)),
                            ("rate".to_string(), J::Num(rate_cp)),
                        ]),
                    ),
                    (
                        "fail_open".to_string(),
                        J::Obj(vec![
                            ("n".to_string(), J::Int(fo.len() as i64)),
                            ("pass_in_fo".to_string(), J::Int(pass_fo as i64)),
                        ]),
                    ),
                ]),
            ));
        }
        files.push(J::Obj(vec![
            ("file".to_string(), J::Str(name.to_string())),
            ("n".to_string(), J::Int(rows.len() as i64)),
            ("models".to_string(), J::Obj(models)),
        ]));
    }
    let value = J::Obj(vec![
        (
            "pipeline".to_string(),
            J::Str("P12.6 C2 可压缩子集口径重算（剔除 fail-open 未压缩题）".to_string()),
        ),
        (
            "method".to_string(),
            J::Str("fail-open = compressed!=True；口径列示 全量 vs 剔除后 两套".to_string()),
        ),
        ("files".to_string(), J::Arr(files)),
    ]);
    io::write_json_raw(&opts.out, &value, 2).map_err(fail)?;
    println!("WROTE {}", opts.out);
    Ok(())
}

fn parse_subset(args: &[String]) -> Result<SubsetOpts, Fail> {
    let mut opts = SubsetOpts {
        names: Vec::new(),
        dir: format!("{ROOT}/config/bench/results"),
        out: format!("{ROOT}/config/bench/results/p12_6_c2_subset.json"),
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dir" | "--out" => {
                let flag = args[i].as_str();
                i += 1;
                let value = args
                    .get(i)
                    .cloned()
                    .ok_or_else(|| usage(format!("{flag} expects a path")))?;
                if flag == "--dir" {
                    opts.dir = value;
                } else {
                    opts.out = value;
                }
            }
            "-h" | "--help" => return Err(usage(String::new())),
            other if other.starts_with('-') => {
                return Err(usage(format!("unrecognized argument: {other}")))
            }
            other => opts.names.push(other.to_string()),
        }
        i += 1;
    }
    Ok(opts)
}

fn flag(row: &HashMap<String, String>, key: &str) -> bool {
    row.get(key)
        .map(|value| value.trim() == "True")
        .unwrap_or(false)
}

fn round_rate(part: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        io::round4(part as f64 / total as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_date_matches_python_subset() {
        assert_eq!(
            normalize_date_bytes(b"id=2026-10-22_abcdef"),
            b"id=<date>_abcdef".to_vec()
        );
        assert_eq!(
            normalize_date_bytes(b"2026-10-2_x"),
            b"2026-10-2_x".to_vec()
        );
    }

    #[test]
    fn holm_matches_reference() {
        assert_eq!(holm_bonferroni(&[0.001, 0.04]), vec![true, true]);
        assert_eq!(holm_bonferroni(&[0.03, 0.01]), vec![true, true]);
        assert_eq!(holm_bonferroni(&[0.03, 0.06]), vec![false, false]);
    }
}
