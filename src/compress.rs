//! compress 引擎主体：证据筛选压缩的完整算法。
//!
//! 纯函数、无 I/O、无随机源；本文件不定义 Config/Outcome（在 crate 根），只实现
//! 从文本到压缩结果的单趟管线。管线拆分为独立阶段函数（切行 / 扫描 / 折叠 /
//! 权重 / 种子 / 区域 / 打分 / 邻域 / 落地 / 预算 / 审计 / 守卫 / 渲染），
//! 各阶段函数圈复杂度 < 10；可调参数与加分权重全部声明于 `policy` 表，
//! 不在函数体内硬编码。独立成文件以维持 crate 根 <500 行门禁。

use crate::analysis::error_line;
use crate::policy::{DEFAULT_RISK, DEFAULT_SCORES};
use crate::terms::terms;
use crate::{prefix, Config, Mode, Outcome, Risk};

/// 确定性快速哈希（FxHash 风格）：压缩内核的查找/计数型 map 统一使用，
/// 避免 std SipHash 的随机种子破坏“同输入同输出”的确定性承诺，同时更快。
/// 使用方均为“先查表计数、按键引用或排序输出”，不依赖裸迭代顺序。
mod plan;
mod render;
mod score;
use plan::*;
use render::*;
use score::*;

mod hash {
    use std::hash::{BuildHasherDefault, Hasher as Digest};

    /// rustc 内部 FxHasher 的等价实现（乘法散列，零依赖）。
    #[derive(Default)]
    pub(crate) struct Hasher {
        hash: u64,
    }

    const SEED: u64 = 0x517c_c1b7_2722_0a95;

    impl Hasher {
        #[inline]
        fn add(&mut self, word: u64) {
            self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(SEED);
        }
    }

    impl Digest for Hasher {
        #[inline]
        fn finish(&self) -> u64 {
            self.hash
        }

        #[inline]
        fn write(&mut self, bytes: &[u8]) {
            let mut chunks = bytes.chunks_exact(8);
            for c in &mut chunks {
                self.add(u64::from_ne_bytes(c.try_into().unwrap()));
            }
            let rest = chunks.remainder();
            if !rest.is_empty() {
                let mut last = [0u8; 8];
                last[..rest.len()].copy_from_slice(rest);
                self.add(u64::from_ne_bytes(last));
            }
        }

        #[inline]
        fn write_u8(&mut self, i: u8) {
            self.add(i as u64);
        }

        #[inline]
        fn write_u16(&mut self, i: u16) {
            self.add(i as u64);
        }

        #[inline]
        fn write_u32(&mut self, i: u32) {
            self.add(i as u64);
        }

        #[inline]
        fn write_u64(&mut self, i: u64) {
            self.add(i);
        }

        #[inline]
        fn write_usize(&mut self, i: usize) {
            self.add(i as u64);
        }
    }

    pub(crate) type Build = BuildHasherDefault<Hasher>;
    pub(crate) type Hash<K, V> = std::collections::HashMap<K, V, Build>;
}
fn chars(text: &str) -> usize {
    if text.is_ascii() {
        text.len()
    } else {
        text.chars().count()
    }
}
pub(crate) fn lines<'b, 'a: 'b>(text: &'a str, buf: &'b mut Option<String>) -> Vec<&'b str> {
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
pub(crate) enum Force {
    /// 字面子串（大小写不敏感，对小写缓冲匹配）。
    Sub(String),
    /// 手写正则（已解析并预编译；非法模式不参与匹配，CLI 层解析时已报错）。
    Re(crate::regex::Compiled),
}
/// P17-D 轻量词形扩展：去常见后缀（s/es/ed/ing/ly/er）并还原双写辅音
/// （running→run）。`--auto-anchor 1` 时并入命中锚词表。
fn word_forms(w: &str) -> Vec<String> {
    let mut v = Vec::new();
    for suf in ["s", "es", "ed", "ing", "ly", "er"] {
        let Some(stem) = w.strip_suffix(suf) else {
            continue;
        };
        if stem.len() < 3 {
            continue;
        }
        v.push(stem.to_string());
        let b = stem.as_bytes();
        if b.len() >= 4 && b[b.len() - 1] == b[b.len() - 2] {
            v.push(stem[..stem.len() - 1].to_string());
        }
    }
    v
}

/// 锚词表构造（`--auto-anchor 1` 时扩展词形，否则原词表）。
fn anchor_terms(cfg: &Config, terms: &[String]) -> Vec<String> {
    if !cfg.auto_anchor || terms.is_empty() {
        return terms.to_vec();
    }
    let mut v = Vec::with_capacity(terms.len() * 2);
    for t in terms {
        v.push(t.clone());
        for f in word_forms(t) {
            if !v.contains(&f) {
                v.push(f);
            }
        }
    }
    v
}

fn forces(forces: &[String]) -> Vec<Force> {
    let mut pats: Vec<Force> = Vec::new();
    for f in forces {
        let p = f.trim().to_lowercase();
        if p.starts_with("re:") {
            if !p.is_empty() {
                force(&mut pats, p);
            }
        } else {
            for part in f.split('|') {
                let p = part.trim().to_lowercase();
                if !p.is_empty() {
                    force(&mut pats, p);
                }
            }
        }
    }
    pats
}
fn force(pats: &mut Vec<Force>, p: String) {
    if let Some(rest) = p.strip_prefix("re:") {
        match crate::regex::parse(rest) {
            Ok(node) => pats.push(Force::Re(crate::regex::compile(&node))),
            Err(e) => {
                // CLI（args::parse_force）已前置校验；库内核绕过 CLI
                // 直接构造 Config 时，非法模式不参与匹配（debug 断言提示）。
                debug_assert!(false, "invalid re: force pattern: {e}");
            }
        }
    } else {
        pats.push(Force::Sub(p));
    }
}
/// `--budget` 裁剪只读上下文：行 + 打分 + 提升 + token 表。
struct BudgetIn<'a> {
    lines: &'a [&'a str],
    scores: &'a [f64],
    boost: &'a [f64],
    tok: &'a [f64],
}

/// `--budget` 应用点：0 = 关闭（原样返回 kept）；否则按 total_tok × budget
/// 硬裁剪并重算 kept。独立成函数保持 compress() 认知复杂度 < 6。
/// 只读侧打包为 `BudgetIn`，参数数压到 clippy 阈值（7）内。
fn apply_budget(
    keep: &mut [bool],
    reason: &mut [u8],
    budget: f64,
    total_tok: f64,
    ctx: BudgetIn,
) -> usize {
    if budget <= 0.0 {
        return keep.iter().filter(|&&k| k).count();
    }
    budget_trim(
        ctx.lines,
        keep,
        reason,
        ctx.scores,
        ctx.boost,
        ctx.tok,
        total_tok * budget,
    );
    keep.iter().filter(|&&k| k).count()
}

pub fn compress(text: &str, query: &str, cfg: &Config) -> Outcome {
    let mut prof = crate::perf::T::start();
    let (ratio, context, window) = config(cfg);
    let kind = cfg.kind;
    // --max-tokens 保守上界：可选填充预算按「用户上限 ÷ 安全系数」计（宁高估勿超预算）；
    // 证据地板/守卫溢出诊断仍按诚实上限（cfg.limit），不翻转模式标签。
    let budget_limit = cfg
        .limit
        .map(|m| m as f64 / crate::terms::BUDGET_SAFETY_FACTOR);
    // 分析使用 LF 归一化文本；passthrough/fail-open 返回输入原文，保持“原样保留”语义。
    let original = text;
    // 零拷贝快路径：无 CR 时直接借用原文本；行以 &str 切片保存，避免整文复制与逐行分配。
    let mut nl_buf: Option<String> = None;
    let lines = lines(original, &mut nl_buf);
    let uniq_terms = terms(query);
    // P17-D 自动锚（--auto-anchor 1，默认关 = 现状逐字节一致）：query 词形
    // 变体（去 s/es/ed/ing/ly/er，双写辅音还原）自动加入命中锚，无需手写
    // --evidence。与 P13-B stem_expand 同语义的轻量规则版（非门控，默认
    // 构建可用）。仅扩展 hits 判定词表，terms 计数/语义打分口径不变。
    let anchor_terms = anchor_terms(cfg, &uniq_terms);
    let force_pats = forces(&cfg.forces);
    let total = lines.len();
    // token 估算只服务小输入直通与可选预算；针尖档大盘无需整份扫描。
    // 小输入先算一次即可（随后可能直接 passthrough）；大输入把估算并入
    // 逐行分析循环，避免为同一批行再走一趟缓存。
    let small_tok = survey(&lines, total);
    let evidence = &cfg.evidence;
    // 输入测量一趟完成：字符数 + token 估算（原 chars + estimate 两趟全扫
    // 合一，CJK 输入上 estimate 是整篇 O(n) 扫描，合一趟省一次全扫）。
    let (chars_a, tokens_in) = crate::terms::measure(original);
    prof.mark("lines+terms");

    let mk = |text_owned: String,
              kept: usize,
              chars_b: usize,
              cut: f64,
              dup: usize,
              seeds: usize,
              risk: Risk,
              guarded: u8,
              regions: usize,
              numbers: Vec<usize>,
              mode: Mode,
              limit: Option<usize>,
              tokens: f64,
              dropped: Vec<crate::Dropped>|
     -> Outcome {
        let ev = ev_retention(evidence, &numbers, &text_owned, kept, total);
        let tokens_out = crate::terms::estimate(&text_owned);
        Outcome {
            text: text_owned,
            numbers,
            kept,
            total,
            input: chars_a,
            output: chars_b,
            cut,
            dup,
            seeds,
            evidence: seeds,
            risk,
            guarded,
            regions,
            window,
            terms: uniq_terms.len(),
            forced: force_pats.len(),
            mode,
            limit,
            tokens,
            tokens_in,
            dropped,
            budget_used_pct: 0.0,
            ev_retention: ev,
            ev_total: evidence.len(),
            buckets: vec![crate::BucketStat {
                kind,
                tokens_in,
                tokens_out,
                kept,
                total,
                mode,
            }],
            explain: Vec::new(),
        }
    };

    // 直通早退：小输入 / 超长行（无查询参与，原样返回）。
    if let Some(mode) = shape(total, small_tok, &lines) {
        prof.finish("compress");
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
            cfg.limit,
            0.0,
            Vec::new(),
        );
    }

    let do_tok = total >= crate::policy::SMALL_LINES && context != 0;
    let masks = masks(
        &lines,
        &anchor_terms,
        &force_pats,
        small_tok,
        do_tok,
        cfg.strict,
    );
    prof.mark("masks");
    let (dup, folds) = dedup(
        &lines,
        &masks.mask,
        masks.words,
        &masks.errs,
        &masks.pinned,
        cfg.code_keep,
    );
    prof.mark("dedup");
    let stat = weights(&masks.mask, masks.words, &dup, uniq_terms.len(), folds);
    let seeds = marks(
        &masks.mask,
        masks.words,
        &dup,
        &stat.df,
        &stat.weights,
        stat.rare_thr,
        &masks.pinned,
    );
    let seeds_total = (0..total).filter(|&i| seeds.seed[i]).count();
    prof.mark("weights+seeds");

    // 无信号 fail-open：零命中且无 --force 命中 => 原样保留。
    if failopen(seeds.hit_lines, seeds.force_any) {
        prof.finish("compress");
        return mk(
            original.to_string(),
            total,
            chars_a,
            0.0,
            folds,
            seeds_total,
            Risk::Low,
            0,
            0,
            Vec::new(),
            Mode::FailOpenZeroSignal,
            cfg.limit,
            0.0,
            Vec::new(),
        );
    }

    // 结构感知邻域（--structure 1）：fence/import/堆栈区域整段并入种子区域。
    let regions = spans(cfg, &seeds.seed, window as usize, total, &lines);
    #[cfg_attr(not(feature = "semantic"), allow(unused_mut))]
    let mut scores = scores(
        &lines,
        &dup,
        &seeds.hits,
        &seeds.sumw,
        &masks.errs,
        &masks.pinned,
        &DEFAULT_SCORES,
    );
    // 语义增强（可选）：feature=semantic 且显式加载权重时，对非 dup 候选行
    // 叠加 sigmoid 打分作为排序加分；不启用时与 v0.4.1 逐字节一致。
    semantic(cfg, &lines, &uniq_terms, total, &dup, &mut scores);
    // P17-C 熵近似评分（--entropy 1，默认关=零影响）。
    crate::compress::plan::entropy_signal(cfg, &lines, &dup, &mut scores);
    apply_sims(cfg, &mut scores, &dup, total);
    let mut boost = boost(&scores, &dup, window as usize, &DEFAULT_SCORES);
    // P17-E 证据簇密度加权（--cluster 1，默认关=零影响）。
    crate::compress::plan::cluster_signal(
        cfg,
        &scores,
        &dup,
        &mut boost,
        window as usize,
        &DEFAULT_SCORES,
    );
    prof.mark("spans+scores");
    // 逐行保留原因（--explain 用）：0=删除，其余见 R_* 常量。
    let mut reason = vec![0u8; total];
    let (mut keep, mut kept) = floor(
        &seeds.seed,
        context,
        total,
        &scores,
        cfg.fill_floor,
        &mut reason,
    );

    // 结构锚点（P6 骨架保护）：结构边界行无条件保留，优先级低于证据（seed/context）、
    // 高于可选填充。默认空 kinds 时为空操作，与现状逐字节一致。
    mark_structure_anchors(cfg, &lines, &mut keep, &mut kept, &mut reason);

    // 行保护策略（--code-keep / --num-line-keep / --sym-line-keep / --zh-keep /
    // --list-keep / --evidence）：整行保留并标记 raw（render 跳过行内 trim）。
    // 默认全 0 且无证据时为空操作，与现状逐字节一致。
    let mut raw = vec![false; total];
    if cfg.code_keep
        || cfg.num_line_keep
        || cfg.sym_line_keep
        || cfg.zh_keep
        || cfg.list_keep
        || !cfg.evidence.is_empty()
    {
        apply_keep_policies(
            cfg,
            &lines,
            &dup,
            &mut keep,
            &mut kept,
            &mut reason,
            &mut raw,
        );
    }

    // --max-tokens 硬上限：证据地板最高优先。证据 + 最小邻域先落，其 token
    // 估算超限时直接进入 OverflowEvidence（保留全部证据，如实报告），不再
    // 填充可选内容；未超限时可选填充受 `min(budget, max - floor)` 双重约束。
    let tok_floor = tokens(&lines, &keep, &masks.tok);
    prof.mark("floor");
    let mut overflow = cap(tok_floor, cfg.limit);

    // --context 0（针尖模式）：完全跳过可选预算填充，只保留证据行与守卫行。
    let used_optional = optional(
        &Select {
            lines: &lines,
            scores: &scores,
            boost: &boost,
            dup: &dup,
            in_region: &regions.in_region,
            struct_region: &regions.struct_region,
            tok: &masks.tok,
        },
        &Fill {
            region_gate: regions.gate,
            total_tok: masks.total_tok,
            ratio,
            edge_head: DEFAULT_SCORES.edge_head,
            remaining: remaining(tok_floor, budget_limit),
            floor: cfg.fill_floor,
        },
        context,
        overflow,
        &mut keep,
        &mut kept,
        &mut reason,
    );
    // 可选预算利用率（%）：实际填充 ÷ 预算上限，展示 `--ratio` 的实际作用。
    // 与 fill 同口径（budget = max(total_tok × ratio, BUDGET_MIN)）。
    let budget_used_pct = budget_used(used_optional, masks.total_tok, ratio);

    let risk = audit(&keep, &masks.errs, &seeds.hits, &DEFAULT_RISK);
    prof.mark("budget");
    let mut guarded = 0u8;
    if active(risk, cfg) {
        let scoped = cfg.scoped.then_some(regions.in_region.as_slice());
        guarded = guard(
            &mut keep,
            &mut kept,
            &dup,
            &masks.guards,
            &masks.marked,
            scoped,
            &mut reason,
        );
    }
    prof.mark("guard");

    // 守卫后最终检查：填充受上限约束，但风险守卫可能追加证据性行；如实报告 OverflowFill。
    let final_tok = tokens(&lines, &keep, &masks.tok);
    if overflow.is_none() {
        overflow = last(final_tok, cfg.limit);
    }

    // --budget 硬 kept 上限（P18 SuperCompress 同预算对照路径）：超预算行按
    // reason 保真优先级（可选填充 → 保真锚 → 证据）反向裁剪；0 = 现状逐字节一致。
    let kept = apply_budget(
        &mut keep,
        &mut reason,
        cfg.budget,
        masks.total_tok,
        BudgetIn {
            lines: &lines,
            scores: &scores,
            boost: &boost,
            tok: &masks.tok,
        },
    );

    // 预算过紧或只有高频单查询命中时可能选不出任何行；此时回退原文，不返回静默空结果。
    if kept == 0 {
        prof.finish("compress");
        return mk(
            original.to_string(),
            total,
            chars_a,
            0.0,
            folds,
            seeds_total,
            risk,
            0,
            regions.count,
            Vec::new(),
            Mode::PassNoKept,
            cfg.limit,
            0.0,
            Vec::new(),
        );
    }

    let (out_text, numbers) = render(&lines, &keep, cfg, &raw, &uniq_terms);
    prof.mark("render");
    let (chars_b, cut) = cut(&out_text, chars_a);
    let mut o = mk(
        out_text,
        kept,
        chars_b,
        cut,
        folds,
        seeds_total,
        risk,
        guarded,
        regions.count,
        numbers,
        overflow.unwrap_or(Mode::Sieve),
        cfg.limit,
        final_tok,
        runs(&keep, &dup, overflow.is_some()),
    );
    o.budget_used_pct = budget_used_pct;
    o.explain = explain(&reason, &dup);
    prof.finish("compress");
    o
}
pub fn compress_bucketed(
    parts: &[(crate::ContentType, &str)],
    query: &str,
    cfg: &Config,
) -> Outcome {
    let mut out_text = String::new();
    let mut buckets = Vec::with_capacity(parts.len());
    let (mut tin, mut tout) = (0.0, 0.0);
    let (mut cin, mut cout) = (0usize, 0usize);
    let (mut kept, mut total) = (0usize, 0usize);
    for (kind, text) in parts {
        let mut c = cfg.clone();
        c.ratio = kind.default_ratio();
        let o = compress(text, query, &c);
        let tok_out = crate::terms::estimate(&o.text);
        buckets.push(crate::BucketStat {
            kind: *kind,
            tokens_in: o.tokens_in,
            tokens_out: tok_out,
            kept: o.kept,
            total: o.total,
            mode: o.mode,
        });
        tin += o.tokens_in;
        tout += tok_out;
        cin += o.input;
        cout += o.output;
        kept += o.kept;
        total += o.total;
        out_text.push_str(&o.text);
        if !o.text.is_empty() && !o.text.ends_with('\n') {
            out_text.push('\n');
        }
    }
    let cut = if cin > 0 {
        (cin as f64 - cout as f64) * 100.0 / cin as f64
    } else {
        0.0
    };
    Outcome {
        text: out_text,
        numbers: Vec::new(),
        kept,
        total,
        input: cin,
        output: cout,
        cut,
        dup: 0,
        seeds: 0,
        evidence: 0,
        risk: Risk::Low,
        guarded: 0,
        regions: 0,
        window: cfg.window,
        terms: 0,
        forced: 0,
        mode: Mode::Sieve,
        limit: cfg.limit,
        tokens: tout,
        tokens_in: tin,
        dropped: Vec::new(),
        budget_used_pct: 0.0,
        ev_retention: 1.0,
        ev_total: 0,
        buckets,
        explain: Vec::new(),
    }
}
/// 行保护策略整体应用（--code-keep / --num-line-keep / --sym-line-keep 三段，
/// 语义与内联时逐字节一致；raw 标记行在 render 跳过行内 trim）。
// 四个平级策略段落（code/num/sym/zh keep）共用同一签名；每段是独立声明式
// 策略，拆函数反而损害可读性，故按项目圈复杂度偏好（<10，当前 7）局部放行
// 认知复杂度 lint。
#[allow(clippy::cognitive_complexity)]
fn apply_keep_policies(
    cfg: &Config,
    lines: &[&str],
    dup: &[bool],
    keep: &mut [bool],
    kept: &mut usize,
    reason: &mut [u8],
    raw: &mut [bool],
) {
    // --code-keep 1（P6g）：先踢掉折叠的 prose 行（重复格式模板）——window 邻域
    // 保底会把 dup 行拉回 keep，这里强制剔除；模板行不是证据（P6g 代码场景深压）。
    // 代码行永不 dup（dedup 围栏禁折叠），不受影响。围栏内代码行整行强制保留并
    // 标记 raw，防短变量/运算符被摘、防 gap elide 整行删代码。
    if cfg.code_keep {
        for i in 0..keep.len() {
            if dup[i] {
                keep[i] = false;
                reason[i] = 0;
            }
        }
        *kept = (0..keep.len()).filter(|&i| keep[i]).count();
        mark_code_fence(lines, keep, kept, reason, raw);
    }
    // --num-line-keep 1（P6j）：数字行整行保留（行内仍 trim）。
    if cfg.num_line_keep {
        mark_num_lines(lines, dup, keep, kept, reason);
    }
    // --sym-line-keep 1（P6m）：符号/签名行整行保留（行内仍 trim）。
    if cfg.sym_line_keep {
        mark_sym_lines(lines, dup, keep, kept, reason, raw);
    }
    // --zh-keep 1（P8-E）：中文问句行整行保留并标记 raw（跳过行内 trim）——
    // 中文代码问答题干被 trim 逐字摘碎是保持率 0.2 主导机制，问句行必须保真。
    if cfg.zh_keep {
        mark_zh_lines(lines, dup, keep, kept, reason, raw);
    }
    // --list-keep 1（P12.5）：列表项行整行保留并标记 raw——枚举型文档的
    // 列表项是全量枚举任务的硬信息，trim 摘词导致模型恢复清单漏项（P11-B2
    // instruct 保持率 0.000 根因；缓解候选，代价是列表场景压缩率下降）。
    if cfg.list_keep {
        mark_list_lines(lines, dup, keep, kept, reason, raw);
    }
    // --evidence（P12.6）：证据行整行保留并标记 raw——Substr/Line 声明的
    // 证据是问答硬锚，行内 trim 会摘词导致证据 marker 从正文消失而报告仍
    // 称"全部保留"；强制 keep+raw 后正文逐字节含证据子串。
    if !cfg.evidence.is_empty() {
        mark_evidence_lines(lines, &cfg.evidence, keep, kept, reason, raw);
    }
}

/// 可选预算利用率（%）：实际填充 ÷ 预算上限，与 fill 同口径
/// （budget = max(total_tok × ratio, BUDGET_MIN)）。
fn budget_used(used: f64, total_tok: f64, ratio: f64) -> f64 {
    let budget = (total_tok * ratio).max(crate::policy::BUDGET_MIN);
    if budget > 0.0 {
        (used / budget * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    }
}

fn config(cfg: &Config) -> (f64, i64, i64) {
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
fn survey(lines: &[&str], total: usize) -> f64 {
    if total < crate::policy::SMALL_LINES {
        lines.iter().map(|s| crate::terms::estimate(s)).sum()
    } else {
        0.0
    }
}
fn long(lines: &[&str]) -> bool {
    lines.iter().any(|s| {
        s.len() > crate::policy::LONGLINE_BYTES && s.chars().count() > crate::policy::LONGLINE_CHARS
    })
}
fn shape(total: usize, small_tok: f64, lines: &[&str]) -> Option<Mode> {
    // long() 是全行扫描，放在最便宜的两个判定之后：行数超限/小输入直接短路，
    // 不必为必然直通的输入再扫一遍全部行。
    if total >= crate::policy::MAX_LINES {
        Some(Mode::PassTooManyLines)
    } else if total < crate::policy::SMALL_LINES && small_tok < crate::policy::SMALL_TOK {
        Some(Mode::PassSmall)
    } else if long(lines) {
        Some(Mode::PassLongline)
    } else {
        None
    }
}
fn failopen(signals: usize, force_any: bool) -> bool {
    signals == 0 && !force_any
}
fn rare(h: usize, lone: usize, df: &[usize], rare_thr: usize, fm: bool) -> bool {
    h >= 2 || (h == 1 && lone != usize::MAX && df[lone] <= rare_thr) || fm
}
fn cap(tok_floor: f64, limit: Option<usize>) -> Option<Mode> {
    match limit {
        Some(max) if tok_floor > max as f64 => Some(Mode::OverflowEvidence),
        _ => None,
    }
}
fn remaining(tok_floor: f64, limit: Option<f64>) -> f64 {
    limit.map(|m| m - tok_floor).unwrap_or(f64::INFINITY)
}
fn last(final_tok: f64, limit: Option<usize>) -> Option<Mode> {
    match limit {
        Some(max) if final_tok > max as f64 => Some(Mode::OverflowFill),
        _ => None,
    }
}
mod tests {
    use super::*;
    #[allow(unused_imports)] // clippy 对 test 内 use 误报（is_import 被 870-873 调用）
    use crate::analysis::import as is_import;
    use crate::analysis::normalize;

    #[allow(dead_code)] // clippy 对 test 参考实现误报
    fn sample_lines<'a>(v: &[&'a str]) -> Vec<&'a str> {
        v.to_vec()
    }

    /// 朴素参考实现：线性查找重复，独立于生产实现的 HashMap/复用路径。
    #[allow(dead_code)] // clippy 对 test 参考实现误报
    fn naive_dedup(
        lines: &[&str],
        mask: &[u64],
        words: usize,
        err: &[bool],
        force: &[bool],
        _code_keep: bool,
    ) -> (Vec<bool>, usize) {
        let mut keys: Vec<(String, usize)> = Vec::with_capacity(lines.len());
        let mut dup = vec![false; lines.len()];
        let mut n = 0usize;
        let mut key = String::new();
        for (i, line) in lines.iter().enumerate() {
            key.clear();
            normalize(line, &mut key);
            if short(&key) {
                continue;
            }
            match keys.iter_mut().find(|(k, _)| *k == key) {
                Some((_, count)) => {
                    *count += 1;
                    let h = hit(mask, words, i);
                    if foldable(count, h, err[i], force[i], _code_keep, false) {
                        dup[i] = true;
                        n += 1;
                    }
                }
                None => keys.push((std::mem::take(&mut key), 1)),
            }
        }
        (dup, n)
    }

    #[test]
    fn zh_keep_preserves_question_lines_and_skips_trim() {
        // zh-keep=1：中文问句行整行保留并跳过行内 trim（题干不被摘碎）。
        let text = [
            "CUDA 编程模型里，kernel<<<4,32>>> 启动后第 2 号块 blockIdx.x=2 内 threadIdx.x=5 的全局线性线程 ID 是多少？",
            "请用答案格式作答：答案是 X #### X。",
            "背景知识行：共享内存是块内线程共享的高速存储区域。",
        ]
        .join("\n");
        let cfg = crate::Config {
            trim: true,
            trim_ratio: 0.3,
            zh_keep: true,
            ..Default::default()
        };
        let out = compress(&text, "CUDA 内核 线程 共享内存 原子 warp", &cfg);
        let q_line = "blockIdx.x=2 内 threadIdx.x=5 的全局线性线程 ID 是多少？";
        assert!(
            out.text.contains(q_line),
            "zh-keep 问句行必须整行保留: {}",
            out.text
        );
        assert!(
            out.text.contains("答案是 X #### X"),
            "格式要求行必须整行保留: {}",
            out.text
        );
    }

    #[test]
    fn zh_keep_off_keeps_previous_behavior() {
        // 默认 zh_keep=0：问句行被行内 trim 摘碎（对照）；zh_keep=1 整行保留。
        // 多行输入避开小输入透传（单行 <SMALL_TOK 会 fail-open 原样返回）。
        let ctx = (0..45)
            .map(|_| "背景：CUDA 编程模型 kernel GPU 线程，共享内存是块内线程共享的高速存储，原子操作保证并发安全。")
            .collect::<Vec<_>>()
            .join("\n");
        let q = "问题：kernel<<<4,32>>> 启动后第 2 号块 blockIdx.x=2 内 threadIdx.x=5 的全局线性线程 ID 是多少？请用答案是 69 #### 69 格式作答。";
        let text = format!("{ctx}\n{q}");
        let cfg = crate::Config {
            trim: true,
            trim_ratio: 0.3,
            zh_keep: false,
            ..Default::default()
        };
        let out = compress(&text, "CUDA 内核 线程 共享内存 原子 warp", &cfg);
        assert!(
            !out.text.contains("全局线性线程 ID 是多少"),
            "zh-keep=0 时问句行应被 trim 摘碎（对照行为）: {}",
            out.text
        );
        let cfg2 = crate::Config {
            trim: true,
            trim_ratio: 0.3,
            zh_keep: true,
            ..Default::default()
        };
        let out2 = compress(&text, "CUDA 内核 线程 共享内存 原子 warp", &cfg2);
        assert!(
            out2.text.contains("全局线性线程 ID 是多少"),
            "zh-keep=1 时问句行必须整行保留: {}",
            out2.text
        );
    }

    #[test]
    fn drop_runs_empty_keep() {
        assert!(runs(&[], &[], false).is_empty());
        assert!(runs(&[true, true, true], &[false; 3], false).is_empty());
    }

    #[test]
    fn drop_runs_budget_runs() {
        let keep = vec![true, false, false, true, false, true];
        let dup = vec![false; 6];
        let dr = runs(&keep, &dup, false);
        assert_eq!(dr.len(), 2);
        assert_eq!((dr[0].start, dr[0].end), (2, 3));
        assert_eq!(dr[0].reason, crate::Reason::Budget);
        assert_eq!((dr[1].start, dr[1].end), (5, 5));
        assert_eq!(dr[1].reason, crate::Reason::Budget);
    }

    #[test]
    fn drop_runs_trailing_tail() {
        let keep = vec![true, false, false];
        let dr = runs(&keep, &[false; 3], false);
        assert_eq!(dr.len(), 1);
        assert_eq!((dr[0].start, dr[0].end), (2, 3));
    }

    #[test]
    fn drop_runs_overflow_reason() {
        let keep = vec![false, false, true];
        let dr = runs(&keep, &[false; 3], true);
        assert_eq!(dr.len(), 1);
        assert_eq!(dr[0].reason, crate::Reason::MaxTokens);
        assert_eq!(dr[0].reason.as_str(), "max-tokens");
        assert_eq!(crate::Reason::Budget.as_str(), "budget");
        assert_eq!(crate::Reason::Dedup.as_str(), "dedup");
    }

    #[test]
    fn drop_runs_dedup_splits_from_budget() {
        // dup 行记 Dedup，与相邻 budget 行拆分为不同区间。
        let keep = vec![true, false, false];
        let dup = vec![false, true, false];
        let dr = runs(&keep, &dup, false);
        assert_eq!(dr.len(), 2);
        assert_eq!((dr[0].start, dr[0].end), (2, 2));
        assert_eq!(dr[0].reason, crate::Reason::Dedup);
        assert_eq!((dr[1].start, dr[1].end), (3, 3));
        assert_eq!(dr[1].reason, crate::Reason::Budget);
        // 同一原因延续不拆分：两个相邻 dup 合并为一个 Dedup 区间。
        let keep2 = vec![true, false, false, false];
        let dup2 = vec![false, true, true, false];
        let dr2 = runs(&keep2, &dup2, false);
        assert_eq!(dr2.len(), 2);
        assert_eq!((dr2[0].start, dr2[0].end), (2, 3));
        assert_eq!(dr2[0].reason, crate::Reason::Dedup);
        assert_eq!((dr2[1].start, dr2[1].end), (4, 4));
        assert_eq!(dr2[1].reason, crate::Reason::Budget);
    }

    #[test]
    fn scan_structures_fence_pairing() {
        let lines = sample_lines(&["```", "import os", "conn 9f2a", "```", "x"]);
        let map = structures(&lines);
        assert_eq!(map.regions.len(), 1);
        assert_eq!((map.regions[0].start, map.regions[0].end), (0, 3));
    }

    #[test]
    fn scan_structures_import_block() {
        let lines = sample_lines(&["import os", "import sys", "import conn_pool", "x"]);
        let map = structures(&lines);
        assert_eq!(map.regions.len(), 1);
        assert_eq!((map.regions[0].start, map.regions[0].end), (0, 2));
    }

    #[test]
    fn dedup_matches_naive_reference_on_generated_corpora() {
        // 确定性 LCG 生成含连续重复、数字/十六进制、空白与短行的语料，
        // 生产实现（含「与上一行相同则复用归一化」快速路径）必须与朴素参考一致。
        let alphabet = b" abcef0129\t#=:xX-";
        let mut state = 0x1234_5678u32;
        let mut next = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            state
        };
        for _ in 0..300 {
            let len = (next() as usize >> 8) % 24;
            let mut raw: Vec<String> = Vec::with_capacity(len);
            let mut prev = String::new();
            for _ in 0..len {
                let roll = next() as usize;
                // 30% 概率重复上一行，触达快速路径。
                if roll % 10 < 3 && !prev.is_empty() {
                    raw.push(prev.clone());
                    continue;
                }
                let n = (next() as usize >> 8) % 20;
                let mut s = String::new();
                for _ in 0..n {
                    s.push(alphabet[(next() as usize >> 16) % alphabet.len()] as char);
                }
                prev = s.clone();
                raw.push(s);
            }
            let lines: Vec<&str> = raw.iter().map(String::as_str).collect();
            // 掩码随机置位（模拟查询命中），错误/force 同理。
            let mut mask = vec![0u64; lines.len() * 2];
            for slot in mask.iter_mut() {
                if next() % 7 == 0 {
                    *slot = 1;
                }
            }
            let err: Vec<bool> = (0..lines.len()).map(|_| next() % 11 == 0).collect();
            let force: Vec<bool> = (0..lines.len()).map(|_| next() % 13 == 0).collect();
            let got = dedup(&lines, &mask, 2, &err, &force, false);
            let want = naive_dedup(&lines, &mask, 2, &err, &force, false);
            assert_eq!(got, want, "dedup mismatch on {:?}", raw);
        }
    }

    #[test]
    fn token_table_matches_estimate_and_falls_back() {
        let lines = ["abc", "数据库连接超时", "x y z 42"];
        let (tok, total) = tally(&lines, 3, 0.0, true);
        let want: f64 = lines.iter().map(|l| crate::terms::estimate(l)).sum();
        assert_eq!(total, want);
        for (i, l) in lines.iter().enumerate() {
            assert_eq!(est(&tok, &lines, i), crate::terms::estimate(l));
        }
        // do_tok=false：空表 + small_tok，调用方回退现算。
        let (empty, small) = tally(&lines, 3, 7.5, false);
        assert!(empty.is_empty());
        assert_eq!(small, 7.5);
        assert_eq!(est(&empty, &lines, 1), crate::terms::estimate(lines[1]));
    }

    #[test]
    fn dedup_skips_short_and_padded_lines() {
        // 归一化键 < 6 字符（含首尾空白填充）的行从不参与折叠；长行仍按
        // 第 3 次起折叠。快速路径（trim 后 < 6 字节直接跳过）不得改变判定。
        let lines: Vec<&str> = vec![
            "abc",
            "   abc   ",
            "a\tb",
            "错误",
            "abcdef",
            "abcdef",
            "abcdef",
            "abcdef",
        ];
        let mask = vec![0u64; lines.len()];
        let err = vec![false; lines.len()];
        let force = vec![false; lines.len()];
        let (dup, n) = dedup(&lines, &mask, 0, &err, &force, false);
        assert!(!dup[0] && !dup[1] && !dup[2] && !dup[3], "短/填充行不折叠");
        assert!(dup[6] && dup[7], "长行第 3、4 次出现应折叠");
        assert_eq!(n, 2);
    }

    #[test]
    fn code_keep_folds_hit_templates_but_never_code() {
        // code_keep=1：重复格式模板（命中 query）折叠；围栏内代码行永不折叠。
        let lines: Vec<&str> = vec![
            "Q: 以下代码输出什么？请用答案是X格式作答。",
            "```python",
            "x = 5",
            "```",
            "Q: 以下代码输出什么？请用答案是X格式作答。",
            "```python",
            "x = 5",
            "```",
            "Q: 以下代码输出什么？请用答案是X格式作答。",
            "```python",
            "x = 5",
            "```",
        ];
        // mask 置满 → 所有行都命中 query（hit 豁免），验证 code_keep 覆盖命中。
        let words = 2usize;
        let mask = vec![u64::MAX; lines.len() * words];
        let err = vec![false; lines.len()];
        let force = vec![false; lines.len()];
        // 关：命中行不折叠（现状行为）。
        let (dup0, n0) = dedup(&lines, &mask, words, &err, &force, false);
        assert_eq!(n0, 0, "命中豁免：默认不折叠任何行");
        assert!(dup0.iter().all(|&d| !d));
        // 开：Q 模板第 3 次起折叠，围栏行与代码行永不折叠。
        let (dup1, n1) = dedup(&lines, &mask, words, &err, &force, true);
        assert_eq!(n1, 1, "第 3 个 Q 模板行折叠");
        assert!(dup1[8], "第 3 个 Q 模板行折叠");
        assert!(!dup1[4], "第 2 个 Q 模板行保留（前两次不折叠）");
        assert!(
            !dup1[1] && !dup1[2] && !dup1[6] && !dup1[10],
            "围栏行与代码行永不折叠"
        );
    }

    #[test]
    fn nan_sims_do_not_poison_scores() {
        // 外部嵌入命令输出 NaN 时不得进入排序分（比较器会失去全序）。
        let text = (0..200)
            .map(|i| format!("line {i}: conn pool timeout db error 9f2a"))
            .collect::<Vec<_>>()
            .join("\n");
        let cfg = crate::Config {
            sims: Some(vec![f64::NAN; 200]),
            ..crate::Config::default()
        };
        let out = compress(&text, "conn timeout", &cfg);
        assert!(out.kept >= 1, "NaN 相似度不应破坏压缩结果");
        assert!(out.text.is_char_boundary(0));
    }

    #[test]
    fn scan_structures_import_block_closed_before_fence() {
        // 进入 fence 前必须先收尾未结束的结构段，否则 import 段整体丢失。
        let lines = sample_lines(&["import os", "import sys", "```", "code", "```", "x"]);
        let map = structures(&lines);
        let mut spans: Vec<(usize, usize)> = map.regions.iter().map(|r| (r.start, r.end)).collect();
        spans.sort();
        assert_eq!(spans, vec![(0, 1), (2, 4)]);
    }

    #[test]
    fn scan_structures_stack_chain_with_caused_by() {
        let lines = sample_lines(&[
            "  at com.a.Pool.acquire(Pool.java:214)",
            "Caused by: java.sql.SQLException: db crash conn 9f2a",
            "  at com.c.Conn.open(Conn.java:9)",
            "x",
        ]);
        let map = structures(&lines);
        assert_eq!(map.regions.len(), 1);
        assert_eq!((map.regions[0].start, map.regions[0].end), (0, 2));
    }

    #[test]
    fn scan_structures_fence_shields_inner_import() {
        // fence 内行不参与 import/stack 判定。
        let lines = sample_lines(&["```", "import os", "```", "import sys"]);
        let map = structures(&lines);
        assert_eq!(map.regions.len(), 2, "fence 区域 + fence 外 import 区域");
        let mut spans: Vec<(usize, usize)> = map.regions.iter().map(|r| (r.start, r.end)).collect();
        spans.sort();
        assert_eq!(spans, vec![(0, 2), (3, 3)]);
    }

    #[test]
    fn use_strict_not_import() {
        assert!(!is_import("use strict"));
        assert!(!is_import("use of"));
        assert!(is_import("use crate::policy::DEFAULT_RATIO"));
        assert!(is_import("#include <stdio.h>"));
        assert!(is_import("import os"));
    }

    #[test]
    fn scan_structures_brace_block_with_multibyte_lines() {
        // 大括号按字节统计：中文行里的 { } 同样是 ASCII 字节，跨多字节字符不漏算。
        let lines = sample_lines(&["函数() {", "  返回 42", "}"]);
        let map = structures(&lines);
        assert_eq!(map.regions.len(), 1);
        assert_eq!((map.regions[0].start, map.regions[0].end), (0, 2));
    }

    #[test]
    fn scan_structures_brace_block() {
        // 大括号块（函数体 + 嵌套 if）：从含 { 的行到匹配的 } 整段并入。
        let lines = sample_lines(&[
            "fn connect() {",
            "  let conn = pool.acquire();",
            "  if conn.is_err() {",
            "    return Err(conn.err());",
            "  }",
            "  Ok(conn)",
            "}",
            "x",
        ]);
        let map = structures(&lines);
        assert_eq!(map.regions.len(), 1, "单个函数体块");
        assert_eq!((map.regions[0].start, map.regions[0].end), (0, 6));
    }

    #[test]
    fn scan_structures_single_line_brace_block() {
        // 单行 { } 自成一段。
        let lines = sample_lines(&["fn empty() {}", "x"]);
        let map = structures(&lines);
        assert_eq!(map.regions.len(), 1);
        assert_eq!((map.regions[0].start, map.regions[0].end), (0, 0));
    }

    #[test]
    fn scan_structures_fence_shields_brace_block() {
        // fence 内的代码块不参与大括号判定（重置 brace）。
        let lines = sample_lines(&[
            "```",
            "fn inside() {",
            "  return 1;",
            "}",
            "```",
            "fn outside() {",
            "  return 2;",
            "}",
        ]);
        let map = structures(&lines);
        // 只有 fence（0..4）和 fence 外的函数体（5..7）。
        assert_eq!(map.regions.len(), 2, "fence 区域 + fence 外函数体");
        let mut spans: Vec<(usize, usize)> = map.regions.iter().map(|r| (r.start, r.end)).collect();
        spans.sort();
        assert_eq!(spans, vec![(0, 4), (5, 7)]);
    }

    #[test]
    fn structure_default_off_region_shape_unchanged() {
        // --structure 默认关：expand 与 v0.4.1 完全一致（区间数与 mask）。
        let seed = vec![false, false, true, false, false, false];
        let a = expand(&seed, 2, 6, None);
        let b = expand(
            &seed,
            2,
            6,
            Some(&Shape {
                regions: Vec::new(),
            }),
        );
        assert_eq!(a.in_region, b.in_region);
        assert_eq!(a.count, b.count);
        assert_eq!(a.gate, b.gate);
    }

    #[test]
    fn structure_fence_merges_region_into_candidate() {
        // 结构并入是叠加：种子邻域基础窗 + 含种子的 fence 区域整段进入候选。
        let lines: Vec<&str> = vec![
            "```",
            "import os",
            "conn 9f2a broken db crash",
            "return pool",
            "```",
            "INFO filler",
        ];
        let seed = vec![false, false, true, false, false, false];
        let _base = expand(&seed, 1, lines.len(), None);
        let map = structures(&lines);
        let merged = expand(&seed, 1, lines.len(), Some(&map));
        assert!(merged.in_region[0], "fence 开启行并入");
        assert!(merged.in_region[4], "fence 闭合行并入");
        assert!(!merged.in_region[5], "fence 外行不并入");
    }

    #[test]
    fn too_many_lines_returns_passthrough() {
        assert_eq!(
            shape(crate::policy::MAX_LINES, 0.0, &[]),
            Some(Mode::PassTooManyLines),
            "行数达到上限应直通"
        );
        assert_eq!(
            shape(crate::policy::MAX_LINES - 1, 0.0, &[]),
            None,
            "上限内不触发行数直通"
        );
    }

    #[test]
    fn json_anchor_detects_braces_and_brackets() {
        assert!(json_anchor("{"));
        assert!(json_anchor("}"));
        assert!(json_anchor("["));
        assert!(json_anchor("]"));
        assert!(json_anchor("},"), "尾随逗号");
        assert!(json_anchor("\"key\": {"), "行尾开括号");
        assert!(!json_anchor("\"key\": 1"));
    }

    #[test]
    fn tag_anchor_detects_xml_tags() {
        assert!(tag_anchor("<root>"));
        assert!(tag_anchor("</root>"));
        assert!(tag_anchor("<tag attr=\"x\">"));
        assert!(!tag_anchor("plain text"));
        assert!(!tag_anchor("<item>data</item>"), "叶行非结构边界");
    }

    #[test]
    fn yaml_anchor_detects_keys_and_list_items() {
        assert!(yaml_anchor("key:"));
        assert!(yaml_anchor("- item"));
        assert!(!yaml_anchor("# comment"));
        assert!(!yaml_anchor("key: value"));
    }

    #[test]
    fn word_forms_strips_suffixes_and_doubled_consonants() {
        // 去 -ing：connecting → connect。
        let mut forms = word_forms("connecting");
        forms.sort();
        assert!(forms.contains(&"connect".to_string()), "{forms:?}");
        // 双写辅音还原：running → runn + run。
        let mut f2 = word_forms("running");
        f2.sort();
        assert!(
            f2.contains(&"runn".to_string()) && f2.contains(&"run".to_string()),
            "{f2:?}"
        );
        assert!(word_forms("errors").contains(&"error".to_string()));
        assert!(word_forms("closely").contains(&"close".to_string()));
        // 短词不剥到 <3 字符。
        assert!(!word_forms("is").contains(&"i".to_string()));
        // 无后缀词零扩展。
        assert!(word_forms("alpha").is_empty());
    }

    #[test]
    fn auto_anchor_off_is_noop_on_masks_terms() {
        // --auto-anchor 默认关：锚词表=原词表，命中行为与现状一致。
        let mut cfg = crate::Config::default();
        let lines = [
            "worker running to server",
            "worker runs the server",
            "idle line here",
        ];
        let q = "running";
        let uniq = crate::terms::terms(q);
        let anchor = anchor_terms(&cfg, &uniq);
        assert_eq!(anchor, uniq, "默认关时锚词表=原词表");
        cfg.auto_anchor = true;
        let anchor_on = anchor_terms(&cfg, &uniq);
        assert!(
            anchor_on.len() > anchor.len(),
            "开启后扩展词形: {} vs {}",
            anchor_on.len(),
            anchor.len()
        );
        assert!(anchor_on.contains(&"run".to_string()));
        assert!(anchor_on.contains(&"runn".to_string()));
        let _ = lines;
    }

    #[test]
    fn anchors_marks_only_enabled_kinds() {
        let lines = ["{", "<tag>", "key:", "plain"];
        let json_only = anchors(&lines, &[crate::AnchorKind::Json]);
        assert_eq!(json_only, vec![true, false, false, false]);
        let all = anchors(
            &lines,
            &[
                crate::AnchorKind::Json,
                crate::AnchorKind::Xml,
                crate::AnchorKind::Yaml,
            ],
        );
        assert_eq!(all, vec![true, true, true, false]);
        let md_lines = ["# Doc", "## Sec", "### Sub", "#", "plain"];
        let md_only = anchors(&md_lines, &[crate::AnchorKind::Markdown]);
        assert_eq!(md_only, vec![true, true, true, true, false]);
    }

    #[test]
    fn code_fence_keeps_and_marks_raw_only_inside() {
        let lines = [
            "Q: 以下代码输出什么？",
            "```python",
            "x = 5",
            "print(a // b)",
            "```",
            "A: 答案是 8",
        ];
        let mut keep = vec![false, false, false, false, false, false];
        let mut kept = 0usize;
        let mut reason = vec![0u8; 6];
        let mut raw = vec![false; 6];
        mark_code_fence(&lines, &mut keep, &mut kept, &mut reason, &mut raw);
        // 围栏行 + 围栏内代码行全部保留且 raw；说明行不受影响。
        assert_eq!(keep, vec![false, true, true, true, true, false]);
        assert_eq!(raw, vec![false, true, true, true, true, false]);
        assert_eq!(kept, 4);
        // 已保留行不重复计数（kept 只统计新保留）。
        mark_code_fence(&lines, &mut keep, &mut kept, &mut reason, &mut raw);
        assert_eq!(kept, 4);
    }

    #[test]
    fn scoped_gates_far_errors() {
        // 全量守卫（None）保留所有错误行；区域门控（Some）只保留证据邻域内的错误行。
        let keep = vec![false; 5];
        let dup = vec![false; 5];
        let guard_lines = vec![true, true, true, true, true];
        let marker = vec![false; 5];
        let in_region = vec![false, false, true, false, false];
        let mut reason = vec![0u8; 5];

        let mut k_full = keep.clone();
        let mut kc = 0;
        guard(
            &mut k_full,
            &mut kc,
            &dup,
            &guard_lines,
            &marker,
            None,
            &mut reason,
        );
        assert_eq!(kc, 5, "全量守卫保留所有错误行");

        let mut k_region = keep.clone();
        let mut kc2 = 0;
        guard(
            &mut k_region,
            &mut kc2,
            &dup,
            &guard_lines,
            &marker,
            Some(&in_region),
            &mut reason,
        );
        assert_eq!(kc2, 1, "区域门控只保留证据邻域内的错误行");
        assert!(k_region[2], "in-region 错误行保留");
    }

    #[test]
    fn list_line_detects_markdown_lists_and_rejects_code() {
        // 命中：无序 / 有序 / 任务清单。
        for hit in [
            "- item",
            "* item",
            "+ item",
            "1. item",
            "12) item",
            "(3) item",
            "a. item",
            "B) item",
            "[ ] todo",
            "[x] done",
            "-中文列表项",
        ] {
            assert!(crate::compress::score::list_line(hit), "应命中: {hit}");
        }
        // 拒绝：代码/数学表达式（标记后无空白或数字字面量）。
        for miss in [
            "*ptr",
            "-x",
            "+1",
            "1.5f",
            "1)",
            "1+2",
            "a.b",
            "-",
            "*",
            "1. ",
            "[x]",
            "fn f(",
            "// - comment",
            "return -1",
        ] {
            assert!(!crate::compress::score::list_line(miss), "不应命中: {miss}");
        }
    }

    #[test]
    fn list_keep_preserves_list_rows_and_skips_trim() {
        let lines: Vec<&str> = vec!["- keep me", "plain row", "3. also keep", "*ptr code"];
        let dup = vec![false; 4];
        let mut keep = vec![false; 4];
        let mut kept = 0;
        let mut reason = vec![0u8; 4];
        let mut raw = vec![false; 4];
        crate::compress::score::mark_list_lines(
            &lines,
            &dup,
            &mut keep,
            &mut kept,
            &mut reason,
            &mut raw,
        );
        assert!(keep[0] && keep[2], "列表行整行保留");
        assert!(raw[0] && raw[2], "列表行跳过行内 trim");
        assert!(!keep[1] && !keep[3], "普通行与代码行不受影响");
        assert_eq!(kept, 2);
        assert_eq!(reason[0], crate::compress::score::R_LIST_LINE);
    }

    #[test]
    fn evidence_lines_kept_and_raw_skip_trim() {
        // Substr 证据：命中的行整行保留 + raw（行内 trim 不再摘 marker）。
        let lines: Vec<&str> = vec!["plain", "关键证据行 UNIQUE_EVIDENCE_XYZ", "plain2"];
        let mut keep = vec![false; 3];
        let mut kept = 0;
        let mut reason = vec![0u8; 3];
        let mut raw = vec![false; 3];
        let ev = vec![crate::Evidence::Substr("UNIQUE_EVIDENCE_XYZ".into())];
        crate::compress::score::mark_evidence_lines(
            &lines,
            &ev,
            &mut keep,
            &mut kept,
            &mut reason,
            &mut raw,
        );
        assert!(keep[1], "Substr 命中行整行保留");
        assert!(raw[1], "Substr 命中行跳过行内 trim");
        assert!(!keep[0] && !keep[2], "未命中行不受影响");
        assert_eq!(kept, 1);
        assert_eq!(reason[1], crate::compress::score::R_EVIDENCE);

        // Line 证据：行号命中整行保留（1-based）。
        let mut keep2 = vec![false; 3];
        let mut kept2 = 0;
        let mut reason2 = vec![0u8; 3];
        let mut raw2 = vec![false; 3];
        let ev2 = vec![crate::Evidence::Line(3)];
        crate::compress::score::mark_evidence_lines(
            &lines,
            &ev2,
            &mut keep2,
            &mut kept2,
            &mut reason2,
            &mut raw2,
        );
        assert!(keep2[2] && raw2[2], "Line 命中行整行保留并 raw");
        assert_eq!(kept2, 1);

        // 越界行号静默忽略，不 panic。
        let mut keep3 = vec![false; 3];
        let mut kept3 = 0;
        let mut reason3 = vec![0u8; 3];
        let mut raw3 = vec![false; 3];
        let ev3 = vec![crate::Evidence::Line(99)];
        crate::compress::score::mark_evidence_lines(
            &lines,
            &ev3,
            &mut keep3,
            &mut kept3,
            &mut reason3,
            &mut raw3,
        );
        assert!(!keep3.iter().any(|&k| k), "越界行号不影响任何行");
        assert_eq!(kept3, 0);
    }

    #[test]
    fn budget_trim_enforces_hard_kept_cap() {
        use crate::compress::score::{budget_trim, R_EVIDENCE, R_FILL, R_NUM_LINE};
        let lines = ["fill low", "fill high", "num 42 line", "evidence line"];
        let tok = [3.0, 3.0, 3.0, 3.0];
        // 全保留：12 token；预算 9（输出 = 行 + gap 标记 ≤ 9）。
        // 裁 i=0 后 [F,T,T,T]：9 行 token + 1.75 gap = 10.75 > 9 → 继续裁；
        // 裁 i=1 后 [F,F,T,T]：6 行 token + 1.75 gap = 7.75 ≤ 9 ✓（保真锚全留）。
        let mut keep = vec![true; 4];
        let mut reason = vec![R_FILL, R_FILL, R_NUM_LINE, R_EVIDENCE];
        let scores = [0.1, 0.9, 0.5, 2.0];
        let boost = [0.0; 4];
        let used = budget_trim(&lines, &mut keep, &mut reason, &scores, &boost, &tok, 9.0);
        // 返回行 token 和：6 = 2 保留行 × 3；输出 = 6 + 1.75×1 段 ≤ 9。
        assert_eq!(used, 6.0, "行 token 与 gap 合计受预算约束");
        assert!(!keep[0] && !keep[1], "低分填充行先裁（先低分后高分）");
        assert!(keep[2] && keep[3], "数字/证据行保留（保真锚最后裁）");
        assert_eq!(reason[2], R_NUM_LINE);
        assert_eq!(reason[3], R_EVIDENCE);
    }

    #[test]
    fn budget_trim_noop_when_within_budget() {
        use crate::compress::score::budget_trim;
        let lines = ["a", "b"];
        let tok = [2.0, 2.0];
        let mut keep = vec![true; 2];
        let mut reason = vec![1u8, 2u8];
        let scores = [1.0, 1.0];
        let boost = [0.0; 2];
        let used = budget_trim(&lines, &mut keep, &mut reason, &scores, &boost, &tok, 5.0);
        assert_eq!(used, 4.0, "预算未超时零操作");
        assert!(keep[0] && keep[1]);
    }

    #[test]
    fn budget_flag_deterministic_and_off_by_default() {
        // 默认路径逐字节一致：budget=0 时压缩输出与无 flag 完全一致。
        let text = "line one has alpha beta\nline two took 42ms id=7\nline three gamma delta\n";
        let cfg = crate::Config::default();
        let out = compress(text, "alpha", &cfg);
        let cfg_b = crate::Config {
            budget: 0.0,
            ..cfg.clone()
        };
        let out_b = compress(text, "alpha", &cfg_b);
        assert_eq!(out.text, out_b.text, "budget=0 与默认逐字节一致");
        // 硬预算下 kept token 占比受控：重复结构文本 + 紧预算。
        let big = "error: timeout on host A\n".repeat(40) + "summary: all ok\n";
        let cfg35 = crate::Config {
            budget: 0.35,
            ..cfg.clone()
        };
        let o35 = compress(&big, "timeout", &cfg35);
        assert!(o35.cut > 0.5, "35% 硬预算下 cut 应显著: {}", o35.cut);
        let o35b = compress(&big, "timeout", &cfg35);
        assert_eq!(o35.text, o35b.text, "硬预算路径确定性");
    }
}
