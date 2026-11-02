//! `scx-tooling calibrate`：BUDGET_SAFETY_FACTOR 上界系数标定（feature = "eval"）。
//!
//! 语料、启发式估计、分位数与报告均在 Rust；tiktoken cl100k_base 真值由
//! 内置词表（`config/tokenizer/cl100k_base.tiktoken`）与贪心 BPE 计算，
//! 不再依赖 Python tiktoken。迁移自 `tools/bench.py calibrate`。

use super::{arg_value as value, fail, load_json as load, Fail};
use scx::json::Json;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

const VOCAB: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/config/tokenizer/cl100k_base.tiktoken"
);

const QS: [f64; 5] = [0.50, 0.90, 0.95, 0.99, 1.00];

pub(crate) fn run(args: &[String]) -> Result<(), Fail> {
    let corpus = value(args, "--corpus");
    let samples = match corpus {
        Some(path) => load_samples(path)?,
        None => default_corpus(),
    };
    if samples.is_empty() {
        return Err(fail("calibrate: empty corpus"));
    }
    let ranks = load_ranks(VOCAB).map_err(fail)?;
    let truth: Vec<f64> = samples
        .iter()
        .map(|sample| encode(&ranks, sample.as_bytes()).len() as f64)
        .collect();
    let mut ratios: Vec<f64> = samples
        .iter()
        .zip(&truth)
        .map(|(sample, tokens)| *tokens / scx::terms::estimate(sample))
        .collect();
    ratios.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = ratios.len();
    let vals: Vec<f64> = QS
        .iter()
        .map(|q| ratios[((q * (n - 1) as f64) as usize).min(n - 1)])
        .collect();
    println!("样本数={n}  比率（真值 / 启发式）分位数：");
    for (q, value) in QS.iter().zip(&vals) {
        println!("  P{:<4} = {value:.3}", (q * 100.0) as i32);
    }
    println!();
    println!(
        "建议 BUDGET_SAFETY_FACTOR = {:.3}  （P95；更保守可改用 P99 = {:.3}）",
        vals[2], vals[3]
    );
    println!(
        "写入 src/terms.rs：`pub(crate) const BUDGET_SAFETY_FACTOR: f64 = {:.3};`",
        vals[2]
    );
    Ok(())
}

fn load_samples(path: &str) -> Result<Vec<String>, Fail> {
    let value = load(Path::new(path))?;
    match value {
        Json::Arr(items) => Ok(items
            .iter()
            .filter_map(Json::str)
            .map(str::to_string)
            .collect()),
        _ => Err(fail("calibrate: corpus must be a JSON array of strings")),
    }
}

/// `tools/bench.py default_corpus()` 的确定性复刻。
fn default_corpus() -> Vec<String> {
    let mut samples = Vec::new();
    for i in 0..40 {
        let mut lines = Vec::new();
        for j in 0..(5 + i % 6) {
            lines.push(format!(
                "INFO heartbeat seq={} ok pool=3 latency=1ms",
                i * 10 + j
            ));
        }
        lines.push("ERROR db timeout at 12:01 pool exhausted (conn_id=9f2a) retry=3".to_string());
        samples.push(lines.join("\n"));
    }
    for i in 0..40 {
        samples.push("The quick brown fox jumps over the lazy dog ".repeat(i % 10 + 1));
        samples.push("数据库连接超时 连接池耗尽 磁盘空间不足 errno=28".to_string());
        samples.push("short".to_string());
        samples.push("hello world".to_string());
    }
    for _ in 0..40 {
        samples.push("fn main() {\n    let x = 1 + 2;\n    println!(\"{}\", x);\n}\n".to_string());
        samples.push(
            "{\n  \"type\": \"object\",\n  \"properties\": {\n    \"id\": {\"type\": \"integer\"}\n  }\n}"
                .to_string(),
        );
    }
    for i in 0..30 {
        samples.push("Привет мир как дела ".repeat(i % 6 + 1));
        samples.push("مرحبا بالعالم كيف الحال ".repeat(i % 6 + 1));
        samples.push("🚀 emoji ✨ mixed 中文".to_string());
    }
    samples
}

fn load_ranks(path: &str) -> Result<HashMap<Vec<u8>, u32>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let mut ranks = HashMap::with_capacity(100_256);
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((encoded, rank)) = line.split_once(' ') else {
            return Err(format!("{path}:{index}: bad line {line:?}"));
        };
        let bytes = base64_decode(encoded).ok_or_else(|| format!("{path}:{index}: bad base64"))?;
        let rank: u32 = rank
            .parse()
            .map_err(|_| format!("{path}:{index}: bad rank {rank:?}"))?;
        ranks.insert(bytes, rank);
    }
    Ok(ranks)
}

/// 最长前缀匹配 BPE。对输入字节流，每个位置取词表中最长的字节前缀。
fn encode(ranks: &HashMap<Vec<u8>, u32>, bytes: &[u8]) -> Vec<u32> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let mut matched = None;
        for len in (1..=bytes.len() - i).rev() {
            if let Some(rank) = ranks.get(&bytes[i..i + len]) {
                matched = Some((len, *rank));
                break;
            }
        }
        match matched {
            Some((len, rank)) => {
                out.push(rank);
                i += len;
            }
            None => {
                out.push(0);
                i += 1;
            }
        }
    }
    out
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for c in s.bytes() {
        let value = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        acc = (acc << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_corpus_shape_is_stable() {
        assert_eq!(default_corpus().len(), 370);
    }

    #[test]
    fn quantile_index_matches_python() {
        let mut ratios = [0.5, 1.5, 2.0, 0.9];
        ratios.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let n = ratios.len();
        let vals: Vec<f64> = QS
            .iter()
            .map(|q| ratios[((q * (n - 1) as f64) as usize).min(n - 1)])
            .collect();
        assert_eq!(vals, vec![0.9, 1.5, 1.5, 1.5, 2.0]);
    }

    #[test]
    fn tokenizer_matches_tiktoken_vectors() {
        let ranks = load_ranks(VOCAB).unwrap();
        for (text, want) in [
            ("hello world", vec![15339, 1917]),
            ("today\n ", vec![31213, 198, 220]),
            ("👍", vec![9468, 239, 235]),
        ] {
            assert_eq!(encode(&ranks, text.as_bytes()), want, "text={text:?}");
        }
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
    }
}
