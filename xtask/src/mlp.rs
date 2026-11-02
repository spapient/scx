//! `xtask mlp`：语义特征线性/MLP 交叉验证。
//!
//! 迁移自原 Python 工具的 `mlp`：从 `distill_cache.json` 重建全行样本，
//! 3 折确定性交叉验证，输出 AUC/AP 与 keep 比例。24 维特征复用
//! `scx::semantic::feats`，与 Python `DO_feats` 同源。

// 交叉验证分支线性；认知复杂度门禁针对压缩内核，不适用于迁移工具。
#![allow(clippy::cognitive_complexity)]

use super::io::{self, J};
use super::rng::PyRng;
use super::{fail, Fail};
use scx::json::Json;
use scx::semantic::{feats, FEATURE_NAMES};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
const MLP_H: usize = 16;

pub(crate) fn run(args: &[String]) -> Result<(), Fail> {
    let cache_path = value(args, "--cache")
        .map(str::to_string)
        .unwrap_or_else(|| format!("{ROOT}/config/bench/results/distill_cache.json"));
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

fn value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
}

fn load(path: &str) -> Result<Json, Fail> {
    let text = fs::read_to_string(path).map_err(|e| fail(format!("{path}: {e}")))?;
    scx::json::parse(&text).map_err(|e| fail(format!("{path}: {e}")))
}

fn build_corpora() -> Result<HashMap<String, String>, Fail> {
    let mut corpora = HashMap::new();
    corpora.insert("code".to_string(), super::bench::corpus_code());
    corpora.insert("log".to_string(), super::bench::corpus_log());
    corpora.insert("zh".to_string(), super::bench::corpus_zh());
    let perf = Path::new(ROOT).join("config/bench/perf/corpus_p1m.txt");
    if let Ok(text) = fs::read_to_string(&perf) {
        let lines: Vec<&str> = text.lines().collect();
        for (name, lo, hi, queries) in [
            ("real_src_a", 0usize, 2000usize, "drop"),
            ("real_src_b", 12000, 14000, "memory"),
            ("real_src_c", 24000, 26000, "stdin"),
        ] {
            let _ = queries;
            let end = hi.min(lines.len());
            let start = lo.min(end);
            corpora.insert(name.to_string(), lines[start..end].join("\n"));
        }
    }
    let datasets = Path::new(ROOT).join("config/bench/datasets");
    for (lang, file) in [
        ("c", "bench_c60.csv"),
        ("cpp", "bench_cpp60.csv"),
        ("cuda", "bench_cuda60.csv"),
        ("java", "bench_java60.csv"),
        ("js", "bench_js60.csv"),
        ("py", "bench_py60.csv"),
    ] {
        let path = datasets.join(file);
        let Ok((_, rows)) = io::read_csv(&path.to_string_lossy()) else {
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

fn sigmoid(z: f64) -> f64 {
    let z = z.clamp(-30.0, 30.0);
    1.0 / (1.0 + (-z).exp())
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
}
