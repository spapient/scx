//! `xtask`：评估工具（feature = "eval"）。
//!
//! 迁移自 `tools/scx.py` 的 `baseline`/`stats`。契约保持不变：
//! - 同一 baseline/verbose/memory 输入 → 同一 CSV/控制台输出；
//! - bootstrap/permutation 复刻 CPython `random.Random`，可与 Python 版逐字节对拍；
//! - 退出码：参数/输入错误 1，LOO gate 失败 2。
//!
//! 该二进制只在 `--features eval` 下构建，不进发布包。

mod bench;
mod blind;
mod calibrate;
mod checks;
mod contain;
mod distill;
mod extract;
mod fidelity;
mod fit;
mod gen;
mod io;
mod meta;
mod mlp;
mod ops;
mod rng;
mod seed;
mod selftest;
mod sha512;
mod stats;

use crate::json::Json;
use std::collections::HashSet;

#[derive(Debug)]
pub struct Fail {
    msg: String,
    code: i32,
}

fn fail(msg: impl Into<String>) -> Fail {
    Fail {
        msg: msg.into(),
        code: 1,
    }
}

fn usage_fail(msg: impl Into<String>) -> Fail {
    Fail {
        msg: format!("{}\n\n{}", msg.into(), usage()),
        code: 2,
    }
}

/// 入口：返回进程退出码。
pub fn run(args: &[String]) -> i32 {
    let result = match args.first().map(String::as_str) {
        Some("baseline") => baseline(&args[1..]),
        Some("stats") => stats_cmd(&args[1..]),
        Some("contain") => contain::run(&args[1..]),
        Some("fidelity") => fidelity::run(&args[1..]),
        Some("bench") => bench::run(&args[1..]),
        Some("gen") => gen::run(&args[1..]),
        Some("ops") => ops::run(&args[1..]),
        Some("verify") => blind::verify(&args[1..]),
        Some("blind") => blind::run(&args[1..]),
        Some("extract") => extract::run(&args[1..]),
        Some("calibrate") => calibrate::run(&args[1..]),
        Some("distill") => distill::run(&args[1..]),
        Some("fit") => fit::run(&args[1..]),
        Some("mlp") => mlp::run(&args[1..]),
        Some("audit") => checks::audit(),
        Some("a2") => checks::a2(),
        Some("manifest") => meta::manifest(&args[1..]),
        Some("report") => meta::report(&args[1..]),
        Some("freeze") => meta::freeze(&args[1..]),
        Some("subset") => meta::subset(&args[1..]),
        Some("seed") => seed::run(&args[1..]),
        Some("selftest") => selftest::run(),
        Some("-h" | "--help") => {
            print!("{}", usage());
            Ok(())
        }
        Some(other) => return unknown(other),
        None => {
            eprint!("{}", usage());
            return 2;
        }
    };
    match result {
        Ok(()) => 0,
        Err(failure) => {
            if !failure.msg.is_empty() {
                eprintln!("{}", failure.msg);
            }
            failure.code
        }
    }
}

fn unknown(command: &str) -> i32 {
    eprintln!("xtask: unknown command: {command}");
    eprint!("{}", usage());
    2
}

fn usage() -> String {
    "\
usage: xtask <command> [options]

commands:
  baseline  eval CSV -> target baseline JSON
  stats     verbose + baseline -> significance CSV
  contain   evidence containment vs head-truncation / random baseline
  fidelity  answerability + token retention + domain cluster bootstrap
  bench     offline benchmarks (real / perf / budget)
  gen       materialize frozen corpora (cuda/js/layers/shape/bench/fid)
  ops       run/install/setup/doctor/check/perf
  verify    blind packet hash + row count
  blind     per-candidate blind review (make/import/check/progress/agree/review)
  extract   Claude Code JSONL -> compression corpus
  calibrate BUDGET_SAFETY_FACTOR quantiles (tiktoken truth via --truth)
  distill   offline weight coordinate ascent (distill weights)
  fit       weak-label linear weight fit (scx-sem-1)
  mlp       linear/MLP cross-validation on distill cache
  audit     production unwrap/panic audit gate
  a2        A2 round-trip fidelity thresholds
  manifest  freeze/verify evaluation-input hashes
  report    compose/check frozen metrics markdown
  freeze    freeze results/ into eval-manifest.json
  subset    C2 compressible-subset recomputation (drop fail-open)
  seed      corpus -> memory cases + eval CSV
  selftest  Rust invariant self-check

baseline options:
  --csv PATH          seed CSV (query,layer,case_id[,note])   [required]
  --out PATH          baseline JSON output                    [required]
  --judge-by NAME     judge metadata                          [mechanical-target-v1]
  --judge-date ISO    judge metadata                          [2026-10-23T00:00:00Z]

stats options:
  --baseline PATH     baseline JSON                           [required]
  --verbose PATH      `--eval --eval-verbose` output          [required]
  --out PATH          significance CSV output                 [required]
  --mem-dir DIR       memory root for automatic strata
  --strata PATH       strata CSV (case_index,tier); overrides --mem-dir
  --metric NAME       mrr | ndcg | recall@1|3|5|10            [mrr]
  --boot N            bootstrap resamples                     [10000]
  --perm N            permutation rounds                      [10000]
  --seed N            RNG seed                                [20261023]
  --mde F             minimum detectable effect               [0.05]
  --cluster-size N    cluster bootstrap size (0 = off)        [0]
  --loo               leave-one-incident-out stability
  --loo-group N       cases per incident                      [5]
  --loo-gate-delta F  min tier:hard delta per LOO run         [0.05]
  --gate              with --loo: exit 2 if advantage is lost
"
    .to_string()
}

#[derive(Default)]
struct Args {
    values: std::collections::HashMap<String, String>,
    flags: HashSet<String>,
}

fn parse(args: &[String], value_opts: &[&str], flag_opts: &[&str]) -> Result<Args, Fail> {
    let mut out = Args::default();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) => (name.to_string(), Some(value.to_string())),
            None => (arg.clone(), None),
        };
        if flag_opts.contains(&name.as_str()) {
            if inline.is_some() {
                return Err(usage_fail(format!("{name} does not take a value")));
            }
            out.flags.insert(name);
            i += 1;
            continue;
        }
        if !value_opts.contains(&name.as_str()) {
            return Err(usage_fail(format!("unrecognized argument: {arg}")));
        }
        let value = match inline {
            Some(value) => value,
            None => {
                i += 1;
                args.get(i)
                    .cloned()
                    .ok_or_else(|| usage_fail(format!("{name} expects a value")))?
            }
        };
        out.values.insert(name, value);
        i += 1;
    }
    Ok(out)
}

impl Args {
    fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    fn req(&self, name: &str) -> Result<&str, Fail> {
        self.get(name)
            .ok_or_else(|| usage_fail(format!("the following arguments are required: {name}")))
    }

    fn number<T: std::str::FromStr>(&self, name: &str, default: T) -> Result<T, Fail> {
        match self.get(name) {
            None => Ok(default),
            Some(text) => text
                .parse()
                .map_err(|_| usage_fail(format!("invalid value for {name}: {text}"))),
        }
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.contains(name)
    }
}

fn baseline(args: &[String]) -> Result<(), Fail> {
    let opts = parse(args, &["--csv", "--out", "--judge-by", "--judge-date"], &[])?;
    let csv = opts.req("--csv")?;
    let out = opts.req("--out")?;
    let judge_by = opts.get("--judge-by").unwrap_or("mechanical-target-v1");
    let judge_date = opts.get("--judge-date").unwrap_or("2026-10-23T00:00:00Z");
    let (_, rows) = io::read_csv(csv).map_err(fail)?;
    if rows.is_empty() {
        return Err(fail(format!("baseline: no data rows: {csv}")));
    }
    let mut cases = Vec::with_capacity(rows.len());
    let mut seen = HashSet::new();
    let mut counts = [0usize; 3];
    for (offset, row) in rows.iter().enumerate() {
        let number = offset + 2;
        let query = field(row, "query");
        let layer = field(row, "layer").to_lowercase();
        let case_id = field(row, "case_id");
        let Some(layer_index) = stats::LAYERS.iter().position(|l| *l == layer) else {
            return Err(fail(format!("baseline: bad row {number}")));
        };
        if query.is_empty() || case_id.is_empty() {
            return Err(fail(format!("baseline: bad row {number}")));
        }
        if query.contains(['\r', '\n', '\t', '\0']) {
            return Err(fail(format!(
                "baseline: row {number} query: must be one line without tabs or NUL"
            )));
        }
        if !seen.insert(case_id.clone()) {
            return Err(fail(format!("baseline: duplicate case_id: {case_id}")));
        }
        let note = field(row, "note");
        let note = if note.is_empty() {
            "target-only".to_string()
        } else {
            format!("target-only; {note}")
        };
        cases.push(Json::Obj(vec![
            ("query".to_string(), Json::Str(query)),
            ("layer".to_string(), Json::Str(layer)),
            (
                "expected".to_string(),
                Json::Arr(vec![Json::Str(case_id.clone())]),
            ),
            (
                "rel".to_string(),
                Json::Obj(vec![(case_id, Json::Num(2.0))]),
            ),
            (
                "judge".to_string(),
                Json::Obj(vec![
                    ("by".to_string(), Json::Str(judge_by.to_string())),
                    ("date".to_string(), Json::Str(judge_date.to_string())),
                ]),
            ),
            ("note".to_string(), Json::Str(note)),
        ]));
        counts[layer_index] += 1;
    }
    let total = cases.len();
    let value = Json::Obj(vec![("cases".to_string(), Json::Arr(cases))]);
    io::write_json(out, &to_pretty(&value)).map_err(fail)?;
    println!(
        "baseline: wrote {out} cases={total} literal={} paraphrase={} semantic={}",
        counts[0], counts[1], counts[2]
    );
    if total < 30 || counts.iter().any(|count| *count < 10) {
        println!("baseline: WARNING not gate-ready (need total>=30 and each layer>=10)");
    }
    Ok(())
}

fn field(row: &std::collections::HashMap<String, String>, key: &str) -> String {
    row.get(key).map(|v| v.trim()).unwrap_or("").to_string()
}

/// 复用 `crate::json::Json` 做数据模型，再走 Python 兼容的缩进输出。
fn to_pretty(value: &Json) -> io::J {
    match value {
        Json::Null => io::J::Null,
        Json::Bool(b) => io::J::Bool(*b),
        Json::Num(n) => {
            if n.fract() == 0.0 && n.abs() < 9e15 {
                io::J::Int(*n as i64)
            } else {
                io::J::Num(*n)
            }
        }
        Json::Str(s) => io::J::Str(s.clone()),
        Json::Arr(items) => io::J::Arr(items.iter().map(to_pretty).collect()),
        Json::Obj(entries) => io::J::Obj(
            entries
                .iter()
                .map(|(key, value)| (key.clone(), to_pretty(value)))
                .collect(),
        ),
    }
}

struct Settings {
    out: String,
    metric: String,
    plan: stats::Plan,
    cluster_size: usize,
    loo_group: usize,
    loo_gate_delta: f64,
    want_loo: bool,
    gate: bool,
}

impl Settings {
    fn from(opts: &Args) -> Result<Self, Fail> {
        let out = opts.req("--out")?.to_string();
        let metric = opts.get("--metric").unwrap_or("mrr").to_string();
        let boot: usize = opts.number("--boot", 10_000)?;
        let perm: usize = opts.number("--perm", 10_000)?;
        let seed: i64 = opts.number("--seed", 20_261_023)?;
        let mde: f64 = opts.number("--mde", 0.05)?;
        let cluster_size: i64 = opts.number("--cluster-size", 0)?;
        let loo_group: usize = opts.number("--loo-group", 5)?;
        let loo_gate_delta: f64 = opts.number("--loo-gate-delta", 0.05)?;
        if boot < 1 || perm < 1 {
            return Err(fail("stats: --boot and --perm must be positive"));
        }
        if !mde.is_finite() || !(0.0..=1.0).contains(&mde) {
            return Err(fail("stats: --mde must be a finite value in [0, 1]"));
        }
        if cluster_size < 0 {
            return Err(fail("stats: --cluster-size must be non-negative"));
        }
        if !valid_metric(&metric) {
            return Err(usage_fail(format!("invalid --metric: {metric}")));
        }
        Ok(Self {
            out,
            metric,
            plan: stats::Plan {
                boot,
                perm,
                seed,
                mde,
            },
            cluster_size: cluster_size as usize,
            loo_group,
            loo_gate_delta,
            want_loo: opts.flag("--loo"),
            gate: opts.flag("--gate"),
        })
    }
}

fn stats_cmd(args: &[String]) -> Result<(), Fail> {
    let opts = parse(
        args,
        &[
            "--baseline",
            "--verbose",
            "--out",
            "--mem-dir",
            "--strata",
            "--metric",
            "--boot",
            "--perm",
            "--seed",
            "--mde",
            "--cluster-size",
            "--loo-group",
            "--loo-gate-delta",
        ],
        &["--loo", "--gate"],
    )?;
    let baseline = opts.req("--baseline")?;
    let verbose = opts.req("--verbose")?;
    let settings = Settings::from(&opts)?;
    let cases = stats::load_baseline(baseline).map_err(fail)?;
    let hits = stats::parse_verbose(verbose, cases.len()).map_err(fail)?;
    let tiers = stats::strata(&cases, opts.get("--mem-dir"), opts.get("--strata")).map_err(fail)?;
    let rows = build_rows(&cases, &hits, &tiers, &settings.metric)?;
    let mut results = grouped(&rows, settings.plan);
    if settings.cluster_size > 0 {
        let hard: Vec<&stats::Row> = rows.iter().filter(|row| row.tier == "hard").collect();
        if let Some(result) = stats::analyze_cluster(&hard, settings.cluster_size, settings.plan) {
            results.push(result);
        }
    }
    let loo = if settings.want_loo {
        Some(run_loo(&rows, &settings)?)
    } else {
        None
    };
    if let Some(run) = &loo {
        results.extend(run.rows.iter().cloned());
    }
    write_csv(&settings.out, &results, &settings.metric)?;
    if let Some(run) = &loo {
        let path = settings.out.replace(".csv", ".loo.json");
        io::write_json(&path, &loo_json(run, &settings.metric)).map_err(fail)?;
    }
    print_results(&results);
    println!(
        "stats: wrote {} groups={} metric={}",
        settings.out,
        results.len(),
        settings.metric
    );
    Ok(())
}

fn grouped(rows: &[stats::Row], plan: stats::Plan) -> Vec<stats::ResultRow> {
    let mut results = Vec::new();
    for layer in stats::LAYERS {
        let group: Vec<&stats::Row> = rows.iter().filter(|row| row.layer == layer).collect();
        if let Some(result) = stats::analyze(layer, &group, plan) {
            results.push(result);
        }
    }
    for tier in tier_names(rows) {
        let group: Vec<&stats::Row> = rows.iter().filter(|row| row.tier == tier).collect();
        if let Some(result) = stats::analyze(&format!("tier:{tier}"), &group, plan) {
            results.push(result);
        }
    }
    let all: Vec<&stats::Row> = rows.iter().collect();
    if let Some(result) = stats::analyze("all", &all, plan) {
        results.push(result);
    }
    results
}

fn tier_names(rows: &[stats::Row]) -> Vec<&str> {
    let mut names: Vec<&str> = rows.iter().map(|row| row.tier.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    names
}

fn run_loo(rows: &[stats::Row], settings: &Settings) -> Result<stats::Loo, Fail> {
    stats::loo(
        rows,
        settings.loo_group,
        settings.plan,
        settings.loo_gate_delta,
        settings.gate,
    )
    .map_err(|msg| Fail {
        code: if msg.starts_with("stats: LOO GATE FAILED") {
            2
        } else {
            1
        },
        msg,
    })
}
fn valid_metric(metric: &str) -> bool {
    matches!(
        metric,
        "mrr" | "ndcg" | "recall@1" | "recall@3" | "recall@5" | "recall@10"
    )
}

fn build_rows(
    cases: &[stats::Case],
    hits: &[stats::Hits],
    tiers: &[String],
    metric: &str,
) -> Result<Vec<stats::Row>, Fail> {
    let mut rows = Vec::with_capacity(cases.len());
    for (index, (case, hit)) in cases.iter().zip(hits).enumerate() {
        let target = &case.expected[0];
        let (a, b) = match metric {
            "mrr" => (
                stats::mrr(stats::target_rank(hit.a.as_deref().unwrap_or(&[]), target)),
                stats::mrr(stats::target_rank(hit.b.as_deref().unwrap_or(&[]), target)),
            ),
            "ndcg" => (
                stats::ndcg(case, hit.a.as_deref().unwrap_or(&[])),
                stats::ndcg(case, hit.b.as_deref().unwrap_or(&[])),
            ),
            _ => {
                let k: usize = metric
                    .split_once('@')
                    .and_then(|(_, k)| k.parse().ok())
                    .ok_or_else(|| usage_fail(format!("invalid --metric: {metric}")))?;
                (
                    stats::recall_at(
                        stats::target_rank(hit.a.as_deref().unwrap_or(&[]), target),
                        k,
                    ),
                    stats::recall_at(
                        stats::target_rank(hit.b.as_deref().unwrap_or(&[]), target),
                        k,
                    ),
                )
            }
        };
        rows.push(stats::Row {
            i: case.case_index.unwrap_or(index as i64),
            layer: case.layer.clone(),
            tier: tiers.get(index).cloned().unwrap_or_default(),
            a,
            b,
        });
    }
    Ok(rows)
}

fn fields(metric: &str, with_clusters: bool) -> Vec<String> {
    let mut fields = vec![
        "group".to_string(),
        "n".to_string(),
        format!("A_{metric}"),
        format!("B_{metric}"),
        "delta".to_string(),
        "ci_lo".to_string(),
        "ci_hi".to_string(),
        "perm_p".to_string(),
        "mde".to_string(),
        "advantage".to_string(),
    ];
    if with_clusters {
        fields.push("n_clusters".to_string());
    }
    fields
}

fn row_values(row: &stats::ResultRow, with_clusters: bool) -> Vec<String> {
    let mut out = vec![
        row.group.clone(),
        row.n.to_string(),
        io::repr(row.a),
        io::repr(row.b),
        io::repr(row.delta),
        io::repr(row.ci_lo),
        io::repr(row.ci_hi),
        io::repr(row.perm_p),
        io::repr(row.mde),
        if row.advantage { "1" } else { "0" }.to_string(),
    ];
    if with_clusters {
        // Python DictWriter 对缺失键写 restval=""（不是 0）。
        out.push(row.n_clusters.map(|n| n.to_string()).unwrap_or_default());
    }
    out
}

fn write_csv(out: &str, results: &[stats::ResultRow], metric: &str) -> Result<(), Fail> {
    let with_clusters = results.iter().any(|row| row.n_clusters.is_some());
    let fields = fields(metric, with_clusters);
    let rows: Vec<Vec<String>> = results
        .iter()
        .map(|row| row_values(row, with_clusters))
        .collect();
    io::write_csv(out, &fields, &rows).map_err(fail)
}

fn print_results(results: &[stats::ResultRow]) {
    for row in results {
        match row.n_clusters {
            None => println!(
                "{:<14} n={:>3} A={:.4} B={:.4} d={:+.4} CI=[{:+.4},{:+.4}] p={:.4}",
                row.group, row.n, row.a, row.b, row.delta, row.ci_lo, row.ci_hi, row.perm_p
            ),
            Some(clusters) => println!(
                "{:<14} n={:>3} clusters={:>3} d={:+.4} CI=[{:+.4},{:+.4}] p={:.4}",
                row.group, row.n, clusters, row.delta, row.ci_lo, row.ci_hi, row.perm_p
            ),
        }
    }
}

fn loo_json(run: &stats::Loo, metric: &str) -> io::J {
    let rows = run
        .rows
        .iter()
        .map(|row| {
            let mut entries = vec![
                ("group".to_string(), io::J::Str(row.group.clone())),
                ("n".to_string(), io::J::Int(row.n as i64)),
                (format!("A_{metric}"), io::J::Num(row.a)),
                (format!("B_{metric}"), io::J::Num(row.b)),
                ("delta".to_string(), io::J::Num(row.delta)),
                ("ci_lo".to_string(), io::J::Num(row.ci_lo)),
                ("ci_hi".to_string(), io::J::Num(row.ci_hi)),
                ("perm_p".to_string(), io::J::Num(row.perm_p)),
                ("mde".to_string(), io::J::Num(row.mde)),
                (
                    "advantage".to_string(),
                    io::J::Int(if row.advantage { 1 } else { 0 }),
                ),
            ];
            if let Some(clusters) = row.n_clusters {
                entries.push(("n_clusters".to_string(), io::J::Int(clusters as i64)));
            }
            io::J::Obj(entries)
        })
        .collect();
    io::J::Obj(vec![
        ("rows".to_string(), io::J::Arr(rows)),
        (
            "summary".to_string(),
            io::J::Obj(vec![
                ("group".to_string(), io::J::Str("loo:all".to_string())),
                ("n_runs".to_string(), io::J::Int(run.summary.n_runs as i64)),
                ("min_delta".to_string(), io::J::Num(run.summary.min_delta)),
                ("mean_delta".to_string(), io::J::Num(run.summary.mean_delta)),
                ("max_delta".to_string(), io::J::Num(run.summary.max_delta)),
                ("min_ci_lo".to_string(), io::J::Num(run.summary.min_ci_lo)),
            ]),
        ),
    ])
}
