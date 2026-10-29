//! 压缩管线·选择与守卫阶段（compress/score）：证据/上下文/锚点/围栏/符号行
//! 标记、填充与预算、风险审计与守卫。纯函数、无 I/O；全部 `pub(crate)`。

use crate::policy::Limits;
use crate::{Config, Risk};

pub(crate) const R_SEED: u8 = 1;
pub(crate) const R_CONTEXT: u8 = 2;
pub(crate) const R_FILL: u8 = 3;
pub(crate) const R_GUARD: u8 = 4;
pub(crate) const R_ANCHOR: u8 = 5;
pub(crate) const R_NUM_LINE: u8 = 6;
pub(crate) const R_SYM_LINE: u8 = 7;
pub(crate) const R_ZH_LINE: u8 = 8;
pub(crate) const R_LIST_LINE: u8 = 9;
pub(crate) const R_EVIDENCE: u8 = 10;

/// 标记阶段共享可变上下文：keep 位图 / 计数 / 原因，与只读 scores/fill_floor。
pub(crate) struct Ctx<'a> {
    pub(crate) scores: &'a [f64],
    pub(crate) fill_floor: f64,
    pub(crate) keep: &'a mut [bool],
    pub(crate) kept: &'a mut usize,
    pub(crate) reason: &'a mut [u8],
}

pub(crate) fn floor(
    seed: &[bool],
    context: i64,
    total: usize,
    scores: &[f64],
    fill_floor: f64,
    reason: &mut [u8],
) -> (Vec<bool>, usize) {
    let mut keep = vec![false; total];
    let mut kept = 0usize;
    mark_seeds(seed, &mut keep, &mut kept, reason);
    mark_context(
        seed,
        context,
        total,
        &mut Ctx {
            scores,
            fill_floor,
            keep: &mut keep,
            kept: &mut kept,
            reason,
        },
    );
    (keep, kept)
}

pub(crate) fn mark_seeds(seed: &[bool], keep: &mut [bool], kept: &mut usize, reason: &mut [u8]) {
    for (i, &is_seed) in seed.iter().enumerate() {
        if is_seed {
            keep[i] = true;
            *kept += 1;
            reason[i] = R_SEED;
        }
    }
}

pub(crate) fn mark_context(seed: &[bool], context: i64, total: usize, ctx: &mut Ctx) {
    for (i, &is_seed) in seed.iter().enumerate() {
        if !is_seed {
            continue;
        }
        // 种子行左右 context 邻域（不含种子自身）：左区间 [i-context, i)，右区间 (i, i+context]。
        let lo = (i as i64 - context).max(0) as usize;
        for j in lo..i {
            keep_neighbor(j, ctx);
        }
        let hi = (i as i64 + context).min(total as i64 - 1) as usize;
        for j in (i + 1)..=hi {
            keep_neighbor(j, ctx);
        }
    }
}

/// 单邻域行落地：未被保留且分数不低于地板时标记 R_CONTEXT。
fn keep_neighbor(j: usize, ctx: &mut Ctx) {
    if !ctx.keep[j] {
        if ctx.fill_floor > 0.0 && ctx.scores[j] < ctx.fill_floor {
            return;
        }
        ctx.keep[j] = true;
        *ctx.kept += 1;
        ctx.reason[j] = R_CONTEXT;
    }
}

pub(crate) fn anchors(lines: &[&str], kinds: &[crate::AnchorKind]) -> Vec<bool> {
    let json = kinds.contains(&crate::AnchorKind::Json);
    let xml = kinds.contains(&crate::AnchorKind::Xml) || kinds.contains(&crate::AnchorKind::Idoc);
    let yaml = kinds.contains(&crate::AnchorKind::Yaml);
    let md = kinds.contains(&crate::AnchorKind::Markdown);
    let mut a = vec![false; lines.len()];
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        a[i] = (json && json_anchor(t))
            || (xml && tag_anchor(t))
            || (yaml && yaml_anchor(t))
            || (md && md_anchor(t));
    }
    a
}

pub(crate) fn json_anchor(t: &str) -> bool {
    let s = t.trim_end_matches(',');
    matches!(s, "{" | "}" | "[" | "]")
        || s.ends_with('{')
        || s.ends_with('[')
        || s.starts_with('}')
        || s.starts_with(']')
}

pub(crate) fn tag_anchor(t: &str) -> bool {
    if !t.starts_with('<') || !t.ends_with('>') {
        return false;
    }
    !t[1..t.len() - 1].contains('>')
}

pub(crate) fn yaml_anchor(t: &str) -> bool {
    if t.starts_with('#') || t.starts_with("---") || t.starts_with("...") {
        return false;
    }
    t.ends_with(':') || t.starts_with("- ")
}

pub(crate) fn md_anchor(t: &str) -> bool {
    if !t.starts_with('#') {
        return false;
    }
    let hashes = t.bytes().take_while(|&b| b == b'#').count();
    (1..=6).contains(&hashes) && (t.as_bytes().get(hashes) == Some(&b' ') || t.len() == hashes)
}

pub(crate) fn mark_anchors(
    anchors: &[bool],
    keep: &mut [bool],
    kept: &mut usize,
    reason: &mut [u8],
) {
    for (i, &a) in anchors.iter().enumerate() {
        if a && !keep[i] {
            keep[i] = true;
            *kept += 1;
            reason[i] = R_ANCHOR;
        }
    }
}

pub(crate) fn mark_structure_anchors(
    cfg: &Config,
    lines: &[&str],
    keep: &mut [bool],
    kept: &mut usize,
    reason: &mut [u8],
) {
    if cfg.structure_anchors.is_empty() {
        return;
    }
    let anchor_mask = anchors(lines, &cfg.structure_anchors);
    mark_anchors(&anchor_mask, keep, kept, reason);
}

pub(crate) fn mark_code_fence(
    lines: &[&str],
    keep: &mut [bool],
    kept: &mut usize,
    reason: &mut [u8],
    raw: &mut [bool],
) {
    let mut in_fence = false;
    for i in 0..lines.len() {
        let t = lines[i].trim();
        if t.starts_with("```") {
            in_fence = !in_fence;
            if !keep[i] {
                keep[i] = true;
                *kept += 1;
                reason[i] = R_ANCHOR;
            }
            raw[i] = true;
            continue;
        }
        if in_fence {
            if !keep[i] {
                keep[i] = true;
                *kept += 1;
                reason[i] = R_ANCHOR;
            }
            raw[i] = true;
        }
    }
}

pub(crate) fn mark_num_lines(
    lines: &[&str],
    dup: &[bool],
    keep: &mut [bool],
    kept: &mut usize,
    reason: &mut [u8],
) {
    for (i, line) in lines.iter().enumerate() {
        if keep[i] || dup[i] {
            continue;
        }
        if line.bytes().any(|b| b.is_ascii_digit()) {
            keep[i] = true;
            reason[i] = R_NUM_LINE;
            *kept += 1;
        }
    }
}

pub(crate) fn sym_line(t: &str) -> bool {
    let heads = [
        "fn ",
        "pub fn",
        "impl ",
        "int ",
        "void ",
        "long ",
        "size_t ",
        "char ",
        "double ",
        "float ",
        "bool ",
        "template ",
        "class ",
        "struct ",
        "static ",
        "const ",
        "unsigned ",
        "typedef ",
        "using ",
        "Signature: ",
    ];
    if heads.iter().any(|h| t.starts_with(h)) && (t.contains('(') || t.ends_with(';')) {
        return true;
    }
    t.contains(" -> ") && t.contains('(')
}

pub(crate) fn mark_sym_lines(
    lines: &[&str],
    dup: &[bool],
    keep: &mut [bool],
    kept: &mut usize,
    reason: &mut [u8],
    raw: &mut [bool],
) {
    for (i, line) in lines.iter().enumerate() {
        if dup[i] {
            continue;
        }
        // 签名行常含 query 命中词（如函数名）已被 keep——此时仍需设置 raw，
        // 否则已保留的签名行仍会被行内 trim 摘掉 `&`/`->`/`:`。
        if sym_line(line.trim()) {
            if !keep[i] {
                keep[i] = true;
                *kept += 1;
            }
            reason[i] = R_SYM_LINE;
            raw[i] = true;
        }
    }
}

pub(crate) fn eligible(sel: &Select, cfg: &Fill, keep: &[bool], i: usize) -> bool {
    !sel.dup[i]
        && !keep[i]
        && (sel.scores[i] > 0.0 || sel.struct_region[i])
        && (!cfg.region_gate || sel.in_region[i] || i < cfg.edge_head)
        && sel.scores[i] >= cfg.floor
}

pub(crate) struct Select<'a> {
    pub(crate) lines: &'a [&'a str],
    pub(crate) scores: &'a [f64],
    pub(crate) boost: &'a [f64],
    pub(crate) dup: &'a [bool],
    pub(crate) in_region: &'a [bool],
    pub(crate) struct_region: &'a [bool],
    /// 每行 token 估算（masks 预计算；空时回退现算）。
    pub(crate) tok: &'a [f64],
}

pub(crate) struct Fill {
    pub(crate) region_gate: bool,
    pub(crate) total_tok: f64,
    pub(crate) ratio: f64,
    pub(crate) edge_head: usize,
    pub(crate) remaining: f64,
    /// 填充入选分数门槛（`--fill-floor`；0 = 现状）。
    pub(crate) floor: f64,
}

pub(crate) fn fill(
    sel: &Select,
    cfg: &Fill,
    keep: &mut [bool],
    kept: &mut usize,
    reason: &mut [u8],
) -> f64 {
    let total = sel.lines.len();
    let budget = (cfg.total_tok * cfg.ratio).max(crate::policy::BUDGET_MIN);
    let cap = budget.min(cfg.remaining);
    let mut order: Vec<(usize, f64)> = (0..total)
        .filter(|&i| eligible(sel, cfg, keep, i))
        .map(|i| (i, sel.scores[i] + sel.boost[i]))
        .collect();
    order.sort_unstable_by(|&(a, sa), &(b, sb)| {
        if sb != sa {
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        } else {
            a.cmp(&b)
        }
    });
    let mut used = 0.0f64;
    for &(i, _) in &order {
        let t = est(sel.tok, sel.lines, i);
        if used + t > cap {
            continue;
        }
        if !keep[i] {
            keep[i] = true;
            *kept += 1;
            reason[i] = R_FILL;
        }
        used += t;
    }
    used
}

pub(crate) fn est(tok: &[f64], lines: &[&str], i: usize) -> f64 {
    tok.get(i)
        .copied()
        .unwrap_or_else(|| crate::terms::estimate(lines[i]))
}

pub(crate) fn tokens(lines: &[&str], keep: &[bool], tok: &[f64]) -> f64 {
    let mut sum = 0.0f64;
    for (i, &k) in keep.iter().enumerate() {
        if k {
            sum += est(tok, lines, i);
        }
    }
    sum
}

pub(crate) fn audit(keep: &[bool], err: &[bool], hits: &[usize], tbl: &Limits) -> Risk {
    let (err_dropped, hit_dropped) = dropped(keep, err, hits);
    let kept = keep.iter().filter(|&&k| k).count();
    let risk_raw = (err_dropped + hit_dropped * tbl.hit_drop_w) as f64 / kept.max(1) as f64;
    level(risk_raw, tbl)
}

pub(crate) fn dropped(keep: &[bool], err: &[bool], hits: &[usize]) -> (usize, usize) {
    let mut err_dropped = 0usize;
    let mut hit_dropped = 0usize;
    for i in 0..keep.len() {
        if keep[i] {
            continue;
        }
        if err[i] {
            err_dropped += 1;
        }
        if hits[i] > 0 {
            hit_dropped += 1;
        }
    }
    (err_dropped, hit_dropped)
}

pub(crate) fn level(raw: f64, tbl: &Limits) -> Risk {
    if raw >= tbl.high {
        Risk::High
    } else if raw >= tbl.med {
        Risk::Medium
    } else {
        Risk::Low
    }
}

pub(crate) fn guard(
    keep: &mut [bool],
    kept: &mut usize,
    dup: &[bool],
    guard: &[bool],
    marker: &[bool],
    region: Option<&[bool]>,
    reason: &mut [u8],
) -> u8 {
    let mut guarded = 0u8;
    for i in 0..keep.len() {
        if keep[i] || dup[i] {
            continue;
        }
        // 区域门控：只保护证据邻域内的错误/标记行（根因聚集在证据附近），
        // 丢弃远处无关错误行；None 时为现状全量保护。
        if region.is_some_and(|r| !r[i]) {
            continue;
        }
        if guard[i] || marker[i] {
            keep[i] = true;
            *kept += 1;
            reason[i] = R_GUARD;
            guarded = 1;
        }
    }
    guarded
}

pub(crate) fn ev_retention(
    evidence: &[crate::Evidence],
    numbers: &[usize],
    text: &str,
    kept: usize,
    total: usize,
) -> f64 {
    if total == 0 {
        return 1.0;
    }
    if evidence.is_empty() {
        return kept as f64 / total as f64;
    }
    let mut hit = 0usize;
    for e in evidence {
        let ok = match e {
            crate::Evidence::Line(n) => {
                // kept == total 表示未删任何行（passthrough/fail-open/no-kept），
                // 此时行号 1..=total 全部存活；否则按保留行号集合判定。
                if kept == total {
                    *n >= 1 && *n <= total
                } else {
                    numbers.contains(n)
                }
            }
            crate::Evidence::Substr(s) => text.contains(s.as_str()),
        };
        if ok {
            hit += 1;
        }
    }
    hit as f64 / evidence.len() as f64
}

/// 中文问句行识别（--zh-keep）：含疑问词/问号的中文行。
/// 疑问词表覆盖中文问答题干（多少/什么/如何/为什么/是否/哪个/哪里/哪些/
/// 请用/请回答/输出什么/等于多少/值是多少 等）；无 CJK 的行直接拒绝，
/// 英文/代码行不受影响。
pub(crate) fn zh_line(t: &str) -> bool {
    const Q: &[&str] = &[
        "多少",
        "什么",
        "如何",
        "为什么",
        "是否",
        "哪个",
        "哪里",
        "哪些",
        "哪种",
        "请用",
        "请回答",
        "请给出",
        "请输出",
        "输出什么",
        "等于多少",
        "值是多少",
        "是多少",
        "哪一",
        "几次",
        "几个",
        "的区别",
    ];
    if !t.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)) {
        return false;
    }
    Q.iter().any(|w| t.contains(w)) || t.trim_end().ends_with('？') || t.trim_end().ends_with('?')
}

/// 中文问句行整行保留并标记 raw（跳过行内 trim）——问句行是题目的
/// 语义核心，trim 逐字摘碎后模型无法理解（P8-E 实测保持率 0.2 根因）。
pub(crate) fn mark_zh_lines(
    lines: &[&str],
    dup: &[bool],
    keep: &mut [bool],
    kept: &mut usize,
    reason: &mut [u8],
    raw: &mut [bool],
) {
    for (i, line) in lines.iter().enumerate() {
        if dup[i] {
            continue;
        }
        if zh_line(line.trim()) {
            if !keep[i] {
                keep[i] = true;
                *kept += 1;
            }
            reason[i] = R_ZH_LINE;
            raw[i] = true;
        }
    }
}

/// 是否为列表行（markdown/文本列表，`--list-keep` 使用）。
/// 保守判定：标记后必须紧跟分隔（ASCII 空格或 CJK），避免误伤代码
/// （`*ptr`/`-x`/`1+2`/`1.5f`/`1)` 函数调用均不命中）。
/// 支持：无序 `- ` / `* ` / `+ `，任务清单 `[ ] ` / `[x] `，
/// 有序 `1. ` / `1) ` / `(1) ` / `a. ` / `A) `。
pub(crate) fn list_line(t: &str) -> bool {
    let t = t.trim();
    let b = t.as_bytes();
    if b.is_empty() {
        return false;
    }
    if b[0] == b'[' {
        return list_task(b);
    }
    if b[0] == b'-' || b[0] == b'*' || b[0] == b'+' {
        return list_sep(b, 1);
    }
    list_ordered(b)
}

/// 列表标记后的分隔检查：ASCII 空格 或 非 ASCII（CJK）——中文文档常写
/// `-中文`（无空格）；行尾无分隔不算列表（markdown 语法要求空格）。
fn list_sep(b: &[u8], i: usize) -> bool {
    match b.get(i) {
        Some(&c) => c == b' ' || !c.is_ascii(),
        None => false,
    }
}

/// 任务清单：`[ ] ` / `[x] ` / `[X] `（严格空格）。
fn list_task(b: &[u8]) -> bool {
    if b.len() <= 3 || b[0] != b'[' {
        return false;
    }
    matches!(&b[..3], b"[ ]" | b"[x]" | b"[X]") && b[3] == b' '
}

/// 有序列表：数字 `1. `/`1) `/`(1) `，或字母 `a. `/`A) `。
fn list_ordered(b: &[u8]) -> bool {
    let mut i = if b[0] == b'(' { 1 } else { 0 };
    let mut digits = 0;
    while i < b.len() && b[i].is_ascii_digit() && digits < 3 {
        digits += 1;
        i += 1;
    }
    if digits > 0 {
        let s = b.get(i).copied();
        return (s == Some(b'.') || s == Some(b')')) && list_sep(b, i + 1);
    }
    b[0].is_ascii_alphabetic()
        && (b.get(1) == Some(&b'.') || b.get(1) == Some(&b')'))
        && list_sep(b, 2)
}

/// 列表行整行保留并标记 raw（跳过行内 trim）——枚举型文档（配置清单、
/// 依赖列表、TODO）的列表项是"全量枚举"任务的硬信息，字符级 trim 会
/// 摘词导致模型恢复清单漏项（P11-B2 实测 instruct 保持率 0.000 根因）。
pub(crate) fn mark_list_lines(
    lines: &[&str],
    dup: &[bool],
    keep: &mut [bool],
    kept: &mut usize,
    reason: &mut [u8],
    raw: &mut [bool],
) {
    for (i, line) in lines.iter().enumerate() {
        if dup[i] {
            continue;
        }
        if list_line(line.trim()) {
            if !keep[i] {
                keep[i] = true;
                *kept += 1;
            }
            reason[i] = R_LIST_LINE;
            raw[i] = true;
        }
    }
}

/// 证据行整行保留并标记 raw（--evidence）——Substr 命中的行与 Line 号命中的
/// 行强制 keep+raw：证据是问答的硬锚，字符级 trim 会摘词导致证据 marker
/// 丢失而保留率报告仍称"全部保留"（P12.6 实测：声明 --evidence 后证据行
/// 被裁剪为残句，正文不含证据子串）。dup 折叠的证据行一并强制保留。
pub(crate) fn mark_evidence_lines(
    lines: &[&str],
    evidence: &[crate::Evidence],
    keep: &mut [bool],
    kept: &mut usize,
    reason: &mut [u8],
    raw: &mut [bool],
) {
    for e in evidence {
        match e {
            crate::Evidence::Substr(s) => {
                mark_evidence_substr(s.as_str(), lines, keep, kept, reason, raw);
            }
            crate::Evidence::Line(n) => {
                mark_evidence_line(*n, lines, keep, kept, reason, raw);
            }
        }
    }
}

/// Substr 证据：命中子串的行整行保留并 raw。
fn mark_evidence_substr(
    s: &str,
    lines: &[&str],
    keep: &mut [bool],
    kept: &mut usize,
    reason: &mut [u8],
    raw: &mut [bool],
) {
    for (i, line) in lines.iter().enumerate() {
        if line.contains(s) {
            if !keep[i] {
                keep[i] = true;
                *kept += 1;
            }
            reason[i] = R_EVIDENCE;
            raw[i] = true;
        }
    }
}

/// Line 证据：行号（1-based）命中的行整行保留并 raw；越界静默忽略。
fn mark_evidence_line(
    n: usize,
    lines: &[&str],
    keep: &mut [bool],
    kept: &mut usize,
    reason: &mut [u8],
    raw: &mut [bool],
) {
    if n >= 1 && n <= lines.len() {
        let i = n - 1;
        if !keep[i] {
            keep[i] = true;
            *kept += 1;
        }
        reason[i] = R_EVIDENCE;
        raw[i] = true;
    }
}
