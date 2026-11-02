//! 离线权重生产（feature = "eval"）：`fit` 弱标签线性拟合（scx-sem-1），
//! `mlp` 在蒸馏缓存上做线性/MLP 交叉验证。权重只写 `config/semantic.json`。
//!
//! 迁移自 `tools/bench.py fit`：从仓库文档/源码行提取 16 维特征，弱监督打标，
//! 纯梯度下降逻辑回归，写入 `config/semantic.json`。

// 特征提取/拟合分支线性；认知复杂度门禁针对压缩内核，不适用于迁移工具。
#![allow(clippy::cognitive_complexity)]

use super::io::{self, J};
use super::rng::PyRng;
use super::{arg_value as value, fail, load_json as load, Fail};
use scx::json::Json;
use scx::semantic::{feats, FEATURE_NAMES};
use std::collections::{HashMap, HashSet};
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

pub(crate) fn fit(args: &[String]) -> Result<(), Fail> {
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
        x.push(features(line, &QUERIES, index, total));
        y.push(label(line));
    }
    let pos = y.iter().filter(|value| **value == 1).count();
    println!(
        "语料 {total} 行, 正样本 {pos} ({:.1}%)",
        pos as f64 / total as f64 * 100.0
    );
    let w = train_logistic(&x, &y);
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

fn features(line: &str, terms: &[&str], i: usize, total: usize) -> Vec<f64> {
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

fn train_logistic(x: &[Vec<f64>], y: &[i32]) -> Vec<f64> {
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

const MLP_H: usize = 16;

pub(crate) fn mlp(args: &[String]) -> Result<(), Fail> {
    let cache_path = value(args, "--cache")
        .map(str::to_string)
        .unwrap_or_else(|| "result:distill-cache.json".to_string());
    let out = value(args, "--out")
        .map(str::to_string)
        .unwrap_or_else(|| "/tmp/mlp_cv.json".to_string());
    let cache = load(&cache_path)?;
    let corpora = build_corpora()?;
    let (x, y) = dataset(&cache, &corpora);
    if x.is_empty() {
        println!("无样本（缓存为空或仅合成语料）");
        return Err(Fail {
            msg: String::new(),
            code: 1,
        });
    }
    let n = x.len();
    let dim = FEATURE_NAMES.len();
    let mut idx: Vec<usize> = (0..n).collect();
    PyRng::new(2024).shuffle(&mut idx);
    let folds: Vec<Vec<usize>> = (0..3)
        .map(|fold| idx.iter().skip(fold).step_by(3).copied().collect())
        .collect();
    let mut linear = Vec::new();
    let mut mlp = Vec::new();
    for (fi, fold) in folds.iter().enumerate() {
        let test: HashSet<usize> = fold.iter().copied().collect();
        let train: Vec<usize> = idx
            .iter()
            .copied()
            .filter(|index| !test.contains(index))
            .collect();
        let xt = train
            .iter()
            .map(|index| x[*index].clone())
            .collect::<Vec<_>>();
        let yt = train.iter().map(|index| y[*index]).collect::<Vec<_>>();
        let xe = fold
            .iter()
            .map(|index| x[*index].clone())
            .collect::<Vec<_>>();
        let ye = fold.iter().map(|index| y[*index]).collect::<Vec<_>>();
        let w = fit_linear(&xt, &yt);
        let ls: Vec<f64> = xe.iter().map(|row| dot(&w, row)).collect();
        let (auc, ap) = auc_ap(&ls, &ye);
        linear.push((auc, ap));
        let keep = ls.iter().filter(|score| **score > 0.0).count() as f64 / ls.len() as f64;
        let model = fit_mlp(&xt, &yt);
        let ms: Vec<f64> = xe.iter().map(|row| model.score(row)).collect();
        let (mauc, map) = auc_ap(&ms, &ye);
        mlp.push((mauc, map));
        let mkeep = ms.iter().filter(|score| **score > 0.5).count() as f64 / ms.len() as f64;
        println!(
            "fold {fi}: linear auc/ap={auc:.4}/{ap:.4} keep={keep:.3}  \
             mlp1 auc/ap={mauc:.4}/{map:.4} keep={mkeep:.3}"
        );
    }
    let result = J::Obj(vec![
        ("n".to_string(), J::Int(n as i64)),
        ("dim".to_string(), J::Int(dim as i64)),
        (
            "linear".to_string(),
            J::Obj(vec![
                (
                    "auc".to_string(),
                    J::Num(linear.iter().map(|(auc, _)| *auc).sum::<f64>() / 3.0),
                ),
                (
                    "ap".to_string(),
                    J::Num(linear.iter().map(|(_, ap)| *ap).sum::<f64>() / 3.0),
                ),
            ]),
        ),
        (
            "mlp1".to_string(),
            J::Obj(vec![
                (
                    "auc".to_string(),
                    J::Num(mlp.iter().map(|(auc, _)| *auc).sum::<f64>() / 3.0),
                ),
                (
                    "ap".to_string(),
                    J::Num(mlp.iter().map(|(_, ap)| *ap).sum::<f64>() / 3.0),
                ),
            ]),
        ),
        (
            "folds_linear".to_string(),
            J::Arr(
                linear
                    .iter()
                    .map(|(auc, ap)| J::Arr(vec![J::Num(*auc), J::Num(*ap)]))
                    .collect(),
            ),
        ),
        (
            "folds_mlp".to_string(),
            J::Arr(
                mlp.iter()
                    .map(|(auc, ap)| J::Arr(vec![J::Num(*auc), J::Num(*ap)]))
                    .collect(),
            ),
        ),
    ]);
    println!("{}", io::render_json_raw(&result, 2));
    io::write_json_raw(&out, &result, 2).map_err(fail)?;
    println!("已写入 {out}（不替换仓库权重）");
    Ok(())
}

fn build_corpora() -> Result<HashMap<String, String>, Fail> {
    let mut corpora = HashMap::new();
    corpora.insert("code".to_string(), super::bench::corpus_code());
    corpora.insert("log".to_string(), super::bench::corpus_log());
    corpora.insert("zh".to_string(), super::bench::corpus_zh());
    let perf = super::bench::perf_corpus(1_000_000);
    let lines: Vec<&str> = perf.lines().collect();
    for (name, lo, hi) in [
        ("real_src_a", 0usize, 2000usize),
        ("real_src_b", 12000, 14000),
        ("real_src_c", 24000, 26000),
    ] {
        let end = hi.min(lines.len());
        let start = lo.min(end);
        corpora.insert(name.to_string(), lines[start..end].join("\n"));
    }
    for (lang, file) in [
        ("c", "code-c-60.csv"),
        ("cpp", "code-cpp-60.csv"),
        ("cuda", "code-cuda-60.csv"),
        ("java", "code-java-60.csv"),
        ("js", "code-js-60.csv"),
        ("py", "code-python-60.csv"),
    ] {
        let Ok((_, rows)) = super::frozen::dataset(file) else {
            continue;
        };
        let questions: Vec<&str> = rows
            .iter()
            .filter_map(|row| row.get("question").map(String::as_str))
            .collect();
        let text = code_fence(&questions);
        if !text.trim().is_empty() {
            corpora.insert(format!("code_{lang}"), text);
        }
    }
    Ok(corpora)
}

fn code_fence(questions: &[&str]) -> String {
    let langs = ["c", "python", "java", "javascript", "cpp"];
    let mut out = Vec::new();
    for question in questions {
        let mut i = 0;
        while let Some(position) = question[i..].find("```") {
            let start = i + position + 3;
            let rest = &question[start..];
            let mut skip = None;
            for lang in langs {
                if rest.starts_with(lang) && rest[lang.len()..].starts_with('\n') {
                    skip = Some(lang.len() + 1);
                    break;
                }
            }
            if skip.is_none() && rest.starts_with('\n') {
                skip = Some(1);
            }
            if let Some(skip) = skip {
                if let Some(end) = rest[skip..].find("```") {
                    out.extend(rest[skip..skip + end].split('\n').map(str::to_string));
                    i = start + skip + end + 3;
                    continue;
                }
            }
            i = start;
        }
    }
    out.join("\n")
}

fn dataset(cache: &Json, corpora: &HashMap<String, String>) -> (Vec<Vec<f64>>, Vec<i32>) {
    let mut x = Vec::new();
    let mut y = Vec::new();
    let Json::Obj(entries) = cache else {
        return (x, y);
    };
    for (key, keep) in entries {
        let Some((name, query)) = key.rsplit_once('|') else {
            continue;
        };
        let Some(text) = corpora.get(name) else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        let total = lines.len();
        let kept: HashSet<i64> = match keep {
            Json::Arr(items) => items
                .iter()
                .filter_map(|value| match value {
                    Json::Num(number) => Some(*number as i64),
                    Json::Str(text) => text.parse().ok(),
                    _ => None,
                })
                .collect(),
            _ => HashSet::new(),
        };
        let terms = [query.to_string()];
        for (index, line) in lines.iter().enumerate() {
            x.push(feats(line, &terms, index, total, FEATURE_NAMES.len()));
            y.push(if kept.contains(&((index + 1) as i64)) {
                1
            } else {
                0
            });
        }
    }
    (x, y)
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).fold(0.0, |sum, (x, y)| sum + x * y)
}

fn fit_linear(x: &[Vec<f64>], y: &[i32]) -> Vec<f64> {
    let n = x.len() as f64;
    let dim = FEATURE_NAMES.len();
    let mut w = vec![0.0f64; dim];
    for _ in 0..150 {
        let mut grad = vec![0.0f64; dim];
        for (row, target) in x.iter().zip(y) {
            let err = sigmoid(dot(&w, row)) - *target as f64;
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

struct Mlp {
    w1: Vec<Vec<f64>>,
    b1: Vec<f64>,
    w2: Vec<f64>,
    b2: f64,
}

impl Mlp {
    fn score(&self, x: &[f64]) -> f64 {
        let hidden: Vec<f64> = (0..MLP_H)
            .map(|j| sigmoid(dot(&self.w1[j], x) + self.b1[j]))
            .collect();
        sigmoid(dot(&self.w2, &hidden) + self.b2)
    }
}

fn fit_mlp(x: &[Vec<f64>], y: &[i32]) -> Mlp {
    let mut rng = PyRng::new(42);
    let dim = FEATURE_NAMES.len();
    let mut w1 = vec![vec![0.0f64; dim]; MLP_H];
    for row in &mut w1 {
        for value in row {
            *value = -0.3 + 0.6 * rng.random();
        }
    }
    let mut b1 = vec![0.0f64; MLP_H];
    let mut w2: Vec<f64> = (0..MLP_H).map(|_| -0.3 + 0.6 * rng.random()).collect();
    let mut b2 = 0.0f64;
    for _ in 0..150 {
        for (index, target) in y.iter().enumerate() {
            let hidden: Vec<f64> = (0..MLP_H)
                .map(|j| sigmoid(dot(&w1[j], &x[index]) + b1[j]))
                .collect();
            let s = sigmoid(dot(&w2, &hidden) + b2);
            let err = s - *target as f64;
            let dz = err * s * (1.0 - s);
            for j in 0..MLP_H {
                w2[j] -= 0.05 * (dz * hidden[j] + 1e-3 * w2[j]);
                b1[j] -= 0.05 * (dz * w2[j] * hidden[j] * (1.0 - hidden[j]));
                for d in 0..dim {
                    w1[j][d] -= 0.05
                        * (dz * w2[j] * hidden[j] * (1.0 - hidden[j]) * x[index][d]
                            + 1e-3 * w1[j][d]);
                }
            }
            b2 -= 0.05 * dz;
        }
    }
    Mlp { w1, b1, w2, b2 }
}

fn auc_ap(scores: &[f64], y: &[i32]) -> (f64, f64) {
    let mut pairs: Vec<(f64, i32)> = scores.iter().copied().zip(y.iter().copied()).collect();
    pairs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let npos = y.iter().filter(|value| **value == 1).count();
    let nneg = y.len() - npos;
    let mut tp = 0usize;
    let mut fp = 0usize;
    let mut auc = 0.0f64;
    let mut prev: Option<f64> = None;
    for (score, label) in &pairs {
        if *label == 1 {
            tp += 1;
        } else {
            fp += 1;
            if prev != Some(*score) {
                auc += (fp * tp) as f64;
            }
        }
        prev = Some(*score);
    }
    let auc = if npos > 0 && nneg > 0 {
        auc / (npos * nneg) as f64
    } else {
        0.5
    };
    let mut tp = 0usize;
    let mut fp = 0usize;
    let mut ap = 0.0f64;
    let mut last_rec = 0.0f64;
    for (_, label) in &pairs {
        if *label == 1 {
            tp += 1;
        } else {
            fp += 1;
        }
        let rec = if npos == 0 {
            0.0
        } else {
            tp as f64 / npos as f64
        };
        let prec = if tp + fp == 0 {
            0.0
        } else {
            tp as f64 / (tp + fp) as f64
        };
        ap += prec * (rec - last_rec);
        last_rec = rec;
    }
    (auc, ap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auc_ap_reference() {
        let (auc, ap) = auc_ap(&[0.9, 0.8, 0.1], &[1, 0, 0]);
        assert_eq!(auc, 1.5);
        assert!(ap > 0.0);
    }

    #[test]
    fn feature_shape_and_label() {
        let f = features("error: database timeout", &QUERIES, 0, 10);
        assert_eq!(f.len(), DIM);
        assert_eq!(label("ERROR: crash"), 1);
        assert_eq!(label("plain text"), 0);
    }

    #[test]
    fn tokens_match_reference() {
        assert_eq!(tokens("abc/def 123"), vec!["abc/def", "123"]);
    }
}
