//! 联合门蒸馏（`scx-gate-1`，feature=eval，零第三方 crate）。
//!
//! 复用生产 `compress::gate_rows`（与服务端同一 analyze+rank 产物）取得逐行
//! per-operator 信号，在同批弱标签的高一致样本上做确定性全批量逻辑回归，
//! 产出每通道尺度/权重与偏置，写进配置并由 sha256 绑定。默认不随发布启用。
use super::io::{self, J};
use super::train::votes;
use super::Fail;
use scx::gate::{Gate, CHANNELS};
use scx::semantic::Weights;
use scx::Config;
use std::fs;
use std::path::Path;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

const LR: f64 = 0.2;
const EPOCHS: usize = 400;
const L2: f64 = 1e-4;
/// 门阈值目标精度（`--precision` 可覆盖）：在训练行上取满足该精度的最宽召回边界。
const TARGET_PRECISION: f64 = 0.85;

fn sigmoid(z: f64) -> f64 {
    1.0 / (1.0 + (-z.clamp(-30.0, 30.0)).exp())
}

struct Row {
    label: u8,
    raw: [f64; 7],
}

fn gather(weights_path: &str) -> Result<Vec<Row>, Fail> {
    let w = Weights::load(weights_path).map_err(|e| Fail {
        msg: format!("{weights_path}: {e}"),
        code: 1,
    })?;
    let cfg = Config {
        semantic: Some(w),
        ..Config::default()
    };
    let corpora: [(String, &[&str]); 3] = [
        (super::bench::corpus_code(), &["build"]),
        (super::bench::corpus_log(), &["timeout"]),
        (super::bench::corpus_zh(), &["超时"]),
    ];
    let mut rows = Vec::new();
    for (text, query) in corpora {
        let terms: Vec<String> = query.iter().map(|s| s.to_string()).collect();
        let split: Vec<&str> = text.split('\n').collect();
        for (i, r) in scx::gate_rows(&text, query[0], &cfg)
            .into_iter()
            .enumerate()
        {
            if r.dup {
                continue;
            }
            let line = split.get(i).copied().unwrap_or("");
            let v = votes(line, &terms);
            if v == 1 {
                continue; // 只用高一致样本（0 强负 / ≥2 强正），单票不参与门拟合
            }
            rows.push(Row {
                label: u8::from(v >= 2),
                raw: r.raw,
            });
        }
    }
    Ok(rows)
}

/// 线性插值分位数（确定性，与训练链同口径）。
fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.len() == 1 {
        return sorted[0];
    }
    let h = q * (sorted.len() - 1) as f64;
    let lo = h.floor() as usize;
    let hi = h.ceil() as usize;
    if lo == hi {
        sorted[lo]
    } else {
        sorted[lo] * (hi as f64 - h) + sorted[hi] * (h - lo as f64)
    }
}

/// 稳健尺度：每通道取绝对值的 0.95 分位数（抗离群），并兜底 1e-6 防零。
fn scales(rows: &[Row]) -> [f64; 7] {
    let mut s = [1e-6f64; 7];
    for (j, slot) in s.iter_mut().enumerate() {
        let mut vals: Vec<f64> = rows.iter().map(|r| r.raw[j].abs()).collect();
        vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        if !vals.is_empty() {
            *slot = quantile(&vals, 0.95).max(1e-6);
        }
    }
    s
}

/// 确定性全批量逻辑回归；各通道均为“证据越多越该保留”，权重钳非负，偏置自由。
fn fit(rows: &[Row], scale: &[f64]) -> (f64, [f64; 7]) {
    let mut bias = 0.0f64;
    let mut w = [0.0f64; 7];
    let n = rows.len().max(1) as f64;
    for _ in 0..EPOCHS {
        let mut g_bias = 0.0;
        let mut g = [0.0f64; 7];
        for r in rows {
            let mut z = bias;
            for j in 0..CHANNELS.len() {
                z += w[j] * (r.raw[j] / scale[j]);
            }
            let err = sigmoid(z) - r.label as f64;
            g_bias += err;
            for j in 0..CHANNELS.len() {
                g[j] += err * (r.raw[j] / scale[j]);
            }
        }
        bias -= LR * (g_bias / n);
        for j in 0..CHANNELS.len() {
            w[j] -= LR * (g[j] / n + L2 * w[j]);
            w[j] = w[j].max(0.0);
        }
    }
    (bias, w)
}

/// 行门分（logit）：偏置 + Σ weight·(raw/scale)，与服务端 `Gate::score` 同口径。
fn logit(r: &Row, bias: f64, w: &[f64], scale: &[f64]) -> f64 {
    let mut z = bias;
    for j in 0..CHANNELS.len() {
        z += w[j] * (r.raw[j] / scale[j]);
    }
    z
}

/// 确定性阈值选择：按门分降序累计，取满足目标精度的最宽（最低分）边界；
/// 全程达不到目标精度时回退到最高分（只放行最强行），保证不放宽保真。
fn select_threshold(rows: &[Row], bias: f64, w: &[f64], scale: &[f64], target: f64) -> f64 {
    let mut scored: Vec<(f64, u8)> = rows
        .iter()
        .map(|r| (logit(r, bias, w, scale), r.label))
        .collect();
    // 稳定排序：门分降序，同分时正例在前，保证边界逐位可复现。
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.1.cmp(&a.1))
    });
    let (mut total, mut pos) = (0usize, 0usize);
    let mut boundary = None;
    for (z, label) in scored {
        total += 1;
        pos += label as usize;
        if pos as f64 / total as f64 >= target {
            boundary = Some(z); // 继续向下找更宽的可行边界
        }
    }
    io::round6(boundary.unwrap_or(f64::INFINITY))
}

pub(crate) fn run(args: &[String]) -> Result<(), Fail> {
    let weights = super::arg_value(args, "--weights")
        .unwrap_or("config/semantic.json")
        .to_string();
    let out = super::arg_value(args, "--out")
        .unwrap_or("config/gate.json")
        .to_string();
    let rows = gather(&weights)?;
    if rows.iter().all(|r| r.label == 0) || rows.iter().all(|r| r.label == 1) {
        return Err(Fail {
            msg: "gate: labels are degenerate, need both positive and negative rows".to_string(),
            code: 1,
        });
    }
    let scale = scales(&rows);
    let (bias, w) = fit(&rows, &scale);
    let r_scale: Vec<f64> = scale.iter().map(|v| io::round6(*v)).collect();
    let r_w: Vec<f64> = w.iter().map(|v| io::round6(*v)).collect();
    let r_bias = io::round6(bias);
    let target = super::arg_value(args, "--precision")
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| (0.5..=1.0).contains(v))
        .unwrap_or(TARGET_PRECISION);
    let threshold = select_threshold(&rows, r_bias, &r_w, &r_scale, target);
    let gate =
        Gate::build(threshold, r_bias, &r_scale, &r_w).map_err(|e| Fail { msg: e, code: 1 })?;
    let (got_threshold, got_bias, got_ch) = gate.parts();
    let channels = J::Arr(
        got_ch
            .iter()
            .zip(CHANNELS.iter())
            .map(|((s, wgt), name)| {
                J::Obj(vec![
                    ("name".to_string(), J::Str((*name).to_string())),
                    ("scale".to_string(), J::Num(*s)),
                    ("weight".to_string(), J::Num(*wgt)),
                ])
            })
            .collect(),
    );
    let value = J::Obj(vec![
        ("schema".to_string(), J::Str("scx-gate-1".to_string())),
        ("threshold".to_string(), J::Num(got_threshold)),
        ("bias".to_string(), J::Num(got_bias)),
        ("channels".to_string(), channels),
        ("sha256".to_string(), J::Str(gate.fingerprint())),
    ]);
    if let Some(parent) = Path::new(&out).parent() {
        fs::create_dir_all(parent).map_err(|e| Fail {
            msg: format!("{out}: {e}"),
            code: 1,
        })?;
    }
    io::write_json_indent(&out, &value, 2).map_err(|e| Fail { msg: e, code: 1 })?;
    let rel = Path::new(&out)
        .strip_prefix(ROOT)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| out.clone());
    println!(
        "写入 {rel}（n={}，sha256={}…）",
        rows.len(),
        &gate.fingerprint()[..16]
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_admits_a_nontrivial_share_of_live_lines() {
        let path = format!("{ROOT}/config/gate.json");
        let gate = Gate::load(&path).expect("gate.json loads");
        let w = Weights::load(&format!("{ROOT}/config/semantic.json")).unwrap();
        let cfg = Config {
            semantic: Some(w),
            ..Config::default()
        };
        let corpora: [(String, &str); 3] = [
            (super::super::bench::corpus_code(), "build"),
            (super::super::bench::corpus_log(), "timeout"),
            (super::super::bench::corpus_zh(), "超时"),
        ];
        let (mut live, mut admitted) = (0usize, 0usize);
        for (text, q) in corpora {
            for sig in scx::gate_rows(&text, q, &cfg) {
                if sig.dup {
                    continue;
                }
                live += 1;
                if gate.admit(&sig.raw) {
                    admitted += 1;
                }
            }
        }
        println!(
            "admit {admitted}/{live} = {:.3}",
            admitted as f64 / live as f64
        );
        assert!(admitted > 0, "gate must admit at least one line");
        assert!(admitted < live, "gate must not admit every line");
    }

    #[test]
    fn gate_rescues_lines_a_tight_sieve_would_drop() {
        let gate = Gate::load(&format!("{ROOT}/config/gate.json")).unwrap();
        let w = Weights::load(&format!("{ROOT}/config/semantic.json")).unwrap();
        let base = Config {
            semantic: Some(w),
            ratio: 0.2,
            ..Config::default()
        };
        let corpora: [(String, &str); 3] = [
            (super::super::bench::corpus_code(), "build"),
            (super::super::bench::corpus_log(), "timeout"),
            (super::super::bench::corpus_zh(), "超时"),
        ];
        let mut rescued = 0usize;
        for (text, q) in corpora {
            let outcome = scx::compress(&text, q, &base);
            let kept: std::collections::HashSet<usize> = outcome.numbers.iter().copied().collect();
            for (i, sig) in scx::gate_rows(&text, q, &base).into_iter().enumerate() {
                if !sig.dup && gate.admit(&sig.raw) && !kept.contains(&(i + 1)) {
                    rescued += 1;
                }
            }
        }
        println!("rescued {rescued} lines beyond the ratio=0.2 sieve");
        assert!(
            rescued > 0,
            "joint gate must force-keep at least one line the sieve drops"
        );
    }
}
