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

/// 行 → dim 维确定性特征向量（前 6 维有语义并向后兼容，7–15 为 P13-A 扩展）。
/// 特征模板与 compress 打分表正交，只作可选加分项。
///
/// 特征清单（16 维，dim>16 时补 0，dim<16 时 score 只取前缀——旧权重文件兼容）：
///   0  overlap            query 子串命中比例（旧）
///   1  hit                是否命中 0/1（旧）
///   2  ln(1+chars)        行长度（旧）
///   3  cap_ratio          大写比例（旧）
///   4  dig_ratio          数字比例（旧）
///   5  structure          结构行标志（旧，前缀扩展）
///   6  word_hit_ratio     词边界整词命中比例（P13-A 新，降子串误报）
///   7  cjk_ratio          CJK 字符比例（中文信号）
///   8  errcode_density    错误码密度（含字母+数字 token，如 404/0x1f/ERR123）
///   9  token_uniq         唯一 token 率（信息熵近似）
///  10  sym_density        非字母数字符号密度（代码/结构化信号）
///  11  pos_norm           行位置归一化 0..1
///  12  edge               文档头尾 5% 标志
///  13  long_flag          超长行（>200 字符）
///  14  code_line          代码行信号（{} / () / 箭头）
///  15  bias               常数 1.0（可学习偏置）
pub fn feats(line: &str, query_terms: &[String], i: usize, total: usize, dim: usize) -> Vec<f64> {
    let lower = line.to_lowercase();
    let hit = hits(query_terms, &lower);
    let overlap = overlap(hit, query_terms.len());
    let chars = line.chars().count();
    let caps = line.chars().filter(|c| c.is_ascii_uppercase()).count();
    let digits = line.chars().filter(|c| c.is_ascii_digit()).count();
    let cap_ratio = ratio(caps as f64, chars as f64);
    let dig_ratio = ratio(digits as f64, chars as f64);
    let structure = shape(line);
    let toks = tokens(&lower);
    let w_hit = word_hit_ratio(query_terms, &lower, &toks);
    let cjk = ratio(cjk_chars(line) as f64, chars as f64);
    let errcode = ratio(err_code_tokens(&toks) as f64, toks.len() as f64);
    let uniq = unique_rate(&toks);
    let sym = sym_ratio(line);
    let pos_norm = if total <= 1 {
        0.0
    } else {
        i as f64 / (total - 1) as f64
    };
    let edge = if i < total / 20 || i >= total.saturating_sub(total / 20) {
        1.0
    } else {
        0.0
    };
    let long_flag = if chars > 200 { 1.0 } else { 0.0 };
    let code = code_signal(line);
    let mut out = vec![
        overlap,
        hit.min(1) as f64,
        (1.0 + chars as f64).ln(),
        cap_ratio,
        dig_ratio,
        structure,
        w_hit,
        cjk,
        errcode,
        uniq,
        sym,
        pos_norm,
        edge,
        long_flag,
        code,
        1.0,
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
        || t.starts_with("->")
        || t.starts_with("=>")
        || t.starts_with("panic! ")
        || t.starts_with("error:")
    {
        1.0
    } else {
        0.0
    }
}

/// CJK 基础区首字节（U+4E00..U+9FFF 的 UTF-8 三字节首字节 0xE4..0xE9）。
#[inline]
fn is_cjk_lead(c: u8) -> bool {
    (0xE4..=0xE9).contains(&c)
}

/// P13-A：轻量分词——ASCII 词内字符连续段 + CJK 连续段，其余字符为单字分隔。
/// 纯 std、确定性；仅 semantic feature 开启时调用（默认路径不受影响）。
fn tokens(line: &str) -> Vec<&str> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut cur: Option<usize> = None;
    let mut i = 0usize;
    while i < b.len() {
        i = next(line, b, i, &mut cur, &mut out);
    }
    if let Some(st) = cur {
        out.push(&line[st..b.len()]);
    }
    out
}

/// 单字符推进：词字符并入当前 token，分隔符结算已积累 token。返回下一偏移。
fn next<'a>(
    line: &'a str,
    b: &'a [u8],
    i: usize,
    cur: &mut Option<usize>,
    out: &mut Vec<&'a str>,
) -> usize {
    let c = b[i];
    let is_word = c.is_ascii_alphanumeric()
        || matches!(c, b'_' | b'-' | b'.' | b':' | b'/' | b'@' | b'\\');
    let is_cjk = is_cjk_lead(c) && i + 2 < b.len();
    if is_word || is_cjk {
        if cur.is_none() {
            *cur = Some(i);
        }
    } else if let Some(st) = cur.take() {
        out.push(&line[st..i]);
    }
    i + if is_cjk { 3 } else { 1 }
}

/// 词边界整词命中比例：query term 在行内作为完整 token 出现（含屈折词根匹配）。
/// 纯 ASCII 单词要求严格整词/词根命中；含词内字符（`_./:\@-`）、CJK 或多词短语
/// 的 term 无法整词匹配，回退子串包含（如 "conn_id"、"caused by:"、"数据库"）。
fn word_hit_ratio(terms: &[String], lower: &str, toks: &[&str]) -> f64 {
    if terms.is_empty() {
        return 0.0;
    }
    let mut n = 0usize;
    for t in terms {
        if t.is_empty() {
            continue;
        }
        let plain = t.bytes().all(|b| b.is_ascii_alphanumeric());
        if toks.iter().any(|tok| root_match(tok, t.as_str()))
            || (!plain && lower.contains(t.as_str()))
        {
            n += 1;
        }
    }
    n as f64 / terms.len() as f64
}

/// P13-B：词根判定——整词相等，或双方规则词干相等，或一方词干等于另一方原词。
/// 覆盖屈折变体召回（connecting↔connected↔connect）、规则复数（errors↔error）。
/// 纯 std、确定性；错误词根最坏不命中任何 doc（保守无害）。
fn root_match(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    match (stem(a), stem(b)) {
        (Some(x), Some(y)) => x == y,
        (Some(x), None) => x == b,
        (None, Some(y)) => a == y,
        (None, None) => false,
    }
}

/// CJK 统一表意文字计数（含扩展区首面基础块）。
fn cjk_chars(line: &str) -> usize {
    line.chars()
        .filter(|&c| ('\u{4E00}'..='\u{9FFF}').contains(&c))
        .count()
}

/// 错误码 token：含字母且含数字（404 / 0x1f / ERR123 / conn_id）。
fn err_code_tokens(toks: &[&str]) -> usize {
    toks.iter()
        .filter(|t| {
            t.bytes().any(|b| b.is_ascii_alphabetic()) && t.bytes().any(|b| b.is_ascii_digit())
        })
        .count()
}

/// 唯一 token 率（信息熵近似）：0 = 全重复行，1 = 全唯一。
fn unique_rate(toks: &[&str]) -> f64 {
    if toks.is_empty() {
        return 0.0;
    }
    let mut seen: Vec<&str> = Vec::new();
    for t in toks {
        if !seen.contains(t) {
            seen.push(t);
        }
    }
    seen.len() as f64 / toks.len() as f64
}

/// 非字母数字空白符号密度（代码/结构化/错误标记信号）。
fn sym_ratio(line: &str) -> f64 {
    let mut sym = 0usize;
    let mut tot = 0usize;
    for c in line.chars() {
        if c.is_whitespace() {
            continue;
        }
        tot += 1;
        if !c.is_ascii_alphanumeric() {
            sym += 1;
        }
    }
    ratio(sym as f64, tot as f64)
}

/// 代码行信号：成对括号或箭头（代码/配置/结构化内容）。
fn code_signal(line: &str) -> f64 {
    let has_brace = line.contains('{') && line.contains('}');
    let has_paren = line.contains('(') && line.contains(')');
    let has_arrow = line.contains("->") || line.contains("=>") || line.contains("::");
    if has_brace || has_paren || has_arrow {
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

/// 最长后缀优先：ing → 去 ing（≥4）；ed → 去 ed（≥4）；ies→y；
/// 咝音后 es 脱落；其余尾 s 脱落。
fn strip_affix(w: &str) -> Option<String> {
    if let Some(r) = w.strip_suffix("ing") {
        return (r.len() >= 4).then(|| r.to_string());
    }
    if let Some(r) = w.strip_suffix("ed") {
        return (r.len() >= 4).then(|| r.to_string());
    }
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
        // json::parse 已在解析层拒绝（"number out of range"），或 weights 校验层
        // 拒绝（"non-finite"）——两条路径均满足确定性门禁。
        let err = match crate::json::parse(r#"{"weights":[1e999, 0.5]}"#) {
            Ok(v) => weights("test", &v, 2).unwrap_err(),
            Err(e) => e,
        };
        assert!(
            err.contains("non-finite") || err.contains("range"),
            "err: {err}"
        );
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
    fn shipped_weights_load_accept() {
        let w = Weights::load("config/semantic/weights.json").expect("随仓权重可加载");
        assert_eq!(w.dim, 16);
        // 打分恒在 (0,1)，确定性。
        let terms = ["conn".to_string(), "error".to_string(), "timeout".to_string()];
        for (i, ln) in [
            "db connection lost: conn 9f2a timeout",
            "warmup complete 2026-10-30 12:00:00 idle",
            "tick: 1",
            "ERROR: query cache miss retry",
        ]
        .iter()
        .enumerate()
        {
            let s = w.score(&feats(ln, &terms, i, 4, w.dim));
            assert!((0.0..1.0).contains(&s), "{ln}: score={s}");
        }
    }

    #[test]
    fn feats_shape_and_bounds() {
        // 16 维全量：overlap/命中/长度/词边界/唯一率等信号可验证。
        let f = feats("conn 9f2a broken db crash", &["conn".to_string()], 0, 10, 16);
        assert_eq!(f.len(), 16);
        assert!(f[0] > 0.0, "overlap 非零");
        assert!(f[0] <= 1.0);
        assert!(f[2] > 0.0, "长度特征非零");
        assert!(f[6] > 0.0, "词边界整词命中非零");
        assert!(f[9] > 0.0, "唯一 token 率非零");
        assert!(f[15] == 1.0, "bias 常数维=1");
        // dim 截断：第 8 维 cjk（"x" 无 CJK）与第 15 维 bias 被截掉。
        let f8 = feats("x", &["y".to_string()], 3, 10, 8);
        assert_eq!(f8.len(), 8);
        assert_eq!(f8[6], 0.0);
        assert_eq!(f8[7], 0.0);
        // 词边界：子串包含但非整词 → word_hit 为 0（旧 overlap 仍可为正，兼容）。
        let g = feats("errorhandler reset", &["error".to_string()], 0, 2, 16);
        assert_eq!(g[6], 0.0, "error 非整词命中");
    }

    #[test]
    fn tokens_splits_ascii_and_cjk() {
        let t = tokens("conn_id 连接超时, retry!");
        assert!(t.contains(&"conn_id"));
        assert!(t.contains(&"连接超时"));
        assert!(t.contains(&"retry"));
        assert!(!t.iter().any(|s| s.contains(',')));
        // 纯 ASCII 混合标点；词内字符（.）并入 token。
        let u = tokens("a+b=c, d.e");
        assert_eq!(u, vec!["a", "b", "c", "d.e"]);
    }

    #[test]
    fn errcode_and_cjk_features() {
        // ERR123 含字母数字 → 错误码；中文行 CJK 比例 > 0。
        let e = feats("ERR123 0x1f 404", &["zzz".to_string()], 1, 3, 16);
        assert!(e[8] > 0.0, "错误码密度非零");
        let z = feats("数据库连接超时 重试", &["数据库".to_string()], 1, 3, 16);
        assert!(z[7] > 0.0, "CJK 比例非零");
        assert!(z[6] > 0.0, "中文整词命中非零");
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

    #[test]
    fn root_match_covers_inflections() {
        // 屈折变体：ing/ed 同根召回。
        assert!(root_match("connecting", "connect"));
        assert!(root_match("connected", "connect"));
        assert!(root_match("connecting", "connected"));
        // 规则复数。
        assert!(root_match("errors", "error"));
        assert!(root_match("boxes", "box"));
        // 非同类不误报。
        assert!(!root_match("errorhandler", "error"));
        assert!(!root_match("running", "run"));
        assert!(!root_match("stress", "stre"));
    }

    #[test]
    fn word_hit_uses_root_match() {
        let toks = tokens("service connecting to db");
        assert!(word_hit_ratio(&["connect".to_string()], "service connecting to db", &toks) > 0.0);
        let miss = tokens("alpha beta");
        assert_eq!(word_hit_ratio(&["connect".to_string()], "alpha beta", &miss), 0.0);
    }
}
