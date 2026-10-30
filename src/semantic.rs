//! 可选语义增强打分器（`cargo feature="semantic"`，零第三方 crate）。
//!
//! # 权重文件（自描述 JSON，两种 schema）
//! - v1 `scx-sem-1`（位置式，向后兼容）：`{"schema":"scx-sem-1","dim":N,
//!   "weights":[...],"sha256":"hex"}`；`sha256` = hex(SHA256(weights 每 f64
//!   大端 8 字节拼接))，dim 必须等于 weights 长度。
//! - v2 `scx-sem-2`（命名式，推荐）：`{"schema":"scx-sem-2","features":[
//!   "overlap",...],"weights":[...],"sha256":"hex"}`；`features` 为
//!   [`FEATURE_NAMES`] 的子集、顺序即点积顺序；`sha256` = hex(SHA256(
//!   weights 大端字节拼接 || 每个特征名接 `\n`))，未知特征/长度不符/哈希
//!   不符一律拒绝加载。
//! - 打分：`sigmoid(w·x)`，全部 `std::f64` 实现；固定权重 + 固定特征 =
//!   固定输出（无随机源）。居中式 `2·sigmoid−1 ∈ (−1,1)` 供管线做**有符号**
//!   叠加：正 = 提升、负 = 降权（旧实现 `sigmoid ∈ (0,1)` 只能加分、无法降权）。
//! - feature 关闭时本模块不参与编译，默认路径与 v0.4.1 逐字节一致；
//!   feature 开启时默认**不启用**（需 `--semantic <weights.json>` 显式加载）。

#![forbid(unsafe_code)]

use crate::hash::{hex, sha256};
use crate::json::{parse, Json};
use std::fs;

/// 特征注册表（canonical 顺序）。v1 的位置式权重按此前缀解释；v2 的
/// `features` 列表通过名称映射到此处下标，可任意子集、任意顺序。
pub const FEATURE_NAMES: &[&str] = &[
    "overlap",    // 0  query 子串命中比例
    "hit",        // 1  是否命中 0/1
    "ln_len",     // 2  ln(1+字符数)
    "cap_ratio",  // 3  大写比例
    "dig_ratio",  // 4  数字比例
    "structure",  // 5  结构行标志
    "word_hit",   // 6  词边界整词命中比例（含屈折词根）
    "cjk_ratio",  // 7  CJK 字符比例
    "errcode",    // 8  错误码 token 密度
    "uniq",       // 9  唯一 token 率
    "sym",        // 10 非字母数字符号密度
    "pos_norm",   // 11 行位置归一化
    "edge",       // 12 文档头尾 5% 标志
    "long",       // 13 超长行（>200 字符）
    "code",       // 14 代码行信号
    "bias",       // 15 常数 1.0（可学习偏置）
    "cjk_bigram", // 16 CJK 查询 bigram 与行重叠比例（中文转述/词序信号）
    "term_prox",  // 17 query 词共现紧密度（短语级语义）
];

fn feat_index(name: &str) -> Option<usize> {
    FEATURE_NAMES.iter().position(|n| *n == name)
}

/// 权重集（自描述 JSON 校验后加载）。
#[derive(Debug, Clone)]
pub struct Weights {
    /// 每个权重对应的 canonical 特征下标（v1 为 0..dim 前缀）。
    idx: Vec<usize>,
    w: Vec<f64>,
    /// canonical 特征总数（`feats` 全量长度）。
    pub dim: usize,
}

fn read(path: &str) -> Result<Json, String> {
    let txt = fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
    parse(&txt).map_err(|e| format!("{path}: {e}"))
}

fn schema(path: &str, v: &Json) -> Result<String, String> {
    v.get("schema")
        .and_then(Json::str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{path}: missing schema"))
}

/// v1 位置式：dim 必须等于 weights 长度。
fn dim(path: &str, v: &Json) -> Result<usize, String> {
    v.get("dim")
        .and_then(Json::num)
        .ok_or_else(|| format!("{path}: missing dim"))
        .map(|d| d.max(0.0) as usize)
}

/// v2 命名式：features 全为字符串，顺序即点积顺序。
fn features(path: &str, v: &Json) -> Result<Vec<String>, String> {
    match v.get("features") {
        Some(Json::Arr(items)) => items
            .iter()
            .map(|it| {
                it.str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("{path}: non-string feature"))
            })
            .collect(),
        _ => Err(format!("{path}: missing features array")),
    }
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

fn verify_v1(path: &str, v: &Json, ws: &[f64]) -> Result<(), String> {
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

fn verify_v2(path: &str, v: &Json, ws: &[f64], names: &[String]) -> Result<(), String> {
    let want = v
        .get("sha256")
        .and_then(Json::str)
        .ok_or_else(|| format!("{path}: missing sha256"))?;
    // 哈希绑定权重与特征名：篡改任一列（含增删特征）都会破坏校验。
    let mut h = encode(ws);
    for n in names {
        h.extend_from_slice(n.as_bytes());
        h.push(b'\n');
    }
    let got = hex(&sha256(&h));
    if got != want.to_ascii_lowercase() {
        return Err(format!(
            "{path}: sha256 mismatch (file {want}, computed {got})"
        ));
    }
    Ok(())
}

fn load_v1(path: &str, v: &Json) -> Result<Weights, String> {
    let d = dim(path, v)?;
    let ws = weights(path, v, d)?;
    verify_v1(path, v, &ws)?;
    Ok(Weights {
        idx: (0..d).collect(),
        w: ws,
        dim: d,
    })
}

fn load_v2(path: &str, v: &Json) -> Result<Weights, String> {
    let names = features(path, v)?;
    let ws = weights(path, v, names.len())?;
    let mut idx = Vec::with_capacity(names.len());
    for n in &names {
        let k = feat_index(n).ok_or_else(|| format!("{path}: unknown feature \"{n}\""))?;
        idx.push(k);
    }
    verify_v2(path, v, &ws, &names)?;
    Ok(Weights {
        idx,
        w: ws,
        dim: FEATURE_NAMES.len(),
    })
}

impl Weights {
    /// 加载并校验权重文件；schema/特征/权重/哈希任一不符即拒绝。
    pub fn load(path: &str) -> Result<Weights, String> {
        let v = read(path)?;
        match schema(path, &v)?.as_str() {
            "scx-sem-1" => load_v1(path, &v),
            "scx-sem-2" => load_v2(path, &v),
            s => Err(format!(
                "{path}: unsupported schema \"{s}\" (want \"scx-sem-1\" or \"scx-sem-2\")"
            )),
        }
    }

    /// 位置式打分（v1 兼容 / 测试用）：x 为特征前缀，逐位点积。
    pub fn score(&self, x: &[f64]) -> f64 {
        let mut dot = 0.0;
        for (i, xi) in x.iter().enumerate() {
            if i < self.w.len() {
                dot += self.w[i] * xi;
            }
        }
        sigmoid(dot)
    }

    /// 原始 logit（w·x，有符号、无界）：命名式投影点积。
    pub fn logit_line(&self, line: &str, terms: &[String], i: usize, total: usize) -> f64 {
        let x = feats(line, terms, i, total, self.dim);
        let mut dot = 0.0;
        for (k, wi) in self.idx.iter().zip(self.w.iter()) {
            dot += wi * x[*k];
        }
        dot
    }

    /// 命名式打分：内部计算全量特征，按注册表下标投影点积。
    pub fn score_line(&self, line: &str, terms: &[String], i: usize, total: usize) -> f64 {
        sigmoid(self.logit_line(line, terms, i, total))
    }

    /// 居中式有符号打分 ∈ (−1, 1)：0 = 中性，正 = 提升，负 = 降权。
    pub fn centered_line(&self, line: &str, terms: &[String], i: usize, total: usize) -> f64 {
        2.0 * self.score_line(line, terms, i, total) - 1.0
    }
}

/// 数值稳定 sigmoid：分支避免 exp 上下溢，结果恒在 (0,1)。
fn sigmoid(z: f64) -> f64 {
    if z >= 0.0 {
        let e = (-z).exp();
        1.0 / (1.0 + e)
    } else {
        let e = z.exp();
        e / (1.0 + e)
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

/// 行 → dim 维确定性特征向量（canonical 顺序见 [`FEATURE_NAMES`]）。
/// 特征模板与 compress 打分表正交，只作可选加分项；`dim < 注册表长度` 时
/// 只取前缀（旧位置式权重兼容），`dim > 注册表长度` 时补 0。
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
    let bigram = cjk_bigram(line, query_terms);
    let prox = term_prox(&lower, query_terms);
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
        bigram,
        prox,
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
    let is_word =
        c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b':' | b'/' | b'@' | b'\\');
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

/// 提取 CJK 统一表意文字字符序列（仅保留 U+4E00..U+9FFF，供 bigram 提取）。
fn cjk_chars_vec(s: &str) -> Vec<char> {
    s.chars()
        .filter(|&c| ('\u{4E00}'..='\u{9FFF}').contains(&c))
        .collect()
}

/// 相邻 CJK 字符 bigram（重叠率按查询侧 bigram 总数归一）。
fn bigram_pairs(v: &[char]) -> Vec<(char, char)> {
    v.windows(2).map(|w| (w[0], w[1])).collect()
}

/// CJK 查询 bigram 与行的重叠比例：中文转述/词序信号。查询侧取所有 CJK 字符
/// 的相邻 bigram，行侧窗口扫描计数命中；查询 CJK 字符不足 2 时返回 0。
fn cjk_bigram(line: &str, terms: &[String]) -> f64 {
    let mut q: Vec<char> = Vec::new();
    for t in terms {
        q.extend(cjk_chars_vec(t));
    }
    if q.len() < 2 {
        return 0.0;
    }
    let pairs = bigram_pairs(&q);
    let lc = cjk_chars_vec(line);
    if lc.len() < 2 {
        return 0.0;
    }
    let mut hit = 0usize;
    for p in &pairs {
        if lc.windows(2).any(|w| w[0] == p.0 && w[1] == p.1) {
            hit += 1;
        }
    }
    hit as f64 / pairs.len() as f64
}

/// query 词共现紧密度：所有 query term 都在行内时，以「最左到最右命中跨度」的
/// 倒数衡量短语级贴近（1.0 = 同点，越散越接近 0）；任一 term 缺失返回 0。
fn term_prox(lower: &str, terms: &[String]) -> f64 {
    let mut lo = usize::MAX;
    let mut hi = 0usize;
    let mut found = false;
    for t in terms {
        if t.is_empty() {
            continue;
        }
        match lower.find(t.as_str()) {
            Some(p) => {
                lo = lo.min(p);
                hi = hi.max(p);
                found = true;
            }
            None => return 0.0,
        }
    }
    if !found {
        0.0
    } else {
        1.0 / (1.0 + hi.saturating_sub(lo) as f64)
    }
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

    fn v2_hash(ws: &[f64], names: &[&str]) -> String {
        let mut h = encode(ws);
        for n in names {
            h.extend_from_slice(n.as_bytes());
            h.push(b'\n');
        }
        hex(&sha256(&h))
    }

    #[test]
    fn weights_load_validates_sha() {
        let ws = vec![0.1, -0.2, 0.3];
        let names = ["overlap", "hit", "ln_len"];
        let want = v2_hash(&ws, &names);
        let path = std::env::temp_dir().join(format!("scx-sem-{}.json", std::process::id()));
        fs::write(
            &path,
            format!(
                "{{\"schema\":\"scx-sem-2\",\"features\":[\"overlap\",\"hit\",\"ln_len\"],\
                 \"weights\":[0.1,-0.2,0.3],\"sha256\":\"{want}\"}}"
            ),
        )
        .unwrap();
        let w = Weights::load(path.to_str().unwrap()).unwrap();
        assert_eq!(w.dim, FEATURE_NAMES.len());
        assert_eq!(w.idx, vec![0, 1, 2]);
        // 篡改权重 → sha 不匹配 → 拒绝。
        fs::write(
            &path,
            format!(
                "{{\"schema\":\"scx-sem-2\",\"features\":[\"overlap\",\"hit\",\"ln_len\"],\
                 \"weights\":[0.9,-0.2,0.3],\"sha256\":\"{want}\"}}"
            ),
        )
        .unwrap();
        let err = Weights::load(path.to_str().unwrap()).unwrap_err();
        assert!(err.contains("sha256 mismatch"), "err: {err}");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn weights_load_rejects_bad_schema_dim_feature() {
        let path = std::env::temp_dir().join(format!("scx-sem-bad-{}.json", std::process::id()));
        fs::write(
            &path,
            r#"{"schema":"scx-sem-9","dim":2,"weights":[0.1,0.2],"sha256":"deadbeef"}"#,
        )
        .unwrap();
        assert!(Weights::load(path.to_str().unwrap())
            .unwrap_err()
            .contains("schema"));
        // v1 dim 与 weights 长度不符。
        fs::write(
            &path,
            r#"{"schema":"scx-sem-1","dim":3,"weights":[0.1,0.2],"sha256":"deadbeef"}"#,
        )
        .unwrap();
        assert!(Weights::load(path.to_str().unwrap())
            .unwrap_err()
            .contains("dim"));
        // v2 未知特征名。
        fs::write(
            &path,
            r#"{"schema":"scx-sem-2","features":["overlap","bogus"],"weights":[0.1,0.2],"sha256":"deadbeef"}"#,
        )
        .unwrap();
        assert!(Weights::load(path.to_str().unwrap())
            .unwrap_err()
            .contains("unknown feature"));
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
    fn score_is_deterministic_and_sigmoid_bounded() {
        let w = Weights {
            idx: vec![0, 1],
            w: vec![1.0, 1.0],
            dim: 2,
        };
        let x = vec![1.0, 2.0];
        assert_eq!(w.score(&x), w.score(&x));
        assert!(w.score(&x) > w.score(&[0.5, 0.5]));
        assert!((0.0..1.0).contains(&w.score(&x)));
        // 数值稳定：大 logit 不产生 NaN/越界。
        let big = Weights {
            idx: vec![0],
            w: vec![1e3],
            dim: 1,
        };
        let s_big = big.score(&[1.0]);
        assert!(s_big.is_finite() && (0.0..=1.0).contains(&s_big));
        let neg = Weights {
            idx: vec![0],
            w: vec![-1e3],
            dim: 1,
        };
        let s_neg = neg.score(&[1.0]);
        assert!(s_neg.is_finite() && (0.0..=1.0).contains(&s_neg));
    }

    #[test]
    fn centered_score_signs_boost_and_demote() {
        // 居中式打分必须有符号：高 logit 提升、低 logit 降权、0 附近接近中性。
        let up = Weights {
            idx: vec![1],
            w: vec![2.0],
            dim: 16,
        };
        let terms = ["x".to_string()];
        assert!(
            up.centered_line("x error crash", &terms, 0, 10) > 0.0,
            "命中行应被提升"
        );
        assert_eq!(
            up.centered_line("nothing here", &terms, 0, 10),
            0.0,
            "无信号行为中性"
        );
        let down = Weights {
            idx: vec![15],
            w: vec![-1.0],
            dim: 16,
        };
        let d = down.centered_line("plain filler line", &["q".to_string()], 0, 10);
        assert!(d < 0.0, "负偏置应降权: {d}");
    }

    #[test]
    fn v1_positional_compat() {
        // v1 位置式文件按 FEATURE_NAMES 前缀解释，与 v2 首 3 维命名式等价。
        let ws = vec![0.5, 0.8, 0.05];
        let path = std::env::temp_dir().join(format!("scx-sem-v1-{}.json", std::process::id()));
        let want = hex(&sha256(&encode(&ws)));
        fs::write(
            &path,
            format!(
                "{{\"schema\":\"scx-sem-1\",\"dim\":3,\"weights\":[0.5,0.8,0.05],\"sha256\":\"{want}\"}}"
            ),
        )
        .unwrap();
        let w = Weights::load(path.to_str().unwrap()).unwrap();
        assert_eq!(w.idx, vec![0, 1, 2]);
        assert_eq!(w.dim, 3);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn shipped_weights_load_accept() {
        let w = Weights::load("config/semantic/weights.json").expect("随仓权重可加载");
        assert_eq!(w.dim, FEATURE_NAMES.len());
        let terms = [
            "conn".to_string(),
            "error".to_string(),
            "timeout".to_string(),
        ];
        for (i, ln) in [
            "db connection lost: conn 9f2a timeout",
            "warmup complete 2026-10-30 12:00:00 idle",
            "tick: 1",
            "ERROR: query cache miss retry",
        ]
        .iter()
        .enumerate()
        {
            let s = w.score_line(ln, &terms, i, 4);
            assert!((0.0..1.0).contains(&s), "{ln}: score={s}");
        }
    }

    #[test]
    fn feats_shape_and_bounds() {
        let f = feats(
            "conn 9f2a broken db crash",
            &["conn".to_string()],
            0,
            10,
            FEATURE_NAMES.len(),
        );
        assert_eq!(f.len(), FEATURE_NAMES.len());
        assert!(f[0] > 0.0, "overlap 非零");
        assert!(f[0] <= 1.0);
        assert!(f[2] > 0.0, "长度特征非零");
        assert!(f[6] > 0.0, "词边界整词命中非零");
        assert!(f[9] > 0.0, "唯一 token 率非零");
        assert!(f[15] == 1.0, "bias 常数维=1");
        // dim 截断：第 8 维 cjk 与第 15 维 bias 被截掉。
        let f8 = feats("x", &["y".to_string()], 3, 10, 8);
        assert_eq!(f8.len(), 8);
        assert_eq!(f8[6], 0.0);
        assert_eq!(f8[7], 0.0);
        // 词边界：子串包含但非整词 → word_hit 为 0（旧 overlap 仍可为正，兼容）。
        let g = feats(
            "errorhandler reset",
            &["error".to_string()],
            0,
            2,
            FEATURE_NAMES.len(),
        );
        assert_eq!(g[6], 0.0, "error 非整词命中");
    }

    #[test]
    fn cjk_bigram_and_term_prox_features() {
        // CJK bigram：查询「数据库超时」与行「数据库超时重试」共享 bigram。
        let q = ["数据库超时".to_string()];
        let hit = feats("数据库超时重试", &q, 0, 3, FEATURE_NAMES.len());
        assert!(hit[16] > 0.0, "CJK bigram 重叠非零");
        // 部分重叠（共享「超时」）> 完全无关。
        let partial = feats("请求超时了", &q, 0, 3, FEATURE_NAMES.len());
        let none = feats("hello world", &q, 0, 3, FEATURE_NAMES.len());
        assert!(hit[16] > partial[16], "共享更多 bigram 应更高");
        assert!(partial[16] > none[16]);
        assert_eq!(none[16], 0.0);
        // term_prox：同点紧贴 > 分散 > 缺失。
        let terms = ["conn".to_string(), "broken".to_string()];
        let tight = feats("conn broken", &terms, 0, 3, FEATURE_NAMES.len());
        let loose = feats(
            "conn ... lots of filler ... broken",
            &terms,
            0,
            3,
            FEATURE_NAMES.len(),
        );
        let missing = feats("conn only", &terms, 0, 3, FEATURE_NAMES.len());
        assert!(tight[17] > loose[17], "共现更紧应更高");
        assert_eq!(missing[17], 0.0);
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
        let e = feats(
            "ERR123 0x1f 404",
            &["zzz".to_string()],
            1,
            3,
            FEATURE_NAMES.len(),
        );
        assert!(e[8] > 0.0, "错误码密度非零");
        let z = feats(
            "数据库连接超时 重试",
            &["数据库".to_string()],
            1,
            3,
            FEATURE_NAMES.len(),
        );
        assert!(z[7] > 0.0, "CJK 比例非零");
        assert!(z[6] > 0.0, "中文整词命中非零");
    }

    #[test]
    fn stem_plurals_and_ies() {
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
        assert_eq!(stem("issues").as_deref(), Some("issue"));
    }

    #[test]
    fn stem_expand_is_additive_dedup_preserves_order() {
        let got = stem_expand(&["errors".into(), "spike".into(), "error".into()]);
        assert_eq!(&got[..3], &["errors", "spike", "error"]);
        assert_eq!(got.iter().filter(|t| t.as_str() == "error").count(), 1);
        assert_eq!(stem_expand(&["zzz".into()]), vec!["zzz".to_string()]);
    }

    #[test]
    fn root_match_covers_inflections() {
        assert!(root_match("connecting", "connect"));
        assert!(root_match("connected", "connect"));
        assert!(root_match("connecting", "connected"));
        assert!(root_match("errors", "error"));
        assert!(root_match("boxes", "box"));
        assert!(!root_match("errorhandler", "error"));
        assert!(!root_match("running", "run"));
        assert!(!root_match("stress", "stre"));
    }

    #[test]
    fn word_hit_uses_root_match() {
        let toks = tokens("service connecting to db");
        assert!(word_hit_ratio(&["connect".to_string()], "service connecting to db", &toks) > 0.0);
        let miss = tokens("alpha beta");
        assert_eq!(
            word_hit_ratio(&["connect".to_string()], "alpha beta", &miss),
            0.0
        );
    }
}
