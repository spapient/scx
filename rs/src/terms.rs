//! 词面层：停用词表、CJK 判定、查询切词与 token 估算（纯函数、零分配复用缓冲）。
//!
//! 与引擎解耦：任何调用方（compress / 未来的记忆库摄取）都可直接复用 build_terms 与 token_est，
//! 保证"同一查询词面 → 同一词表"的确定性。

pub(crate) const MAX_QUERY_TERMS: usize = 256;
/// 单 token 上限（字节）。记忆索引的 doc 词表长度字段为 u8；超长 token
/// 不能静默截断，否则索引会错位。线性与索引路径共用本切词器，在入口统一
/// 丢弃超长 token，保持两条路径同口径。
pub(crate) const MAX_TERM_BYTES: usize = u8::MAX as usize;
pub(crate) const STOP: &[&str] = &[
    "the", "and", "for", "with", "that", "this", "from", "your", "are", "was", "were", "not",
    "you", "use", "using", "的", "了", "是", "在", "和", "与", "或", "把", "被", "我", "你", "请",
    "帮", "一下", "一个", "这个", "那个", "怎么", "如何", "什么",
];

pub(crate) fn is_cjk(c: char) -> bool {
    let u = c as u32;
    (0x3400..=0x4DBF).contains(&u)
        || (0x4E00..=0x9FFF).contains(&u)
        || (0x3040..=0x30FF).contains(&u)
        || (0xAC00..=0xD7AF).contains(&u)
        || (0x20000..=0x2FA1F).contains(&u)
}
pub(crate) fn is_cjk_or_fullwidth(c: char) -> bool {
    let u = c as u32;
    is_cjk(c) || (0x3000..=0x303F).contains(&u) || (0xFF00..=0xFFEF).contains(&u)
}

pub(crate) fn build_terms(query: &str) -> Vec<String> {
    let stop = |s: &str| STOP.contains(&s);
    let is_tok = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || "_./\\:@-".contains(c);
    let mut terms: Vec<String> = Vec::new();
    let mut cur = String::new();
    for c in query.to_lowercase().chars() {
        if is_tok(c) {
            cur.push(c);
            continue;
        }
        if cur.chars().count() >= 2 && cur.len() <= MAX_TERM_BYTES && !stop(&cur) {
            terms.push(cur.clone());
        }
        cur.clear();
    }
    if cur.chars().count() >= 2 && cur.len() <= MAX_TERM_BYTES && !stop(&cur) {
        terms.push(cur);
    }
    let mut run = String::new();
    let mut runs: Vec<String> = Vec::new();
    for c in query.chars() {
        if is_cjk(c) {
            run.push(c);
            continue;
        }
        if run.chars().count() >= 2 {
            runs.push(run.clone());
        }
        run.clear();
    }
    if run.chars().count() >= 2 {
        runs.push(run);
    }
    for r in runs {
        // CJK 没有空格分词：保留整段，并保证尾部不因 6 字切块而丢掉单字。
        if stop(&r) {
            continue;
        }
        let chars: Vec<char> = r.chars().collect();
        if chars.len() <= 6 {
            if chars.len() >= 2 {
                terms.push(r);
            }
            continue;
        }
        let mut start = 0;
        while start < chars.len() {
            let end = (start + 6).min(chars.len());
            if end - start >= 2 {
                terms.push(chars[start..end].iter().collect());
            }
            start += 6;
        }
        if chars.len() % 6 == 1 {
            terms.push(chars[chars.len() - 2..].iter().collect());
        }
    }
    let mut uniq: Vec<String> = Vec::new();
    for t in terms {
        if uniq.len() >= MAX_QUERY_TERMS {
            break;
        }
        if !uniq.contains(&t) {
            uniq.push(t);
        }
    }
    uniq
}

pub(crate) fn token_est(s: &str) -> f64 {
    if s.is_ascii() {
        return s.len() as f64 / 4.0 + 1.0;
    }
    let mut cjk = 0usize;
    let mut len = 0usize;
    for c in s.chars() {
        len += 1;
        if is_cjk_or_fullwidth(c) {
            cjk += 1;
        }
    }
    (len - cjk) as f64 / 4.0 + cjk as f64 * 0.6 + 1.0
}
