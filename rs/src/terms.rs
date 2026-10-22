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

/// P4.4+ 优化⑪（P1-3）：记忆库召回用词表——在 build_terms 基础上对 **CJK 段/块
/// （≤6 字）** 追加重叠 bigram/trigram（相邻 2/3 字），保留整段用于精确匹配。
///
/// 动机：整段/6 字块对中文释义层几乎不重叠（"数据库连接池耗尽" vs "连接池满了"
/// 词面无交集）。重叠 n-gram 让"症状不同、根因相同"的中文变体桥接（共享 trigram
/// "连接池"）。>6 字长段切成 6 字块后，**每块仍走 n-gram**（块 ≤6 字，膨胀可控，
/// 长证据 doc 词表约 170 tokens/百字中文段）。尾部 1 字按现有补齐。
///
/// 记忆库 query 与 doc（index/edge/eval-B/feedback/annotate）统一走本函数，
/// A/B 对拍保持同词表口径。compress 引擎维持 build_terms 不动（压缩语义不变）。
pub(crate) fn build_mem_terms(query: &str) -> Vec<String> {
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
        if stop(&r) {
            continue;
        }
        let chars: Vec<char> = r.chars().collect();
        if chars.len() < 2 {
            continue;
        }
        // CJK 段 → ≤6 字块（每块整段+bigram+trigram）；整段 ≤6 字直接一块。
        let mut start = 0;
        while start < chars.len() {
            let end = (start + 6).min(chars.len());
            if end - start >= 2 {
                push_zh(&mut terms, &chars[start..end]);
            }
            start += 6;
        }
        if chars.len() % 6 == 1 {
            push_zh(&mut terms, &chars[chars.len() - 2..]);
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

/// 一个 ≤6 字 CJK 块：整段（精确）+ 重叠 bigram + 重叠 trigram（模糊桥接）。
fn push_zh(terms: &mut Vec<String>, chars: &[char]) {
    let seg: String = chars.iter().collect();
    terms.push(seg);
    for w in chars.windows(2) {
        terms.push(w.iter().collect());
    }
    if chars.len() >= 3 {
        for w in chars.windows(3) {
            terms.push(w.iter().collect());
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// P4.4+ 优化⑪（P1-3）：中文释义层桥接——query 与 doc 的 CJK 段即使整段不重叠，
    /// 重叠 trigram/bigram 也必须提供公共词（共享根因桥）。10 组真实感中/中英混排
    /// 变体：每组 (query, doc) 词表交集必须非空；其中至少 4 对存在 ≥3 字强桥
    /// （trigram 级词面锚，如"连接池""检测到"）。
    #[test]
    fn mem_terms_bridge_zh_paraphrases() {
        let pairs: &[(&str, &str)] = &[
            ("数据库连接池耗尽", "连接池满了"),
            ("数据库连接池耗尽", "数据库连接池空闲不足"),
            ("磁盘空间不足", "磁盘空间不够用"),
            ("tls握手超时", "SSL握手超时"),
            ("死锁检测到", "检测到死锁等待链"),
            ("连接pool最小空闲不足", "连接池空闲连接太少"),
            ("端口被占用", "8080端口已被使用"),
            ("内存溢出堆栈", "out of memory 堆栈溢出"),
            ("队列积压太多", "队列backlog溢出"),
            ("磁盘io读取超时", "磁盘读写卡死"),
        ];
        let mut strong = 0usize;
        for (q, d) in pairs {
            let qt = build_mem_terms(q);
            let dt = build_mem_terms(d);
            let common: Vec<&String> = qt.iter().filter(|t| dt.contains(t)).collect();
            assert!(!common.is_empty(), "{} vs {}: no shared term", q, d);
            if common.iter().any(|t| t.chars().count() >= 3) {
                strong += 1;
            }
        }
        assert!(strong >= 4, "strong bridges: {strong}/10");
    }

    /// build_terms（compress 侧）语义不变：6 字块切 + 尾部补齐，无滑动 n-gram。
    #[test]
    fn build_terms_keeps_compress_semantics() {
        let t = build_terms("数据库连接池耗尽");
        assert!(t.contains(&"数据库连接池".to_string()), "{t:?}");
        assert!(t.contains(&"耗尽".to_string()), "{t:?}");
        // 无滑动 n-gram（"据库连"等 3 字滑动产物必须不存在）。
        assert!(!t.iter().any(|x| x == "据库连" || x == "连接池耗"), "{t:?}");
        let t2 = build_terms("数据库连接池耗尽磁盘空间不足");
        assert!(t2.iter().any(|x| x == "数据库连接池"), "{t2:?}");
        assert!(t2.iter().any(|x| x == "耗尽磁盘空间"), "{t2:?}");
        assert!(t2.iter().any(|x| x == "不足"), "{t2:?}");
    }
}
