//! `xtask distill weights`：评测信号坐标上升蒸馏（scx-sem-2）。
//!
//! 迁移自原 Python 工具的 `distill weights`：3 语料 cut/fidelity 目标下逐维坐标
//! 上升，写入 `config/semantic/weights.json`。`distill online` 仍由 Python 承担
//! （在线教师 API）。
//!
//! 蒸馏分支线性；认知复杂度门禁针对压缩内核，不适用于迁移工具。
#![allow(clippy::cognitive_complexity)]

use super::bench;
use super::contain;
use super::io::{self, J};
use super::{fail, Fail};
use std::fs;
use std::path::{Path, PathBuf};

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
const TOL: f64 = 0.01;
const FEATURES: [&str; 18] = [
    "overlap",
    "hit",
    "ln_len",
    "cap_ratio",
    "dig_ratio",
    "structure",
    "word_hit",
    "cjk_ratio",
    "errcode",
    "uniq",
    "sym",
    "pos_norm",
    "edge",
    "long",
    "code",
    "bias",
    "cjk_bigram",
    "term_prox",
];
const INIT: [f64; 18] = [
    0.072325, 0.496615, -0.310477, -0.005689, -0.006987, -0.0548, 0.016532, -0.037252, -0.013018,
    -0.120063, -0.245746, -0.111923, -0.075548, 0.009463, -0.010174, -0.276553, 0.0, 0.0,
];
const GRIDS: [&[f64]; 2] = [&[0.05, -0.05, 0.15, -0.15], &[0.02, -0.02]];

struct Row {
    set: String,
    cut: f64,
    num_cov: f64,
    token_cov: f64,
}

struct LogRow {
    round: usize,
    dim: &'static str,
    w: f64,
    score: f64,
}

pub(crate) fn run(args: &[String]) -> Result<(), Fail> {
    match args.first().map(String::as_str) {
        Some("weights") => weights(&args[1..]),
        Some("online") => Err(fail("distill online 依赖在线教师 API，本轮未迁移")),
        Some(other) => Err(usage(format!("unknown distill mode: {other}"))),
        None => Err(usage("missing distill mode")),
    }
}

fn usage(message: impl Into<String>) -> Fail {
    let body = "usage: xtask distill weights [--sem-bin PATH] [--out PATH] [--log PATH]\n";
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

fn weights(args: &[String]) -> Result<(), Fail> {
    validate(args)?;
    let sem_bin = value(args, "--sem-bin")
        .map(str::to_string)
        .unwrap_or_else(|| format!("{ROOT}/target/ablate-sem/release/scx"));
    let out = value(args, "--out")
        .map(str::to_string)
        .unwrap_or_else(|| format!("{ROOT}/config/semantic/weights.json"));
    let log_path = value(args, "--log")
        .map(str::to_string)
        .unwrap_or_else(|| format!("{ROOT}/config/bench/results/distill_log.csv"));
    if !Path::new(&sem_bin).is_file() {
        return Err(fail(format!(
            "distill: missing semantic binary: {sem_bin}\n\
             build it with: cargo build --release --features semantic --target-dir target/ablate-sem"
        )));
    }
    let (weights, log) = distill(&sem_bin)?;
    let base = baseline(&sem_bin)?;
    let rows = eval(&sem_bin, &weights)?;
    println!();
    println!("蒸馏后终验（semantic 开）:");
    for row in &rows {
        let (base_num, base_tok) = base
            .iter()
            .find(|(name, _)| *name == row.set)
            .map(|(_, values)| *values)
            .unwrap_or((0.0, 0.0));
        let flag = if row.num_cov >= base_num - 0.01 && row.token_cov >= base_tok - 0.01 {
            "OK"
        } else {
            "FID-DROP"
        };
        println!(
            "  {}: cut={:.4} num_cov={:.4} token_cov={:.4} vs base ({base_num:.4}/{base_tok:.4}) {flag}",
            row.set, row.cut, row.num_cov, row.token_cov
        );
    }
    write_weights(&out, &weights)?;
    let final_sha = sha(&weights);
    let relative = Path::new(&out)
        .strip_prefix(ROOT)
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|_| out.clone());
    println!("写入 {relative} (sha256={}…)", &final_sha[..16]);
    write_log(&log_path, &log)?;
    println!("蒸馏日志写入 config/bench/results/distill_log.csv");
    Ok(())
}

fn validate(args: &[String]) -> Result<(), Fail> {
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--sem-bin" | "--out" | "--log" => {
                if args.get(i + 1).is_none() {
                    return Err(usage(format!("{} expects a value", args[i])));
                }
                i += 2;
            }
            "-h" | "--help" => return Err(usage(String::new())),
            other => return Err(usage(format!("unrecognized argument: {other}"))),
        }
    }
    Ok(())
}

fn value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
}

fn corpora() -> [(&'static str, String, &'static str); 3] {
    [
        ("code", bench::corpus_code(), "build"),
        ("log", bench::corpus_log(), "timeout"),
        ("zh", bench::corpus_zh(), "超时"),
    ]
}

fn write_weights(path: &str, weights: &[f64]) -> Result<(), Fail> {
    let rounded: Vec<f64> = weights.iter().map(|value| round6(*value)).collect();
    let mut payload = Vec::new();
    for value in &rounded {
        payload.extend_from_slice(&value.to_be_bytes());
    }
    for feature in FEATURES {
        payload.extend_from_slice(feature.as_bytes());
        payload.push(b'\n');
    }
    let sha = scx::hash::hex(&scx::hash::sha256(&payload));
    let value = J::Obj(vec![
        ("schema".to_string(), J::Str("scx-sem-2".to_string())),
        (
            "features".to_string(),
            J::Arr(
                FEATURES
                    .iter()
                    .map(|name| J::Str(name.to_string()))
                    .collect(),
            ),
        ),
        (
            "weights".to_string(),
            J::Arr(rounded.iter().map(|value| J::Num(*value)).collect()),
        ),
        ("sha256".to_string(), J::Str(sha)),
    ]);
    io::write_json_indent(path, &value, 2).map_err(fail)
}

fn eval(bin: &str, weights: &[f64]) -> Result<Vec<Row>, Fail> {
    let dir = temp_dir();
    fs::create_dir_all(&dir).map_err(|e| fail(format!("{}: {e}", dir.display())))?;
    let wp = dir.join("weights.json");
    write_weights(&wp.to_string_lossy(), weights)?;
    let mut rows = Vec::new();
    for (name, text, query) in corpora() {
        let out = bench::compress_text(
            bin,
            &text,
            query,
            &["--semantic".to_string(), wp.to_string_lossy().to_string()],
        )?;
        let full_words = bench::tokens(&text);
        let full_nums = bench::num_tokens(&text);
        let out_words = bench::tokens(&out);
        let out_nums = bench::num_tokens(&out);
        rows.push(Row {
            set: name.to_string(),
            cut: 1.0 - scx::terms::estimate(&out) / scx::terms::estimate(&text),
            num_cov: bench::cov(&full_nums, &out_nums),
            token_cov: bench::cov(&full_words, &out_words),
        });
    }
    let _ = fs::remove_dir_all(&dir);
    Ok(rows)
}

type Fidelity = (f64, f64);

fn baseline(bin: &str) -> Result<Vec<(String, Fidelity)>, Fail> {
    let mut out = Vec::new();
    for (name, text, query) in corpora() {
        let o = bench::compress_text(bin, &text, query, &[])?;
        out.push((
            name.to_string(),
            (
                bench::cov(&bench::num_tokens(&text), &bench::num_tokens(&o)),
                bench::cov(&bench::tokens(&text), &bench::tokens(&o)),
            ),
        ));
    }
    Ok(out)
}

fn score(rows: &[Row], base: &[(String, Fidelity)]) -> f64 {
    let cut = contain::compensated(rows.iter().map(|row| row.cut)) / rows.len() as f64;
    let mut penalty = 0.0;
    for row in rows {
        let (num, tok) = base
            .iter()
            .find(|(name, _)| *name == row.set)
            .map(|(_, values)| *values)
            .unwrap_or((0.0, 0.0));
        for (base_value, value) in [(num, row.num_cov), (tok, row.token_cov)] {
            let drop = base_value - value - TOL;
            if drop > 0.0 {
                penalty += drop * 5.0;
            }
        }
    }
    cut - penalty
}

fn distill(bin: &str) -> Result<(Vec<f64>, Vec<LogRow>), Fail> {
    let base = baseline(bin)?;
    println!("教师基线（semantic 关）fidelity:");
    for (name, (num, tok)) in &base {
        println!("  {name}: num_cov={num:.4} token_cov={tok:.4}");
    }
    let mut cur = INIT.to_vec();
    let mut score0 = score(&eval(bin, &cur)?, &base);
    println!("初始: cut_avg={score0:.4}");
    let mut log = Vec::new();
    for (round, grid) in GRIDS.iter().enumerate() {
        let round = round + 1;
        let mut improved = false;
        for (dim, feature) in FEATURES.iter().enumerate() {
            let mut best_v = cur[dim];
            let mut best_s = score0;
            for dv in *grid {
                let mut cand = cur.clone();
                cand[dim] = round6(cur[dim] + dv);
                cand[dim] = cand[dim].clamp(-0.6, 0.6);
                if (cand[dim] - cur[dim]).abs() < 1e-9 {
                    continue;
                }
                let s = score(&eval(bin, &cand)?, &base);
                if s > best_s + 1e-9 {
                    best_v = cand[dim];
                    best_s = s;
                }
            }
            if best_v != cur[dim] {
                cur[dim] = best_v;
                score0 = best_s;
                improved = true;
            }
            log.push(LogRow {
                round,
                dim: feature,
                w: cur[dim],
                score: round5(score0),
            });
            println!(
                "  r{round} {feature:>10} w={:+.6} score={score0:.4}",
                cur[dim]
            );
        }
        if !improved {
            println!("轮 {round} 无改进，收敛");
            break;
        }
    }
    println!(
        "蒸馏完成: {}",
        cur.iter()
            .map(|value| format!("{value:+.4}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok((cur, log))
}

fn write_log(path: &str, rows: &[LogRow]) -> Result<(), Fail> {
    let fields = vec![
        "round".to_string(),
        "dim".to_string(),
        "w".to_string(),
        "score".to_string(),
    ];
    let values: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            vec![
                row.round.to_string(),
                row.dim.to_string(),
                io::repr(row.w),
                io::repr(row.score),
            ]
        })
        .collect();
    io::write_csv_with(path, &fields, &values, "\r\n").map_err(fail)
}

fn sha(weights: &[f64]) -> String {
    let mut payload = Vec::new();
    for value in weights {
        payload.extend_from_slice(&round6(*value).to_be_bytes());
    }
    for feature in FEATURES {
        payload.extend_from_slice(feature.as_bytes());
        payload.push(b'\n');
    }
    scx::hash::hex(&scx::hash::sha256(&payload))
}

fn round6(value: f64) -> f64 {
    format!("{value:.6}").parse().unwrap_or(value)
}

fn round5(value: f64) -> f64 {
    format!("{value:.5}").parse().unwrap_or(value)
}

fn temp_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "scx-distill-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weight_payload_shape_is_stable() {
        let weights = [0.0; 18];
        assert_eq!(round6(0.1234567), 0.123457);
        assert_eq!(sha(&weights).len(), 64);
    }
}
