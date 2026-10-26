//! 词面层：停用词表、CJK 判定、查询切词与 token 估算（纯函数、零分配复用缓冲）。
//!
//! 与引擎解耦：压缩与记忆库都直接复用 `terms`、`memory` 与 `estimate`，
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

pub(crate) fn ideograph(c: char) -> bool {
    let u = c as u32;
    (0x3400..=0x4DBF).contains(&u)
        || (0x4E00..=0x9FFF).contains(&u)
        || (0x3040..=0x30FF).contains(&u)
        || (0xAC00..=0xD7AF).contains(&u)
        || (0x20000..=0x2FA1F).contains(&u)
}
pub(crate) fn full(c: char) -> bool {
    let u = c as u32;
    ideograph(c) || (0x3000..=0x303F).contains(&u) || (0xFF00..=0xFFEF).contains(&u)
}

/// 压缩与记忆库共享的词法扫描；两种口径只在 CJK 子词展开策略上不同。
fn scan(query: &str, memory: bool) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let lower = query.to_lowercase();
    ascii(&lower, &mut terms, &mut seen);
    if terms.len() < MAX_QUERY_TERMS {
        cjk(query, &mut terms, &mut seen, memory);
    }
    terms
}

/// ASCII/标点词法扫描：按分隔符切词，停用词与超长 token 不入词表。
fn ascii(lower: &str, terms: &mut Vec<String>, seen: &mut HashSet<String>) {
    let mut cur = String::new();
    for c in lower.chars() {
        if token(c) {
            cur.push(c);
            continue;
        }
        if valid(&cur) {
            push(terms, seen, cur.clone());
            if terms.len() >= MAX_QUERY_TERMS {
                return;
            }
        }
        cur.clear();
    }
    if valid(&cur) {
        push(terms, seen, cur);
    }
}

/// CJK 词法扫描：连续 CJK 字符成段，段长 >=2 进入词表。
fn cjk(query: &str, terms: &mut Vec<String>, seen: &mut HashSet<String>, memory: bool) {
    let mut run = String::new();
    for c in query.chars() {
        if ideograph(c) {
            run.push(c);
            continue;
        }
        if run.chars().count() >= 2 {
            segment(terms, seen, &run, memory);
            if terms.len() >= MAX_QUERY_TERMS {
                return;
            }
        }
        run.clear();
    }
    if run.chars().count() >= 2 {
        segment(terms, seen, &run, memory);
    }
}

fn token(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || "_./\\:@-".contains(c)
}

fn valid(cur: &str) -> bool {
    cur.len() >= 2 && cur.len() <= MAX_TERM_BYTES && !STOP.contains(&cur)
}

fn push(terms: &mut Vec<String>, seen: &mut HashSet<String>, term: String) {
    if terms.len() >= MAX_QUERY_TERMS {
        return;
    }
    if seen.insert(term.clone()) {
        terms.push(term);
    }
}

/// CJK 没有空格分词：≤6 字整段成词；长段按 6 字块切分，尾部单字用最后两字
/// 补齐。记忆库在块内追加重叠 bigram/trigram 以桥接释义变体。
fn segment(terms: &mut Vec<String>, seen: &mut HashSet<String>, run: &str, memory: bool) {
    if STOP.contains(&run) {
        return;
    }
    let chars: Vec<char> = run.chars().collect();
    if chars.len() < 2 {
        return;
    }
    if !memory && chars.len() <= 6 {
        push(terms, seen, run.to_string());
        return;
    }
    let mut start = 0;
    while start < chars.len() {
        let end = (start + 6).min(chars.len());
        if end - start >= 2 {
            part(terms, seen, &chars[start..end], memory);
        }
        start += 6;
    }
    if chars.len() % 6 == 1 {
        let tail = &chars[chars.len() - 2..];
        part(terms, seen, tail, memory);
    }
}

fn part(terms: &mut Vec<String>, seen: &mut HashSet<String>, segment: &[char], memory: bool) {
    if memory {
        grams(terms, seen, segment);
    } else {
        push(terms, seen, segment.iter().collect());
    }
}

/// 压缩引擎词表（CJK 整段/6 字块，不展开滑动 n-gram）。
pub(crate) fn terms(query: &str) -> Vec<String> {
    scan(query, false)
}

/// 记忆库召回用词表——在 `terms` 基础上对 **CJK 段/块
/// （≤6 字）** 追加重叠 bigram/trigram（相邻 2/3 字），保留整段用于精确匹配。
///
/// 动机：整段/6 字块对中文释义层几乎不重叠（"数据库连接池耗尽" vs "连接池满了"
/// 词面无交集）。重叠 n-gram 让"症状不同、根因相同"的中文变体桥接（共享 trigram
/// "连接池"）。>6 字长段切成 6 字块后，**每块仍走 n-gram**（块 ≤6 字，膨胀可控，
/// 长证据 doc 词表约 170 tokens/百字中文段）。尾部 1 字按现有补齐。
///
/// 记忆库 query 与 doc（index/edge/eval-B/feedback/annotate）统一走本函数，
/// A/B 对拍保持同词表口径。compress 引擎维持 `terms` 不动（压缩语义不变）。
pub(crate) fn memory(query: &str) -> Vec<String> {
    scan(query, true)
}

/// case 的 query 词表与 query∪text 去重词表（索引/线性/实体桥共用同一口径）。
pub(crate) fn document(query: &str, text: &str) -> (Vec<String>, Vec<String>) {
    let query_terms = memory(query);
    let mut terms = query_terms.clone();
    terms.extend(memory(text));
    terms.sort_unstable();
    terms.dedup();
    (query_terms, terms)
}

// ---------------------------------------------------------------------------
// 运维领域确定性同义词表。
//
// 两个使用方：
// - `annotate`：按 seed 做字面改写，生成 paraphrase 标注（PARAPHRASE_PAIRS）；
// - `recall`：把 query 词扩展为同义 token 补召回（expand_recall_terms）。
// 只做精确 token 匹配，不做子串替换；表顺序固定，输出确定。

/// 双向词对（中英混排）；新增词对必须同时覆盖两个方向，避免检索不对称。
pub(crate) const PARAPHRASE_PAIRS: &[(&str, &str)] = &[
    ("db", "database"),
    ("database", "db"),
    ("timeout", "超时"),
    ("超时", "timeout"),
    ("conn_id", "连接id"),
    ("连接id", "conn_id"),
    ("conn", "connection"),
    ("connection", "conn"),
    ("connection", "连接"),
    ("连接", "connection"),
    ("pool", "连接池"),
    ("连接池", "pool"),
    ("池", "pool"),
    ("error", "错误"),
    ("错误", "error"),
    ("errno", "错误码"),
    ("错误码", "errno"),
    ("disk", "磁盘"),
    ("磁盘", "disk"),
    ("disk", "storage"),
    ("storage", "disk"),
    ("storage", "存储"),
    ("存储", "storage"),
    ("full", "满"),
    ("满", "full"),
    ("queue", "队列"),
    ("队列", "queue"),
    ("fail", "失败"),
    ("失败", "fail"),
    ("failure", "failed"),
    ("failed", "failure"),
    ("crash", "崩溃"),
    ("崩溃", "crash"),
    ("worker", "线程"),
    ("线程", "worker"),
    ("replication", "replica"),
    ("replica", "replication"),
    ("delay", "lag"),
    ("lag", "delay"),
    ("memory", "heap"),
    ("heap", "memory"),
    ("memory", "oom"),
    ("oom", "memory"),
    ("certificate", "cert"),
    ("cert", "certificate"),
    ("config", "configuration"),
    ("configuration", "config"),
    ("deploy", "deployment"),
    ("deployment", "deploy"),
    ("auth", "authentication"),
    ("authentication", "auth"),
    ("permission", "denied"),
    ("denied", "permission"),
];

/// 召回专用的高精度子集：只收无字面交集的领域释义对。
///
/// 不使用 `db/database`、`error/错误`、`timeout/超时` 这类高频宽词：
/// 它们会把 `db write` 扩成 `database`，让 database connection pool case
/// 反超真正的 db write 目标（104 条截断诊断中已实测 literal MRR 退化）。
///
/// 宽泛→具体的对可以单向（`memory→gc`、`disk→filesystem`）：反向扩展会把
/// 具体查询稀释成整类，实测让 `gc` 查询的 OOM case 反超目标。
const RECALL_PAIRS: &[(&str, &str)] = &[
    ("replication", "replica"),
    ("replica", "replication"),
    ("delay", "lag"),
    ("lag", "delay"),
    ("memory", "heap"),
    ("heap", "memory"),
    ("memory", "oom"),
    ("oom", "memory"),
    ("certificate", "cert"),
    ("cert", "certificate"),
    ("config", "configuration"),
    ("configuration", "config"),
    ("deploy", "deployment"),
    ("deployment", "deploy"),
    ("failure", "failed"),
    ("failed", "failure"),
    ("disk", "storage"),
    ("storage", "disk"),
    ("permission", "denied"),
    ("denied", "permission"),
    ("auth", "authentication"),
    ("authentication", "auth"),
    ("conn", "connection"),
    ("connection", "conn"),
    ("fmt", "format"),
    ("format", "fmt"),
    ("msg", "message"),
    ("message", "msg"),
    ("内存", "memory"),
    ("memory", "内存"),
    ("memory", "gc"),
    ("disk", "filesystem"),
    ("存储", "storage"),
    ("storage", "存储"),
    ("队列", "queue"),
    ("queue", "队列"),
    ("连接", "connection"),
    ("connection", "连接"),
    ("连接池", "pool"),
    ("pool", "连接池"),
    ("崩溃", "crash"),
    ("crash", "崩溃"),
    ("配置", "config"),
    ("config", "配置"),
    ("部署", "deploy"),
    ("deploy", "部署"),
    ("权限", "permission"),
    ("permission", "权限"),
    ("线程", "thread"),
    ("thread", "线程"),
];

/// 对每个精确命中的 token 追加召回用同义 token，并去重保序。
pub(crate) fn expand(terms: &[String]) -> Vec<String> {
    let mut out = terms.to_vec();
    for t in terms {
        for (from, to) in RECALL_PAIRS {
            if t == from && !out.iter().any(|x| x == to) {
                out.push((*to).to_string());
            }
        }
    }
    out
}

/// 一个 ≤6 字 CJK 块：整段（精确）+ 重叠 bigram + 重叠 trigram（模糊桥接）。
fn grams(terms: &mut Vec<String>, seen: &mut HashSet<String>, chars: &[char]) {
    push(terms, seen, chars.iter().collect());
    for w in chars.windows(2) {
        push(terms, seen, w.iter().collect());
    }
    if chars.len() >= 3 {
        for w in chars.windows(3) {
            push(terms, seen, w.iter().collect());
        }
    }
}

/// 计量口径系数：对齐 `token-meter` 的字符三分类启发式。
/// ASCII 4 字符/token；CJK 族（表意/假名/谚文/全角）0.6 token/字；
/// other（emoji/西里尔/阿拉伯/泰文等非 ASCII 非 CJK）2.5 字符/token。
/// 纯 ASCII 与纯 CJK 输入与旧口径逐位一致，仅修正 other 脚本此前按 0.25
/// token/字符低估 4–12 倍的问题。
const ASCII_CHARS_PER_TOKEN: f64 = 4.0;
const CJK_TOKENS_PER_CHAR: f64 = 0.6;
const OTHER_CHARS_PER_TOKEN: f64 = 2.5;

pub(crate) fn estimate(s: &str) -> f64 {
    if s.is_ascii() {
        return s.len() as f64 / ASCII_CHARS_PER_TOKEN + 1.0;
    }
    let (mut ascii, mut cjk, mut other) = (0usize, 0usize, 0usize);
    for c in s.chars() {
        if c.is_ascii() {
            ascii += 1;
        } else if full(c) {
            cjk += 1;
        } else {
            other += 1;
        }
    }
    ascii as f64 / ASCII_CHARS_PER_TOKEN
        + cjk as f64 * CJK_TOKENS_PER_CHAR
        + other as f64 / OTHER_CHARS_PER_TOKEN
        + 1.0
}

/// 整数化 token 计数（`estimate` 向上取整；空串为 0，供计量/预算用整数口径）。
pub(crate) fn count(s: &str) -> usize {
    if s.is_empty() {
        0
    } else {
        estimate(s).ceil() as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 中文释义层桥接——query 与 doc 的 CJK 段即使整段不重叠，
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
            let qt = memory(q);
            let dt = memory(d);
            let common: Vec<&String> = qt.iter().filter(|t| dt.contains(t)).collect();
            assert!(!common.is_empty(), "{} vs {}: no shared term", q, d);
            if common.iter().any(|t| t.chars().count() >= 3) {
                strong += 1;
            }
        }
        assert!(strong >= 4, "strong bridges: {strong}/10");
    }

    /// `terms`（compress 侧）语义不变：6 字块切 + 尾部补齐，无滑动 n-gram。
    #[test]
    fn terms_keeps_compress_semantics() {
        let t = terms("数据库连接池耗尽");
        assert!(t.contains(&"数据库连接池".to_string()), "{t:?}");
        assert!(t.contains(&"耗尽".to_string()), "{t:?}");
        // 无滑动 n-gram（"据库连"等 3 字滑动产物必须不存在）。
        assert!(!t.iter().any(|x| x == "据库连" || x == "连接池耗"), "{t:?}");
        let t2 = terms("数据库连接池耗尽磁盘空间不足");
        assert!(t2.iter().any(|x| x == "数据库连接池"), "{t2:?}");
        assert!(t2.iter().any(|x| x == "耗尽磁盘空间"), "{t2:?}");
        assert!(t2.iter().any(|x| x == "不足"), "{t2:?}");
    }

    #[test]
    fn term_cap_keeps_first_occurrence_order() {
        let q: Vec<String> = (0..300).map(|i| format!("w{i}")).collect();
        let terms = terms(&q.join(" "));
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
        let (query_terms, terms) = document("Beta alpha beta", "alpha gamma alpha");
        assert_eq!(query_terms, ["beta", "alpha"]);
        assert_eq!(terms, ["alpha", "beta", "gamma"]);
    }

    #[test]
    fn expands_only_exact_known_tokens() {
        let got = expand(&["replication".into(), "delay".into(), "x".into()]);
        assert!(got.contains(&"replica".to_string()));
        assert!(got.contains(&"lag".to_string()));
        assert!(!got.contains(&"y".to_string()));
    }

    #[test]
    fn does_not_expand_broad_db_alias_in_recall() {
        assert_eq!(expand(&["db".into()]), vec!["db".to_string()]);
    }

    #[test]
    fn expands_high_precision_abbrev_pairs() {
        // P3.2 保守补充：标准技术缩写 ↔ 全称（双向、去重）。
        let got = expand(&["fmt".into()]);
        assert!(got.contains(&"format".to_string()));
        let got2 = expand(&["message".into()]);
        assert!(got2.contains(&"msg".to_string()));
        let got3 = expand(&["format".into(), "fmt".into()]);
        assert_eq!(
            got3.iter().filter(|t| *t == "fmt").count(),
            1,
            "双向扩展必须去重"
        );
        // 字面前缀交集对（param/parameter）不加入词表。
        assert_eq!(expand(&["param".into()]), vec!["param".to_string()]);
    }

    #[test]
    fn expansion_is_bidirectional_and_deduplicated() {
        let got = expand(&["memory".into(), "heap".into()]);
        assert_eq!(got.iter().filter(|t| *t == "memory").count(), 1);
        assert_eq!(got.iter().filter(|t| *t == "heap").count(), 1);
        assert!(got.contains(&"oom".to_string()));
    }

    #[test]
    fn expands_cjk_domain_terms_to_english() {
        let got = expand(&["内存".into(), "存储".into()]);
        assert!(got.contains(&"memory".to_string()));
        assert!(got.contains(&"storage".to_string()));
    }

    #[test]
    fn broad_terms_expand_one_way_only() {
        let broad = expand(&["memory".into(), "disk".into()]);
        assert!(broad.contains(&"gc".to_string()));
        assert!(broad.contains(&"filesystem".to_string()));
        let specific = expand(&["gc".into(), "filesystem".into()]);
        assert!(!specific.contains(&"memory".to_string()));
        assert!(!specific.contains(&"disk".to_string()));
    }
}
