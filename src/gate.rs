//! 可选联合门（`scx-gate-1`，零第三方 crate，默认关）。
//!
//! 把语义、簇、锚点、基础分四个 per-operator 信号归一后加权成单一门分，
//! 超阈整行保留，替代只看语义单通道的 `--sem-gate` 硬阈值。权重/尺度/
//! 阈值全部写进配置并由 sha256 绑定，确定性、可冻结；未配置即空操作。
#![forbid(unsafe_code)]

use crate::hash::{hex, sha256};
use crate::json::{parse, Json};
use std::fs;

/// 联合门通道（canonical 顺序，即原始信号的传入顺序）。
pub const CHANNELS: &[&str] = &[
    "sem",
    "cluster",
    "anchor",
    "entropy",
    "score",
    "sem_anchor",
    "anchor_score",
];

/// 生产 rank 产出的单行联合门信号（离线蒸馏与服务同源）。
pub struct Signal {
    pub dup: bool,
    pub raw: [f64; 7],
}

/// 单通道参数：尺度（归一除数）与权重（加权系数）。
#[derive(Clone, Copy, Debug)]
pub struct Channel {
    scale: f64,
    weight: f64,
}

/// 联合门：偏置 + 单阈值 + 每通道尺度/权重，sha 校验后加载。
#[derive(Clone, Debug)]
pub struct Gate {
    threshold: f64,
    bias: f64,
    channels: Vec<Channel>,
}

fn read(path: &str) -> Result<Json, String> {
    let txt = fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
    parse(&txt).map_err(|e| format!("{path}: {e}"))
}

fn num(v: &Json, key: &str, path: &str) -> Result<f64, String> {
    let n = v
        .get(key)
        .and_then(Json::num)
        .ok_or_else(|| format!("{path}: missing {key}"))?;
    if !n.is_finite() {
        return Err(format!("{path}: non-finite {key} {n}"));
    }
    Ok(n)
}

impl Gate {
    /// 加载并校验联合门；schema/通道/尺度/哈希任一不符即拒绝。
    pub fn load(path: &str) -> Result<Gate, String> {
        let v = read(path)?;
        let schema = v
            .get("schema")
            .and_then(Json::str)
            .ok_or_else(|| format!("{path}: missing schema"))?;
        if schema != "scx-gate-1" {
            return Err(format!("{path}: unsupported schema \"{schema}\""));
        }
        let threshold = num(&v, "threshold", path)?;
        let bias = num(&v, "bias", path)?;
        let rows = match v.get("channels") {
            Some(Json::Arr(items)) => items,
            _ => return Err(format!("{path}: missing channels array")),
        };
        if rows.len() != CHANNELS.len() {
            return Err(format!(
                "{path}: channels.len={} want {}",
                rows.len(),
                CHANNELS.len()
            ));
        }
        let mut channels = Vec::with_capacity(rows.len());
        for (i, row) in rows.iter().enumerate() {
            let name = row
                .get("name")
                .and_then(Json::str)
                .ok_or_else(|| format!("{path}: channel missing name"))?;
            if name != CHANNELS[i] {
                return Err(format!(
                    "{path}: channel[{i}] name \"{name}\" want \"{}\"",
                    CHANNELS[i]
                ));
            }
            let scale = num(row, "scale", path)?;
            if scale <= 0.0 {
                return Err(format!("{path}: channel \"{name}\" scale must be > 0"));
            }
            let weight = num(row, "weight", path)?;
            channels.push(Channel { scale, weight });
        }
        let gate = Gate {
            threshold,
            bias,
            channels,
        };
        let want = v
            .get("sha256")
            .and_then(Json::str)
            .ok_or_else(|| format!("{path}: missing sha256"))?;
        let got = gate.sha();
        if got != want.to_ascii_lowercase() {
            return Err(format!(
                "{path}: sha256 mismatch (file {want}, computed {got})"
            ));
        }
        Ok(gate)
    }

    /// 联合门分：偏置 + Σ weight·(raw/scale)；raw 按 [`CHANNELS`] 顺序传入。
    pub fn score(&self, raw: &[f64]) -> f64 {
        let mut z = self.bias;
        for (i, r) in raw.iter().enumerate() {
            z += self.channels[i].weight * (r / self.channels[i].scale);
        }
        z
    }

    /// 门判定：门分 ≥ 阈值即整行保留。
    pub fn admit(&self, raw: &[f64]) -> bool {
        self.score(raw) >= self.threshold
    }

    /// 确定性指纹：绑定阈值/偏置与每通道名+尺度+权重。
    fn sha(&self) -> String {
        let mut h = Vec::new();
        h.extend_from_slice(&self.threshold.to_be_bytes());
        h.extend_from_slice(&self.bias.to_be_bytes());
        for (i, ch) in self.channels.iter().enumerate() {
            h.extend_from_slice(CHANNELS[i].as_bytes());
            h.push(b'\n');
            h.extend_from_slice(&ch.scale.to_be_bytes());
            h.extend_from_slice(&ch.weight.to_be_bytes());
        }
        hex(&sha256(&h))
    }

    /// 离线蒸馏构造：校验通道数与正尺度，返回可序列化并已带指纹的门。
    pub fn build(
        threshold: f64,
        bias: f64,
        scales: &[f64],
        weights: &[f64],
    ) -> Result<Gate, String> {
        if scales.len() != CHANNELS.len() || weights.len() != CHANNELS.len() {
            return Err(format!(
                "gate build: need {} scales and {} weights",
                CHANNELS.len(),
                CHANNELS.len()
            ));
        }
        for (i, s) in scales.iter().enumerate() {
            if !s.is_finite() || *s <= 0.0 {
                return Err(format!(
                    "gate build: channel \"{}\" scale must be > 0",
                    CHANNELS[i]
                ));
            }
        }
        let channels = scales
            .iter()
            .zip(weights.iter())
            .map(|(scale, weight)| {
                if !weight.is_finite() {
                    return Err("gate build: non-finite weight".to_string());
                }
                Ok(Channel {
                    scale: *scale,
                    weight: *weight,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if !threshold.is_finite() || !bias.is_finite() {
            return Err("gate build: non-finite threshold/bias".to_string());
        }
        Ok(Gate {
            threshold,
            bias,
            channels,
        })
    }

    /// 暴露指纹供离线写配置（与 `load` 的校验逐位一致）。
    pub fn fingerprint(&self) -> String {
        self.sha()
    }

    /// 序列化参数（阈值/偏置/每通道尺度与权重），供 tooling 写 JSON。
    pub fn parts(&self) -> (f64, f64, Vec<(f64, f64)>) {
        (
            self.threshold,
            self.bias,
            self.channels.iter().map(|c| (c.scale, c.weight)).collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate_json(threshold: f64, bias: f64, scales: &[f64], weights: &[f64]) -> String {
        let mut ch = String::new();
        for i in 0..CHANNELS.len() {
            ch.push_str(&format!(
                "{{\"name\":\"{}\",\"scale\":{},\"weight\":{}}}",
                CHANNELS[i], scales[i], weights[i]
            ));
            if i + 1 != CHANNELS.len() {
                ch.push(',');
            }
        }
        format!(
            "{{\"schema\":\"scx-gate-1\",\"threshold\":{threshold},\"bias\":{bias},\"channels\":[{ch}]}}"
        )
    }

    #[test]
    fn score_is_linear_and_thresholds() {
        let g = Gate {
            threshold: 1.0,
            bias: 0.0,
            channels: vec![
                Channel {
                    scale: 2.0,
                    weight: 1.0,
                },
                Channel {
                    scale: 1.0,
                    weight: 0.5,
                },
                Channel {
                    scale: 1.0,
                    weight: 0.0,
                },
                Channel {
                    scale: 1.0,
                    weight: 0.0,
                },
            ],
        };
        // raw [2, 2, ..] → 1·(2/2) + 0.5·(2/1) = 1 + 1 = 2。
        assert!((g.score(&[2.0, 2.0, 0.0, 0.0]) - 2.0).abs() < 1e-12);
        assert!(g.admit(&[2.0, 2.0, 0.0, 0.0]));
        assert!(!g.admit(&[0.0, 0.0, 0.0, 0.0]));
    }

    #[test]
    fn load_validates_sha_and_rejects_bad_scale() {
        let scales = [1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        let weights = [1.0, 0.5, 0.2, 0.1, 0.3, 0.4, 0.2];
        let body = gate_json(1.0, 0.0, &scales, &weights);
        let g = Gate {
            threshold: 1.0,
            bias: 0.0,
            channels: scales
                .iter()
                .zip(weights.iter())
                .map(|(s, w)| Channel {
                    scale: *s,
                    weight: *w,
                })
                .collect(),
        };
        let path = std::env::temp_dir().join(format!("scx-gate-{}.json", std::process::id()));
        std::fs::write(
            &path,
            body.replace("}", &format!(",\"sha256\":\"{}\"}}", g.sha())),
        )
        .unwrap();
        assert!(Gate::load(path.to_str().unwrap()).is_ok());
        // 非正尺度必须拒绝。
        let mut bad_scales = scales;
        bad_scales[0] = 0.0;
        let bad = gate_json(1.0, 0.0, &bad_scales, &weights);
        std::fs::write(&path, bad).unwrap();
        assert!(Gate::load(path.to_str().unwrap()).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
