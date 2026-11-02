//! `xtask contain`：evidence containment 协议编排。
//!
//! 迁移自原 Python 工具的 `contain`：逐切片调用 `scx` 二进制，和头截断/随机行
//! 采样在同一字节预算下对照，输出逐切片表、聚合指标与可选 JSON。
//! 判定复用 [`scx::contain::judge`]；随机基线复刻 CPython 字符串 seed +
//! `shuffle`，保证与 Python 版确定性字段逐字节一致（wall 时间除外）。

use super::io::{self, J};
use super::rng::PyRng;
use super::{fail, Fail};
use scx::contain;
use scx::json::Json;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

const KINDS: [&str; 5] = ["build", "stack", "diff", "log", "test"];

pub(crate) struct Slice {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) query: String,
    pub(crate) full_text: String,
    pub(crate) evidence: Vec<String>,
}

pub(crate) struct Stats {
    pub(crate) mode: String,
    pub(crate) risk: String,
    pub(crate) kept: Option<i64>,
    pub(crate) total: Option<i64>,
    pub(crate) seeds: Option<i64>,
    pub(crate) evidence: Option<i64>,
}

struct Row {
    id: String,
    kind: String,
    query: String,
    mode: String,
    risk: String,
    kept: Option<i64>,
    total: Option<i64>,
    seeds: Option<i64>,
    evidence_seeds: Option<i64>,
    chars_in: usize,
    chars_out: usize,
    chars_out_rand: usize,
    token_in: usize,
    token_out: usize,
    char_cut: f64,
    char_cut_rand: f64,
    token_cut: f64,
    ev_total: usize,
    ev_kept: usize,
    ev_missed: Vec<String>,
    trunc_ev_kept: usize,
    rand_ev_kept: usize,
    trunc_budget_chars: usize,
    wall_scx_ms: Vec<f64>,
    wall_trunc_ms: Vec<f64>,
    wall_rand_ms: Vec<f64>,
}

struct Opts {
    corpus: String,
    binary: Option<String>,
    ratio: f64,
    context: i64,
    out: Option<String>,
    seed: String,
    rounds: usize,
}

pub(crate) fn run(args: &[String]) -> Result<(), Fail> {
    let opts = parse(args)?;
    let (slices, kind_counts) = load_corpus(&opts.corpus)?;
    let binary = match opts.binary.clone().or_else(default_binary) {
        Some(path) if Path::new(&path).is_file() => path,
        _ => {
            return Err(fail(
                "contain: missing release binary; run `cargo build --release --locked` first",
            ))
        }
    };
    let work = TempDir::new();
    let mut rows = Vec::with_capacity(slices.len());
    for slice in &slices {
        rows.push(run_slice(
            &binary,
            slice,
            opts.ratio,
            opts.context,
            work.path(),
            &opts.seed,
            opts.rounds,
        )?);
    }
    let agg = aggregate(&rows);
    print(&rows, &agg, &opts, &kind_counts, &binary);
    if let Some(path) = &opts.out {
        io::write_json(path, &json(&rows, &agg, &opts, &kind_counts, &binary)).map_err(fail)?;
        println!("contain: wrote {path}");
    }
    Ok(())
}

fn parse(args: &[String]) -> Result<Opts, Fail> {
    let mut opts = Opts {
        corpus: String::new(),
        binary: None,
        ratio: 0.35,
        context: 1,
        out: None,
        seed: "contain-rand-v1".to_string(),
        rounds: 5,
    };
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--binary" | "--ratio" | "--context" | "--out" | "--seed" | "--timing-rounds" => {
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| usage(format!("{arg} expects a value")))?;
                match arg.as_str() {
                    "--binary" => opts.binary = Some(value.clone()),
                    "--ratio" => opts.ratio = number(value, arg)?,
                    "--context" => opts.context = number(value, arg)?,
                    "--out" => opts.out = Some(value.clone()),
                    "--seed" => opts.seed = value.clone(),
                    "--timing-rounds" => opts.rounds = number(value, arg)?,
                    _ => return Err(usage(format!("unrecognized argument: {arg}"))),
                }
            }
            "-h" | "--help" => return Err(usage(String::new())),
            _ if arg.starts_with('-') => {
                return Err(usage(format!("unrecognized argument: {arg}")))
            }
            _ if opts.corpus.is_empty() => opts.corpus = arg.clone(),
            _ => return Err(usage(format!("unexpected argument: {arg}"))),
        }
        i += 1;
    }
    if opts.corpus.is_empty() {
        return Err(usage(
            "the following arguments are required: corpus".to_string(),
        ));
    }
    if opts.rounds < 1 {
        return Err(usage("--timing-rounds must be positive".to_string()));
    }
    Ok(opts)
}

fn number<T: std::str::FromStr>(text: &str, name: &str) -> Result<T, Fail> {
    text.parse()
        .map_err(|_| usage(format!("invalid value for {name}: {text}")))
}

fn usage(message: String) -> Fail {
    let text = if message.is_empty() {
        contain_usage()
    } else {
        format!("{message}\n\n{}", contain_usage())
    };
    Fail { msg: text, code: 2 }
}

fn contain_usage() -> String {
    "\
usage: xtask contain <corpus> [options]

  --binary PATH        scx binary (default: target/release/scx)
  --ratio F            optional-content budget [0.35]
  --context N          minimum neighborhood [1]
  --out PATH           write per-slice detail JSON
  --seed TEXT          random-sampling seed [contain-rand-v1]
  --timing-rounds N    repeat each method per slice [5]
"
    .to_string()
}

fn load_corpus(path: &str) -> Result<(Vec<Slice>, Vec<usize>), Fail> {
    let source = fs::read_to_string(path).map_err(|e| fail(format!("{path}: {e}")))?;
    let value = scx::json::parse(&source).map_err(|e| fail(format!("{path}: {e}")))?;
    let items = match value.get("slices") {
        Some(Json::Arr(items)) => items,
        _ => return Err(fail("contain: corpus missing \"slices\" array")),
    };
    if !(20..=30).contains(&items.len()) {
        return Err(fail(format!(
            "contain: slices={} must be 20..30",
            items.len()
        )));
    }
    let mut counts = vec![0usize; KINDS.len()];
    let mut seen = HashSet::new();
    let mut slices = Vec::with_capacity(items.len());
    for item in items {
        let ident = string(item, "id");
        let kind = string(item, "kind");
        let query = string(item, "query");
        let full_text = raw(item, "full_text");
        let note = string(item, "note");
        let evidence = strings(item, "evidence");
        if ident.is_empty() || !seen.insert(ident.clone()) {
            return Err(fail(format!("contain: bad/duplicate slice id: {ident:?}")));
        }
        let Some(kind_index) = KINDS.iter().position(|k| *k == kind) else {
            return Err(fail(format!("contain: slice {ident} bad kind {kind:?}")));
        };
        if query.is_empty() || full_text.is_empty() || evidence.is_empty() {
            return Err(fail(format!(
                "contain: slice {ident} empty query/text/evidence"
            )));
        }
        if note.is_empty() {
            return Err(fail(format!("contain: slice {ident} missing note")));
        }
        for ev in &evidence {
            if !full_text.contains(ev) {
                return Err(fail(format!(
                    "contain: slice {ident} evidence line not verbatim in full_text:\n{ev:?}"
                )));
            }
        }
        counts[kind_index] += 1;
        slices.push(Slice {
            id: ident,
            kind,
            query,
            full_text,
            evidence,
        });
    }
    let missing: Vec<&str> = KINDS
        .iter()
        .zip(&counts)
        .filter(|(_, count)| **count == 0)
        .map(|(kind, _)| *kind)
        .collect();
    if !missing.is_empty() {
        return Err(fail(format!(
            "contain: corpus missing kinds: {}",
            list(&missing)
        )));
    }
    Ok((slices, counts))
}

pub(crate) fn string(value: &Json, key: &str) -> String {
    value
        .get(key)
        .and_then(Json::str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// 原文/证据行不做 trim：判定要求逐字节一致。
pub(crate) fn raw(value: &Json, key: &str) -> String {
    value
        .get(key)
        .and_then(Json::str)
        .unwrap_or_default()
        .to_string()
}

fn strings(value: &Json, key: &str) -> Vec<String> {
    match value.get(key) {
        Some(Json::Arr(items)) => items
            .iter()
            .filter_map(Json::str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn run_slice(
    binary: &str,
    slice: &Slice,
    ratio: f64,
    context: i64,
    work: &Path,
    seed: &str,
    rounds: usize,
) -> Result<Row, Fail> {
    let rounds = rounds.max(1);
    let mut stats = None;
    let mut body = String::new();
    let mut wall_scx = Vec::with_capacity(rounds);
    for _ in 0..rounds {
        let start = Instant::now();
        let (next_stats, next_body) = invoke(binary, slice, ratio, context, work)?;
        wall_scx.push(start.elapsed().as_secs_f64() * 1000.0);
        stats = Some(next_stats);
        body = next_body;
    }
    let stats = stats.expect("rounds >= 1");
    let budget = body.len();
    let mut trunc_text = String::new();
    let mut wall_trunc = Vec::with_capacity(rounds);
    for _ in 0..rounds {
        let start = Instant::now();
        trunc_text = trunc(&slice.full_text, budget);
        wall_trunc.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let mut rand_text = String::new();
    let mut wall_rand = Vec::with_capacity(rounds);
    for _ in 0..rounds {
        let start = Instant::now();
        rand_text = random_sample(&slice.full_text, budget, seed, &slice.id);
        wall_rand.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let evidence: Vec<&str> = slice.evidence.iter().map(String::as_str).collect();
    let (ev_kept, ev_missed) = contain::judge(&body, &evidence);
    let trunc_ev_kept = evidence
        .iter()
        .filter(|ev| trunc_text.contains(**ev))
        .count();
    let rand_ev_kept = evidence
        .iter()
        .filter(|ev| rand_text.contains(**ev))
        .count();
    let in_chars = slice.full_text.chars().count();
    let out_chars = body.chars().count();
    let in_tokens = token_count(&slice.full_text);
    let out_tokens = token_count(&body);
    Ok(Row {
        id: slice.id.clone(),
        kind: slice.kind.clone(),
        query: slice.query.clone(),
        mode: stats.mode,
        risk: stats.risk,
        kept: stats.kept,
        total: stats.total,
        seeds: stats.seeds,
        evidence_seeds: stats.evidence,
        chars_in: in_chars,
        chars_out: out_chars,
        chars_out_rand: rand_text.chars().count(),
        token_in: in_tokens,
        token_out: out_tokens,
        char_cut: cut(out_chars, in_chars),
        char_cut_rand: cut(rand_text.chars().count(), in_chars),
        token_cut: cut(out_tokens, in_tokens),
        ev_total: evidence.len(),
        ev_kept,
        ev_missed,
        trunc_ev_kept,
        rand_ev_kept,
        trunc_budget_chars: budget,
        wall_scx_ms: wall_scx,
        wall_trunc_ms: wall_trunc,
        wall_rand_ms: wall_rand,
    })
}

pub(crate) fn cut(out: usize, input: usize) -> f64 {
    if input == 0 {
        0.0
    } else {
        1.0 - out as f64 / input as f64
    }
}

pub(crate) fn invoke(
    binary: &str,
    slice: &Slice,
    ratio: f64,
    context: i64,
    work: &Path,
) -> Result<(Stats, String), Fail> {
    let path = work.join(format!("{}.txt", slice.id));
    fs::write(&path, &slice.full_text).map_err(|e| fail(format!("{}: {e}", path.display())))?;
    let ratio = format!("{ratio}");
    let context = context.to_string();
    let output = Command::new(binary)
        .args([
            "--query",
            &slice.query,
            "--ratio",
            &ratio,
            "--context",
            &context,
            "--json",
            path.to_str().unwrap_or_default(),
        ])
        .output()
        .map_err(|e| fail(format!("contain: failed to run {binary}: {e}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(fail(format!(
            "contain: scx failed for {} rc={}\n{stdout}{stderr}",
            slice.id,
            output.status.code().unwrap_or(-1)
        )));
    }
    let stats = parse_stats(&stderr).ok_or_else(|| {
        fail(format!(
            "contain: no scxjson line for {}\n{stderr}",
            slice.id
        ))
    })?;
    let body: Vec<&str> = stdout
        .lines()
        .filter(|line| !line.starts_with("compressed:"))
        .collect();
    let mut body = body.join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    Ok((stats, body))
}

fn parse_stats(stderr: &str) -> Option<Stats> {
    for line in stderr.lines() {
        let Some(text) = line.strip_prefix("scxjson: ") else {
            continue;
        };
        let value = scx::json::parse(text).ok()?;
        return Some(Stats {
            mode: value
                .get("mode")
                .and_then(Json::str)
                .unwrap_or("")
                .to_string(),
            risk: value
                .get("risk")
                .and_then(Json::str)
                .unwrap_or("")
                .to_string(),
            kept: value.get("kept").and_then(as_i64),
            total: value.get("total").and_then(as_i64),
            seeds: value.get("seeds").and_then(as_i64),
            evidence: value.get("evidence").and_then(as_i64),
        });
    }
    None
}

pub(crate) fn as_i64(value: &Json) -> Option<i64> {
    match value {
        Json::Num(n) if n.fract() == 0.0 => Some(*n as i64),
        Json::Str(s) => s.parse().ok(),
        _ => None,
    }
}

pub(crate) fn token_count(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut count = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if word_start(bytes[i]) {
            count += 1;
            i += 1;
            while i < bytes.len() && word_part(bytes[i]) {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    count + text.chars().filter(|c| cjk(*c)).count()
}

fn word_start(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
}

fn word_part(byte: u8) -> bool {
    word_start(byte) || matches!(byte, b'_' | b'.' | b'-')
}

fn cjk(c: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&c)
}

pub(crate) fn trunc(text: &str, budget: usize) -> String {
    let bytes = text.as_bytes();
    let end = budget.min(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

pub(crate) fn random_sample(text: &str, budget: usize, seed: &str, ident: &str) -> String {
    let chunks = splitlines(text);
    let mut order: Vec<usize> = (0..chunks.len()).collect();
    PyRng::from_seed_bytes(format!("{seed}:{ident}").as_bytes()).shuffle(&mut order);
    let (kept, partial) = select(&chunks, budget, order);
    let mut out = Vec::with_capacity(budget);
    for (index, chunk) in chunks.iter().enumerate() {
        if kept[index] {
            out.extend_from_slice(chunk.as_bytes());
        } else if let Some(limit) = partial[index] {
            out.extend_from_slice(&chunk.as_bytes()[..limit]);
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 按 shuffle 顺序贪心装入整行；下一行装不下时保留其字节前缀并停止。
fn select(chunks: &[&str], budget: usize, order: Vec<usize>) -> (Vec<bool>, Vec<Option<usize>>) {
    let mut kept = vec![false; chunks.len()];
    let mut partial = vec![None; chunks.len()];
    let mut used = 0usize;
    for index in order {
        let size = chunks[index].len();
        if used + size <= budget {
            kept[index] = true;
            used += size;
        } else {
            let slack = budget - used;
            if slack > 0 {
                partial[index] = Some(slack);
            }
            break;
        }
    }
    (kept, partial)
}

pub(crate) fn splitlines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if !line_boundary(c) {
            continue;
        }
        let mut end = i + c.len_utf8();
        if c == '\r' {
            if let Some(&(next, '\n')) = chars.peek() {
                chars.next();
                end = next + 1;
            }
        }
        out.push(&text[start..end]);
        start = end;
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

fn line_boundary(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r'
            | '\u{0b}'
            | '\u{0c}'
            | '\u{1c}'
            | '\u{1d}'
            | '\u{1e}'
            | '\u{85}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

struct Agg {
    n: usize,
    ev_sum: usize,
    ev_kept: usize,
    trunc_kept: usize,
    rand_kept: usize,
    char_cut: f64,
    char_cut_rand: f64,
    token_cut: f64,
    lat_scx: f64,
    lat_trunc: f64,
    lat_rand: f64,
    modes: Vec<(String, usize)>,
    kinds: BTreeMap<String, KindAgg>,
}

struct KindAgg {
    count: usize,
    ev_kept: usize,
    ev_total: usize,
    rand_ev: usize,
    char_cut: f64,
    token_cut: f64,
}

fn aggregate(rows: &[Row]) -> Agg {
    let n = rows.len();
    let ev_sum: usize = rows.iter().map(|row| row.ev_total).sum();
    let ev_kept: usize = rows.iter().map(|row| row.ev_kept).sum();
    let trunc_kept: usize = rows.iter().map(|row| row.trunc_ev_kept).sum();
    let rand_kept: usize = rows.iter().map(|row| row.rand_ev_kept).sum();
    let mut modes: Vec<(String, usize)> = Vec::new();
    let mut kinds: BTreeMap<String, KindAgg> = BTreeMap::new();
    for row in rows {
        match modes.iter_mut().find(|(mode, _)| *mode == row.mode) {
            Some((_, count)) => *count += 1,
            None => modes.push((row.mode.clone(), 1)),
        }
        let entry = kinds.entry(row.kind.clone()).or_insert(KindAgg {
            count: 0,
            ev_kept: 0,
            ev_total: 0,
            rand_ev: 0,
            char_cut: 0.0,
            token_cut: 0.0,
        });
        entry.count += 1;
        entry.ev_kept += row.ev_kept;
        entry.ev_total += row.ev_total;
        entry.rand_ev += row.rand_ev_kept;
        entry.char_cut += row.char_cut;
        entry.token_cut += row.token_cut;
    }
    for entry in kinds.values_mut() {
        entry.char_cut /= entry.count as f64;
        entry.token_cut /= entry.count as f64;
    }
    Agg {
        n,
        ev_sum,
        ev_kept,
        trunc_kept,
        rand_kept,
        char_cut: mean(rows, |row| row.char_cut),
        char_cut_rand: mean(rows, |row| row.char_cut_rand),
        token_cut: mean(rows, |row| row.token_cut),
        lat_scx: mean(rows, |row| median(&row.wall_scx_ms)),
        lat_trunc: mean(rows, |row| median(&row.wall_trunc_ms)),
        lat_rand: mean(rows, |row| median(&row.wall_rand_ms)),
        modes,
        kinds,
    }
}

fn mean(rows: &[Row], f: impl Fn(&Row) -> f64) -> f64 {
    if rows.is_empty() {
        0.0
    } else {
        compensated(rows.iter().map(f)) / rows.len() as f64
    }
}

/// Python 3.12 的 `sum()` 对浮点使用 Neumaier 补偿求和；为与冻结输出逐位
/// 一致，这里复刻同一算法（普通累加会差 1 ULP）。
pub(crate) fn compensated(values: impl Iterator<Item = f64>) -> f64 {
    let mut sum = 0.0f64;
    let mut carry = 0.0f64;
    for value in values {
        let next = sum + value;
        if sum.abs() >= value.abs() {
            carry += (sum - next) + value;
        } else {
            carry += (value - next) + sum;
        }
        sum = next;
    }
    sum + carry
}

fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    }
}

fn print(rows: &[Row], agg: &Agg, opts: &Opts, kind_counts: &[usize], binary: &str) {
    let kinds: Vec<(&str, usize)> = KINDS
        .iter()
        .copied()
        .zip(kind_counts.iter().copied())
        .collect();
    println!(
        "contain: slices={} kinds={} binary={binary}",
        agg.n,
        dict(&kinds)
    );
    let modes: Vec<(&str, usize)> = agg
        .modes
        .iter()
        .map(|(mode, count)| (mode.as_str(), *count))
        .collect();
    println!(
        "contain: ratio={} context={} modes={}",
        opts.ratio,
        opts.context,
        dict(&modes)
    );
    println!(
        "contain: rand-seed={} timing-rounds={}",
        opts.seed, opts.rounds
    );
    println!();
    println!(
        "{:<10} {:<6} {:<18} {:>7} {:>9} {:>8} {:>7} {:>9} {:>8}",
        "id", "kind", "mode", "kept", "char_cut", "tok_cut", "scx_ev", "trunc_ev", "rand_ev"
    );
    for row in rows {
        println!(
            "{:<10} {:<6} {:<18} {:>7} {:>8.1}% {:>7.1}% {}/{:>3} {:>4}/{} {:>4}/{}",
            row.id,
            row.kind,
            row.mode,
            pair(row.kept, row.total),
            row.char_cut * 100.0,
            row.token_cut * 100.0,
            row.ev_kept,
            row.ev_total,
            row.trunc_ev_kept,
            row.ev_total,
            row.rand_ev_kept,
            row.ev_total
        );
    }
    println!();
    println!(
        "contain: scx   evidence retention = {}/{} = {:.2}%",
        agg.ev_kept,
        agg.ev_sum,
        pct(agg.ev_kept, agg.ev_sum)
    );
    println!(
        "contain: trunc evidence retention = {}/{} = {:.2}%",
        agg.trunc_kept,
        agg.ev_sum,
        pct(agg.trunc_kept, agg.ev_sum)
    );
    println!(
        "contain: rand  evidence retention = {}/{} = {:.2}%",
        agg.rand_kept,
        agg.ev_sum,
        pct(agg.rand_kept, agg.ev_sum)
    );
    println!(
        "contain: mean char cut   = {:.2}% (scx) / {:.2}% (rand, same byte budget)",
        agg.char_cut * 100.0,
        agg.char_cut_rand * 100.0
    );
    println!(
        "contain: mean token cut  = {:.2}% (scx)",
        agg.token_cut * 100.0
    );
    println!(
        "contain: mean per-slice wall = scx {:.3} ms / trunc {:.1} us / rand {:.1} us",
        agg.lat_scx,
        agg.lat_trunc * 1000.0,
        agg.lat_rand * 1000.0
    );
    let misses: Vec<(&str, &str)> = rows
        .iter()
        .flat_map(|row| {
            row.ev_missed
                .iter()
                .map(move |m| (row.id.as_str(), m.as_str()))
        })
        .collect();
    if !misses.is_empty() {
        println!("contain: MISSED evidence lines:");
        for (ident, line) in misses {
            println!("  {ident}: {line}");
        }
    }
    println!("contain: per-kind (scx_ev / rand_ev / char_cut / token_cut):");
    for (kind, entry) in &agg.kinds {
        println!(
            "  {:<6} ev={}/{} rand_ev={}/{} char={:5.1}% tok={:5.1}%",
            kind,
            entry.ev_kept,
            entry.ev_total,
            entry.rand_ev,
            entry.ev_total,
            entry.char_cut * 100.0,
            entry.token_cut * 100.0
        );
    }
}

fn pair(value: Option<i64>, total: Option<i64>) -> String {
    format!("{}/{}", opt(value), opt(total))
}

pub(crate) fn opt(value: Option<i64>) -> String {
    value
        .map(|v| v.to_string())
        .unwrap_or_else(|| "None".to_string())
}

pub(crate) fn pct(part: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        part as f64 / total as f64 * 100.0
    }
}

pub(crate) fn dict(items: &[(&str, usize)]) -> String {
    let body: Vec<String> = items
        .iter()
        .map(|(key, value)| format!("'{key}': {value}"))
        .collect();
    format!("{{{}}}", body.join(", "))
}

pub(crate) fn list(items: &[&str]) -> String {
    let body: Vec<String> = items.iter().map(|item| format!("'{item}'")).collect();
    format!("[{}]", body.join(", "))
}

fn json(rows: &[Row], agg: &Agg, opts: &Opts, kind_counts: &[usize], binary: &str) -> J {
    let kinds: Vec<(String, J)> = KINDS
        .iter()
        .zip(kind_counts)
        .map(|(kind, count)| (kind.to_string(), J::Int(*count as i64)))
        .collect();
    let modes: Vec<(String, J)> = agg
        .modes
        .iter()
        .map(|(mode, count)| (mode.clone(), J::Int(*count as i64)))
        .collect();
    let aggregate = J::Obj(vec![
        ("slices".to_string(), J::Int(agg.n as i64)),
        ("kinds".to_string(), J::Obj(kinds)),
        ("modes".to_string(), J::Obj(modes)),
        (
            "scx_evidence_retention".to_string(),
            J::Num(ratio(agg.ev_kept, agg.ev_sum)),
        ),
        (
            "trunc_evidence_retention".to_string(),
            J::Num(ratio(agg.trunc_kept, agg.ev_sum)),
        ),
        (
            "rand_evidence_retention".to_string(),
            J::Num(ratio(agg.rand_kept, agg.ev_sum)),
        ),
        ("mean_char_cut".to_string(), J::Num(agg.char_cut)),
        ("mean_char_cut_rand".to_string(), J::Num(agg.char_cut_rand)),
        ("mean_token_cut".to_string(), J::Num(agg.token_cut)),
        ("mean_wall_scx_ms".to_string(), J::Num(agg.lat_scx)),
        ("mean_wall_trunc_ms".to_string(), J::Num(agg.lat_trunc)),
        ("mean_wall_rand_ms".to_string(), J::Num(agg.lat_rand)),
    ]);
    let rows: Vec<J> = rows.iter().map(row_json).collect();
    J::Obj(vec![
        ("binary".to_string(), J::Str(binary.to_string())),
        ("ratio".to_string(), J::Num(opts.ratio)),
        ("context".to_string(), J::Int(opts.context)),
        ("rand_seed".to_string(), J::Str(opts.seed.clone())),
        ("timing_rounds".to_string(), J::Int(opts.rounds as i64)),
        ("aggregate".to_string(), aggregate),
        ("rows".to_string(), J::Arr(rows)),
    ])
}

pub(crate) fn ratio(part: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        part as f64 / total as f64
    }
}

fn row_json(row: &Row) -> J {
    J::Obj(vec![
        ("id".to_string(), J::Str(row.id.clone())),
        ("kind".to_string(), J::Str(row.kind.clone())),
        ("query".to_string(), J::Str(row.query.clone())),
        ("mode".to_string(), J::Str(row.mode.clone())),
        ("risk".to_string(), J::Str(row.risk.clone())),
        ("kept".to_string(), opt_json(row.kept)),
        ("total".to_string(), opt_json(row.total)),
        ("seeds".to_string(), opt_json(row.seeds)),
        ("evidence_seeds".to_string(), opt_json(row.evidence_seeds)),
        ("chars_in".to_string(), J::Int(row.chars_in as i64)),
        ("chars_out".to_string(), J::Int(row.chars_out as i64)),
        (
            "chars_out_rand".to_string(),
            J::Int(row.chars_out_rand as i64),
        ),
        ("token_in".to_string(), J::Int(row.token_in as i64)),
        ("token_out".to_string(), J::Int(row.token_out as i64)),
        ("char_cut".to_string(), J::Num(row.char_cut)),
        ("char_cut_rand".to_string(), J::Num(row.char_cut_rand)),
        ("token_cut".to_string(), J::Num(row.token_cut)),
        ("ev_total".to_string(), J::Int(row.ev_total as i64)),
        ("ev_kept".to_string(), J::Int(row.ev_kept as i64)),
        (
            "ev_missed".to_string(),
            J::Arr(row.ev_missed.iter().cloned().map(J::Str).collect()),
        ),
        (
            "trunc_ev_kept".to_string(),
            J::Int(row.trunc_ev_kept as i64),
        ),
        ("rand_ev_kept".to_string(), J::Int(row.rand_ev_kept as i64)),
        (
            "trunc_budget_chars".to_string(),
            J::Int(row.trunc_budget_chars as i64),
        ),
        ("wall_scx_ms".to_string(), floats(&row.wall_scx_ms)),
        ("wall_trunc_ms".to_string(), floats(&row.wall_trunc_ms)),
        ("wall_rand_ms".to_string(), floats(&row.wall_rand_ms)),
    ])
}

pub(crate) fn opt_json(value: Option<i64>) -> J {
    value.map(J::Int).unwrap_or(J::Null)
}

fn floats(values: &[f64]) -> J {
    J::Arr(values.iter().copied().map(J::Num).collect())
}

pub(crate) fn default_binary() -> Option<String> {
    for name in ["scx.exe", "scx"] {
        let path = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.."))
            .join("target/release")
            .join(name);
        if path.is_file() {
            return Some(path.to_string_lossy().to_string());
        }
    }
    None
}

pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("scx-contain-{}-{nanos}", std::process::id()));
        let _ = fs::create_dir_all(&path);
        Self(path)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_count_matches_python_examples() {
        assert_eq!(token_count("error E0382 borrow value"), 4);
        assert_eq!(token_count("a.b-c_d e"), 2);
        assert_eq!(token_count("数据库 超时"), 5);
    }

    #[test]
    fn trunc_is_byte_exact() {
        assert_eq!(trunc("abcdef", 3), "abc");
        assert_eq!(trunc("abc", 10), "abc");
    }

    #[test]
    fn random_sample_matches_cpython_shape() {
        let text = "aaa\nbbb\nccc\nddd\n";
        let got = random_sample(text, 8, "contain-rand-v1", "slice-1");
        assert_eq!(got.len(), 8);
        assert!(text.contains(got.trim_end_matches('\n')));
    }

    #[test]
    fn splitlines_keepends_matches_python_subset() {
        assert_eq!(splitlines("a\nb\r\nc"), vec!["a\n", "b\r\n", "c"]);
        assert_eq!(splitlines(""), Vec::<&str>::new());
    }
}
