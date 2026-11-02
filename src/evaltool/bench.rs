//! `xtask bench`：离线评测子命令（feature = "eval"）。
//!
//! 迁移自 `tools/bench.py` 的离线部分：
//! - `real`：real-dump 压缩指标矩阵（ctx × gate × scoped）；
//! - `perf`：压缩吞吐基线冻结；
//! - `budget`：同预算对照（--budget vs SuperCompress 官方口径）。
//!
//! 在线模型相关子命令（gsm8k/arch/code/kernel/long/fidelity）不在此迁移。
//! 解析/格式化分支是线性映射，认知复杂度门禁针对压缩内核，故本模块局部关闭。

#![allow(clippy::cognitive_complexity)]

use super::contain;
use super::io::{self, J};
use super::{fail, Fail};
use crate::json::Json;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

pub(crate) fn run(args: &[String]) -> Result<(), Fail> {
    let Some(command) = args.first().map(String::as_str) else {
        return Err(usage("missing bench subcommand"));
    };
    match command {
        "real" => real(&args[1..]),
        "perf" => perf(&args[1..]),
        "budget" => budget(&args[1..]),
        "ablate" => ablate(&args[1..]),
        "-h" | "--help" => {
            print!("{}", bench_usage());
            Ok(())
        }
        other => Err(usage(format!("unknown bench subcommand: {other}"))),
    }
}

fn bench_usage() -> String {
    "\
usage: xtask bench <command> [options]

commands:
  real    real-dump compression matrix
  perf    compression throughput baseline
  budget  same-budget comparison (--budget vs SuperCompress)
  ablate  signal ablation matrix (+ --combo semantic interaction)
"
    .to_string()
}

fn usage(message: impl Into<String>) -> Fail {
    Fail {
        msg: format!("{}\n\n{}", message.into(), bench_usage()),
        code: 2,
    }
}

fn value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|arg| arg == name)
}

fn default_binary() -> String {
    for name in ["scx.exe", "scx"] {
        let path = Path::new(ROOT).join("target/release").join(name);
        if path.is_file() {
            return path.to_string_lossy().to_string();
        }
    }
    "target/release/scx".to_string()
}

// ---------------------------------------------------------------------------
// metrics real
// ---------------------------------------------------------------------------

fn real(args: &[String]) -> Result<(), Fail> {
    let dumps = value(args, "--dumps").ok_or_else(|| usage("--dumps is required"))?;
    let binary = value(args, "--binary").ok_or_else(|| usage("--binary is required"))?;
    let query = value(args, "--query").unwrap_or("compress dedup evidence");
    let ratio = value(args, "--ratio").unwrap_or("0.35");
    let mut files: Vec<PathBuf> = fs::read_dir(dumps)
        .map_err(|e| fail(format!("{dumps}: {e}")))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.to_string_lossy().ends_with(".dump.txt"))
        .collect();
    files.sort();
    if files.is_empty() {
        return Err(fail(format!("no .dump.txt under {dumps}")));
    }
    println!(
        "{:12} {:>3} {:>4} {:>3} {:>6} {:>6} {:>6} {:>5} {:>5}",
        "dump", "ctx", "gate", "sc", "cut%", "kept", "seeds", "guard", "risk"
    );
    for path in files {
        let base = base_name(&path);
        for ctx in [0, 1] {
            for gate in [0, 1] {
                for sc in [0, 1] {
                    let data = scx_json(binary, &path, query, ctx, gate, ratio, sc);
                    let Some(data) = data else {
                        println!("{base} ctx={ctx} gate={gate} sc={sc}  ERROR: no scxjson");
                        continue;
                    };
                    let cut = data.get("cut_pct").and_then(Json::num).unwrap_or(0.0);
                    let kept = data.get("kept").and_then(as_i64).unwrap_or(0);
                    let seeds = data.get("seeds").and_then(as_i64).unwrap_or(0);
                    let guarded = data.get("guarded").and_then(as_i64).unwrap_or(0);
                    let risk = data.get("risk").and_then(Json::str).unwrap_or("");
                    println!(
                        "{base:12} {ctx:3} {gate:4} {sc:3} {cut:5.1}% {kept:6} {seeds:6} {guarded:5} {risk:>5}"
                    );
                }
            }
        }
    }
    Ok(())
}

fn base_name(path: &Path) -> String {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let name = name.strip_suffix(".dump.txt").unwrap_or(&name);
    name.chars().take(12).collect()
}

fn scx_json(
    binary: &str,
    path: &Path,
    query: &str,
    ctx: i32,
    gate: i32,
    ratio: &str,
    sc: i32,
) -> Option<Json> {
    let output = Command::new(binary)
        .args([
            path.to_str().unwrap_or_default(),
            "--query",
            query,
            "--context",
            &ctx.to_string(),
            "--risk-gate",
            &gate.to_string(),
            "--scoped",
            &sc.to_string(),
            "--ratio",
            ratio,
            "--json",
        ])
        .output()
        .ok()?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    for line in stderr.lines() {
        if let Some(text) = line.strip_prefix("scxjson:") {
            return crate::json::parse(text).ok();
        }
    }
    None
}

fn as_i64(value: &Json) -> Option<i64> {
    match value {
        Json::Num(n) if n.fract() == 0.0 => Some(*n as i64),
        Json::Str(s) => s.parse().ok(),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// metrics perf
// ---------------------------------------------------------------------------

const PERF_CORPORA: [&str; 3] = ["corpus_p1m.txt", "corpus_p10m.txt", "corpus_p30m.txt"];
const RATIOS: [(&str, &str); 3] = [("lossless", "1.0"), ("r0.3", "0.3"), ("r0.2", "0.2")];

fn perf(_args: &[String]) -> Result<(), Fail> {
    let binary = default_binary();
    let dir = Path::new(ROOT).join("config/bench/perf");
    let mut results: Vec<(String, PerfRow)> = Vec::new();
    for corpus in PERF_CORPORA {
        let path = dir.join(corpus);
        let size = fs::metadata(&path)
            .map_err(|e| fail(format!("{}: {e}", path.display())))?
            .len() as f64;
        let mut row = PerfRow {
            chars: size as u64,
            results: Vec::new(),
        };
        for (label, ratio) in RATIOS {
            let mut best = f64::INFINITY;
            for _ in 0..3 {
                best = best.min(run_once(&binary, &path, ratio));
            }
            let mbps = size / best / 1024.0 / 1024.0;
            row.results
                .push((label.to_string(), round(best, 4), round(mbps, 1)));
        }
        println!("{corpus} {}", row.display());
        results.push((corpus.to_string(), row));
    }
    let out = dir.join("perf_baseline.json");
    let value = J::Obj(vec![
        (
            "tool".to_string(),
            J::Str("tools/bench_metrics.py perf".to_string()),
        ),
        ("date".to_string(), J::Str("2026-10-29".to_string())),
        (
            "corpus_dir".to_string(),
            J::Str(dir.to_string_lossy().to_string()),
        ),
        (
            "results".to_string(),
            J::Obj(results.iter().map(|(k, v)| (k.clone(), v.json())).collect()),
        ),
    ]);
    io::write_json(&out.to_string_lossy(), &value).map_err(fail)?;
    println!("WROTE {}", out.display());
    Ok(())
}

struct PerfRow {
    chars: u64,
    results: Vec<(String, f64, f64)>,
}

impl PerfRow {
    fn display(&self) -> String {
        let parts: Vec<String> = self
            .results
            .iter()
            .map(|(label, best, mbps)| {
                format!(
                    "'{label}': {{'best_s': {}, 'mbps': {}}}",
                    io::repr(*best),
                    io::repr(*mbps)
                )
            })
            .collect();
        format!("{{'chars': {}, {}}}", self.chars, parts.join(", "))
    }

    fn json(&self) -> J {
        let mut entries = vec![("chars".to_string(), J::Int(self.chars as i64))];
        for (label, best, mbps) in &self.results {
            entries.push((
                label.clone(),
                J::Obj(vec![
                    ("best_s".to_string(), J::Num(*best)),
                    ("mbps".to_string(), J::Num(*mbps)),
                ]),
            ));
        }
        J::Obj(entries)
    }
}

fn round(value: f64, digits: usize) -> f64 {
    let text = format!("{value:.digits$}");
    text.parse().unwrap_or(value)
}

fn run_once(binary: &str, path: &Path, ratio: &str) -> f64 {
    let start = Instant::now();
    let _ = Command::new(binary)
        .args([
            "--query",
            "fn compress",
            "--ratio",
            ratio,
            "--trim",
            "1",
            "--trim-ratio",
            ratio,
            "--trim-op-keep",
            "1",
            "--num-line-keep",
            "1",
            path.to_str().unwrap_or_default(),
        ])
        .output();
    start.elapsed().as_secs_f64()
}

// ---------------------------------------------------------------------------
// budget
// ---------------------------------------------------------------------------

const OFFICIAL_CUT: f64 = 0.641;
const OFFICIAL_KEPT: f64 = 1.0 - OFFICIAL_CUT;

struct BudgetRow {
    set: String,
    cfg: String,
    budget: f64,
    cut: f64,
    kept_pct: f64,
    num_cov: f64,
    token_cov: f64,
}

fn budget(args: &[String]) -> Result<(), Fail> {
    let bin = value(args, "--bin").unwrap_or("target/release/scx");
    let gate = flag(args, "--gate");
    let out = value(args, "--out")
        .map(str::to_string)
        .unwrap_or_else(|| format!("{ROOT}/config/bench/results/bench_p18_budget.csv"));
    let budgets_text = value(args, "--budgets").unwrap_or("0.35,0.5,0.65");
    let mut budgets = Vec::new();
    for part in budgets_text.split(',') {
        if part.is_empty() {
            continue;
        }
        budgets.push(
            part.parse::<f64>()
                .map_err(|_| usage(format!("invalid --budgets value: {part}")))?,
        );
    }
    let corpora = [
        ("code", corpus_code(), "build"),
        ("log", corpus_log(), "timeout"),
        ("zh", corpus_zh(), "超时"),
    ];
    let mut rows = Vec::new();
    for (name, text, query) in &corpora {
        let base = run_budget(bin, name, text, query, None)?;
        print_row(&base);
        rows.push(base);
        for budget in &budgets {
            let row = run_budget(bin, name, text, query, Some(*budget))?;
            print_row(&row);
            if gate && row.kept_pct > budget + 0.02 {
                println!(
                    "  FAIL [{name}] budget-{budget:.2}: kept={:.4} 超预算",
                    row.kept_pct
                );
                return Err(Fail {
                    msg: String::new(),
                    code: 1,
                });
            }
            rows.push(row);
        }
    }
    rows.push(BudgetRow {
        set: "official".to_string(),
        cfg: "supercompress-v2".to_string(),
        budget: OFFICIAL_KEPT,
        cut: OFFICIAL_CUT,
        kept_pct: OFFICIAL_KEPT,
        num_cov: f64::NAN,
        token_cov: f64::NAN,
    });
    write_budget_csv(&out, &rows)?;
    println!(
        "写入 {out}（3 语料 × {} 档 + 官方对照 = {} 行）",
        1 + budgets.len(),
        rows.len()
    );
    if !gate {
        return Ok(());
    }
    let mut ok = true;
    for (name, _, _) in &corpora {
        let base = rows
            .iter()
            .find(|row| row.set == *name && row.cfg == "baseline")
            .expect("baseline row");
        for budget in &budgets {
            let cfg = format!("budget-{budget:.2}");
            let row = rows
                .iter()
                .find(|row| row.set == *name && row.cfg == cfg)
                .expect("budget row");
            if *budget <= 0.35 + 1e-9 && row.cut < OFFICIAL_CUT - 0.04 {
                println!(
                    "  FAIL [{name}] budget-{budget:.2}: cut={:.4} < 官方 0.641 − 0.04",
                    row.cut
                );
                ok = false;
            }
            if row.kept_pct >= base.kept_pct - 0.001 && (row.cut - base.cut).abs() > 0.001 {
                println!("  FAIL [{name}] budget-{budget:.2}: 预算未收紧但输出变化（cut 漂移）");
                ok = false;
            }
        }
    }
    println!(
        "  {}",
        if ok {
            "OK: 同预算对照门禁全部通过（预算精确达成 + 0.35 档 cut 不低于官方口径）"
        } else {
            "FAIL: 存在门禁失败项"
        }
    );
    if ok {
        Ok(())
    } else {
        Err(Fail {
            msg: String::new(),
            code: 1,
        })
    }
}

fn print_row(row: &BudgetRow) {
    if row.cfg == "baseline" {
        println!(
            "[{:4}] baseline    cut={:.4} kept={:.4} num_cov={:.4} token_cov={:.4}",
            row.set, row.cut, row.kept_pct, row.num_cov, row.token_cov
        );
    } else {
        println!(
            "[{:4}] {} cut={:.4} kept={:.4} num_cov={:.4} token_cov={:.4}",
            row.set, row.cfg, row.cut, row.kept_pct, row.num_cov, row.token_cov
        );
    }
}

fn write_budget_csv(path: &str, rows: &[BudgetRow]) -> Result<(), Fail> {
    let fields = vec![
        "set".to_string(),
        "cfg".to_string(),
        "budget".to_string(),
        "cut".to_string(),
        "kept_pct".to_string(),
        "num_cov".to_string(),
        "token_cov".to_string(),
    ];
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            vec![
                row.set.clone(),
                row.cfg.clone(),
                csv_float(row.budget),
                csv_float(row.cut),
                csv_float(row.kept_pct),
                csv_float(row.num_cov),
                csv_float(row.token_cov),
            ]
        })
        .collect();
    io::write_csv_with(path, &fields, &rows, "\r\n").map_err(fail)
}

fn csv_float(value: f64) -> String {
    if value.is_nan() {
        "nan".to_string()
    } else if value.is_infinite() {
        if value > 0.0 { "inf" } else { "-inf" }.to_string()
    } else {
        io::repr(value)
    }
}

fn run_budget(
    binary: &str,
    name: &str,
    text: &str,
    query: &str,
    budget: Option<f64>,
) -> Result<BudgetRow, Fail> {
    let mut extra = Vec::new();
    if let Some(budget) = budget {
        extra.push("--budget".to_string());
        extra.push(format!("{budget:.2}"));
    }
    let out = compress_text(binary, text, query, &extra)?;
    let fid_out = rebuild_folded(&out);
    let out_tok = tok_lines(&out);
    let full_tok = tok_lines(text);
    Ok(BudgetRow {
        set: name.to_string(),
        cfg: budget
            .map(|b| format!("budget-{b:.2}"))
            .unwrap_or_else(|| "baseline".to_string()),
        budget: budget.unwrap_or(0.0),
        cut: 1.0 - out_tok / full_tok,
        kept_pct: out_tok / full_tok,
        num_cov: cov(&num_tokens(text), &num_tokens(&fid_out)),
        token_cov: cov(&tokens(text), &tokens(&fid_out)),
    })
}

// ---------------------------------------------------------------------------
// shared benchmark helpers
// ---------------------------------------------------------------------------

pub(crate) fn compress_text(
    binary: &str,
    text: &str,
    query: &str,
    extra: &[String],
) -> Result<String, Fail> {
    let path =
        std::env::temp_dir().join(format!("scx-bench-{}-{}.txt", std::process::id(), nanos()));
    fs::write(&path, text).map_err(|e| fail(format!("{}: {e}", path.display())))?;
    let mut args = vec![
        path.to_string_lossy().to_string(),
        "--query".to_string(),
        query.to_string(),
        "--ratio".to_string(),
        "0.35".to_string(),
        "--context".to_string(),
        "1".to_string(),
    ];
    args.extend_from_slice(extra);
    let output = Command::new(binary)
        .args(&args)
        .output()
        .map_err(|e| fail(format!("{binary}: {e}")))?;
    let _ = fs::remove_file(&path);
    if !output.status.success() {
        return Ok(text.to_string());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        if line.trim_start().starts_with("compressed:") {
            let out = lines[..index].join("\n");
            return Ok(if out.trim().is_empty() {
                text.to_string()
            } else {
                out
            });
        }
    }
    Ok(text.to_string())
}

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn rebuild_folded(out: &str) -> String {
    let lines: Vec<&str> = out.lines().collect();
    let mut rebuilt: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.starts_with("scx-fold: ") && i >= 1 && i + 1 < lines.len() {
            let first = if rebuilt.last().is_some_and(|last| !last.is_empty()) {
                rebuilt.pop().unwrap_or_default()
            } else {
                lines[i - 1].to_string()
            };
            let params = lines[i + 1];
            if params.starts_with("scx-fold-params: ") {
                if let Some(rows) = crate::fold::rebuild(line, params) {
                    rebuilt.push(first);
                    rebuilt.extend(rows);
                    i += 2;
                    continue;
                }
            }
            rebuilt.push(first);
            rebuilt.push(line.to_string());
        } else {
            rebuilt.push(line.to_string());
        }
        i += 1;
    }
    let mut text = rebuilt.join("\n");
    if out.ends_with('\n') {
        text.push('\n');
    }
    text
}

fn tok_lines(text: &str) -> f64 {
    // Python 3.12 的 sum() 对浮点使用 Neumaier 补偿求和。
    contain::compensated(text.lines().map(crate::terms::estimate))
}

pub(crate) fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut ascii = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            ascii.push(c);
            continue;
        }
        if !ascii.is_empty() {
            out.push(std::mem::take(&mut ascii));
        }
        if ideograph(c) {
            out.push(c.to_string());
        }
    }
    if !ascii.is_empty() {
        out.push(ascii);
    }
    out
}

fn ideograph(c: char) -> bool {
    let u = c as u32;
    (0x3400..=0x4DBF).contains(&u)
        || (0x4E00..=0x9FFF).contains(&u)
        || (0xF900..=0xFAFF).contains(&u)
}

pub(crate) fn num_tokens(text: &str) -> Vec<String> {
    tokens(text)
        .into_iter()
        .filter(|token| token.chars().any(|c| c.is_ascii_digit()))
        .collect()
}

pub(crate) fn cov(full: &[String], out: &[String]) -> f64 {
    if full.is_empty() {
        return 1.0;
    }
    let mut pool: Vec<&str> = out.iter().map(String::as_str).collect();
    let mut hit = 0usize;
    for token in full {
        if let Some(index) = pool.iter().position(|candidate| *candidate == token) {
            pool[index] = "";
            hit += 1;
        }
    }
    hit as f64 / full.len() as f64
}

// ---------------------------------------------------------------------------
// deterministic corpora (LCG seed 42, mirrors tools/bench.py)
// ---------------------------------------------------------------------------

struct Lcg(u32);

impl Lcg {
    fn new(seed: u32) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u32 {
        // Python 用任意精度整数后掩码 31 位；必须用 u64 保留乘积高位。
        self.0 = ((self.0 as u64 * 1_103_515_245 + 12_345) & 0x7FFF_FFFF) as u32;
        self.0
    }

    fn choice<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.next() as usize % items.len()]
    }
}

pub(crate) fn corpus_code() -> String {
    let mut rng = Lcg::new(42);
    let kinds = ["u64", "i32", "String", "Vec<u8>", "bool", "f64"];
    let verbs = [
        "parse", "build", "merge", "scan", "render", "flush", "retry", "dedup",
    ];
    let mut lines = Vec::new();
    for i in 0..300 {
        let verb = rng.choice(&verbs);
        let kind = rng.choice(&kinds);
        lines.push(format!(
            "pub fn {verb}_{i}(arg: u64) -> Result<{kind}, Error> {{"
        ));
        lines.push(format!("    let tmp = arg.wrapping_mul({i} + 1);"));
        lines.push(format!(
            "    if tmp % 2 == 0 {{ return Ok(tmp) }} else {{ Err(Error::Idx({i})) }}"
        ));
        lines.push("}".to_string());
    }
    lines.push("// unique tail: final answer is 42".to_string());
    lines.join("\n") + "\n"
}

pub(crate) fn corpus_log() -> String {
    let mut rng = Lcg::new(42);
    let levels = ["ERROR", "WARN"];
    let comps = ["conn", "auth", "cache", "rpc", "mq", "disk"];
    let states = ["timeout", "retry", "reset", "refused", "slow", "evicted"];
    let mut lines = Vec::new();
    for b in 0..160 {
        let level = rng.choice(&levels);
        let comp = rng.choice(&comps);
        let state = rng.choice(&states);
        let base = format!(
            "2026-10-30 12:{:02}:{:02}.{:03} {level} {comp}.{state}",
            b % 60,
            b % 60,
            b % 1000
        );
        for k in 0..5 {
            lines.push(format!(
                "{base} id={:04x} took={}ms retry={k}",
                b * 5 + k,
                b % 500 + k * 7
            ));
        }
        lines.push(format!("  at worker.retry(worker.rs:{})", b % 120 + 1));
        lines.push(format!(
            "  {{\"err\": \"{state}\", \"id\": \"{:04x}\", \"took\": {}}}",
            b * 5,
            b % 500
        ));
    }
    lines.push("final: status=ok attempts=0".to_string());
    lines.join("\n") + "\n"
}

pub(crate) fn corpus_zh() -> String {
    let mut rng = Lcg::new(42);
    let msgs = [
        "连接超时",
        "缓存未命中",
        "权限不足",
        "参数校验失败",
        "服务不可用",
        "重试次数超限",
    ];
    let comps = ["网关", "认证中心", "订单服务", "存储引擎", "调度器"];
    let mut lines = Vec::new();
    for i in 0..300 {
        let comp = rng.choice(&comps);
        let msg = rng.choice(&msgs);
        lines.push(format!(
            "[{:02}] {comp} {msg}：请求 {i} 重试 {} 次",
            i % 100,
            i % 5
        ));
        lines.push(format!(
            "  详情：code={} id={i:04x} 耗时 {}ms",
            1000 + i,
            i % 900
        ));
    }
    lines.push("最终状态：成功 总耗时 3200ms".to_string());
    lines.join("\n") + "\n"
}

// ---------------------------------------------------------------------------
// ablate
// ---------------------------------------------------------------------------

const CUT_TOL: f64 = 0.005;
const FID_TOL: f64 = 0.01;

fn configs() -> Vec<(&'static str, Vec<String>, bool)> {
    vec![
        ("baseline", vec![], false),
        ("fold", vec!["--fold".into(), "1".into()], false),
        ("entropy", vec!["--entropy".into(), "1".into()], false),
        ("anchor", vec!["--auto-anchor".into(), "1".into()], false),
        ("cluster", vec!["--cluster".into(), "1".into()], false),
        ("semantic", Vec::new(), true),
    ]
}

fn combos(weights: &str) -> Vec<(&'static str, Vec<String>)> {
    vec![
        (
            "sem+fold",
            vec![
                "--fold".into(),
                "1".into(),
                "--semantic".into(),
                weights.into(),
            ],
        ),
        (
            "sem+entropy",
            vec![
                "--entropy".into(),
                "1".into(),
                "--semantic".into(),
                weights.into(),
            ],
        ),
        (
            "sem+anchor",
            vec![
                "--auto-anchor".into(),
                "1".into(),
                "--semantic".into(),
                weights.into(),
            ],
        ),
        (
            "sem+cluster",
            vec![
                "--cluster".into(),
                "1".into(),
                "--semantic".into(),
                weights.into(),
            ],
        ),
    ]
}

struct AblateRow {
    set: String,
    cfg: String,
    cut: f64,
    num_cov: f64,
    token_cov: f64,
}

fn ablate(args: &[String]) -> Result<(), Fail> {
    let default_bin = value(args, "--bin")
        .map(str::to_string)
        .unwrap_or_else(default_binary);
    let sem_bin = value(args, "--sem-bin")
        .map(str::to_string)
        .unwrap_or_else(|| format!("{ROOT}/target/ablate-sem/release/scx"));
    let gate = flag(args, "--gate");
    let combo = flag(args, "--combo");
    let weights = format!("{ROOT}/config/semantic/weights.json");
    let corpora = [
        ("code", corpus_code(), "build"),
        ("log", corpus_log(), "timeout"),
        ("zh", corpus_zh(), "超时"),
    ];
    if !Path::new(&sem_bin).is_file() {
        return Err(fail(format!(
            "ablate: missing semantic binary: {sem_bin}\n\
             build it with: cargo build --release --features semantic --target-dir target/ablate-sem"
        )));
    }
    if combo {
        return ablate_combo(&default_bin, &sem_bin, &corpora, &weights, gate);
    }
    let configs = configs();
    let mut rows = Vec::new();
    for (name, text, query) in &corpora {
        let full_tok = crate::terms::estimate(text);
        let full_words = tokens(text);
        let full_nums = num_tokens(text);
        for (cfg_name, flags, need_sem) in &configs {
            let binary = if *need_sem { &sem_bin } else { &default_bin };
            let mut extra = flags.clone();
            if *need_sem {
                extra.extend(["--semantic".to_string(), weights.clone()]);
            }
            let out = compress_text(binary, text, query, &extra)?;
            let fid_out = if *cfg_name == "fold" {
                rebuild_folded(&out)
            } else {
                out.clone()
            };
            let row = AblateRow {
                set: name.to_string(),
                cfg: cfg_name.to_string(),
                cut: 1.0 - crate::terms::estimate(&out) / full_tok,
                num_cov: cov(&full_nums, &num_tokens(&fid_out)),
                token_cov: cov(&full_words, &tokens(&fid_out)),
            };
            println!(
                "[{name:4}] {cfg_name:<9} cut={:.4} num_cov={:.4} token_cov={:.4}",
                row.cut, row.num_cov, row.token_cov
            );
            rows.push(row);
        }
    }
    let out = value(args, "--out")
        .map(str::to_string)
        .unwrap_or_else(|| format!("{ROOT}/config/bench/results/bench_p17_ablate.csv"));
    write_ablate_csv(&out, &rows)?;
    println!("写入 {out}（3 语料 × 6 配置 = {} 行）", rows.len());
    if !gate {
        return Ok(());
    }
    let mut ok = true;
    for (name, _, _) in &corpora {
        let base = rows
            .iter()
            .find(|row| row.set == *name && row.cfg == "baseline")
            .expect("baseline row");
        for (cfg_name, _, _) in &configs {
            if *cfg_name == "baseline" {
                continue;
            }
            let row = rows
                .iter()
                .find(|row| row.set == *name && row.cfg == *cfg_name)
                .expect("config row");
            for metric in ["num_cov", "token_cov"] {
                let (got, want) = if metric == "num_cov" {
                    (row.num_cov, base.num_cov)
                } else {
                    (row.token_cov, base.token_cov)
                };
                if got < want - FID_TOL {
                    println!(
                        "  FAIL [{name}] {cfg_name}: {metric} {got:.4} < 基线 {want:.4} - {FID_TOL}"
                    );
                    ok = false;
                }
            }
            if *cfg_name != "entropy" && row.cut < base.cut - CUT_TOL {
                println!(
                    "  FAIL [{name}] {cfg_name}: cut {:.4} < 基线 {:.4} - {CUT_TOL}",
                    row.cut, base.cut
                );
                ok = false;
            }
        }
    }
    println!(
        "  {}",
        if ok {
            "OK: 消融单调性门禁全部通过（单开不劣于基线）"
        } else {
            "FAIL: 存在劣化信号"
        }
    );
    if ok {
        Ok(())
    } else {
        Err(Fail {
            msg: String::new(),
            code: 1,
        })
    }
}

fn ablate_combo(
    default_bin: &str,
    sem_bin: &str,
    corpora: &[(&str, String, &str)],
    weights: &str,
    gate: bool,
) -> Result<(), Fail> {
    let off_configs: Vec<_> = configs()
        .into_iter()
        .filter(|(name, _, _)| *name != "semantic")
        .collect();
    let combos = combos(weights);
    let mut rows = Vec::new();
    for (name, text, query) in corpora {
        let full_tok = crate::terms::estimate(text);
        let full_words = tokens(text);
        let full_nums = num_tokens(text);
        for (cfg_name, flags, need_sem) in &off_configs {
            let binary = if *need_sem { sem_bin } else { default_bin };
            let out = compress_text(binary, text, query, flags)?;
            let fid_out = if *cfg_name == "fold" {
                rebuild_folded(&out)
            } else {
                out.clone()
            };
            let row = AblateRow {
                set: name.to_string(),
                cfg: cfg_name.to_string(),
                cut: 1.0 - crate::terms::estimate(&out) / full_tok,
                num_cov: cov(&full_nums, &num_tokens(&fid_out)),
                token_cov: cov(&full_words, &tokens(&fid_out)),
            };
            println!(
                "[combo off {name:4}] {cfg_name:<12} cut={:.4} num_cov={:.4} token_cov={:.4}",
                row.cut, row.num_cov, row.token_cov
            );
            rows.push(row);
        }
    }
    let mut on_configs = vec![(
        "semantic",
        vec!["--semantic".to_string(), weights.to_string()],
    )];
    on_configs.extend(combos.iter().map(|(name, flags)| (*name, flags.clone())));
    for (name, text, query) in corpora {
        let full_tok = crate::terms::estimate(text);
        let full_words = tokens(text);
        let full_nums = num_tokens(text);
        for (cfg_name, flags) in &on_configs {
            let out = compress_text(sem_bin, text, query, flags)?;
            let fid_out = if cfg_name.contains("fold") {
                rebuild_folded(&out)
            } else {
                out.clone()
            };
            let row = AblateRow {
                set: name.to_string(),
                cfg: cfg_name.to_string(),
                cut: 1.0 - crate::terms::estimate(&out) / full_tok,
                num_cov: cov(&full_nums, &num_tokens(&fid_out)),
                token_cov: cov(&full_words, &tokens(&fid_out)),
            };
            println!(
                "[combo on {name:4}] {cfg_name:<12} cut={:.4} num_cov={:.4} token_cov={:.4}",
                row.cut, row.num_cov, row.token_cov
            );
            rows.push(row);
        }
    }
    let out = format!("{ROOT}/config/bench/results/bench_p18_combo.csv");
    write_ablate_csv(&out, &rows)?;
    println!("写入 {out}（3 语料 × 5 配置 = {} 行）", rows.len());
    if !gate {
        return Ok(());
    }
    let mut ok = true;
    for (name, _, _) in corpora {
        for (cfg_name, _) in &combos {
            let off_name = &cfg_name[4..];
            let off = rows
                .iter()
                .find(|row| row.set == *name && row.cfg == off_name)
                .expect("off row");
            let on = rows
                .iter()
                .find(|row| row.set == *name && row.cfg == *cfg_name)
                .expect("on row");
            for metric in ["num_cov", "token_cov"] {
                let (got, want) = if metric == "num_cov" {
                    (on.num_cov, off.num_cov)
                } else {
                    (on.token_cov, off.token_cov)
                };
                if got < want - FID_TOL {
                    println!(
                        "  FAIL [{name}] {cfg_name}: {metric} {got:.4} < off {want:.4} - {FID_TOL}"
                    );
                    ok = false;
                }
            }
            if !cfg_name.contains("entropy") && on.cut < off.cut - CUT_TOL {
                println!(
                    "  FAIL [{name}] {cfg_name}: cut {:.4} < off {:.4} - {CUT_TOL}",
                    on.cut, off.cut
                );
                ok = false;
            }
        }
    }
    println!(
        "  {}",
        if ok {
            "OK: 组合门禁全过（semantic × 各信号交互正交，fidelity/cut 不劣化）"
        } else {
            "FAIL: 存在 semantic 组合劣化"
        }
    );
    if ok {
        Ok(())
    } else {
        Err(Fail {
            msg: String::new(),
            code: 1,
        })
    }
}

fn write_ablate_csv(path: &str, rows: &[AblateRow]) -> Result<(), Fail> {
    let fields = vec![
        "set".to_string(),
        "cfg".to_string(),
        "cut".to_string(),
        "num_cov".to_string(),
        "token_cov".to_string(),
    ];
    let values: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            vec![
                row.set.clone(),
                row.cfg.clone(),
                csv_float(row.cut),
                csv_float(row.num_cov),
                csv_float(row.token_cov),
            ]
        })
        .collect();
    io::write_csv_with(path, &fields, &values, "\r\n").map_err(fail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpora_match_python_fixtures() {
        // 参考值由 python3.12 tools/bench.py 的生成器输出冻结（LCG seed=42）。
        assert_eq!(
            sha(&corpus_code()),
            "2f36cf0186c23e25d8d579f3a57306f438644d31576eb10eeaf0cb8726872e4a"
        );
        assert_eq!(
            sha(&corpus_log()),
            "2aa4d8c6f597cd8645b4e689f067af090c8bbc77ff988b17f9c2fb65ad39e4a4"
        );
        assert_eq!(
            sha(&corpus_zh()),
            "8556b049df80618633a86beee80190961384b4bb08fefd3fa0dede99f6a51b87"
        );
        assert_eq!(corpus_code().lines().count(), 1201);
        assert_eq!(corpus_log().lines().count(), 1121);
        assert_eq!(corpus_zh().lines().count(), 601);
    }

    fn sha(text: &str) -> String {
        crate::hash::hex(&crate::hash::sha256(text.as_bytes()))
    }

    #[test]
    fn cov_is_greedy_multiset() {
        let full = vec!["a".to_string(), "b".to_string(), "a".to_string()];
        let out = vec!["a".to_string(), "a".to_string()];
        assert_eq!(cov(&full, &out), 2.0 / 3.0);
        assert_eq!(cov(&[], &out), 1.0);
    }

    #[test]
    fn token_rules_match_python() {
        assert_eq!(tokens("a.b c_1 数据"), vec!["a", "b", "c_1", "数", "据"]);
        assert_eq!(num_tokens("a1 b 数据"), vec!["a1"]);
        assert_eq!(tok_lines("abcd\n"), 2.0);
    }
}
