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

use crate::hash::{hex, sha256};
use crate::json::{parse, Json};
use std::fs;

/// 权重集（自描述 JSON 校验后加载）。
#[derive(Debug, Clone)]
pub struct Weights {
    pub dim: usize,
    w: Vec<f64>,
}

fn read(path: &str) -> Result<Json, String> {
    let txt = fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
    parse(&txt).map_err(|e| format!("{path}: {e}"))
}

fn dim(path: &str, v: &Json) -> Result<usize, String> {
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

fn weights(path: &str, v: &Json, dim: usize) -> Result<Vec<f64>, String> {
    let ws = match v.get("weights") {
        Some(Json::Arr(items)) => items
            .iter()
            .map(|it| {
                let n = it
                    .num()
                    .ok_or_else(|| format!("{path}: non-numeric weight"))?;
                if !n.is_finite() {
                    // JSON 数字 `1e999` 溢出为 f64::INFINITY，NaN/inf 会污染打分破坏确定性。
                    return Err(format!("{path}: non-finite weight {n}"));
                }
                Ok(n)
            })
            .collect::<Result<Vec<f64>, String>>()?,
        _ => return Err(format!("{path}: missing weights array")),
    };
    if ws.len() != dim {
        return Err(format!("{path}: dim={dim} but weights.len={}", ws.len()));
    }
    Ok(ws)
}

fn verify(path: &str, v: &Json, ws: &[f64]) -> Result<(), String> {
    let want = v
        .get("sha256")
        .and_then(Json::str)
        .ok_or_else(|| format!("{path}: missing sha256"))?;
    let got = hex(&sha256(&encode(ws)));
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
        let v = read(path)?;
        let dim = dim(path, &v)?;
        let ws = weights(path, &v, dim)?;
        verify(path, &v, &ws)?;
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
fn encode(ws: &[f64]) -> Vec<u8> {
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
    let hit = hits(query_terms, &lower);
    let overlap = overlap(hit, query_terms.len());
    let chars = line.chars().count();
    let caps = line.chars().filter(|c| c.is_ascii_uppercase()).count();
    let digits = line.chars().filter(|c| c.is_ascii_digit()).count();
    let cap_ratio = ratio(caps as f64, chars as f64);
    let dig_ratio = ratio(digits as f64, chars as f64);
    let structure = shape(line);
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

fn hits(query_terms: &[String], lower: &str) -> usize {
    query_terms
        .iter()
        .filter(|t| !t.is_empty() && lower.contains(t.as_str()))
        .count()
}

fn overlap(hit: usize, terms: usize) -> f64 {
    if terms == 0 {
        0.0
    } else {
        hit as f64 / terms as f64
    }
}

fn ratio(num: f64, den: f64) -> f64 {
    if den == 0.0 {
        0.0
    } else {
        num / den
    }
}

fn shape(line: &str) -> f64 {
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

// ---------------------------------------------------------------------------
// 英文屈折词干扩展（recall 侧加性补召回）。
//
// 与 `terms::expand`（手写领域同义对）同机制：只追加词根形式、不替换原词，
// 原词仍居 Plan 词表前缀，因此打分时追加项自动按 synonym_weight 半权
// （见 recall::Plan::original）。纯 std、确定性；错误词根最坏不命中任何 doc。
// 仅在 feature=semantic 编译、且 meta [params] stem_enable=1 时生效。

/// 对每个 token 追加其规则词根并去重保序（加性，原词全部保留）。
pub fn stem_expand(terms: &[String]) -> Vec<String> {
    let mut out = terms.to_vec();
    for t in terms {
        if let Some(s) = stem(t) {
            if !out.iter().any(|x| x == &s) {
                out.push(s);
            }
        }
    }
    out
}

/// 单个 token 的规则词根；非纯小写字母或太短（<4 字节）返回 None。
fn stem(w: &str) -> Option<String> {
    if w.len() < 4 || !w.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    strip_affix(w).filter(|s| s.len() >= 3)
}

/// 最长后缀优先：ies→y；咝音后 es 脱落；其余尾 s 脱落。
fn strip_affix(w: &str) -> Option<String> {
    if let Some(r) = w.strip_suffix("ies") {
        return Some(format!("{r}y"));
    }
    if w.ends_with("es") && sibilant(&w[..w.len() - 2]) {
        return Some(w[..w.len() - 2].to_string());
    }
    let r = w.strip_suffix('s')?;
    (!w.ends_with("ss") && r.len() >= 3).then(|| r.to_string())
}

/// 去 es 后词根须以咝音结尾（ch/sh/s/x/z），避免 issues→issu、dishes→dish。
fn sibilant(stem: &str) -> bool {
    stem.ends_with("ch")
        || stem.ends_with("sh")
        || stem.ends_with('s')
        || stem.ends_with('x')
        || stem.ends_with('z')
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
        let want = hex(&sha256(&encode(&ws)));
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
    fn weights_reject_non_finite_value() {
        // 1e999 溢出为 f64::INFINITY，会污染打分破坏确定性，必须在加载时拒绝。
        let v = crate::json::parse(r#"{"weights":[1e999, 0.5]}"#).unwrap();
        let err = weights("test", &v, 2).unwrap_err();
        assert!(err.contains("non-finite"), "err: {err}");
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

    #[test]
    fn stem_plurals_and_ies() {
        // 规则复数 / ies→y：高精度技术词形桥接。
        for (word, want) in [
            ("errors", "error"),
            ("timeouts", "timeout"),
            ("connections", "connection"),
            ("failures", "failure"),
            ("restarts", "restart"),
            ("retries", "retry"),
            ("dependencies", "dependency"),
            ("crashes", "crash"),
            ("boxes", "box"),
            ("passes", "pass"),
            ("issues", "issue"),
            ("datasets", "dataset"),
        ] {
            assert_eq!(stem(word).as_deref(), Some(want), "{word}");
        }
    }

    #[test]
    fn stem_skips_short_nonalpha_ambiguous() {
        // 短词 / 数字符 / CJK / 含标点 token 不触发，避免垃圾词根。
        for w in [
            "err",
            "oom",
            "gc",
            "as",
            "8080",
            "conn_id",
            "数据库连接",
            "us",
        ] {
            assert_eq!(stem(w), None, "{w} must not stem");
        }
        // issues 经咝音判定后回退尾 s 规则，得到 issue 而非 issu。
        assert_eq!(stem("issues").as_deref(), Some("issue"));
    }

    #[test]
    fn stem_expand_is_additive_dedup_preserves_order() {
        let got = stem_expand(&["errors".into(), "spike".into(), "error".into()]);
        // 原词全保留且居前缀；追加词根去重（error 已在原词中）。
        assert_eq!(&got[..3], &["errors", "spike", "error"]);
        assert_eq!(got.iter().filter(|t| t.as_str() == "error").count(), 1);
        // 无词形可追的 token 原样透传。
        assert_eq!(stem_expand(&["zzz".into()]), vec!["zzz".to_string()]);
    }
}
