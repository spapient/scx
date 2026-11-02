//! `xtask fidelity`：compression fidelity 协议（feature = "eval"）。
//!
//! 迁移自 `tools/scx.py fidelity`：逐切片调用 `scx`，在同字节预算下对比
//! 头截断与随机行采样，统计证据保留、answerability、token-set recall，
//! 并对跨域（kind）做 cluster bootstrap 与冻结门禁。
//!
//! 确定性字段与 Python 版逐字节一致；wall 时间除外。

// 解析/聚合/格式化分支是线性映射；认知复杂度门禁针对压缩内核，不适用于迁移工具。
#![allow(clippy::cognitive_complexity)]

use super::contain::{self, Slice, TempDir};
use super::io::{self, J};
use super::rng::PyRng;
use super::{fail, Fail};
use scx::json::Json;
use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

const STOP_Q: [&str; 46] = [
    "a", "an", "the", "and", "or", "of", "in", "to", "for", "with", "on", "at", "from", "by", "be",
    "been", "being", "is", "are", "was", "were", "do", "does", "did", "not", "no", "it", "its",
    "that", "this", "these", "those", "as", "can", "could", "would", "should", "will", "why",
    "what", "which", "how", "when", "where", "who", "whom",
];

const STOP_SET: [&str; 51] = [
    "a", "an", "the", "and", "or", "of", "in", "to", "for", "with", "on", "at", "from", "by", "be",
    "been", "being", "is", "are", "was", "were", "do", "does", "did", "not", "no", "it", "its",
    "that", "this", "these", "those", "as", "can", "could", "would", "should", "will", "have",
    "has", "had", "i", "you", "we", "they", "he", "she", "but", "if", "then", "else",
];

struct Corpus {
    slices: Vec<Slice>,
    kinds: Vec<String>,
    counts: Vec<usize>,
}

struct Row {
    id: String,
    kind: String,
    query: String,
    mode: String,
    risk: String,
    ev_total: usize,
    ev_kept: usize,
    trunc_ev_kept: usize,
    rand_ev_kept: usize,
    answerable: bool,
    answerable_trunc: bool,
    answerable_rand: bool,
    token_recall: f64,
    token_recall_trunc: f64,
    token_recall_rand: f64,
    char_cut: f64,
    token_cut: f64,
    trunc_budget_chars: usize,
    wall_scx_ms: f64,
}

struct KindAgg {
    n: usize,
    ev_total: usize,
    ev_kept: usize,
    trunc_ev: usize,
    ans: usize,
    tok: f64,
    char_cut: f64,
}

struct DomainBoot {
    n_domains: usize,
    mean_delta: Option<f64>,
    p: Option<f64>,
    ci_lo: Option<f64>,
    ci_hi: Option<f64>,
    positive_domains: usize,
    short: bool,
}

struct Agg {
    n: usize,
    modes: Vec<(String, usize)>,
    ev_total: usize,
    ev_kept: usize,
    trunc_ev: usize,
    rand_ev: usize,
    ans: usize,
    ans_trunc: usize,
    ans_rand: usize,
    tok: f64,
    tok_trunc: f64,
    tok_rand: f64,
    char_cut: f64,
    token_cut: f64,
    wall: f64,
    kinds: Vec<(String, KindAgg)>,
    domain_boot: Option<DomainBoot>,
    domain_gate_p: f64,
}

struct Opts {
    corpus: String,
    binary: Option<String>,
    ratio: f64,
    context: i64,
    seed: String,
    out: Option<String>,
    gate: bool,
    gate_ev: f64,
    gate_ans: f64,
    domain_boot: usize,
    domain_gate_p: f64,
}

pub(crate) fn run(args: &[String]) -> Result<(), Fail> {
    let opts = parse(args)?;
    let corpus = load_corpus(&opts.corpus)?;
    let binary = match opts.binary.clone().or_else(contain::default_binary) {
        Some(path) if Path::new(&path).is_file() => path,
        _ => {
            return Err(fail(
                "fidelity: missing release binary; run `cargo build --release --locked` first",
            ))
        }
    };
    let work = TempDir::new();
    let mut rows = Vec::with_capacity(corpus.slices.len());
    for slice in &corpus.slices {
        rows.push(run_slice(
            &binary,
            slice,
            opts.ratio,
            opts.context,
            work.path(),
            &opts.seed,
        )?);
    }
    let mut agg = aggregate(&rows);
    agg.domain_boot = Some(domain_boot(&agg, opts.domain_boot, "domain-cluster-v1"));
    agg.domain_gate_p = opts.domain_gate_p;
    print(&rows, &agg, &opts, &corpus, &binary);
    if opts.gate {
        let failed = gate_failures(&agg, opts.gate_ev, opts.gate_ans, opts.domain_gate_p);
        if !failed.is_empty() {
            println!("fidelity: GATE FAILED: {}", failed.join("; "));
            return Err(Fail {
                msg: String::new(),
                code: 2,
            });
        }
        println!(
            "fidelity: gate PASS (ev>={}, ans>={}, both strictly better than trunc; \
             per-kind zero-regression; domain cluster-bootstrap p<{}; token recall report-only)",
            opts.gate_ev, opts.gate_ans, opts.domain_gate_p
        );
    }
    if let Some(path) = &opts.out {
        io::write_json(path, &json(&rows, &agg, &opts, &binary)).map_err(fail)?;
        println!("fidelity: wrote {path}");
    }
    Ok(())
}

fn parse(args: &[String]) -> Result<Opts, Fail> {
    let mut opts = Opts {
        corpus: String::new(),
        binary: None,
        ratio: 0.35,
        context: 1,
        seed: "fidelity-rand-v1".to_string(),
        out: None,
        gate: false,
        gate_ev: 0.95,
        gate_ans: 0.90,
        domain_boot: 10_000,
        domain_gate_p: 0.05,
    };
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--binary" | "--ratio" | "--context" | "--seed" | "--out" | "--gate-ev"
            | "--gate-ans" | "--domain-boot" | "--domain-gate-p" => {
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| usage(format!("{arg} expects a value")))?;
                match arg.as_str() {
                    "--binary" => opts.binary = Some(value.clone()),
                    "--ratio" => opts.ratio = number(value, arg)?,
                    "--context" => opts.context = number(value, arg)?,
                    "--seed" => opts.seed = value.clone(),
                    "--out" => opts.out = Some(value.clone()),
                    "--gate-ev" => opts.gate_ev = number(value, arg)?,
                    "--gate-ans" => opts.gate_ans = number(value, arg)?,
                    "--domain-boot" => opts.domain_boot = number(value, arg)?,
                    "--domain-gate-p" => opts.domain_gate_p = number(value, arg)?,
                    _ => return Err(usage(format!("unrecognized argument: {arg}"))),
                }
            }
            "-h" | "--help" => return Err(usage(String::new())),
            "--gate" => opts.gate = true,
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
    Ok(opts)
}

fn number<T: std::str::FromStr>(text: &str, name: &str) -> Result<T, Fail> {
    text.parse()
        .map_err(|_| usage(format!("invalid value for {name}: {text}")))
}

fn usage(message: String) -> Fail {
    let text = if message.is_empty() {
        fidelity_usage()
    } else {
        format!("{message}\n\n{}", fidelity_usage())
    };
    Fail { msg: text, code: 2 }
}

fn fidelity_usage() -> String {
    "\
usage: xtask fidelity <corpus> [options]

  --binary PATH        scx binary (default: target/release/scx)
  --ratio F            optional-content budget [0.35]
  --context N          minimum neighborhood [1]
  --seed TEXT          random-sampling seed [fidelity-rand-v1]
  --out PATH           write per-slice detail JSON
  --gate               enforce frozen thresholds
  --gate-ev F          evidence retention floor [0.95]
  --gate-ans F         answerable-rate floor [0.90]
  --domain-boot N      domain cluster-bootstrap replicates [10000]
  --domain-gate-p F    one-sided p threshold [0.05]
"
    .to_string()
}

fn load_corpus(path: &str) -> Result<Corpus, Fail> {
    let value = super::load_json(path)?;
    let items = match value.get("slices") {
        Some(Json::Arr(items)) => items,
        _ => return Err(fail("fidelity: corpus missing \"slices\" array")),
    };
    if !(20..=60).contains(&items.len()) {
        return Err(fail(format!(
            "fidelity: slices={} must be 20..60",
            items.len()
        )));
    }
    let mut kinds: Vec<String> = Vec::new();
    for kind in strings(&value, "expected_kinds") {
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    if kinds.is_empty() {
        return Err(fail("fidelity: corpus must declare expected_kinds"));
    }
    let mut counts = vec![0usize; kinds.len()];
    let mut seen = HashSet::new();
    let mut slices = Vec::with_capacity(items.len());
    for item in items {
        let ident = contain::string(item, "id");
        let kind = contain::string(item, "kind");
        let query = contain::string(item, "query");
        let full_text = contain::raw(item, "full_text");
        let note = contain::string(item, "note");
        let evidence = strings(item, "evidence");
        if ident.is_empty() || !seen.insert(ident.clone()) {
            return Err(fail(format!("fidelity: bad/duplicate slice id: {ident:?}")));
        }
        let Some(kind_index) = kinds.iter().position(|k| *k == kind) else {
            return Err(fail(format!("fidelity: slice {ident} bad kind {kind:?}")));
        };
        if query.is_empty() || full_text.is_empty() || evidence.is_empty() {
            return Err(fail(format!(
                "fidelity: slice {ident} empty query/text/evidence"
            )));
        }
        if note.is_empty() {
            return Err(fail(format!("fidelity: slice {ident} missing note")));
        }
        if query_tokens(&query).is_empty() {
            return Err(fail(format!(
                "fidelity: slice {ident} query has no substantive tokens"
            )));
        }
        for ev in &evidence {
            if !full_text.contains(ev) {
                return Err(fail(format!(
                    "fidelity: slice {ident} evidence line not verbatim in full_text:\n{ev:?}"
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
    let sparse: Vec<&str> = kinds
        .iter()
        .zip(&counts)
        .filter(|(_, count)| **count < 3)
        .map(|(kind, _)| kind.as_str())
        .collect();
    if !sparse.is_empty() {
        return Err(fail(format!(
            "fidelity: corpus kinds too sparse for per-kind gate: {}",
            contain::list(&sparse)
        )));
    }
    Ok(Corpus {
        slices,
        kinds,
        counts,
    })
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
) -> Result<Row, Fail> {
    let start = Instant::now();
    let (stats, body) = contain::invoke(binary, slice, ratio, context, work)?;
    let wall = start.elapsed().as_secs_f64() * 1000.0;
    let budget = body.len();
    let trunc_text = contain::trunc(&slice.full_text, budget);
    let rand_text = contain::random_sample(&slice.full_text, budget, seed, &slice.id);
    let ev_total = slice.evidence.len();
    let ev_kept = slice
        .evidence
        .iter()
        .filter(|ev| body.contains(ev.as_str()))
        .count();
    let trunc_ev_kept = slice
        .evidence
        .iter()
        .filter(|ev| trunc_text.contains(ev.as_str()))
        .count();
    let rand_ev_kept = slice
        .evidence
        .iter()
        .filter(|ev| rand_text.contains(ev.as_str()))
        .count();
    let in_chars = slice.full_text.chars().count();
    let out_chars = body.chars().count();
    let in_tokens = contain::token_count(&slice.full_text);
    let out_tokens = contain::token_count(&body);
    Ok(Row {
        id: slice.id.clone(),
        kind: slice.kind.clone(),
        query: slice.query.clone(),
        mode: stats.mode,
        risk: stats.risk,
        ev_total,
        ev_kept,
        trunc_ev_kept,
        rand_ev_kept,
        answerable: answerable(slice, &body),
        answerable_trunc: answerable(slice, &trunc_text),
        answerable_rand: answerable(slice, &rand_text),
        token_recall: token_recall(&slice.full_text, &body),
        token_recall_trunc: token_recall(&slice.full_text, &trunc_text),
        token_recall_rand: token_recall(&slice.full_text, &rand_text),
        char_cut: contain::cut(out_chars, in_chars),
        token_cut: contain::cut(out_tokens, in_tokens),
        trunc_budget_chars: budget,
        wall_scx_ms: wall,
    })
}

fn query_tokens(query: &str) -> Vec<String> {
    words(&query.to_lowercase())
        .into_iter()
        .filter(|token| !STOP_Q.contains(&token.as_str()))
        .collect()
}

fn words(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i].is_ascii_alphanumeric() {
            let start = i;
            i += 1;
            while i < bytes.len() && word_part(bytes[i]) {
                i += 1;
            }
            out.push(text[start..i].to_string());
        } else {
            i += 1;
        }
    }
    out
}

fn word_part(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-')
}

fn token_set(text: &str) -> HashSet<String> {
    words(&text.to_lowercase())
        .into_iter()
        .filter(|token| !STOP_SET.contains(&token.as_str()))
        .collect()
}

fn token_recall(text: &str, body: &str) -> f64 {
    let source = token_set(text);
    if source.is_empty() {
        return 0.0;
    }
    let out = token_set(body);
    source.intersection(&out).count() as f64 / source.len() as f64
}

fn answerable(slice: &Slice, body: &str) -> bool {
    let kept: Vec<&str> = slice
        .evidence
        .iter()
        .map(String::as_str)
        .filter(|ev| body.contains(ev))
        .collect();
    if kept.len() != slice.evidence.len() {
        return false;
    }
    let tokens = query_tokens(&slice.query);
    if tokens.is_empty() {
        return true;
    }
    let kept_lower = kept
        .iter()
        .map(|ev| ev.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    let hits = tokens
        .iter()
        .filter(|token| kept_lower.contains(token.as_str()))
        .count();
    hits as f64 / tokens.len() as f64 >= 0.5
}

fn aggregate(rows: &[Row]) -> Agg {
    let n = rows.len();
    let mut modes: Vec<(String, usize)> = Vec::new();
    let mut kinds: Vec<(String, KindAgg)> = Vec::new();
    let mut agg = Agg {
        n,
        modes: Vec::new(),
        ev_total: 0,
        ev_kept: 0,
        trunc_ev: 0,
        rand_ev: 0,
        ans: 0,
        ans_trunc: 0,
        ans_rand: 0,
        tok: 0.0,
        tok_trunc: 0.0,
        tok_rand: 0.0,
        char_cut: 0.0,
        token_cut: 0.0,
        wall: 0.0,
        kinds: Vec::new(),
        domain_boot: None,
        domain_gate_p: 0.0,
    };
    for row in rows {
        match modes.iter_mut().find(|(mode, _)| *mode == row.mode) {
            Some((_, count)) => *count += 1,
            None => modes.push((row.mode.clone(), 1)),
        }
        agg.ev_total += row.ev_total;
        agg.ev_kept += row.ev_kept;
        agg.trunc_ev += row.trunc_ev_kept;
        agg.rand_ev += row.rand_ev_kept;
        agg.ans += if row.answerable { 1 } else { 0 };
        agg.ans_trunc += if row.answerable_trunc { 1 } else { 0 };
        agg.ans_rand += if row.answerable_rand { 1 } else { 0 };
        agg.tok += row.token_recall;
        agg.tok_trunc += row.token_recall_trunc;
        agg.tok_rand += row.token_recall_rand;
        agg.char_cut += row.char_cut;
        agg.token_cut += row.token_cut;
        agg.wall += row.wall_scx_ms;
        let entry_index = match kinds.iter().position(|(kind, _)| *kind == row.kind) {
            Some(index) => index,
            None => {
                kinds.push((
                    row.kind.clone(),
                    KindAgg {
                        n: 0,
                        ev_total: 0,
                        ev_kept: 0,
                        trunc_ev: 0,
                        ans: 0,
                        tok: 0.0,
                        char_cut: 0.0,
                    },
                ));
                kinds.len() - 1
            }
        };
        let entry = &mut kinds[entry_index].1;
        entry.n += 1;
        entry.ev_total += row.ev_total;
        entry.ev_kept += row.ev_kept;
        entry.trunc_ev += row.trunc_ev_kept;
        entry.ans += if row.answerable { 1 } else { 0 };
        entry.tok += row.token_recall;
        entry.char_cut += row.char_cut;
    }
    if n > 0 {
        let n = n as f64;
        agg.tok /= n;
        agg.tok_trunc /= n;
        agg.tok_rand /= n;
        agg.char_cut /= n;
        agg.token_cut /= n;
        agg.wall /= n;
    }
    for (_, entry) in kinds.iter_mut() {
        if entry.n > 0 {
            entry.tok /= entry.n as f64;
            entry.char_cut /= entry.n as f64;
        }
    }
    agg.modes = modes;
    agg.kinds = kinds;
    agg
}

fn domain_boot(agg: &Agg, boot: usize, seed: &str) -> DomainBoot {
    let mut kinds: Vec<&(String, KindAgg)> = agg.kinds.iter().collect();
    kinds.sort_by(|a, b| a.0.cmp(&b.0));
    let mut deltas = Vec::new();
    for (_, kind) in kinds {
        if kind.ev_total == 0 {
            continue;
        }
        let scx = kind.ev_kept as f64 / kind.ev_total as f64;
        let trunc = kind.trunc_ev as f64 / kind.ev_total as f64;
        deltas.push(scx - trunc);
    }
    if deltas.len() < 2 {
        return DomainBoot {
            n_domains: deltas.len(),
            mean_delta: None,
            p: None,
            ci_lo: None,
            ci_hi: None,
            positive_domains: 0,
            short: true,
        };
    }
    let m = deltas.len();
    let mut rng = PyRng::from_seed_bytes(seed.as_bytes());
    let mut means = Vec::with_capacity(boot);
    for _ in 0..boot {
        let draws = (0..m).map(|_| deltas[rng.randrange(m)]);
        means.push(contain::compensated(draws) / m as f64);
    }
    means.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let lo = means[(0.025 * (boot as f64 - 1.0)) as usize];
    let hi = means[(0.975 * (boot as f64 - 1.0)) as usize];
    let hits = means.iter().filter(|value| **value <= 0.0).count();
    let p = (hits as f64 + 1.0) / (boot as f64 + 1.0);
    DomainBoot {
        n_domains: m,
        mean_delta: Some(contain::compensated(deltas.iter().copied()) / m as f64),
        p: Some(p),
        ci_lo: Some(lo),
        ci_hi: Some(hi),
        positive_domains: deltas.iter().filter(|value| **value > 0.0).count(),
        short: false,
    }
}

fn gate_failures(agg: &Agg, gate_ev: f64, gate_ans: f64, domain_gate_p: f64) -> Vec<String> {
    let mut failed = Vec::new();
    let ev_ret = contain::ratio(agg.ev_kept, agg.ev_total);
    let trunc_ret = contain::ratio(agg.trunc_ev, agg.ev_total);
    let ans_rate = contain::ratio(agg.ans, agg.n);
    let ans_trunc = contain::ratio(agg.ans_trunc, agg.n);
    if ev_ret < gate_ev {
        failed.push(format!("evidence retention={ev_ret:.4} < {gate_ev:.4}"));
    }
    if ans_rate < gate_ans {
        failed.push(format!("answerable rate={ans_rate:.4} < {gate_ans:.4}"));
    }
    if ev_ret <= trunc_ret {
        failed.push(format!(
            "ev vs trunc: scx={ev_ret:.4} <= trunc={trunc_ret:.4}"
        ));
    }
    if ans_rate <= ans_trunc {
        failed.push(format!(
            "ans vs trunc: scx={ans_rate:.4} <= trunc={ans_trunc:.4}"
        ));
    }
    let mut kinds: Vec<&(String, KindAgg)> = agg.kinds.iter().collect();
    kinds.sort_by(|a, b| a.0.cmp(&b.0));
    for (kind, entry) in kinds {
        let ret = contain::ratio(entry.ev_kept, entry.ev_total);
        if ret < gate_ev {
            failed.push(format!("kind {kind} ev={ret:.4} < {gate_ev:.4}"));
        }
    }
    if let Some(db) = &agg.domain_boot {
        if let Some(mean_delta) = db.mean_delta {
            let p = db.p.unwrap_or(1.0);
            if mean_delta <= 0.0 || p >= domain_gate_p {
                failed.push(format!(
                    "domain cluster-bootstrap p={p:.4} mean_delta={mean_delta:+.4} \
                     (need p<0.05 and mean_delta>0)"
                ));
            }
        }
    }
    failed
}

fn print(rows: &[Row], agg: &Agg, opts: &Opts, corpus: &Corpus, binary: &str) {
    let kinds: Vec<(&str, usize)> = corpus
        .kinds
        .iter()
        .zip(&corpus.counts)
        .map(|(kind, count)| (kind.as_str(), *count))
        .collect();
    println!(
        "fidelity: slices={} kinds={} binary={binary}",
        agg.n,
        contain::dict(&kinds)
    );
    let modes: Vec<(&str, usize)> = agg
        .modes
        .iter()
        .map(|(mode, count)| (mode.as_str(), *count))
        .collect();
    println!(
        "fidelity: ratio={} context={} modes={} rand-seed={}",
        opts.ratio,
        opts.context,
        contain::dict(&modes),
        opts.seed
    );
    println!();
    println!(
        "{:<10} {:<6} {:<18} {:>7} {:>5} {:>6} {:>9}",
        "id", "kind", "mode", "kept", "ans", "tokR", "char_cut"
    );
    for row in rows {
        println!(
            "{:<10} {:<6} {:<18} {:>7} {:>5} {:>5.1}% {:>8.1}%",
            row.id,
            row.kind,
            row.mode,
            format!("{}/{}", row.ev_kept, row.ev_total),
            if row.answerable { "Y" } else { "." },
            row.token_recall * 100.0,
            row.char_cut * 100.0
        );
    }
    println!();
    println!(
        "fidelity: evidence retention = {}/{} = {:.2}%  (trunc {}/{} = {:.2}% / rand {}/{} = {:.2}%)",
        agg.ev_kept,
        agg.ev_total,
        contain::pct(agg.ev_kept, agg.ev_total),
        agg.trunc_ev,
        agg.ev_total,
        contain::pct(agg.trunc_ev, agg.ev_total),
        agg.rand_ev,
        agg.ev_total,
        contain::pct(agg.rand_ev, agg.ev_total)
    );
    println!(
        "fidelity: answerable rate   = {}/{} = {:.2}%  (trunc {:.2}% / rand {:.2}%)",
        agg.ans,
        agg.n,
        contain::pct(agg.ans, agg.n),
        contain::ratio(agg.ans_trunc, agg.n) * 100.0,
        contain::ratio(agg.ans_rand, agg.n) * 100.0
    );
    println!(
        "fidelity: token recall      = {:.2}%  (trunc {:.2}% / rand {:.2}%)",
        agg.tok * 100.0,
        agg.tok_trunc * 100.0,
        agg.tok_rand * 100.0
    );
    println!(
        "fidelity: mean char cut     = {:.2}% / token cut {:.2}%",
        agg.char_cut * 100.0,
        agg.token_cut * 100.0
    );
    println!("fidelity: mean wall         = {:.3} ms", agg.wall);
    if let Some(db) = &agg.domain_boot {
        if let (Some(mean), Some(p), Some(lo), Some(hi)) = (db.mean_delta, db.p, db.ci_lo, db.ci_hi)
        {
            println!(
                "fidelity: domain cluster-bootstrap (n_domains={}, cluster=kind): \
                 mean_delta={:+.2}pp CI95=[{:+.2},{:+.2}]pp p={:.4} \
                 (one-sided, positive_domains={})",
                db.n_domains,
                mean * 100.0,
                lo * 100.0,
                hi * 100.0,
                p,
                db.positive_domains
            );
        }
    }
    println!("fidelity: per-kind (ev / trunc_ev / ans / tok / char_cut):");
    let mut kinds: Vec<&(String, KindAgg)> = agg.kinds.iter().collect();
    kinds.sort_by(|a, b| a.0.cmp(&b.0));
    for (kind, entry) in kinds {
        println!(
            "  {:<10} ev={}/{} ({:5.1}%) trunc={:5.1}% ans={}/{} tok={:5.1}% cut={:5.1}%",
            kind,
            entry.ev_kept,
            entry.ev_total,
            contain::pct(entry.ev_kept, entry.ev_total),
            contain::pct(entry.trunc_ev, entry.ev_total),
            entry.ans,
            entry.n,
            entry.tok * 100.0,
            entry.char_cut * 100.0
        );
    }
    let unans: Vec<(&str, &str)> = rows
        .iter()
        .filter(|row| !row.answerable)
        .map(|row| (row.id.as_str(), row.mode.as_str()))
        .collect();
    if !unans.is_empty() {
        println!("fidelity: NOT answerable slices:");
        for (ident, mode) in unans {
            println!("  {ident}: mode={mode}");
        }
    }
}

fn json(rows: &[Row], agg: &Agg, opts: &Opts, binary: &str) -> J {
    let modes: Vec<(String, J)> = agg
        .modes
        .iter()
        .map(|(mode, count)| (mode.clone(), J::Int(*count as i64)))
        .collect();
    let mut aggregate = vec![
        ("n".to_string(), J::Int(agg.n as i64)),
        ("modes".to_string(), J::Obj(modes)),
        ("ev_total".to_string(), J::Int(agg.ev_total as i64)),
        ("ev_kept".to_string(), J::Int(agg.ev_kept as i64)),
        ("trunc_ev".to_string(), J::Int(agg.trunc_ev as i64)),
        ("rand_ev".to_string(), J::Int(agg.rand_ev as i64)),
        ("ans".to_string(), J::Int(agg.ans as i64)),
        ("ans_trunc".to_string(), J::Int(agg.ans_trunc as i64)),
        ("ans_rand".to_string(), J::Int(agg.ans_rand as i64)),
        ("tok".to_string(), J::Num(agg.tok)),
        ("tok_trunc".to_string(), J::Num(agg.tok_trunc)),
        ("tok_rand".to_string(), J::Num(agg.tok_rand)),
        ("char_cut".to_string(), J::Num(agg.char_cut)),
        ("token_cut".to_string(), J::Num(agg.token_cut)),
        ("wall".to_string(), J::Num(agg.wall)),
    ];
    let kind_json: Vec<(String, J)> = agg
        .kinds
        .iter()
        .map(|(kind, entry)| {
            (
                kind.clone(),
                J::Obj(vec![
                    ("n".to_string(), J::Int(entry.n as i64)),
                    ("ev_total".to_string(), J::Int(entry.ev_total as i64)),
                    ("ev_kept".to_string(), J::Int(entry.ev_kept as i64)),
                    ("trunc_ev".to_string(), J::Int(entry.trunc_ev as i64)),
                    ("ans".to_string(), J::Int(entry.ans as i64)),
                    ("tok".to_string(), J::Num(entry.tok)),
                    ("char_cut".to_string(), J::Num(entry.char_cut)),
                ]),
            )
        })
        .collect();
    aggregate.push(("kinds".to_string(), J::Obj(kind_json)));
    if let Some(db) = &agg.domain_boot {
        aggregate.push(("domain_boot".to_string(), domain_json(db)));
        aggregate.push(("domain_gate_p".to_string(), J::Num(agg.domain_gate_p)));
    }
    J::Obj(vec![
        ("binary".to_string(), J::Str(binary.to_string())),
        ("ratio".to_string(), J::Num(opts.ratio)),
        ("context".to_string(), J::Int(opts.context)),
        ("rand_seed".to_string(), J::Str(opts.seed.clone())),
        ("aggregate".to_string(), J::Obj(aggregate)),
        (
            "rows".to_string(),
            J::Arr(
                rows.iter()
                    .map(|row| {
                        J::Obj(vec![
                            ("id".to_string(), J::Str(row.id.clone())),
                            ("kind".to_string(), J::Str(row.kind.clone())),
                            ("query".to_string(), J::Str(row.query.clone())),
                            ("mode".to_string(), J::Str(row.mode.clone())),
                            ("risk".to_string(), J::Str(row.risk.clone())),
                            ("ev_total".to_string(), J::Int(row.ev_total as i64)),
                            ("ev_kept".to_string(), J::Int(row.ev_kept as i64)),
                            (
                                "trunc_ev_kept".to_string(),
                                J::Int(row.trunc_ev_kept as i64),
                            ),
                            ("rand_ev_kept".to_string(), J::Int(row.rand_ev_kept as i64)),
                            ("answerable".to_string(), J::Bool(row.answerable)),
                            (
                                "answerable_trunc".to_string(),
                                J::Bool(row.answerable_trunc),
                            ),
                            ("answerable_rand".to_string(), J::Bool(row.answerable_rand)),
                            ("token_recall".to_string(), J::Num(row.token_recall)),
                            (
                                "token_recall_trunc".to_string(),
                                J::Num(row.token_recall_trunc),
                            ),
                            (
                                "token_recall_rand".to_string(),
                                J::Num(row.token_recall_rand),
                            ),
                            ("char_cut".to_string(), J::Num(row.char_cut)),
                            ("token_cut".to_string(), J::Num(row.token_cut)),
                            (
                                "trunc_budget_chars".to_string(),
                                J::Int(row.trunc_budget_chars as i64),
                            ),
                            ("wall_scx_ms".to_string(), J::Num(row.wall_scx_ms)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

fn domain_json(db: &DomainBoot) -> J {
    let mean = db.mean_delta.map(J::Num).unwrap_or(J::Null);
    let p = db.p.map(J::Num).unwrap_or(J::Null);
    let lo = db.ci_lo.map(J::Num).unwrap_or(J::Null);
    let hi = db.ci_hi.map(J::Num).unwrap_or(J::Null);
    // Python 的 key 顺序在样本不足 2 域与正常路径下不同；保持一致。
    if db.short {
        J::Obj(vec![
            ("n_domains".to_string(), J::Int(db.n_domains as i64)),
            ("mean_delta".to_string(), mean),
            ("p".to_string(), p),
            ("ci_lo".to_string(), lo),
            ("ci_hi".to_string(), hi),
        ])
    } else {
        J::Obj(vec![
            ("n_domains".to_string(), J::Int(db.n_domains as i64)),
            ("mean_delta".to_string(), mean),
            ("ci_lo".to_string(), lo),
            ("ci_hi".to_string(), hi),
            ("p".to_string(), p),
            (
                "positive_domains".to_string(),
                J::Int(db.positive_domains as i64),
            ),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slice(query: &str, evidence: &[&str]) -> Slice {
        Slice {
            id: "s".to_string(),
            kind: "log".to_string(),
            query: query.to_string(),
            full_text: evidence.join("\n"),
            evidence: evidence.iter().map(|ev| ev.to_string()).collect(),
        }
    }

    #[test]
    fn query_tokens_drops_function_words() {
        assert_eq!(
            query_tokens("why did the build fail"),
            vec!["build", "fail"]
        );
        assert!(query_tokens("the and or").is_empty());
    }

    #[test]
    fn answerable_requires_all_evidence_and_half_query() {
        let case = slice("database timeout", &["db timeout", "conn lost"]);
        assert!(answerable(&case, "db timeout\nconn lost\n"));
        assert!(!answerable(&case, "db timeout\n"));
        let unrelated = slice("database timeout", &["db conn"]);
        assert!(!answerable(&unrelated, "db conn\n"));
    }

    #[test]
    fn token_recall_is_set_based_and_stopword_filtered() {
        assert_eq!(token_recall("alpha beta gamma", "alpha beta"), 2.0 / 3.0);
        assert_eq!(token_recall("the and", "the and"), 0.0);
    }

    #[test]
    fn words_match_python_ascii_regex() {
        assert_eq!(words("a.b-c_d e"), vec!["a.b-c_d", "e"]);
        assert_eq!(words("x=1, y=2"), vec!["x", "1", "y", "2"]);
    }
}
