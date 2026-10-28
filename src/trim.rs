//! 行内 token 级裁剪（`--trim 1`，LLMLingua-2 思路的零依赖启发式）。
//!
//! 与行级 `fill_floor` 互补：行级决定「哪些行保留」，token 级决定「保留行
//! 内部哪些 token 值得留下」。目标场景是 LLM 上下文压缩——GSM8K 8-shot
//! 这类密集推理文本行级几乎不可压（证据邻域膨胀），但行内大量 token 是
//! 停用词/纯标点/低信息量填充，可按重要性预算摘除而不破坏数字与语义锚。
//!
//! 打分规则（配置声明在 `policy`，此处只消费常量，零硬编码）：
//! - 数字/数值 token（含数字字符）最高重要：数学题的关键是数字。
//! - 结构锚 token（`<|`、`|>`、`<`、`>`、`[`、`]`、`{`、`}` 等）保留：
//!   摘除会破坏 chat/JSON/XML 骨架。
//! - 命中 query 词根的 token 高重要：与检索意图直接相关。
//! - 行首/行尾 token 强制保留：保持行边界可读与拼接完整。
//! - 停用词与纯标点低重要：优先摘除。
//!
//! 预算：每行可保留 token 数 = `max(1, 行内 token 数 × trim_ratio)`，
//! 先摘除低分 token，分数相同时按原顺序（确定性）。默认 `trim=0` 时
//! 本模块完全跳过，输出与现状逐字节一致。

/// 行内 token 视图：拆分规则对齐 `terms::ascii` 的 token 字符集
/// （ASCII 字母/数字/`_./:\@-`），但**不做**停用词剔除与长度下限——
/// 裁剪必须能看到停用词才能摘除它。
pub(crate) struct Tokens {
    /// token 起始字节偏移（升序）。
    pub(crate) start: Vec<usize>,
    /// token 结束字节偏移（不含）。
    pub(crate) end: Vec<usize>,
    /// 是否为数字 token（含至少一个 ASCII 数字）。
    pub(crate) numeric: Vec<bool>,
    /// 是否结构锚（单字符括号/尖括号/管道）。
    pub(crate) anchor: Vec<bool>,
    /// 是否命中 query 词根（小写词在 query 的切词表中出现过）。
    pub(crate) hit: Vec<bool>,
    /// 是否停用词（`terms::STOP` 命中）。
    pub(crate) stop: Vec<bool>,
    /// 是否语义敏感词（`terms::KEEP` 命中）：否定/比较/转折/数值语义，
    /// 永不摘除（与命中同级 score 2）。
    pub(crate) keep: Vec<bool>,
    /// 是否低信息量单字符（单 ASCII 字符/全角标点）：得分 0，优先摘除。
    pub(crate) punct: Vec<bool>,
}

/// 切分一行文本的 token 视图。`query_terms` 为 `terms::terms` 的词表（小写）。
///
/// ASCII 连续段成 token（与 `terms::ascii` 同字符集）；**CJK/全角单字各成
/// 一个 token**（`terms::full` 判定），使中文裁剪能保留命中字、摘除虚词，
/// 而不再把整段中文当非 token 丢弃（此前 `--trim` 对中文行只留 ASCII）。
pub(crate) fn tokens(line: &str, query_terms: &[String], op_keep: bool) -> Tokens {
    let lower = line.to_lowercase();
    let mut t = Tokens {
        start: Vec::new(),
        end: Vec::new(),
        numeric: Vec::new(),
        anchor: Vec::new(),
        hit: Vec::new(),
        stop: Vec::new(),
        keep: Vec::new(),
        punct: Vec::new(),
    };
    let bytes = line.as_bytes();
    let n = bytes.len();
    let mut cur = 0usize;
    while cur < n {
        if bytes[cur].is_ascii() {
            cur = scan_ascii(line, &lower, query_terms, cur, op_keep, &mut t);
        } else {
            cur = scan_cjk(line, query_terms, cur, op_keep, &mut t);
        }
    }
    t
}

/// ASCII 分支：跳过非 token 字符；连续 token 字符段成一个 token。
/// 返回下一个扫描位置。
fn scan_ascii(
    line: &str,
    lower: &str,
    query_terms: &[String],
    cur: usize,
    op_keep: bool,
    t: &mut Tokens,
) -> usize {
    let bytes = line.as_bytes();
    let n = bytes.len();
    if !token_char(bytes[cur], op_keep) {
        return cur + 1;
    }
    let s = cur;
    let mut c = cur;
    while c < n && bytes[c].is_ascii() && token_char(bytes[c], op_keep) {
        c += 1;
    }
    if c <= s {
        return c + 1;
    }
    let tok = &line[s..c];
    let tok_lower = &lower[s..c];
    t.start.push(s);
    t.end.push(c);
    t.numeric.push(tok.bytes().any(|b| b.is_ascii_digit()));
    t.anchor.push(is_anchor(tok, op_keep));
    t.hit
        .push(query_terms.iter().any(|q| q.as_str() == tok_lower));
    t.stop.push(crate::terms::STOP.contains(&tok_lower));
    t.keep.push(crate::terms::KEEP.contains(&tok_lower));
    // 单 ASCII 字符（字母/标点）非数字非锚：低信息量（保持旧口径）。
    t.punct
        .push(tok.len() == 1 && !tok.bytes().any(|b| b.is_ascii_digit()));
    c
}

/// 非 ASCII 分支：解码单个 UTF-8 char。CJK/全角字符（表意/假名/谚文/全角
/// 标点数字）作为单字 token；其余（emoji 等）整体跳过，不切坏字节。
/// 返回下一个扫描位置。
fn scan_cjk(line: &str, query_terms: &[String], cur: usize, op_keep: bool, t: &mut Tokens) -> usize {
    match line[cur..].chars().next() {
        Some(ch) if crate::terms::full(ch) => {
            let w = ch.len_utf8();
            let s = cur;
            let c = cur + w;
            let tok = &line[s..c];
            t.start.push(s);
            t.end.push(c);
            t.numeric.push(ch.is_numeric());
            t.anchor.push(is_anchor(tok, op_keep));
            // CJK 无大小写：单字命中任一 query term 的包含关系（"内存"
            // 在词表 → "内"/"存" 均命中）。
            t.hit.push(query_terms.iter().any(|q| q.contains(ch)));
            t.stop.push(crate::terms::STOP.contains(&tok));
            // CJK 无大小写：单字命中任一 KEEP 词（"不/没/无/每/共/总"等）。
            t.keep.push(crate::terms::KEEP.contains(&tok));
            // 全角标点（非表意/非数字）：低信息量，可摘除。
            t.punct
                .push(!crate::terms::ideograph(ch) && !ch.is_numeric());
            c
        }
        Some(ch) => cur + ch.len_utf8(),
        None => cur + 1,
    }
}

fn token_char(b: u8, op_keep: bool) -> bool {
    b.is_ascii_alphanumeric()
        || b"_./:\\@-<>|[]{}".contains(&b)
        || b == b'%'
        || b == b'$'
        || (op_keep && (b == b'+' || b == b'=' || b == b'*'))
}

/// 结构锚：单字节括号/尖括号/管道/大括号（JSON/XML/chat 骨架）、
/// 以及 chat 模板标记词 `im_start`/`im_end`（`<|im_start|>` 拆出的内部词）。
/// `<|im_start|>` 会被拆成 `<` `|` `im_start` `|` `>`，各成分均强制保留。
/// `op_keep` 打开时算术符号（`+` `=` `*` `-`）也强制保留——代码/算式
/// 语义核心，删了 `n ** 3` 变 `n 3`。默认关：GSM8K 数学场景无益（P6d
/// 实测），代码场景由 `--trim-op-keep 1` 打开（P6f）。
fn is_anchor(tok: &str, op_keep: bool) -> bool {
    matches!(
        tok,
        "<" | ">" | "[" | "]" | "{" | "}" | "|" | "im_start" | "im_end"
    ) || (op_keep && matches!(tok, "+" | "=" | "*" | "-"))
}

/// 裁剪一行：返回裁剪后的文本。`trim_ratio` ∈ (0,1]：**低分 token** 的保留比例。
///
/// 语义（对齐 LLMLingua-2 的 token 重要性裁剪，零依赖启发式）：
/// - 高重要 token 永不摘除：数字（答案关键）、命中 query 词根、结构锚。
/// - 低分 token（停用词/普通词/单字符标点）按 `trim_ratio` 保留——
///   先保分数高的，分数相同按原顺序。
/// - 行首/行尾 token 强制保留（保持行边界可读与拼接完整）。
/// - 组装按原文相邻性拼接：两 token 在原文中相邻（无分隔字符）则直接
///   相连，否则单空格分隔——CJK 逐字 token 因此保持成词（"内存" 不会
///   变成 "内 存"），ASCII token 间仍保留单空格。
pub(crate) fn trim_line(line: &str, query_terms: &[String], trim_ratio: f64, op_keep: bool) -> String {
    let t = tokens(line, query_terms, op_keep);
    let total = t.start.len();
    if total <= 1 {
        return line.trim().to_string();
    }
    // 高重要（数字/命中/锚）与行首尾：强制保留。
    let mut keep = vec![false; total];
    for (i, k) in keep.iter_mut().enumerate() {
        if score(&t, i) >= 2 || t.anchor[i] || i == 0 || i == total - 1 {
            *k = true;
        }
    }
    // 低分 token 候选（未强制保留）：保留分数较高的前 keep_low 个。
    // 同分按原顺序（稳定）。
    let low: Vec<usize> = (0..total).filter(|&i| !keep[i]).collect();
    if low.is_empty() {
        return line.trim().to_string();
    }
    let keep_low = ((low.len() as f64) * trim_ratio).ceil() as usize;
    let mut order: Vec<usize> = low.clone();
    order.sort_unstable_by(|&a, &b| {
        let sa = score(&t, a);
        let sb = score(&t, b);
        sb.partial_cmp(&sa)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.cmp(&b))
    });
    for &i in order.iter().take(keep_low) {
        keep[i] = true;
    }
    assemble(line, &t, &keep)
}

/// 组装：按原文相邻性拼接保留 token（原文相邻 → 直接相连，否则单空格）。
fn assemble(line: &str, t: &Tokens, keep: &[bool]) -> String {
    let mut out = String::new();
    let mut prev_end: Option<usize> = None;
    for (i, &k) in keep.iter().enumerate() {
        if k {
            if let Some(pe) = prev_end {
                if pe != t.start[i] {
                    out.push(' ');
                }
            }
            out.push_str(&line[t.start[i]..t.end[i]]);
            prev_end = Some(t.end[i]);
        }
    }
    out
}

fn score(t: &Tokens, i: usize) -> i32 {
    if t.anchor[i] {
        4
    } else if t.numeric[i] {
        3
    } else if t.hit[i] || t.keep[i] {
        2
    } else if t.stop[i] || t.punct[i] {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_numerics_and_structure() {
        let terms: Vec<String> = vec!["janet".into(), "ducks".into()];
        let line = "Q: Janet's ducks lay 16 eggs per day.";
        let out = trim_line(line, &terms, 0.5, false);
        assert!(out.contains("16"), "数字必须保留: {out}");
        assert!(
            out.to_lowercase().contains("janet"),
            "命中词必须保留: {out}"
        );
        assert!(out.len() < line.len(), "应有裁剪: {} -> {}", line, out);
    }

    #[test]
    fn trims_stopwords() {
        let terms: Vec<String> = vec![];
        let line = "the quick brown fox jumps over the lazy dog";
        let out = trim_line(line, &terms, 0.5, false);
        assert!(!out.contains(" the "), "停用词应被摘除: {out}");
        assert!(
            out.starts_with("the ") || out.starts_with("quick"),
            "行首保留: {out}"
        );
    }

    #[test]
    fn preserves_first_last() {
        let terms: Vec<String> = vec![];
        let line = "aaaa bbbb cccc dddd eeee ffff";
        let out = trim_line(line, &terms, 0.2, false);
        assert!(out.starts_with("aaaa"), "行首 token 必须保留: {out}");
        assert!(out.ends_with("ffff"), "行尾 token 必须保留: {out}");
    }

    #[test]
    fn empty_or_single_token_passthrough() {
        let terms: Vec<String> = vec![];
        assert_eq!(trim_line("", &terms, 0.5, false), "");
        assert_eq!(trim_line("alpha", &terms, 0.5, false), "alpha");
    }

    #[test]
    fn budget_full_preserves_line() {
        let terms: Vec<String> = vec![];
        let line = "alpha beta gamma";
        assert_eq!(trim_line(line, &terms, 1.0, false), line);
    }

    #[test]
    fn chat_anchor_survives_aggressive_trim() {
        let terms: Vec<String> = vec![];
        let line = "<|im_start|> user hello world <|im_end|>";
        let out = trim_line(line, &terms, 0.3, false);
        assert!(out.contains("<|im_start|>"), "chat 锚必须保留: {out}");
        assert!(out.contains("<|im_end|>"), "chat 锚必须保留: {out}");
    }

    #[test]
    fn cjk_keeps_hits_and_numbers_without_space() {
        let terms: Vec<String> = vec!["内存".into()];
        let line = "节点0 内存占用 512MB 缓存命中 耗时 0.3秒";
        let out = trim_line(line, &terms, 0.5, false);
        assert!(out.contains("内存"), "命中 CJK 词必须保留成词: {out}");
        assert!(!out.contains("内 存"), "CJK 字间不得插空格: {out}");
        assert!(out.contains('0'), "数字必须保留: {out}");
        assert!(out.contains('5'), "数字必须保留: {out}");
        assert!(
            out.len() < line.len(),
            "中文行应有裁剪: {} -> {}",
            line,
            out
        );
    }

    #[test]
    fn cjk_drops_stopword_chars() {
        let terms: Vec<String> = vec![];
        let line = "这个问题非常重要";
        let out = trim_line(line, &terms, 0.3, false);
        // 行首尾强制保留；中间的虚词/普通字按预算摘除，长度必须下降。
        assert!(
            out.chars().count() < line.chars().count(),
            "虚词应被摘除: {out}"
        );
    }

    #[test]
    fn keeps_semantic_sensitive_words() {
        let terms: Vec<String> = vec![];
        // not 在 STOP 里（0 分最易摘），但 KEEP 强制保留——语义反转不可接受。
        let line = "8 is not divisible by 5, but 10 is";
        let out = trim_line(line, &terms, 0.2, false);
        assert!(out.to_lowercase().contains("not"), "否定词必须保留: {out}");
        assert!(out.to_lowercase().contains("but"), "转折词必须保留: {out}");
        assert!(out.contains("8"), "数字必须保留: {out}");
        assert!(out.contains("10"), "数字必须保留: {out}");
    }

    #[test]
    fn cjk_negation_kept() {
        let terms: Vec<String> = vec![];
        let line = "这个数不能被 5 整除，但可以被 10 整除";
        let out = trim_line(line, &terms, 0.2, false);
        assert!(out.contains('不'), "中文否定字必须保留: {out}");
        assert!(out.contains('但'), "中文转折字必须保留: {out}");
        assert!(out.contains('5'), "数字必须保留: {out}");
        assert!(out.contains('1'), "数字必须保留: {out}");
    }
    #[test]
    fn percent_and_dollar_stay_with_numbers() {
        let terms: Vec<String> = vec![];
        // "30% of the trees die"：% 必须随数字保留，否则 30% 变成 30 棵（语义反转）。
        // "$2 per ticket"：$ 随数字保留，价格语义不丢。
        let line = "30% of the trees die, each costs $2";
        let out = trim_line(line, &terms, 0.2, false);
        assert!(out.contains("30%"), "百分比必须保留: {out}");
        assert!(out.contains("$2"), "货币符号必须随数字保留: {out}");
    }

    #[test]
    fn keeps_code_operators() {
        let terms: Vec<String> = vec![];
        // 代码场景：运算符删了会破坏语义（n ** 3 变 n 3）。
        let line = "print(n ** 3)  x -= 30  t = s * 2";
        let out = trim_line(line, &terms, 0.2, true);
        assert!(out.contains("**"), "幂运算符必须保留: {out}");
        assert!(out.contains("="), "赋值号必须保留: {out}");
        assert!(out.contains("*"), "乘号必须保留: {out}");
        assert!(out.contains("-"), "减号必须保留: {out}");
    }

    #[test]
    fn cjk_numeric_fullwidth_is_kept() {
        let terms: Vec<String> = vec![];
        let line = "总共 １２３ 个苹果";
        let out = trim_line(line, &terms, 0.2, false);
        assert!(out.contains('１'), "全角数字必须保留: {out}");
        assert!(out.contains('２'), "全角数字必须保留: {out}");
    }
}
