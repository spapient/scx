//! 可选语义增强打分器（`cargo feature="semantic"`，零第三方 crate）。
//!
//! - 权重文件为自描述 JSON：`{"schema":"scx-sem-1","dim":N,"weights":[...],
//!   "sha256":"hex"}`；`sha256` 为 weights 的二进制规范编码（每 f64 大端 8
//!   字节拼接）的 SHA-256 十六进制，校验失败即拒绝加载。
//! - 打分：`sigmoid(w·x)`，全部 `std::f64` 实现；固定权重 + 固定特征 =
//!   固定输出（无随机源）。
//! - feature 关闭时本模块不参与编译，默认路径与 v0.4.1 逐字节一致；
//!   feature 开启时默认**不启用**（需 `--semantic <weights.json>` 显式加载）。

#![forbid(unsafe_code)]

use crate::json::{parse_json, Json};
use std::fs;

/// SHA-256 轮常量（FIPS 180-4）。
const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
    0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
    0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
    0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
    0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
    0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
    0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
    0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
    0xc67178f2,
];

/// SHA-256（FIPS 180-4，纯 std 实现；用于权重文件完整性校验）。
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let msg = sha256_pad(data);
    let mut w = [0u32; 64];
    for chunk in msg.chunks_exact(64) {
        sha256_compress(chunk, &mut w, &mut h);
    }
    let mut out = [0u8; 32];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

/// 消息填充：追加 `0x80`、补零至 56 mod 64，尾接 64 位大端位长。
fn sha256_pad(data: &[u8]) -> Vec<u8> {
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    msg
}

/// 单块压缩：消息调度 + 64 轮压缩。
fn sha256_compress(chunk: &[u8], w: &mut [u32; 64], h: &mut [u32; 8]) {
    for (i, b) in chunk.chunks_exact(4).enumerate() {
        w[i] = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let (mut a, mut b, mut c, mut d) = (h[0], h[1], h[2], h[3]);
    let (mut e, mut f, mut g, mut hh) = (h[4], h[5], h[6], h[7]);
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ (!e & g);
        let t1 = hh
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(SHA256_K[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        hh = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    h[0] = h[0].wrapping_add(a);
    h[1] = h[1].wrapping_add(b);
    h[2] = h[2].wrapping_add(c);
    h[3] = h[3].wrapping_add(d);
    h[4] = h[4].wrapping_add(e);
    h[5] = h[5].wrapping_add(f);
    h[6] = h[6].wrapping_add(g);
    h[7] = h[7].wrapping_add(hh);
}

/// 字节 → 小写十六进制。
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 权重集（自描述 JSON 校验后加载）。
#[derive(Debug, Clone)]
pub struct Weights {
    pub dim: usize,
    w: Vec<f64>,
}

fn parse_weights_json(path: &str) -> Result<Json, String> {
    let txt = fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
    parse_json(&txt).map_err(|e| format!("{path}: {e}"))
}

fn load_dim(path: &str, v: &Json) -> Result<usize, String> {
    let schema = v
        .get("schema")
        .and_then(Json::str)
        .ok_or_else(|| format!("{path}: missing schema"))?;
    if schema != "scx-sem-1" {
        return Err(format!(
            "{path}: unsupported schema \"{schema}\" (want \"scx-sem-1\")"
        ));
    }
    v.get("dim")
        .and_then(Json::num)
        .ok_or_else(|| format!("{path}: missing dim"))
        .map(|d| d.max(0.0) as usize)
}

fn load_weight_vec(path: &str, v: &Json, dim: usize) -> Result<Vec<f64>, String> {
    let ws = match v.get("weights") {
        Some(Json::Arr(items)) => items
            .iter()
            .map(|it| {
                it.num()
                    .ok_or_else(|| format!("{path}: non-numeric weight"))
            })
            .collect::<Result<Vec<f64>, String>>()?,
        _ => return Err(format!("{path}: missing weights array")),
    };
    if ws.len() != dim {
        return Err(format!("{path}: dim={dim} but weights.len={}", ws.len()));
    }
    Ok(ws)
}

fn check_sha(path: &str, v: &Json, ws: &[f64]) -> Result<(), String> {
    let want = v
        .get("sha256")
        .and_then(Json::str)
        .ok_or_else(|| format!("{path}: missing sha256"))?;
    let got = hex(&sha256(&encode_be(ws)));
    if got != want.to_ascii_lowercase() {
        return Err(format!(
            "{path}: sha256 mismatch (file {want}, computed {got})"
        ));
    }
    Ok(())
}

impl Weights {
    /// 加载并校验权重文件；schema/dim/weights/sha256 任一不符即拒绝。
    pub fn load(path: &str) -> Result<Weights, String> {
        let v = parse_weights_json(path)?;
        let dim = load_dim(path, &v)?;
        let ws = load_weight_vec(path, &v, dim)?;
        check_sha(path, &v, &ws)?;
        Ok(Weights { dim, w: ws })
    }

    /// 打分：`sigmoid(w·x)`，确定性（固定权重 + 固定特征 = 固定输出）。
    pub fn score(&self, x: &[f64]) -> f64 {
        let mut dot = 0.0;
        for (i, xi) in x.iter().enumerate() {
            if i < self.w.len() {
                dot += self.w[i] * xi;
            }
        }
        1.0 / (1.0 + (-dot).exp())
    }
}

/// weights 的二进制规范编码：每 f64 大端 8 字节拼接（IEEE-754 位模式稳定）。
fn encode_be(ws: &[f64]) -> Vec<u8> {
    let mut out = Vec::with_capacity(ws.len() * 8);
    for w in ws {
        out.extend_from_slice(&w.to_be_bytes());
    }
    out
}

/// 行 → dim 维确定性特征向量（前 6 维有语义，其余补 0）。
/// 特征模板与 compress 打分表正交，只作可选加分项。
pub fn feats(line: &str, query_terms: &[String], dim: usize) -> Vec<f64> {
    let lower = line.to_lowercase();
    let hit = hit_count(query_terms, &lower);
    let overlap = overlap_of(hit, query_terms.len());
    let chars = line.chars().count();
    let caps = line.chars().filter(|c| c.is_ascii_uppercase()).count();
    let digits = line.chars().filter(|c| c.is_ascii_digit()).count();
    let cap_ratio = ratio_or_zero(caps as f64, chars as f64);
    let dig_ratio = ratio_or_zero(digits as f64, chars as f64);
    let structure = structure_feat(line);
    let mut out = vec![
        overlap,
        hit.min(1) as f64,
        (1.0 + chars as f64).ln(),
        cap_ratio,
        dig_ratio,
        structure,
    ];
    out.resize(dim, 0.0);
    out
}

fn hit_count(query_terms: &[String], lower: &str) -> usize {
    query_terms
        .iter()
        .filter(|t| !t.is_empty() && lower.contains(t.as_str()))
        .count()
}

fn overlap_of(hit: usize, terms: usize) -> f64 {
    if terms == 0 {
        0.0
    } else {
        hit as f64 / terms as f64
    }
}

fn ratio_or_zero(num: f64, den: f64) -> f64 {
    if den == 0.0 {
        0.0
    } else {
        num / den
    }
}

fn structure_feat(line: &str) -> f64 {
    let t = line.trim_start();
    if t.starts_with("```")
        || t.starts_with("~~~")
        || t.starts_with("import ")
        || t.starts_with("use ")
        || t.starts_with("#include")
        || t.starts_with("at ")
        || t.to_lowercase().starts_with("caused by:")
    {
        1.0
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vectors() {
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn weights_load_validates_sha() {
        let ws = vec![0.1, -0.2, 0.3];
        let want = hex(&sha256(&encode_be(&ws)));
        let path = std::env::temp_dir().join(format!("scx-sem-{}.json", std::process::id()));
        fs::write(
            &path,
            format!(
                "{{\"schema\":\"scx-sem-1\",\"dim\":3,\"weights\":[0.1,-0.2,0.3],\"sha256\":\"{want}\"}}"
            ),
        )
        .unwrap();
        let w = Weights::load(path.to_str().unwrap()).unwrap();
        assert_eq!(w.dim, 3);
        // 篡改权重 → sha 不匹配 → 拒绝。
        fs::write(
            &path,
            format!(
                "{{\"schema\":\"scx-sem-1\",\"dim\":3,\"weights\":[0.9,-0.2,0.3],\"sha256\":\"{want}\"}}"
            ),
        )
        .unwrap();
        let err = Weights::load(path.to_str().unwrap()).unwrap_err();
        assert!(err.contains("sha256 mismatch"), "err: {err}");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn weights_load_rejects_bad_schema_dim() {
        let path = std::env::temp_dir().join(format!("scx-sem-bad-{}.json", std::process::id()));
        fs::write(
            &path,
            r#"{"schema":"scx-sem-9","dim":2,"weights":[0.1,0.2],"sha256":"deadbeef"}"#,
        )
        .unwrap();
        assert!(Weights::load(path.to_str().unwrap())
            .unwrap_err()
            .contains("schema"));
        fs::write(
            &path,
            r#"{"schema":"scx-sem-1","dim":3,"weights":[0.1,0.2],"sha256":"deadbeef"}"#,
        )
        .unwrap();
        assert!(Weights::load(path.to_str().unwrap())
            .unwrap_err()
            .contains("dim"));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn score_is_deterministic() {
        let w = Weights {
            dim: 2,
            w: vec![1.0, 1.0],
        };
        let x = vec![1.0, 2.0];
        assert_eq!(w.score(&x), w.score(&x));
        // sigmoid(3) 介于 0..1 且 > sigmoid(1)。
        assert!(w.score(&x) > w.score(&[0.5, 0.5]));
        assert!(w.score(&x) < 1.0);
    }

    #[test]
    fn feats_shape_and_bounds() {
        let f = feats("conn 9f2a broken db crash", &["conn".to_string()], 6);
        assert_eq!(f.len(), 6);
        assert!(f[0] > 0.0, "overlap 非零");
        assert!(f[0] <= 1.0);
        assert!(f[2] > 0.0, "长度特征非零");
        let f8 = feats("x", &["y".to_string()], 8);
        assert_eq!(f8.len(), 8);
        assert_eq!(f8[6], 0.0);
        assert_eq!(f8[7], 0.0);
    }
}
