//! `xtask fit`：弱标签线性权重拟合（scx-sem-1，feature = "eval"）。
//!
//! 迁移自 `tools/bench.py fit`：从仓库文档/源码行提取 16 维特征，弱监督打标，
//! 纯梯度下降逻辑回归，写入 `config/semantic.json`。

// 特征提取/拟合分支线性；认知复杂度门禁针对压缩内核，不适用于迁移工具。
#![allow(clippy::cognitive_complexity)]

use super::io;
use super::{arg_value as value, fail, Fail};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const DIM: usize = 16;
const PREFIXES: [&str; 11] = [
    "```",
    "~~~",
    "import ",
    "use ",
    "#include",
    "at ",
    "caused by:",
    "->",
    "=>",
    "panic! ",
    "error:",
];
const WORD_CHARS: &str = "abcdefghijklmnopqrstuvwxyz0123456789_-./:@\\";
const QUERIES: [&str; 7] = [
    "error",
    "compress",
    "token",
    "memory",
    "performance",
    "crash",
    "panic",
];

pub(crate) fn run(args: &[String]) -> Result<(), Fail> {
    validate(args)?;
    let out = value(args, "--out")
        .map(str::to_string)
        .unwrap_or_else(|| format!("{ROOT}/config/semantic.json"));
    let raw = load_lines();
    if raw.len() < 200 {
        eprintln!("语料不足: {} 行", raw.len());
        return Err(Fail {
            msg: String::new(),
            code: 1,
        });
    }
    let step = (raw.len() / 1500).max(1);
    let sampled: Vec<&String> = raw.iter().step_by(step).take(1500).collect();
    let total = sampled.len();
    let mut x = Vec::with_capacity(total);
    let mut y = Vec::with_capacity(total);
    for (index, line) in sampled.iter().enumerate() {
        x.push(feats(line, &QUERIES, index, total));
        y.push(label(line));
    }
    let pos = y.iter().filter(|value| **value == 1).count();
    println!(
        "语料 {total} 行, 正样本 {pos} ({:.1}%)",
        pos as f64 / total as f64 * 100.0
    );
    let w = fit(&x, &y);
    println!(
        "权重: {}",
        w.iter()
            .map(|value| format!("{value:+.3}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let weights: Vec<f64> = w.iter().map(|value| io::round6(*value)).collect();
    let sha = scx::hash::hex(&scx::hash::sha256(&encode(&weights)));
    let body = format!(
        "{{\n  \"schema\": \"scx-sem-1\",\n  \"dim\": {DIM},\n  \"weights\": [{}],\n  \"sha256\": \"{sha}\"\n}}\n",
        weights
            .iter()
            .map(|value| io::repr(*value))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if let Some(parent) = Path::new(&out).parent() {
        fs::create_dir_all(parent).map_err(|e| fail(format!("{out}: {e}")))?;
    }
    fs::write(&out, body).map_err(|e| fail(format!("{out}: {e}")))?;
    let relative = Path::new(&out)
        .strip_prefix(ROOT)
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|_| out.clone());
    println!("写入 {relative} (sha256={}…)", &sha[..16]);
    Ok(())
}

fn validate(args: &[String]) -> Result<(), Fail> {
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--out" => {
                if args.get(i + 1).is_none() {
                    return Err(usage("--out expects a path"));
                }
                i += 2;
            }
            "-h" | "--help" => return Err(usage(String::new())),
            other => return Err(usage(format!("unrecognized argument: {other}"))),
        }
    }
    Ok(())
}

fn usage(message: impl Into<String>) -> Fail {
    let body = "usage: xtask fit [--out PATH]\n";
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

fn load_lines() -> Vec<String> {
    let files = [
        "README.md",
        "docs/benchmarks.md",
        "docs/release-checklist.md",
        "src/lib.rs",
        "src/trim.rs",
        "src/semantic.rs",
    ];
    let mut lines = Vec::new();
    for file in files {
        let path = PathBuf::from(ROOT).join(file);
        if let Ok(text) = fs::read_to_string(path) {
            lines.extend(
                text.split('\n')
                    .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
                    .filter(|line| !line.trim().is_empty()),
            );
        }
    }
    lines
}

fn label(line: &str) -> i32 {
    let lower = line.to_lowercase();
    let failure = ["error", "panic", "fail", "exception", "crash", "fatal"]
        .iter()
        .any(|key| lower.contains(key));
    if failure {
        return 1;
    }
    if [
        "import ", "use ", "#include", "fn ", "pub ", "struct ", "impl ",
    ]
    .iter()
    .any(|prefix| line.starts_with(prefix))
    {
        return 1;
    }
    if QUERIES.iter().any(|query| lower.contains(query)) {
        return 1;
    }
    0
}

fn feats(line: &str, terms: &[&str], i: usize, total: usize) -> Vec<f64> {
    let lower = line.to_lowercase();
    let hit = terms
        .iter()
        .filter(|term| !term.is_empty() && lower.contains(**term))
        .count();
    let overlap = if terms.is_empty() {
        0.0
    } else {
        hit as f64 / terms.len() as f64
    };
    let chars = line.chars().count();
    let caps = line.chars().filter(|c| c.is_uppercase()).count();
    let digits = line.chars().filter(|c| c.is_numeric()).count();
    let cap_ratio = if chars == 0 {
        0.0
    } else {
        caps as f64 / chars as f64
    };
    let dig_ratio = if chars == 0 {
        0.0
    } else {
        digits as f64 / chars as f64
    };
    let trimmed = line.trim_start();
    let trimmed_lower = trimmed.to_lowercase();
    let structure = if PREFIXES
        .iter()
        .any(|prefix| trimmed.starts_with(prefix) || trimmed_lower.starts_with(prefix))
    {
        1.0
    } else {
        0.0
    };
    let toks = tokens(&lower);
    let mut n_hit = 0usize;
    for query in terms {
        if query.is_empty() {
            continue;
        }
        let plain = query.is_ascii() && query.chars().all(|c| c.is_ascii_alphabetic());
        if toks.iter().any(|tok| root_match(tok, query)) || (!plain && lower.contains(query)) {
            n_hit += 1;
        }
    }
    let w_hit = if terms.is_empty() {
        0.0
    } else {
        n_hit as f64 / terms.len() as f64
    };
    let cjk = line
        .chars()
        .filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c))
        .count();
    let cjk_r = if chars == 0 {
        0.0
    } else {
        cjk as f64 / chars as f64
    };
    let err = toks
        .iter()
        .filter(|tok| tok.chars().any(|c| c.is_alphabetic()) && tok.chars().any(|c| c.is_numeric()))
        .count();
    let err_r = if toks.is_empty() {
        0.0
    } else {
        err as f64 / toks.len() as f64
    };
    let uniq = if toks.is_empty() {
        0.0
    } else {
        toks.iter().collect::<HashSet<_>>().len() as f64 / toks.len() as f64
    };
    let sym = line
        .chars()
        .filter(|c| !c.is_whitespace() && !c.is_alphanumeric())
        .count();
    let non_space = line.chars().filter(|c| !c.is_whitespace()).count();
    let sym_r = if non_space == 0 {
        0.0
    } else {
        sym as f64 / non_space as f64
    };
    let pos = if total > 1 {
        i as f64 / (total - 1) as f64
    } else {
        0.0
    };
    let edge = if i < total / 20 || i >= total - total / 20 {
        1.0
    } else {
        0.0
    };
    let long_f = if chars > 200 { 1.0 } else { 0.0 };
    let code = if (line.contains('{') && line.contains('}'))
        || (line.contains('(') && line.contains(')'))
        || line.contains("->")
        || line.contains("=>")
        || line.contains("::")
    {
        1.0
    } else {
        0.0
    };
    vec![
        overlap,
        hit.min(1) as f64,
        (chars as f64).ln_1p(),
        cap_ratio,
        dig_ratio,
        structure,
        w_hit,
        cjk_r,
        err_r,
        uniq,
        sym_r,
        pos,
        edge,
        long_f,
        code,
        1.0,
    ]
}

fn tokens(lower: &str) -> Vec<String> {
    let bytes = lower.as_bytes();
    let chars: Vec<char> = lower.chars().collect();
    let n = bytes.len();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut has = false;
    let mut i = 0;
    while i < n {
        let byte = bytes[i];
        let is_word =
            byte < 128 && ((byte as char).is_alphanumeric() || WORD_CHARS.contains(byte as char));
        let is_cjk = (0xE4..=0xE9).contains(&byte) && i + 2 < n;
        if is_word || is_cjk {
            let take = if is_cjk { 3 } else { 1 };
            let token: String = chars.iter().skip(i).take(take).collect();
            cur.push_str(&token);
            has = true;
            i += take;
        } else {
            if has {
                out.push(std::mem::take(&mut cur));
                has = false;
            }
            i += 1;
        }
    }
    if has {
        out.push(cur);
    }
    out
}

fn stem(word: &str) -> Option<String> {
    if word.chars().count() < 4
        || !word.is_ascii()
        || !word.chars().all(|c| c.is_ascii_lowercase())
        || !word.chars().all(|c| c.is_ascii_alphabetic())
    {
        return None;
    }
    if word.strip_suffix("ing").is_some() && word.len() - 3 >= 4 {
        return Some(word[..word.len() - 3].to_string());
    }
    if word.strip_suffix("ed").is_some() && word.len() - 2 >= 4 {
        return Some(word[..word.len() - 2].to_string());
    }
    if word.strip_suffix("ies").is_some() {
        return Some(format!("{}y", &word[..word.len() - 3]));
    }
    if word.strip_suffix("es").is_some() {
        let base = &word[..word.len() - 2];
        if base.ends_with("ch")
            || base.ends_with("sh")
            || base.ends_with('s')
            || base.ends_with('x')
            || base.ends_with('z')
        {
            return Some(base.to_string());
        }
    }
    if let Some(base) = word.strip_suffix('s') {
        if !word.ends_with("ss") && base.len() >= 3 {
            return Some(base.to_string());
        }
    }
    None
}

fn root_match(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let sa = stem(a);
    let sb = stem(b);
    match (sa, sb) {
        (Some(sa), Some(sb)) => sa == sb,
        (Some(sa), None) => sa == b,
        (None, Some(sb)) => a == sb,
        (None, None) => false,
    }
}

fn sigmoid(z: f64) -> f64 {
    if z >= 0.0 {
        1.0 / (1.0 + (-z).exp())
    } else {
        let e = z.exp();
        e / (1.0 + e)
    }
}

fn fit(x: &[Vec<f64>], y: &[i32]) -> Vec<f64> {
    let n = x.len() as f64;
    let mut w = vec![0.0f64; DIM];
    for _ in 0..120 {
        let mut grad = vec![0.0f64; DIM];
        for (row, target) in x.iter().zip(y) {
            let z = w
                .iter()
                .zip(row)
                .fold(0.0, |sum, (weight, value)| sum + weight * value);
            let err = sigmoid(z) - *target as f64;
            for (slot, value) in grad.iter_mut().zip(row) {
                *slot += err * value;
            }
        }
        for (weight, gradient) in w.iter_mut().zip(grad) {
            *weight -= 0.05 * (gradient / n + 1e-3 * *weight);
        }
    }
    w
}

fn encode(weights: &[f64]) -> Vec<u8> {
    weights
        .iter()
        .flat_map(|value| value.to_be_bytes())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_shape_and_label() {
        let f = feats("error: database timeout", &QUERIES, 0, 10);
        assert_eq!(f.len(), DIM);
        assert_eq!(label("ERROR: crash"), 1);
        assert_eq!(label("plain text"), 0);
    }

    #[test]
    fn tokens_match_reference() {
        assert_eq!(tokens("abc/def 123"), vec!["abc/def", "123"]);
    }
}
