//! 词面层：停用词表、CJK 判定、查询切词与 token 估算（纯函数、零分配复用缓冲）。
//!
//! 与引擎解耦：任何调用方（compress / 未来的记忆库摄取）都可直接复用 build_terms 与 token_est，
//! 保证"同一查询词面 → 同一词表"的确定性。

use std::collections::HashSet;

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

/// 压缩与记忆库共享的词法扫描；两种口径只在 CJK 子词展开策略上不同。
fn build_terms_mode(query: &str, memory: bool) -> Vec<String> {
    let stop = |s: &str| STOP.contains(&s);
    let is_tok = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || "_./\\:@-".contains(c);
    let mut terms: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut cur = String::new();
    for c in query.to_lowercase().chars() {
        if is_tok(c) {
            cur.push(c);
            continue;
        }
        if cur.len() >= 2 && cur.len() <= MAX_TERM_BYTES && !stop(&cur) {
            push_unique(&mut terms, &mut seen, cur.clone());
            if terms.len() >= MAX_QUERY_TERMS {
                return terms;
            }
        }
        cur.clear();
    }
    if cur.len() >= 2 && cur.len() <= MAX_TERM_BYTES && !stop(&cur) {
        push_unique(&mut terms, &mut seen, cur);
    }
    if terms.len() >= MAX_QUERY_TERMS {
        return terms;
    }
    let mut run = String::new();
    for c in query.chars() {
        if is_cjk(c) {
            run.push(c);
            continue;
        }
        if run.chars().count() >= 2 {
            push_cjk(&mut terms, &mut seen, &run, memory);
            if terms.len() >= MAX_QUERY_TERMS {
                return terms;
            }
        }
        run.clear();
    }
    if run.chars().count() >= 2 {
        push_cjk(&mut terms, &mut seen, &run, memory);
    }
    terms
}

fn push_unique(terms: &mut Vec<String>, seen: &mut HashSet<String>, term: String) {
    if terms.len() >= MAX_QUERY_TERMS {
        return;
    }
    if seen.insert(term.clone()) {
        terms.push(term);
    }
}

/// CJK 没有空格分词：≤6 字整段成词；长段按 6 字块切分，尾部单字用最后两字
/// 补齐。记忆库在块内追加重叠 bigram/trigram 以桥接释义变体。
fn push_cjk(terms: &mut Vec<String>, seen: &mut HashSet<String>, run: &str, memory: bool) {
    if STOP.contains(&run) {
        return;
    }
    let chars: Vec<char> = run.chars().collect();
    if chars.len() < 2 {
        return;
    }
    if !memory && chars.len() <= 6 {
        push_unique(terms, seen, run.to_string());
        return;
    }
    let mut start = 0;
    while start < chars.len() {
        let end = (start + 6).min(chars.len());
        if end - start >= 2 {
            if memory {
                push_zh(terms, seen, &chars[start..end]);
            } else {
                push_unique(terms, seen, chars[start..end].iter().collect());
            }
        }
        start += 6;
    }
    if chars.len() % 6 == 1 {
        let tail = &chars[chars.len() - 2..];
        if memory {
            push_zh(terms, seen, tail);
        } else {
            push_unique(terms, seen, tail.iter().collect());
        }
    }
}

/// 压缩引擎词表（CJK 整段/6 字块，不展开滑动 n-gram）。
pub(crate) fn build_terms(query: &str) -> Vec<String> {
    build_terms_mode(query, false)
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
    build_terms_mode(query, true)
}

/// case 的 query 词表与 query∪text 去重词表（索引/线性/实体桥共用同一口径）。
pub(crate) fn build_doc_terms(query: &str, text: &str) -> (Vec<String>, Vec<String>) {
    let query_terms = build_mem_terms(query);
    let mut terms = query_terms.clone();
    terms.extend(build_mem_terms(text));
    terms.sort_unstable();
    terms.dedup();
    (query_terms, terms)
}

/// 一个 ≤6 字 CJK 块：整段（精确）+ 重叠 bigram + 重叠 trigram（模糊桥接）。
fn push_zh(terms: &mut Vec<String>, seen: &mut HashSet<String>, chars: &[char]) {
    push_unique(terms, seen, chars.iter().collect());
    for w in chars.windows(2) {
        push_unique(terms, seen, w.iter().collect());
    }
    if chars.len() >= 3 {
        for w in chars.windows(3) {
            push_unique(terms, seen, w.iter().collect());
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

    #[test]
    fn term_cap_keeps_first_occurrence_order() {
        let q: Vec<String> = (0..300).map(|i| format!("w{i}")).collect();
        let terms = build_terms(&q.join(" "));
        assert_eq!(terms.len(), MAX_QUERY_TERMS);
        assert_eq!(terms.first().map(String::as_str), Some("w0"));
        assert_eq!(
            terms.last().map(String::as_str),
            Some("w255"),
            "cap must keep the first 256 unique terms"
        );
    }

    #[test]
    fn doc_terms_are_sorted_unique_with_query_order_preserved() {
        let (query_terms, terms) = build_doc_terms("Beta alpha beta", "alpha gamma alpha");
        assert_eq!(query_terms, ["beta", "alpha"]);
        assert_eq!(terms, ["alpha", "beta", "gamma"]);
    }
}
