//! compress 引擎主体：证据筛选压缩的完整算法。
//!
//! 纯函数、无 I/O、无随机源；本文件不定义 Config/Outcome（在 crate 根），只实现
//! 从文本到压缩结果的单趟管线。管线拆分为独立阶段函数（切行 / 扫描 / 折叠 /
//! 权重 / 种子 / 区域 / 打分 / 邻域 / 落地 / 预算 / 审计 / 守卫 / 渲染），
//! 各阶段函数圈复杂度 < 10；可调参数与加分权重全部声明于 `policy` 表，
//! 不在函数体内硬编码。独立成文件以维持 crate 根 <500 行门禁。

use crate::analysis::{
    error_flags, is_diffline, is_diffmeta, is_frame, is_heading, is_incident_marker, is_lineref,
};
use crate::analysis::{norm_into, push_analysis_lower};
use crate::policy::{RiskTable, ScoreTable, DEFAULT_RISK, DEFAULT_SCORES};
use crate::terms::{build_terms, token_est};
use crate::{push_lineno, Config, Mode, Outcome, Risk};
use std::collections::HashMap;

/// 字符数用于报告与压缩比；日志常见纯 ASCII，可跳过逐字符迭代。
fn char_count(text: &str) -> usize {
    if text.is_ascii() {
        text.len()
    } else {
        text.chars().count()
    }
}

/// 切行：CRLF 归一化到 LF 后按行切分并去掉末尾空行。
/// 含 CR 时归一化结果暂存于 `buf`（由调用者持有），返回行切片借自 `buf`；
/// 无 CR 时零拷贝直接借用输入。
fn split_lines<'b, 'a: 'b>(text: &'a str, buf: &'b mut Option<String>) -> Vec<&'b str> {
    let src: &str = if text.contains('\r') {
        buf.replace(text.replace("\r\n", "\n"));
        buf.as_ref().unwrap().as_str()
    } else {
        text
    };
    let mut lines: Vec<&str> = src.split('\n').collect();
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines
}

/// 解析 `--force`：按 `|` 拆分的原始子串，trim + 小写，跳过空项。
fn parse_forces(forces: &[String]) -> Vec<String> {
    let mut force_subs: Vec<String> = Vec::new();
    for f in forces {
        for part in f.split('|') {
            let p = part.trim().to_lowercase();
            if !p.is_empty() {
                force_subs.push(p);
            }
        }
    }
    force_subs
}

/// 逐行 × 逐词位掩码扫描结果：命中掩码、错误/标记/守卫/force 判定与 token 估算。
struct Masks {
    mask_v: Vec<u64>,
    words: usize,
    err_v: Vec<bool>,
    marker_v: Vec<bool>,
    guard_v: Vec<bool>,
    force_v: Vec<bool>,
    total_tok: f64,
}

/// 一次逐行×逐词扫描出命中位掩码；错误/标记/守卫/force 判定复用同一小写缓冲（无逐行分配）。
/// `do_tok` 为真时在循环内累计 token 估算（大输入 + 非针尖档），避免为同一批行再走一趟。
fn scan_masks(
    lines: &[&str],
    terms: &[String],
    forces: &[String],
    small_tok: f64,
    do_tok: bool,
) -> Masks {
    let total = lines.len();
    let words = terms.len().div_ceil(64).max(1);
    let mut mask_v = vec![0u64; total * words];
    let mut err_v = vec![false; total];
    let mut marker_v = vec![false; total];
    let mut guard_v = vec![false; total];
    let mut force_v = vec![false; total];
    let mut total_tok = if do_tok { 0.0 } else { small_tok };
    let mut low = String::new();
    for i in 0..total {
        if do_tok {
            total_tok += token_est(lines[i]);
        }
        low.clear();
        push_analysis_lower(lines[i], &mut low);
        let ll = low.as_str();
        (err_v[i], guard_v[i]) = error_flags(ll);
        marker_v[i] = is_incident_marker(ll);
        force_v[i] = forces.iter().any(|p| ll.contains(p.as_str()));
        let base = i * words;
        for (k, t) in terms.iter().enumerate() {
            if ll.contains(t.as_str()) {
                mask_v[base + k / 64] |= 1u64 << (k % 64);
            }
        }
    }
    Masks {
        mask_v,
        words,
        err_v,
        marker_v,
        guard_v,
        force_v,
        total_tok,
    }
}

/// 去重：归一化后第 3 次起的重复行折叠（除非含查询命中、错误行或 force 命中）。
fn fold_dups(
    lines: &[&str],
    mask: &[u64],
    words: usize,
    err: &[bool],
    force: &[bool],
) -> (Vec<bool>, usize) {
    let total = lines.len();
    let has_hit = |i: usize| (0..words).any(|w| mask[i * words + w] != 0);
    let mut seen_count: HashMap<String, usize> = HashMap::new();
    let mut dup = vec![false; total];
    let mut dup_n = 0usize;
    let mut key = String::new();
    let mut run = String::new();
    for i in 0..total {
        key.clear();
        norm_into(lines[i], &mut key, &mut run);
        if key.chars().count() < 6 {
            continue;
        }
        if let Some(n) = seen_count.get_mut(key.as_str()) {
            *n += 1;
            if *n > 2 && !has_hit(i) && !err[i] && !force[i] {
                dup[i] = true;
                dup_n += 1;
            }
        } else {
            seen_count.insert(key.clone(), 1);
        }
    }
    (dup, dup_n)
}

/// 词频（df）与 IDF 权重：只统计非折叠行。
struct TermStat {
    df: Vec<usize>,
    weights: Vec<f64>,
    rare_thr: usize,
}

fn term_weights(mask: &[u64], words: usize, dup: &[bool], n_terms: usize) -> TermStat {
    let total = dup.len();
    let live: usize = (0..total).filter(|&i| !dup[i]).count();
    let n_live = live.max(1) as f64;
    let mut df = vec![0usize; n_terms];
    for (i, &is_dup) in dup.iter().enumerate() {
        if is_dup {
            continue;
        }
        let base = i * words;
        for w in 0..words {
            let mut bits = mask[base + w];
            while bits != 0 {
                let k = w * 64 + bits.trailing_zeros() as usize;
                bits &= bits - 1;
                if k < n_terms {
                    df[k] += 1;
                }
            }
        }
    }
    let weights: Vec<f64> = df
        .iter()
        .map(|&d| (1.0 + n_live / (1.0 + d as f64)).ln())
        .collect();
    let rare_thr = (live / 5).max(1);
    TermStat {
        df,
        weights,
        rare_thr,
    }
}

/// 逐行证据判定：种子只来自查询证据（≥2 命中或稀有单词命中）与 `--force`。
struct Seeds {
    seed: Vec<bool>,
    hits: Vec<usize>,
    sumw: Vec<f64>,
    hit_lines: usize,
    force_any: bool,
}

fn seed_marks(
    mask: &[u64],
    words: usize,
    dup: &[bool],
    df: &[usize],
    weights: &[f64],
    rare_thr: usize,
    force: &[bool],
) -> Seeds {
    let total = dup.len();
    let mut hits = vec![0usize; total];
    let mut sumw = vec![0.0f64; total];
    let mut seed = vec![false; total];
    let mut hit_lines = 0usize;
    let mut force_any = false;
    for i in 0..total {
        if dup[i] {
            continue;
        }
        let base = i * words;
        let mut h = 0usize;
        let mut sw = 0.0f64;
        let mut lone = usize::MAX;
        for w in 0..words {
            let mut bits = mask[base + w];
            while bits != 0 {
                let k = w * 64 + bits.trailing_zeros() as usize;
                bits &= bits - 1;
                if k >= df.len() {
                    continue;
                }
                h += 1;
                sw += weights[k];
                lone = if h == 1 { k } else { usize::MAX };
            }
        }
        let fm = force[i];
        force_any |= fm;
        hits[i] = h;
        sumw[i] = sw;
        seed[i] = is_seed_candidate(h, lone, df, rare_thr, fm);
        if h > 0 {
            hit_lines += 1;
        }
    }
    Seeds {
        seed,
        hits,
        sumw,
        hit_lines,
        force_any,
    }
}

/// 种子区域：±window 扩展，间隔 <= window*3 合并；区域外素材不参与可选预算。
struct Regions {
    in_region: Vec<bool>,
    count: usize,
    gate: bool,
}

fn expand_regions(seed: &[bool], window: usize, total: usize) -> Regions {
    let w = window;
    let maxgap = (w * crate::policy::GAP_RATIO).max(2);
    let seed_idx: Vec<usize> = (0..total).filter(|&i| seed[i]).collect();
    let mut in_region = vec![false; total];
    let mut count = 0usize;
    if !seed_idx.is_empty() {
        let mut a = seed_idx[0].saturating_sub(w);
        let mut b = (seed_idx[0] + w).min(total - 1);
        for &s in &seed_idx[1..] {
            if s <= b + maxgap {
                b = (s + w).min(total - 1);
            } else {
                for slot in in_region.iter_mut().take(b + 1).skip(a) {
                    *slot = true;
                }
                count += 1;
                a = s.saturating_sub(w);
                b = (s + w).min(total - 1);
            }
        }
        for slot in in_region.iter_mut().take(b + 1).skip(a) {
            *slot = true;
        }
        count += 1;
    }
    Regions {
        in_region,
        count,
        gate: !seed_idx.is_empty(),
    }
}

/// 单行结构加分：`tbl` 表驱动，判定顺序与 v0.4.1 硬编码 if 链一致（浮点加法顺序不变）。
fn score_line(
    lines: &[&str],
    hits: &[usize],
    sumw: &[f64],
    err: &[bool],
    force: &[bool],
    i: usize,
    tbl: &ScoreTable,
) -> f64 {
    let total = lines.len();
    let line = lines[i];
    let mut sc = sumw[i] * tbl.sumw_mul;
    if hits[i] >= 2 {
        sc += tbl.pair_bonus;
    }
    if err[i] {
        sc += tbl.error_bonus;
    }
    if is_heading(line) {
        sc += tbl.heading_bonus;
    }
    if is_diffmeta(line) {
        sc += tbl.diffmeta_bonus;
    }
    if diff_hit(line, hits[i]) {
        sc += tbl.diffline_hit_bonus;
    }
    if frame_hint(line) {
        sc += tbl.frame_bonus;
    }
    if force[i] {
        sc += tbl.force_bonus;
    }
    if edge_line(i, total, tbl) {
        sc += tbl.edge_bonus;
    }
    sc
}

/// diff 内容行且命中（结构判定与命中判定合并为单布尔，降低 score_line 判定点）。
fn diff_hit(line: &str, hits: usize) -> bool {
    is_diffline(line) && hits > 0
}

/// 堆栈帧 / 行引用（score_line 的结构加分判定）。
fn frame_hint(line: &str) -> bool {
    is_frame(line) || is_lineref(line)
}

/// 首 `edge_head` / 尾 `edge_tail` 身份行。
fn edge_line(i: usize, total: usize, tbl: &ScoreTable) -> bool {
    i < tbl.edge_head || i >= total.saturating_sub(tbl.edge_tail)
}

/// 逐行打分：基础 = IDF 加权命中；结构加分只提升排序价值。
fn score_lines(
    lines: &[&str],
    dup: &[bool],
    hits: &[usize],
    sumw: &[f64],
    err: &[bool],
    force: &[bool],
    tbl: &ScoreTable,
) -> Vec<f64> {
    let total = lines.len();
    let mut scores = vec![0.0f64; total];
    for i in 0..total {
        if dup[i] {
            continue;
        }
        scores[i] = score_line(lines, hits, sumw, err, force, i, tbl);
    }
    scores
}

/// 邻域权重：±window 范围内的高分给排序加权（不授予独立入选资格）。
/// 合成最终排序分，后续比较只读一次，避免 O(n log n) 次重复浮点加法。
fn neighborhood_boost(scores: &[f64], dup: &[bool], window: usize, tbl: &ScoreTable) -> Vec<f64> {
    let total = scores.len();
    let mut boost = vec![0.0f64; total];
    for (i, b) in boost.iter_mut().enumerate() {
        let mut m = 0.0f64;
        for d in -(window as i64)..=window as i64 {
            let j = i as i64 + d;
            if j >= 0 && (j as usize) < total && !dup[j as usize] {
                m = m.max(scores[j as usize]);
            }
        }
        *b = (m / tbl.boost_div).min(tbl.boost_cap);
    }
    boost
}

/// 先落种子与最小可读邻域；预算只约束其后的可选内容。
fn seed_floor(seed: &[bool], context: i64, total: usize) -> (Vec<bool>, usize) {
    let mut keep = vec![false; total];
    let mut keep_count = 0usize;
    for (i, &is_seed) in seed.iter().enumerate() {
        if !is_seed {
            continue;
        }
        for d in -context..=context {
            let j = i as i64 + d;
            if j >= 0 && (j as usize) < total && !keep[j as usize] {
                keep[j as usize] = true;
                keep_count += 1;
            }
        }
    }
    (keep, keep_count)
}

/// 预算填充候选资格：非折叠、未保留、真实得分，且位于种子区域（首 `edge_head`
/// 身份行豁免区域门控）。
fn budget_eligible(
    i: usize,
    edge_head: usize,
    dup: &[bool],
    keep: &[bool],
    scores: &[f64],
    region_gate: bool,
    in_region: &[bool],
) -> bool {
    !dup[i] && !keep[i] && scores[i] > 0.0 && (!region_gate || in_region[i] || i < edge_head)
}

/// 预算选择：种子区域门控（首两行身份行除外）；独立入选须真实得分（scores > 0）。
/// 预算填充的只读切片上下文（slice 借用归组，避免超长参数表）。
struct Select<'a> {
    lines: &'a [&'a str],
    scores: &'a [f64],
    boost: &'a [f64],
    dup: &'a [bool],
    in_region: &'a [bool],
}

/// 预算填充的标量配置。
struct FillCfg {
    region_gate: bool,
    total_tok: f64,
    ratio: f64,
    edge_head: usize,
    remaining: f64,
}

/// 预算选择：种子区域门控（首两行身份行除外）；独立入选须真实得分（scores > 0）。
/// 排序按 `scores + boost` 降序、行号升序（确定性 tie-break）。
/// `remaining` 为 `--max-tokens` 下可选填充的可分配余量（无上限时 +∞）；
/// 填充总量受 `min(budget, remaining)` 双重约束。
fn fill_budget(sel: &Select, cfg: &FillCfg, keep: &mut [bool], keep_count: &mut usize) {
    let total = sel.lines.len();
    let budget = (cfg.total_tok * cfg.ratio).max(crate::policy::BUDGET_MIN);
    let cap = budget.min(cfg.remaining);
    let mut order: Vec<(usize, f64)> = (0..total)
        .filter(|&i| {
            budget_eligible(
                i,
                cfg.edge_head,
                sel.dup,
                keep,
                sel.scores,
                cfg.region_gate,
                sel.in_region,
            )
        })
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
        let t = token_est(sel.lines[i]);
        if used + t > cap {
            continue;
        }
        if !keep[i] {
            keep[i] = true;
            *keep_count += 1;
        }
        used += t;
    }
}

/// 已保留行的 token 估算总和（`--max-tokens` 记账口径）。
fn kept_tokens(lines: &[&str], keep: &[bool]) -> f64 {
    let mut sum = 0.0f64;
    for (i, &k) in keep.iter().enumerate() {
        if k {
            sum += token_est(lines[i]);
        }
    }
    sum
}

/// 风险审计：统计被丢弃的失败级错误行与命中行，按 `keep_count` 归一化得出 risk_raw。
fn audit_risk(keep: &[bool], err: &[bool], hits: &[usize], tbl: &RiskTable) -> Risk {
    let mut err_dropped = 0usize;
    let mut hit_dropped = 0usize;
    let keep_count = keep.iter().filter(|&&k| k).count();
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
    let risk_raw = (err_dropped + hit_dropped * tbl.hit_drop_w) as f64 / keep_count.max(1) as f64;
    if risk_raw >= tbl.high {
        Risk::High
    } else if risk_raw >= tbl.med {
        Risk::Medium
    } else {
        Risk::Low
    }
}

/// 风险守卫：HIGH 时保护失败级错误行与事故标记行（warn/警告 级除外）。
fn guard_high(
    keep: &mut [bool],
    keep_count: &mut usize,
    dup: &[bool],
    guard: &[bool],
    marker: &[bool],
) -> u8 {
    let mut guarded = 0u8;
    for i in 0..keep.len() {
        if keep[i] || dup[i] {
            continue;
        }
        if guard[i] || marker[i] {
            keep[i] = true;
            *keep_count += 1;
            guarded = 1;
        }
    }
    guarded
}

/// 输出：零克隆直装；省略区间以单行 "..." 标记。保留行原始行号同步收集（i+1）。
fn render(lines: &[&str], keep: &[bool], elide: bool, lineno: bool) -> (String, Vec<usize>) {
    let mut out_text = String::new();
    let mut kept_lno = Vec::new();
    let mut in_gap = false;
    for (i, &k) in keep.iter().enumerate() {
        if k {
            if in_gap && elide {
                out_text.push_str("...\n");
            }
            in_gap = false;
            if lineno {
                push_lineno(&mut out_text, i + 1);
            }
            out_text.push_str(lines[i]);
            out_text.push('\n');
            kept_lno.push(i + 1);
        } else {
            in_gap = true;
        }
    }
    if in_gap && elide {
        out_text.push_str("...\n");
    }
    (out_text, kept_lno)
}

/// 对 `text` 按 `query` 做证据筛选压缩。纯函数：无文件、环境与进程 I/O。
/// 编排器只做阶段调用与决策合并（McCabe 判定点 < 10）；各阶段逻辑在
/// 独立小函数中（`norm_config` / `pass_shape_decision` / `cap_*` 等）。
pub fn compress(text: &str, query: &str, cfg: &Config) -> Outcome {
    let (ratio, context, window) = norm_config(cfg);
    // 分析使用 LF 归一化文本；passthrough/fail-open 返回输入原文，保持“原样保留”语义。
    let original = text;
    // 零拷贝快路径：无 CR 时直接借用原文本；行以 &str 切片保存，避免整文复制与逐行分配。
    let mut nl_buf: Option<String> = None;
    let lines = split_lines(original, &mut nl_buf);
    let uniq_terms = build_terms(query);
    let force_subs = parse_forces(&cfg.forces);
    let total = lines.len();
    // token 估算只服务小输入直通与可选预算；针尖档大盘无需整份扫描。
    // 小输入先算一次即可（随后可能直接 passthrough）；大输入把估算并入
    // 逐行分析循环，避免为同一批行再走一趟缓存。
    let small_tok = small_token_est(&lines, total);
    let chars_a = char_count(original);

    let mk = |text_owned: String,
              kept: usize,
              chars_b: usize,
              cut: f64,
              dup: usize,
              seeds: usize,
              risk: Risk,
              guarded: u8,
              regions: usize,
              kept_lno: Vec<usize>,
              mode: Mode,
              max_tokens: Option<usize>,
              tok_est: f64|
     -> Outcome {
        Outcome {
            text: text_owned,
            kept_lno,
            kept,
            total,
            chars_in: chars_a,
            chars_out: chars_b,
            cut_pct: cut,
            dup,
            seeds,
            evidence: seeds,
            risk,
            guarded,
            regions,
            window,
            terms: uniq_terms.len(),
            forced: force_subs.len(),
            mode,
            max_tokens,
            tok_est,
        }
    };

    // 直通早退：小输入 / 超长行（无查询参与，原样返回）。
    if let Some(mode) = pass_shape_decision(total, small_tok, any_longline(&lines)) {
        return mk(
            original.to_string(),
            total,
            chars_a,
            0.0,
            0,
            0,
            Risk::Low,
            0,
            0,
            Vec::new(),
            mode,
            cfg.max_tokens,
            0.0,
        );
    }

    let do_tok = total >= crate::policy::SMALL_LINES && context != 0;
    let masks = scan_masks(&lines, &uniq_terms, &force_subs, small_tok, do_tok);
    let (dup, dup_n) = fold_dups(
        &lines,
        &masks.mask_v,
        masks.words,
        &masks.err_v,
        &masks.force_v,
    );
    let stat = term_weights(&masks.mask_v, masks.words, &dup, uniq_terms.len());
    let seeds = seed_marks(
        &masks.mask_v,
        masks.words,
        &dup,
        &stat.df,
        &stat.weights,
        stat.rare_thr,
        &masks.force_v,
    );
    let seeds_total = (0..total).filter(|&i| seeds.seed[i]).count();

    // 无信号 fail-open：零命中且无 --force 命中 => 原样保留。
    if failopen_decision(seeds.hit_lines, seeds.force_any) {
        return mk(
            original.to_string(),
            total,
            chars_a,
            0.0,
            dup_n,
            seeds_total,
            Risk::Low,
            0,
            0,
            Vec::new(),
            Mode::FailOpenZeroSignal,
            cfg.max_tokens,
            0.0,
        );
    }

    let regions = expand_regions(&seeds.seed, window as usize, total);
    let scores = score_lines(
        &lines,
        &dup,
        &seeds.hits,
        &seeds.sumw,
        &masks.err_v,
        &masks.force_v,
        &DEFAULT_SCORES,
    );
    let boost = neighborhood_boost(&scores, &dup, window as usize, &DEFAULT_SCORES);
    let (mut keep, mut keep_count) = seed_floor(&seeds.seed, context, total);

    // --max-tokens 硬上限：证据地板最高优先。证据 + 最小邻域先落，其 token
    // 估算超限时直接进入 OverflowEvidence（保留全部证据，如实报告），不再
    // 填充可选内容；未超限时可选填充受 `min(budget, max - floor)` 双重约束。
    let tok_floor = kept_tokens(&lines, &keep);
    let mut overflow = cap_floor(tok_floor, cfg.max_tokens);

    // --context 0（针尖模式）：完全跳过可选预算填充，只保留证据行与守卫行。
    if context != 0 && overflow.is_none() {
        let remaining = cap_remaining(tok_floor, cfg.max_tokens);
        fill_budget(
            &Select {
                lines: &lines,
                scores: &scores,
                boost: &boost,
                dup: &dup,
                in_region: &regions.in_region,
            },
            &FillCfg {
                region_gate: regions.gate,
                total_tok: masks.total_tok,
                ratio,
                edge_head: DEFAULT_SCORES.edge_head,
                remaining,
            },
            &mut keep,
            &mut keep_count,
        );
    }

    let risk = audit_risk(&keep, &masks.err_v, &seeds.hits, &DEFAULT_RISK);
    let mut guarded = 0u8;
    if risk == Risk::High && cfg.risk_gate {
        guarded = guard_high(
            &mut keep,
            &mut keep_count,
            &dup,
            &masks.guard_v,
            &masks.marker_v,
        );
    }

    // 守卫后最终检查：填充受上限约束，但风险守卫可能追加证据性行；如实报告 OverflowFill。
    let final_tok = kept_tokens(&lines, &keep);
    if overflow.is_none() {
        overflow = cap_final(final_tok, cfg.max_tokens);
    }

    // 预算过紧或只有高频单查询命中时可能选不出任何行；此时回退原文，不返回静默空结果。
    if keep_count == 0 {
        return mk(
            original.to_string(),
            total,
            chars_a,
            0.0,
            dup_n,
            seeds_total,
            risk,
            0,
            regions.count,
            Vec::new(),
            Mode::PassNoKept,
            cfg.max_tokens,
            0.0,
        );
    }

    let (out_text, kept_lno) = render(&lines, &keep, cfg.elide, cfg.lineno);
    let (chars_b, cut_pct) = cut_stats(&out_text, chars_a);
    mk(
        out_text,
        keep_count,
        chars_b,
        cut_pct,
        dup_n,
        seeds_total,
        risk,
        guarded,
        regions.count,
        kept_lno,
        overflow.unwrap_or(Mode::Sieve),
        cfg.max_tokens,
        final_tok,
    )
}

/// 配置规范化（库是公开接口：入口统一钳制，避免 CLI 之外的调用触发溢出或超大循环）。
fn norm_config(cfg: &Config) -> (f64, i64, i64) {
    let ratio = if cfg.ratio.is_nan() {
        crate::policy::DEFAULT_RATIO
    } else {
        cfg.ratio.clamp(crate::policy::MIN_RATIO, 1.0)
    };
    let context = cfg.context.clamp(0, crate::policy::MAX_CONTEXT);
    let window = if cfg.window < crate::policy::MIN_WINDOW {
        crate::policy::MIN_WINDOW
    } else {
        cfg.window.min(crate::policy::MAX_WINDOW)
    };
    (ratio, context, window)
}

/// 小输入 token 估算（仅 total < SMALL_LINES 时计算，其余为 0）。
fn small_token_est(lines: &[&str], total: usize) -> f64 {
    if total < crate::policy::SMALL_LINES {
        lines.iter().map(|s| token_est(s)).sum()
    } else {
        0.0
    }
}

/// 超长行检测：字节与字符双口径（先 O(1) 字节检查，只有超长行才付字符计数成本）。
fn any_longline(lines: &[&str]) -> bool {
    lines.iter().any(|s| {
        s.len() > crate::policy::LONGLINE_BYTES && s.chars().count() > crate::policy::LONGLINE_CHARS
    })
}

/// 直通判定：小输入（行数与 token 估算双低）或超长行，返回对应模式；否则 None。
fn pass_shape_decision(total: usize, small_tok: f64, has_longline: bool) -> Option<Mode> {
    if total < crate::policy::SMALL_LINES && small_tok < crate::policy::SMALL_TOK {
        Some(Mode::PassSmall)
    } else if has_longline {
        Some(Mode::PassLongline)
    } else {
        None
    }
}

/// 无信号 fail-open 判定（零命中且无 --force 命中）。
fn failopen_decision(signals: usize, force_any: bool) -> bool {
    signals == 0 && !force_any
}

/// 证据种子判定：≥2 命中，或单稀有命中（df <= rare_thr），或 --force 命中。
fn is_seed_candidate(h: usize, lone: usize, df: &[usize], rare_thr: usize, fm: bool) -> bool {
    h >= 2 || (h == 1 && lone != usize::MAX && df[lone] <= rare_thr) || fm
}

/// --max-tokens：证据地板（证据 + 最小邻域）已超上限 → OverflowEvidence。
fn cap_floor(tok_floor: f64, max_tokens: Option<usize>) -> Option<Mode> {
    match max_tokens {
        Some(max) if tok_floor > max as f64 => Some(Mode::OverflowEvidence),
        _ => None,
    }
}

/// --max-tokens：可选填充可分配余量（无上限时为 +∞）。
fn cap_remaining(tok_floor: f64, max_tokens: Option<usize>) -> f64 {
    max_tokens
        .map(|m| m as f64 - tok_floor)
        .unwrap_or(f64::INFINITY)
}

/// --max-tokens：风险守卫追加证据性行后总估算仍超 → OverflowFill。
fn cap_final(final_tok: f64, max_tokens: Option<usize>) -> Option<Mode> {
    match max_tokens {
        Some(max) if final_tok > max as f64 => Some(Mode::OverflowFill),
        _ => None,
    }
}

/// 输出统计：字符数（去掉尾随换行）与压减率。
fn cut_stats(out_text: &str, chars_a: usize) -> (usize, f64) {
    let chars_b = if out_text.ends_with('\n') {
        char_count(out_text).saturating_sub(1)
    } else {
        char_count(out_text)
    };
    let cut_pct = if chars_a > 0 {
        100.0 * (1.0 - chars_b as f64 / chars_a as f64)
    } else {
        0.0
    };
    (chars_b, cut_pct)
}
