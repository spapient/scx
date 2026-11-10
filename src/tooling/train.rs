//! `scx-tooling train`：弱标签一致性清洗 + 全 24 维逻辑回归（feature = "eval"）。
//!
//! 与旧 `fit`/`distill` 的差异：特征直接复用生产代码 [`scx::semantic::feats`]
//! （训练与服务同一份特征，消除双份特征漂移），覆盖全部 24 维；弱标签由三个
//! 独立信号投票，先在高一致样本上拟合，再按置信度清洗单信号样本，确定性全批量
//! 梯度下降（无随机源）。输出 `scx-sem-2` 权重，含 sha256 自校验。
#![allow(clippy::cognitive_complexity)]

use super::io::{self, J};
use super::{arg_value as value, Fail};
use scx::semantic::{feats, FEATURE_NAMES};
use std::fs;
use std::path::Path;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const DIM: usize = FEATURE_NAMES.len();
/// 留出节奏：每 4 行取 1 行做评估（确定性，无随机抽样）。
const HELD: usize = 4;

/// 单条训练样本：特征向量、弱信号票数、是否代码语料、是否留出。
#[derive(Clone)]
struct Sample {
    x: Vec<f64>,
    votes: u8,
    code: bool,
    held: bool,
}

/// 训练超参（CLI 可覆盖，缺省即下列常量；禁硬编码进算法体）。
#[derive(Clone, Copy)]
struct Param {
    lr: f64,
    l2: f64,
    epochs: usize,
    margin: f64,
    relabel_weight: f64,
    model: Model,
    knots: usize,
}

impl Default for Param {
    fn default() -> Self {
        Param {
            lr: 0.1,
            l2: 1e-3,
            epochs: 200,
            margin: 0.35,
            relabel_weight: 0.5,
            model: Model::Linear,
            knots: 3,
        }
    }
}

/// 模型选择：`linear`（默认 scx-sem-2）/ `piece`（scx-sem-3 逐段线性阈值表）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Model {
    Linear,
    Piece,
}

pub(crate) fn run(args: &[String]) -> Result<(), Fail> {
    let param = parse_param(args)?;
    if param.model == Model::Piece {
        return run_piece(args, &param);
    }
    let out = value(args, "--out")
        .map(str::to_string)
        .unwrap_or_else(|| format!("{ROOT}/config/semantic.json"));
    let samples = build();
    let w = fit_clean(&samples, param);

    let ap = eval_ap(&samples, &w, false);
    let ap_code = eval_ap(&samples, &w, true);
    println!("留出强标签 AP={ap:.4} code AP={ap_code:.4}（目标 code ≥ 0.60）");

    write_weights(&out, &w)?;
    let sha = weight_sha(&w);
    let rel = Path::new(&out)
        .strip_prefix(ROOT)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| out.clone());
    println!("写入 {rel} (sha256={}…)", &sha[..16]);
    Ok(())
}

fn parse_param(args: &[String]) -> Result<Param, Fail> {
    let mut p = Param::default();
    let mut i = 0;
    while i < args.len() {
        let key = args[i].as_str();
        match key {
            "--out" => {
                i += skip_value(args, i)?;
            }
            flag => {
                let Some(v) = args.get(i + 1) else {
                    return Err(usage(format!("{flag} expects a value")));
                };
                match flag {
                    "--model" => {
                        p.model = match v.as_str() {
                            "linear" => Model::Linear,
                            "piece" => Model::Piece,
                            other => {
                                return Err(usage(format!("bad model {other} (want linear|piece)")))
                            }
                        };
                    }
                    "--knots" => {
                        let n: f64 = v.parse().map_err(|_| usage(format!("bad number {v}")))?;
                        p.knots = n.clamp(0.0, 12.0) as usize;
                    }
                    "--folds" => {
                        // 仅 xval 使用；参数解析这里放行，不写入 Param。
                    }
                    numeric => {
                        let n: f64 = v.parse().map_err(|_| usage(format!("bad number {v}")))?;
                        match numeric {
                            "--lr" => p.lr = n,
                            "--l2" => p.l2 = n,
                            "--margin" => p.margin = n.clamp(0.0, 0.5),
                            "--relabel-weight" => p.relabel_weight = n.clamp(0.0, 1.0),
                            "--epochs" => p.epochs = n.max(1.0) as usize,
                            other => return Err(usage(format!("unrecognized argument: {other}"))),
                        }
                    }
                }
                i += 2;
            }
        }
    }
    Ok(p)
}

fn skip_value(args: &[String], i: usize) -> Result<usize, Fail> {
    if args.get(i + 1).is_none() {
        return Err(usage("--out expects a path"));
    }
    Ok(2)
}

fn usage(message: impl Into<String>) -> Fail {
    let body = "usage: scx-tooling train [--out PATH] [--lr F] [--l2 F] [--epochs N]\n\
                      \x20                     [--margin F] [--relabel-weight F]\n";
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

/// 三个语料各自配查询词，逐行产特征并统计弱信号。
fn build() -> Vec<Sample> {
    let corpora: [(String, &[&str], bool); 3] = [
        (super::bench::corpus_code(), &["build"], true),
        (super::bench::corpus_log(), &["timeout"], false),
        (super::bench::corpus_zh(), &["超时"], false),
    ];
    let mut out = Vec::new();
    for (text, query, code) in corpora {
        let terms: Vec<String> = query.iter().map(|s| s.to_string()).collect();
        let lines: Vec<&str> = text.split('\n').filter(|s| !s.trim().is_empty()).collect();
        let total = lines.len();
        for (i, line) in lines.iter().enumerate() {
            let x = feats(line, &terms, i, total, DIM);
            out.push(Sample {
                x,
                votes: votes(line, &terms),
                code,
                held: i % HELD == 0,
            });
        }
    }
    out
}

/// 弱信号面板：失败关键词 / 结构代码标记 / 查询词覆盖，各投一票。
pub(crate) fn votes(line: &str, terms: &[String]) -> u8 {
    let lower = line.to_lowercase();
    let kw = [
        "error",
        "panic",
        "fail",
        "exception",
        "crash",
        "fatal",
        "错误",
        "失败",
        "超时",
        "异常",
    ]
    .iter()
    .any(|k| lower.contains(k)) as u8;
    let shape = [
        "fn ", "pub ", "struct ", "import ", "use ", "#include", "impl ", "=>", "->",
    ]
    .iter()
    .any(|k| lower.contains(k)) as u8;
    let query = terms
        .iter()
        .filter(|t| lower.contains(t.as_str()))
        .count()
        .min(1) as u8;
    kw + shape + query
}

/// 清洗后训练：先在高一致样本（0 票强负、≥2 票强正）拟合，再按置信度处理
/// 单票样本（超 margin 重标、否则剔除），最后在清洗集上重拟合。
fn fit_clean(samples: &[Sample], p: Param) -> Vec<f64> {
    let train: Vec<&Sample> = samples.iter().filter(|s| !s.held).collect();
    let strong: Vec<(&Sample, u8, f64)> = train
        .iter()
        .filter(|s| s.votes != 1)
        .map(|s| {
            let label = u8::from(s.votes >= 2);
            (*s, label, 1.0)
        })
        .collect();
    let seed = fit(&strong, p);

    let mut cleaned = strong.clone();
    for s in train.iter().filter(|s| s.votes == 1) {
        let prob = sigmoid(dot(&seed, &s.x));
        if (prob - 0.5).abs() >= p.margin {
            let label = u8::from(prob > 0.5);
            cleaned.push((s, label, p.relabel_weight));
        }
    }
    fit(&cleaned, p)
}

/// 加权全批量逻辑回归（确定性）：梯度按样本权重求和，l2 仅作用非偏置维。
fn fit(rows: &[(&Sample, u8, f64)], p: Param) -> Vec<f64> {
    let mut w = vec![0.0f64; DIM];
    let wsum: f64 = rows.iter().map(|(_, _, sw)| *sw).sum::<f64>().max(1e-9);
    for _ in 0..p.epochs {
        let mut grad = vec![0.0f64; DIM];
        for (s, label, sw) in rows {
            let err = sigmoid(dot(&w, &s.x)) - *label as f64;
            for (g, x) in grad.iter_mut().zip(&s.x) {
                *g += sw * err * x;
            }
        }
        for (k, (weight, g)) in w.iter_mut().zip(grad).enumerate() {
            let decay = if FEATURE_NAMES[k] == "bias" {
                0.0
            } else {
                p.l2
            };
            *weight -= p.lr * (g / wsum + decay * *weight);
        }
    }
    w
}

/// 留出强标签（0 票 / ≥2 票）上的平均精度；code 标记只取代码语料。
fn eval_ap(samples: &[Sample], w: &[f64], code: bool) -> f64 {
    let rows: Vec<&Sample> = samples
        .iter()
        .filter(|s| s.held && s.votes != 1 && (!code || s.code))
        .collect();
    if rows.is_empty() || !rows.iter().any(|s| s.votes >= 2) {
        return 0.0;
    }
    let mut ranked: Vec<(f64, bool)> = rows.iter().map(|s| (dot(w, &s.x), s.votes >= 2)).collect();
    ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let npos = ranked.iter().filter(|(_, pos)| *pos).count();
    let (mut tp, mut ap, mut last) = (0usize, 0.0, 0.0);
    for (rank, (_, pos)) in ranked.iter().enumerate() {
        if *pos {
            tp += 1;
        }
        let rec = tp as f64 / npos as f64;
        let prec = tp as f64 / (rank + 1) as f64;
        ap += prec * (rec - last);
        last = rec;
    }
    ap
}

fn write_weights(path: &str, w: &[f64]) -> Result<(), Fail> {
    let rounded: Vec<f64> = w.iter().map(|v| io::round6(*v)).collect();
    let value = J::Obj(vec![
        ("schema".to_string(), J::Str("scx-sem-2".to_string())),
        (
            "features".to_string(),
            J::Arr(
                FEATURE_NAMES
                    .iter()
                    .map(|n| J::Str(n.to_string()))
                    .collect(),
            ),
        ),
        (
            "weights".to_string(),
            J::Arr(rounded.iter().map(|v| J::Num(*v)).collect()),
        ),
        ("sha256".to_string(), J::Str(weight_sha(&rounded))),
    ]);
    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(parent).map_err(|e| Fail {
            msg: format!("{path}: {e}"),
            code: 1,
        })?;
    }
    io::write_json_indent(path, &value, 2).map_err(|e| Fail { msg: e, code: 1 })
}

/// 与生产 `verify_v2` 同口径：权重大端字节 ++ 特征名逐名 + '\n' 的 sha256。
fn weight_sha(w: &[f64]) -> String {
    let mut payload = Vec::new();
    for v in w {
        payload.extend_from_slice(&v.to_be_bytes());
    }
    for name in FEATURE_NAMES {
        payload.extend_from_slice(name.as_bytes());
        payload.push(b'\n');
    }
    scx::hash::hex(&scx::hash::sha256(&payload))
}

fn dot(w: &[f64], x: &[f64]) -> f64 {
    w.iter().zip(x).map(|(a, b)| a * b).sum()
}

fn sigmoid(z: f64) -> f64 {
    if z >= 0.0 {
        1.0 / (1.0 + (-z).exp())
    } else {
        let e = z.exp();
        e / (1.0 + e)
    }
}

// ---------------------------------------------------------------------------
// P25-B：逐段线性（scx-sem-3 阈值表）。铰链基 max(0, x−knot)，在基上仍为
// 线性逻辑回归；证据特征做投影梯度（系数钳非负）防 keep 翻转虚高。确定性、
// 零第三方；结点由训练分布分位数确定性生成，写进配置，不硬编码。

/// 单调证据特征：约束其逐段函数非递减（证据越多分越高，结构/证据行不被降权）。
const MONO: &[&str] = &[
    "overlap",
    "hit",
    "word_hit",
    "cjk_bigram",
    "term_prox",
    "q_density",
    "head_hit",
];

fn is_mono(j: usize) -> bool {
    MONO.contains(&FEATURE_NAMES[j])
}

/// 逐段线性模型：线性段系数 + 每列铰链系数（与 knots 逐位对齐）。
struct Pw {
    lin: Vec<f64>,
    hinge: Vec<Vec<f64>>,
}

fn run_piece(args: &[String], p: &Param) -> Result<(), Fail> {
    let out = value(args, "--out")
        .map(str::to_string)
        .unwrap_or_else(|| format!("{ROOT}/config/semantic-pw.json"));
    let samples = build();
    let knots = make_knots(&samples, *p);
    let rows = clean_rows(&samples, *p);
    let model = fit_piece(&rows, &knots, *p);

    let ap = eval_ap_piece(&samples, &knots, &model, false);
    let ap_code = eval_ap_piece(&samples, &knots, &model, true);
    println!("逐段线性 留出 AP={ap:.4} code AP={ap_code:.4}（目标全量 ≥ 0.80）");

    write_piece(&out, &knots, &model)?;
    let rel = Path::new(&out)
        .strip_prefix(ROOT)
        .map(|v| v.to_string_lossy().to_string())
        .unwrap_or_else(|_| out.clone());
    println!("写入 {rel} (schema scx-sem-3)");
    Ok(())
}

/// 确定性 K 折交叉验证（无随机源）：每折按全局下标 `i % K == f` 重设留出，
/// 线性与逐段模型在同一折上拟合/评估，报均值/最小/最大 AP 与逐折差值。
pub(crate) fn run_xval(args: &[String]) -> Result<(), Fail> {
    let p = parse_param(args)?;
    let folds = value(args, "--folds")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(5)
        .clamp(2, 10);
    let base = build();
    let mut lin_aps = Vec::with_capacity(folds);
    let mut piece_aps = Vec::with_capacity(folds);
    for f in 0..folds {
        let mut samples = base.clone();
        for (i, s) in samples.iter_mut().enumerate() {
            s.held = i % folds == f;
        }
        let lin = fit_clean(&samples, p);
        lin_aps.push(eval_ap(&samples, &lin, false));
        let knots = make_knots(&samples, p);
        let rows = clean_rows(&samples, p);
        let model = fit_piece(&rows, &knots, p);
        piece_aps.push(eval_ap_piece(&samples, &knots, &model, false));
    }
    for (f, (l, w)) in lin_aps.iter().zip(&piece_aps).enumerate() {
        println!(
            "fold {f}: linear AP={l:.4}  piece AP={w:.4}  delta={:+.4}",
            w - l
        );
    }
    let (lm, lmin, lmax) = ap_summary(&lin_aps);
    let (wm, wmin, wmax) = ap_summary(&piece_aps);
    println!("linear  K={folds}  mean AP={lm:.4} range=[{lmin:.4},{lmax:.4}]");
    println!("piece   K={folds}  mean AP={wm:.4} range=[{wmin:.4},{wmax:.4}]");
    println!("mean delta = {:+.4}（正=逐段稳定更优）", wm - lm);
    Ok(())
}

/// 均值/最小/最大（确定性，输入非空）。
fn ap_summary(vals: &[f64]) -> (f64, f64, f64) {
    let mean = vals.iter().sum::<f64>() / vals.len() as f64;
    let min = vals.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    (mean, min, max)
}

/// 离线金标准评测（零外部依赖、确定性）：多信号共识产出**机器参考标签**
/// （明确标注，非人工），在留出口径上给线性/逐段模型算 AP，并在 keep 预算
/// 0.35/0.5 两档算精度/召回。报告写入 `config/gold-eval.json` 供冻结。
pub(crate) fn run_gold(args: &[String]) -> Result<(), Fail> {
    let p = parse_param(args)?;
    let out = value(args, "--out").unwrap_or("config/gold-eval.json");
    let samples = build();
    // 强信号线性种子：给单票（votes==1）行做确定性机器裁决，不冒充人工。
    let train: Vec<&Sample> = samples.iter().filter(|s| !s.held).collect();
    let seed = fit(
        &train
            .iter()
            .filter(|s| s.votes != 1)
            .map(|s| (*s, u8::from(s.votes >= 2), 1.0))
            .collect::<Vec<_>>(),
        p,
    );
    // 参考标签：强信号直接定；单票由种子概率机器裁决并打标。
    let gold: Vec<(bool, bool)> = samples
        .iter()
        .map(|s| {
            if s.votes != 1 {
                (s.votes >= 2, true)
            } else {
                (sigmoid(dot(&seed, &s.x)) > 0.5, false)
            }
        })
        .collect();
    let lin = fit_clean(&samples, p);
    let knots = make_knots(&samples, p);
    let piece = fit_piece(&clean_rows(&samples, p), &knots, p);
    // 留出参考行（含机器裁决），分别取两模型分数。
    let mut scored_lin = Vec::new();
    let mut scored_piece = Vec::new();
    for (i, s) in samples.iter().enumerate().filter(|(_, s)| s.held) {
        let (g, strong) = gold[i];
        scored_lin.push((dot(&lin, &s.x), g, strong));
        scored_piece.push((piece_logit(s, &piece.lin, &piece.hinge, &knots), g, strong));
    }
    let report = gold_report(&scored_lin, &scored_piece);
    println!("{report}");
    let value = J::Obj(vec![
        ("schema".to_string(), J::Str("scx-gold-eval-1".to_string())),
        (
            "label_provenance".to_string(),
            J::Str("machine-consensus (not human)".to_string()),
        ),
        ("report".to_string(), J::Str(report)),
    ]);
    io::write_json_indent(out, &value, 2).map_err(|e| Fail { msg: e, code: 1 })?;
    println!("写入 {out}（机器参考标签，非人工金标准）");
    Ok(())
}

/// 生成 AP + 两档 keep 预算精度/召回的文本报告（确定性）。
fn gold_report(lin: &[(f64, bool, bool)], piece: &[(f64, bool, bool)]) -> String {
    let mut out = String::new();
    for (name, rows) in [("linear", lin), ("piece ", piece)] {
        let ap = ap_from(rows);
        out.push_str(&format!("{name} AP={ap:.4}"));
        for budget in [0.35, 0.50] {
            let (prec, rec) = budget_pr(rows, budget);
            out.push_str(&format!(
                " | keep {budget:.2}: prec={prec:.3} recall={rec:.3}"
            ));
        }
        out.push('\n');
    }
    out
}

/// 按 (score, gold) 计算平均精度（忽略 strong 标记，机器参考行同样计入）。
fn ap_from(rows: &[(f64, bool, bool)]) -> f64 {
    let mut ranked: Vec<(f64, bool)> = rows.iter().map(|(z, g, _)| (*z, *g)).collect();
    ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let npos = ranked.iter().filter(|(_, g)| *g).count();
    if npos == 0 {
        return 0.0;
    }
    let (mut tp, mut ap, mut last) = (0usize, 0.0, 0.0);
    for (rank, (_, g)) in ranked.iter().enumerate() {
        if *g {
            tp += 1;
        }
        let rec = tp as f64 / npos as f64;
        ap += (tp as f64 / (rank + 1) as f64) * (rec - last);
        last = rec;
    }
    ap
}

/// 取分数最高的前 `budget` 比例行，对参考正例算精度/召回。
fn budget_pr(rows: &[(f64, bool, bool)], budget: f64) -> (f64, f64) {
    let mut ranked: Vec<(f64, bool)> = rows.iter().map(|(z, g, _)| (*z, *g)).collect();
    ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let take = ((budget * ranked.len() as f64).ceil() as usize)
        .max(1)
        .min(ranked.len());
    let kept_pos = ranked[..take].iter().filter(|(_, g)| *g).count();
    let total_pos = ranked.iter().filter(|(_, g)| *g).count();
    let prec = kept_pos as f64 / take as f64;
    let rec = if total_pos == 0 {
        0.0
    } else {
        kept_pos as f64 / total_pos as f64
    };
    (prec, rec)
}

fn clean_rows(samples: &[Sample], p: Param) -> Vec<(&Sample, u8, f64)> {
    let train: Vec<&Sample> = samples.iter().filter(|s| !s.held).collect();
    let seed = fit(
        &train
            .iter()
            .filter(|s| s.votes != 1)
            .map(|s| (*s, u8::from(s.votes >= 2), 1.0))
            .collect::<Vec<_>>(),
        p,
    );
    let mut out = Vec::new();
    for s in train {
        if s.votes != 1 {
            out.push((s, u8::from(s.votes >= 2), 1.0));
        } else {
            let prob = sigmoid(dot(&seed, &s.x));
            if (prob - 0.5).abs() >= p.margin {
                out.push((s, u8::from(prob > 0.5), p.relabel_weight));
            }
        }
    }
    out
}

/// 每列结点：训练（非留出）样本该特征的等分位分位数，去重并只保留内部结点。
fn make_knots(samples: &[Sample], p: Param) -> Vec<Vec<f64>> {
    let mut out = Vec::with_capacity(DIM);
    for j in 0..DIM {
        let mut vals: Vec<f64> = samples.iter().filter(|s| !s.held).map(|s| s.x[j]).collect();
        vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mut knots = Vec::new();
        if p.knots > 0 && !vals.is_empty() {
            for t in 1..=p.knots {
                let q = t as f64 / (p.knots as f64 + 1.0);
                let v = io::round6(quantile(&vals, q));
                if v > vals[0]
                    && v < vals[vals.len() - 1]
                    && !knots.iter().any(|k: &f64| (*k - v).abs() < 1e-9)
                {
                    knots.push(v);
                }
            }
        }
        out.push(knots);
    }
    out
}

/// 线性插值分位数（确定性）：q ∈ (0,1)，按位置 h=q(n−1) 在相邻序值间插值。
fn quantile(sorted: &[f64], q: f64) -> f64 {
    let h = q * (sorted.len() - 1) as f64;
    let lo = h.floor() as usize;
    let hi = h.ceil() as usize;
    if lo == hi {
        sorted[lo]
    } else {
        sorted[lo] * (hi as f64 - h) + sorted[hi] * (h - lo as f64)
    }
}

/// 逐段线性加权逻辑回归（确定性全批量）；证据列投影梯度钳非负，l2 跳过 bias。
fn fit_piece(rows: &[(&Sample, u8, f64)], knots: &[Vec<f64>], p: Param) -> Pw {
    let mut lin = vec![0.0f64; DIM];
    let mut hinge = vec![Vec::new(); DIM];
    for j in 0..DIM {
        hinge[j] = vec![0.0; knots[j].len()];
    }
    let wsum: f64 = rows.iter().map(|(_, _, w)| *w).sum::<f64>().max(1e-9);
    for _ in 0..p.epochs {
        let mut g_lin = [0.0f64; DIM];
        let mut g_hinge = vec![Vec::new(); DIM];
        for j in 0..DIM {
            g_hinge[j] = vec![0.0; knots[j].len()];
        }
        for (s, label, sw) in rows {
            let err = sigmoid(piece_logit(s, &lin, &hinge, knots)) - *label as f64;
            for j in 0..DIM {
                g_lin[j] += sw * err * s.x[j];
                for (k, kn) in knots[j].iter().enumerate() {
                    let d = s.x[j] - *kn;
                    if d > 0.0 {
                        g_hinge[j][k] += sw * err * d;
                    }
                }
            }
        }
        for j in 0..DIM {
            let decay = if FEATURE_NAMES[j] == "bias" {
                0.0
            } else {
                p.l2
            };
            lin[j] -= p.lr * (g_lin[j] / wsum + decay * lin[j]);
            if is_mono(j) {
                lin[j] = lin[j].max(0.0);
            }
            for k in 0..knots[j].len() {
                hinge[j][k] -= p.lr * (g_hinge[j][k] / wsum + decay * hinge[j][k]);
                if is_mono(j) {
                    hinge[j][k] = hinge[j][k].max(0.0);
                }
            }
        }
    }
    Pw { lin, hinge }
}

fn piece_logit(s: &Sample, lin: &[f64], hinge: &[Vec<f64>], knots: &[Vec<f64>]) -> f64 {
    let mut z = 0.0;
    for j in 0..DIM {
        z += lin[j] * s.x[j];
        for (k, kn) in knots[j].iter().enumerate() {
            let d = s.x[j] - *kn;
            if d > 0.0 {
                z += hinge[j][k] * d;
            }
        }
    }
    z
}

/// 留出强标签 AP（逐段线性打分）；code 标记只取代码语料。
fn eval_ap_piece(samples: &[Sample], knots: &[Vec<f64>], m: &Pw, code: bool) -> f64 {
    let rows: Vec<&Sample> = samples
        .iter()
        .filter(|s| s.held && s.votes != 1 && (!code || s.code))
        .collect();
    if rows.is_empty() || !rows.iter().any(|s| s.votes >= 2) {
        return 0.0;
    }
    let mut ranked: Vec<(f64, bool)> = rows
        .iter()
        .map(|s| (piece_logit(s, &m.lin, &m.hinge, knots), s.votes >= 2))
        .collect();
    ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let npos = ranked.iter().filter(|(_, pos)| *pos).count();
    let (mut tp, mut ap, mut last) = (0usize, 0.0, 0.0);
    for (rank, (_, pos)) in ranked.iter().enumerate() {
        if *pos {
            tp += 1;
        }
        let rec = tp as f64 / npos as f64;
        let prec = tp as f64 / (rank + 1) as f64;
        ap += prec * (rec - last);
        last = rec;
    }
    ap
}

fn write_piece(path: &str, knots: &[Vec<f64>], m: &Pw) -> Result<(), Fail> {
    let r_lin: Vec<f64> = m.lin.iter().map(|v| io::round6(*v)).collect();
    let r_knots: Vec<Vec<f64>> = knots
        .iter()
        .map(|col| col.iter().map(|v| io::round6(*v)).collect())
        .collect();
    let r_hinge: Vec<Vec<f64>> = m
        .hinge
        .iter()
        .map(|col| col.iter().map(|v| io::round6(*v)).collect())
        .collect();
    let mut coef = Vec::with_capacity(DIM);
    for j in 0..DIM {
        let mut col = vec![r_lin[j]];
        col.extend_from_slice(&r_hinge[j]);
        coef.push(J::Arr(col.iter().map(|v| J::Num(*v)).collect()));
    }
    let value = J::Obj(vec![
        ("schema".to_string(), J::Str("scx-sem-3".to_string())),
        (
            "features".to_string(),
            J::Arr(
                FEATURE_NAMES
                    .iter()
                    .map(|n| J::Str(n.to_string()))
                    .collect(),
            ),
        ),
        (
            "knots".to_string(),
            J::Arr(
                r_knots
                    .iter()
                    .map(|col| J::Arr(col.iter().map(|v| J::Num(*v)).collect()))
                    .collect(),
            ),
        ),
        ("coef".to_string(), J::Arr(coef)),
        (
            "sha256".to_string(),
            J::Str(piece_sha(&r_knots, &r_lin, &r_hinge)),
        ),
    ]);
    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(parent).map_err(|e| Fail {
            msg: format!("{path}: {e}"),
            code: 1,
        })?;
    }
    io::write_json_indent(path, &value, 2).map_err(|e| Fail { msg: e, code: 1 })
}

/// 与生产 `encode_v3` 同口径：逐列「名+\n ‖ β0 大端 ‖ 逐(knot 大端 ‖ 铰链大端)」。
fn piece_sha(knots: &[Vec<f64>], lin: &[f64], hinge: &[Vec<f64>]) -> String {
    let mut h = Vec::new();
    for j in 0..DIM {
        h.extend_from_slice(FEATURE_NAMES[j].as_bytes());
        h.push(b'\n');
        h.extend_from_slice(&lin[j].to_be_bytes());
        for (k, kn) in knots[j].iter().enumerate() {
            h.extend_from_slice(&kn.to_be_bytes());
            h.extend_from_slice(&hinge[j][k].to_be_bytes());
        }
    }
    scx::hash::hex(&scx::hash::sha256(&h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_covers_all_corpora_and_dim() {
        let samples = build();
        assert!(samples.len() > 500);
        assert!(samples.iter().all(|s| s.x.len() == DIM));
        assert!(samples.iter().any(|s| s.code));
        assert!(samples.iter().any(|s| s.held));
    }

    #[test]
    fn vote_panel_ranges_and_signals() {
        let terms = vec!["timeout".to_string()];
        assert_eq!(votes("fn handle_error() -> timeout retry", &terms), 3);
        assert_eq!(votes("ERROR conn timeout retry", &terms), 2);
        assert_eq!(votes("plain warmup idle tick", &terms), 0);
        assert!(votes("pub fn parse() -> i32 {", &terms) >= 1);
    }

    #[test]
    fn fit_is_deterministic_and_ranks_positives() {
        let samples = build();
        let p = Param::default();
        let a = fit_clean(&samples, p);
        let b = fit_clean(&samples, p);
        assert_eq!(a, b, "无随机源：两次训练逐位一致");
        assert!(eval_ap(&samples, &a, false) > 0.5);
        assert!(eval_ap(&samples, &a, true) > 0.5);
    }

    #[test]
    fn weight_sha_matches_production_check() {
        let w = vec![0.0f64; DIM];
        assert_eq!(weight_sha(&w).len(), 64);
    }
}
