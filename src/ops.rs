#![forbid(unsafe_code)]

//! scx 操作编排层（ops）：把「命令动作」从 CLI 外壳中独立出来。
//!
//! - [`Op`]：所有 CLI 一等操作的类型化表示（压缩 / 记忆库 / 评估 …）。
//! - [`parse`]：`args::Opts` → `Op` 的映射（优先级与原 `main::run` 分派顺序一致）。
//! - [`run`]：执行一个 `Op` 并渲染输出；不直接终止进程，失败收敛到 [`Failure`]。
//! - `stats` / [`render`]：纯渲染函数（无 I/O），`render` 返回
//!   无前缀纯 JSON，可被 MCP 等集成层直接复用；CLI 输出前缀由调用方拼接。
//!
//! 设计约束：本层只依赖 `crate::compress` / `crate::memory` 与 `args::Opts`，
//! 不依赖任何第三方 crate；任何程序可直接构造 `Op::Compress` 复用压缩内核。

use crate::args::Opts;
use crate::input::{fix, input};
use crate::memory::{Case, Eval, Hit, Input, Stats};
use crate::{compress, Mode, Outcome};
use std::io::{self, Write};
use std::sync::mpsc;

/// CLI 失败必须收敛到单一出口；库与业务层不直接终止进程。
pub enum Failure {
    Usage(String),
    Message(String),
    GateFailed,
}

impl Failure {
    fn message(text: impl Into<String>) -> Self {
        Self::Message(text.into())
    }
}

pub type Reply<T = ()> = Result<T, Failure>;
type Stored = io::Result<(bool, String)>;
type Receiver = mpsc::Receiver<Stored>;

/// 压缩动作的独立入参（与 CLI 解析解耦，MCP 等集成层直接构造）。
#[derive(Clone)]
pub struct Compress {
    pub config: crate::Config,
    pub files: Vec<String>,
    pub query: String,
    pub fix: String,
    pub fix_file: Option<String>,
    pub store: bool,
    pub recall: usize,
    pub recall_detail: bool,
    pub stat: bool,
    pub no_stats: bool,
    pub json: bool,
    pub explain: bool,
    pub max_bytes: Option<u64>,
    pub mem_dir: String,
    /// 可插拔相似度后端命令（`--embed-cmd`）；`None` = 不启用。
    pub embed_cmd: Option<String>,
    /// MinHash-LSH 召回权重（`--minhash-weight`，默认 0 = 关）。
    pub minhash_weight: f64,
    /// 压缩前脱敏种类（`--redact`，默认空 = 不脱敏）。
    pub redact: Vec<crate::redact::RedactKind>,
}

/// 标注动作入参。
pub struct Annotate {
    pub mem_dir: String,
    pub seed: u64,
    pub annotate_max: usize,
    pub annotate_raw: bool,
    pub annotate_out: Option<String>,
}

/// 所有 CLI 一等操作。
pub enum Op {
    RecallOnly {
        mem_dir: String,
        query: String,
        k: usize,
        detail: bool,
        embed_cmd: Option<String>,
        sim_weight: f64,
        minhash_weight: f64,
    },
    Eval {
        mem_dir: String,
        path: String,
        mode: Eval,
        verbose: bool,
    },
    L1Hist {
        mem_dir: String,
    },
    Stats {
        mem_dir: String,
    },
    Rate {
        mem_dir: String,
        id: String,
        good: bool,
    },
    Rebuild {
        mem_dir: String,
    },
    Gc {
        mem_dir: String,
    },
    Delete {
        mem_dir: String,
        id: String,
    },
    Annotate(Annotate),
    Judge {
        mem_dir: String,
        path: String,
        by: String,
    },
    Compress(Box<Compress>),
}

/// `Opts` → `Op`：分派顺序与原 `main::run` 完全一致（recall-only → eval →
/// l1-hist → stats → rate → rebuild → gc → delete → annotate → judge →
/// compress 兜底），保证 CLI 行为逐字节不变。
pub fn parse(opts: &Opts) -> Op {
    if let Some(op) = select(opts) {
        op
    } else if let Some(op) = maintain(opts) {
        op
    } else {
        Op::Compress(Box::new(job(opts)))
    }
}

/// 前五个记忆/评估动作（判定点 ≤ 8）。
fn select(opts: &Opts) -> Option<Op> {
    if opts.recall_only {
        return Some(Op::RecallOnly {
            mem_dir: opts.mem_dir.clone(),
            query: opts.query.clone(),
            k: opts.recall,
            detail: opts.recall_detail,
            embed_cmd: opts.embed_cmd.clone(),
            sim_weight: opts.config.sim_weight,
            minhash_weight: opts.minhash_weight,
        });
    }
    if let Some(path) = &opts.eval {
        return Some(Op::Eval {
            mem_dir: opts.mem_dir.clone(),
            path: path.clone(),
            mode: mode(opts),
            verbose: opts.eval_verbose,
        });
    }
    if opts.l1_hist {
        return Some(Op::L1Hist {
            mem_dir: opts.mem_dir.clone(),
        });
    }
    if opts.stats {
        return Some(Op::Stats {
            mem_dir: opts.mem_dir.clone(),
        });
    }
    if let Some((good, id)) = &opts.rate {
        return Some(Op::Rate {
            mem_dir: opts.mem_dir.clone(),
            id: id.clone(),
            good: *good,
        });
    }
    None
}

fn mode(opts: &Opts) -> Eval {
    if opts.eval_gate_human {
        Eval::Human
    } else if opts.eval_gate_items {
        Eval::Items
    } else if opts.eval_gate {
        Eval::Mechanical
    } else {
        Eval::Report
    }
}

/// 后五个维护/标注动作（判定点 ≤ 6）。
fn maintain(opts: &Opts) -> Option<Op> {
    if opts.rebuild {
        return Some(Op::Rebuild {
            mem_dir: opts.mem_dir.clone(),
        });
    }
    if opts.gc {
        return Some(Op::Gc {
            mem_dir: opts.mem_dir.clone(),
        });
    }
    if let Some(id) = &opts.delete {
        return Some(Op::Delete {
            mem_dir: opts.mem_dir.clone(),
            id: id.clone(),
        });
    }
    if opts.annotate || opts.annotate_raw {
        return Some(Op::Annotate(Annotate {
            mem_dir: opts.mem_dir.clone(),
            seed: opts.seed,
            annotate_max: opts.annotate_max,
            annotate_raw: opts.annotate_raw,
            annotate_out: opts.annotate_out.clone(),
        }));
    }
    if let Some(path) = &opts.judge {
        return Some(Op::Judge {
            mem_dir: opts.mem_dir.clone(),
            path: path.clone(),
            by: opts.judge_by.clone().unwrap_or_else(|| {
                std::env::var("USERNAME").unwrap_or_else(|_| "anonymous".into())
            }),
        });
    }
    None
}

/// `Opts` → `Compress`（字段搬运，无判定）。
fn job(opts: &Opts) -> Compress {
    Compress {
        config: opts.config.clone(),
        files: opts.files.clone(),
        query: opts.query.clone(),
        fix: opts.fix.clone(),
        fix_file: opts.fix_file.clone(),
        store: opts.store,
        recall: opts.recall,
        recall_detail: opts.recall_detail,
        stat: opts.stat,
        no_stats: opts.no_stats,
        json: opts.json,
        explain: opts.explain,
        max_bytes: opts.max_bytes,
        mem_dir: opts.mem_dir.clone(),
        embed_cmd: opts.embed_cmd.clone(),
        minhash_weight: opts.minhash_weight,
        redact: opts.redact.clone(),
    }
}

/// 执行一个操作（3-arm 分派，V(G)=3；具体执行在 inspect/b 与 filter）。
pub fn run(op: Op) -> Reply {
    match op {
        Op::Compress(job) => filter(&job),
        Op::RecallOnly { .. }
        | Op::Eval { .. }
        | Op::L1Hist { .. }
        | Op::Stats { .. }
        | Op::Rate { .. } => dispatch(op),
        Op::Rebuild { .. }
        | Op::Gc { .. }
        | Op::Delete { .. }
        | Op::Annotate(_)
        | Op::Judge { .. } => apply(op),
    }
}

/// 记忆查询/评估类动作（5-arm match，V(G)=5）。
fn dispatch(op: Op) -> Reply {
    match op {
        Op::RecallOnly {
            mem_dir,
            query,
            k,
            detail,
            embed_cmd,
            sim_weight,
            minhash_weight,
        } => result(
            &mem_dir,
            &query,
            k,
            detail,
            embed_cmd,
            sim_weight,
            minhash_weight,
        ),
        Op::Eval {
            mem_dir,
            path,
            mode,
            verbose,
        } => eval(crate::memory::evaluate(&mem_dir, &path, mode, verbose)),
        Op::L1Hist { mem_dir } => emit(crate::memory::history(&mem_dir), "l1-hist"),
        Op::Stats { mem_dir } => emit(crate::memory::stats(&mem_dir), "stats"),
        Op::Rate { mem_dir, id, good } => rate(&mem_dir, &id, good),
        Op::Compress(_)
        | Op::Rebuild { .. }
        | Op::Gc { .. }
        | Op::Delete { .. }
        | Op::Annotate(_)
        | Op::Judge { .. } => {
            unreachable!("inspect called with non-special-a op")
        }
    }
}

fn result(
    mem_dir: &str,
    query: &str,
    k: usize,
    detail: bool,
    embed_cmd: Option<String>,
    sim_weight: f64,
    minhash_weight: f64,
) -> Reply {
    match recall(
        mem_dir,
        query,
        k,
        detail,
        embed_cmd,
        sim_weight,
        minhash_weight,
    ) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(Failure::message(format!("--recall-only failed: {e}"))),
    }
}

fn rate(mem_dir: &str, id: &str, good: bool) -> Reply {
    let case = crate::memory::rate(mem_dir, id, good)
        .map_err(|e| Failure::message(format!("rate failed: {e}")))?;
    println!(
        "rate: good={} bad={} quality={:.2} status={} case={}",
        case.feedback.good, case.feedback.bad, case.quality, case.status, id
    );
    if good {
        let (base, _) = crate::memory::params(mem_dir);
        println!("l1_base: {base:.3} (自适应)");
    }
    Ok(())
}

/// 维护/标注类动作（5-arm match，V(G)=5）。
fn apply(op: Op) -> Reply {
    match op {
        Op::Rebuild { mem_dir } => {
            let (docs, terms) = crate::memory::rebuild(&mem_dir)
                .map_err(|e| Failure::message(format!("rebuild failed: {e}")))?;
            println!("rebuild: docs={docs} terms={terms}");
            Ok(())
        }
        Op::Gc { mem_dir } => {
            let removed = crate::memory::gc(&mem_dir)
                .map_err(|e| Failure::message(format!("gc failed: {e}")))?;
            println!("gc: removed={removed}");
            Ok(())
        }
        Op::Delete { mem_dir, id } => delete(&mem_dir, &id),
        Op::Annotate(job) => annotate(&job),
        Op::Judge {
            mem_dir: _,
            path,
            by,
        } => emit(crate::memory::judge(&path, &by), "judge"),
        Op::RecallOnly { .. }
        | Op::Eval { .. }
        | Op::L1Hist { .. }
        | Op::Stats { .. }
        | Op::Rate { .. }
        | Op::Compress(_) => unreachable!("maintain called with non-special-b op"),
    }
}

fn delete(mem_dir: &str, id: &str) -> Reply {
    match crate::memory::delete(mem_dir, id) {
        Ok(true) => {
            println!("delete: removed case={id}");
            Ok(())
        }
        Ok(false) => Err(Failure::message(format!("no such case: {id}"))),
        Err(e) => Err(Failure::message(format!("delete failed: {e}"))),
    }
}

/// `--recall-only` 查看模式：只读召回、零库状态写入，库缺失时不创建目录。
fn recall(
    mem_dir: &str,
    query: &str,
    k: usize,
    detail: bool,
    embed_cmd: Option<String>,
    sim_weight: f64,
    minhash_weight: f64,
) -> io::Result<()> {
    let hits = crate::memory::search_fused(
        mem_dir,
        query,
        k,
        true,
        embed_cmd.as_deref(),
        sim_weight,
        minhash_weight,
    )?;
    let stdout = io::stdout();
    let mut out = stdout.lock();
    lines(&mut out, &hits, mem_dir, detail, false)
}

/// 压缩统计行渲染（纯文本，无 I/O；格式与 v0.4.1 逐字节一致）。
fn stats(r: &Outcome) -> String {
    let base = format!(
        "compressed: kept={}/{} lines chars={}->{} ({:.1}% cut) dup={} seeds={} evidence={} risk={} guarded={} regions={} window={} terms={} forced={} mode={}",
        r.kept, r.total, r.input, r.output, r.cut, r.dup, r.seeds, r.evidence, r.risk, r.guarded, r.regions, r.window, r.terms, r.forced, r.mode
    );
    // 硬上限溢出时追加可解析的记账字段；正常模式统计行格式保持不变（golden 零漂移）。
    match r.mode {
        Mode::OverflowEvidence | Mode::OverflowFill => format!(
            "{base} max_tokens={} tok_est={:.0}",
            r.limit.unwrap_or(0),
            r.tokens
        ),
        _ => base,
    }
}

/// 压缩结果 JSON 渲染（无前缀纯 JSON，MCP 等集成层可直接复用）。
pub fn render(r: &Outcome) -> String {
    let mut out = format!(
        "{{\"kept\":{},\"total\":{},\"chars_in\":{},\"chars_out\":{},\"cut_pct\":{:.1},\"dup\":{},\"seeds\":{},\"evidence\":{},\"risk\":\"{}\",\"guarded\":{},\"regions\":{},\"window\":{},\"terms\":{},\"forced\":{},\"mode\":\"{}\"",
        r.kept, r.total, r.input, r.output, r.cut, r.dup, r.seeds, r.evidence, r.risk, r.guarded, r.regions, r.window, r.terms, r.forced, r.mode
    );
    // --json 新增字段向后兼容：仅启用 --max-tokens 时附加（v0.4.1 契约不变）。
    if let Some(m) = r.limit {
        out.push_str(&format!(
            ",\"max_tokens\":{},\"tok_est\":{:.0}",
            m, r.tokens
        ));
    }
    // v0.6.4 效果口径：输入/输出 token 估算（`terms::estimate`）与缩减率，全模式
    // 恒定填充（passthrough/fail-open 时 tokens_out == tokens_in，cut 为 0）。
    out.push_str(&format!(
        ",\"tokens_in\":{:.0},\"tokens_out\":{:.0},\"token_cut_pct\":{:.1}",
        r.tokens_in,
        crate::terms::estimate(&r.text),
        token_cut(r.tokens_in, crate::terms::estimate(&r.text))
    ));
    // 删除区间（每段一条，1-based 闭区间 + 原因）；默认 stdout 统计行不输出。
    out.push_str(&format!(
        ",\"dropped\":[{}]",
        r.dropped
            .iter()
            .map(|d| format!(
                "{{\"start\":{},\"end\":{},\"reason\":\"{}\"}}",
                d.start,
                d.end,
                d.reason.as_str()
            ))
            .collect::<Vec<_>>()
            .join(",")
    ));
    // 可选预算利用率（%）：--ratio 预算实际填充了多少。
    out.push_str(&format!(",\"budget_used_pct\":{:.1}", r.budget_used_pct));
    // 信息留存率（0..=1）：显式证据或默认行保留率代理（P4），全模式恒填充。
    out.push_str(&format!(",\"ev_retention\":{:.4}", r.ev_retention));
    // 逐行决策解释（数组下标 = 行号-1）；仅筛选模式非空。
    out.push_str(&format!(
        ",\"explain\":[{}]",
        r.explain
            .iter()
            .map(|e| format!("\"{}\"", e.as_str()))
            .collect::<Vec<_>>()
            .join(",")
    ));
    out.push('}');
    out
}

/// 逐行决策解释（stderr，一行；`<line>:<reason>` 空格分隔，1-based）。
fn explain_report(r: &Outcome) -> String {
    r.explain
        .iter()
        .enumerate()
        .map(|(i, e)| format!("{}:{}", i + 1, e.as_str()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 脱敏命中审计（stderr，一行；`kind=count` 空格分隔）。
fn redact_report(hits: &[crate::redact::Hit]) -> String {
    let parts: Vec<String> = hits
        .iter()
        .map(|h| format!("{}={}", h.kind, h.count))
        .collect();
    format!("scx: redact: {}", parts.join(" "))
}

/// 附加 stderr 诊断（json/explain/redact），从 `filter` 抽出以压认知复杂度。
fn diagnostics(job: &Compress, outcome: &Outcome, redact_hits: &[crate::redact::Hit]) {
    if job.json {
        eprintln!("scxjson: {}", render(outcome));
    }
    if job.explain {
        eprintln!("scx: explain: {}", explain_report(outcome));
    }
    if !redact_hits.is_empty() {
        eprintln!("{}", redact_report(redact_hits));
    }
}

/// 压缩前脱敏入口：空 kinds 时原样返回（无命中）。
fn redact_input(
    text: String,
    kinds: &[crate::redact::RedactKind],
) -> (String, Vec<crate::redact::Hit>) {
    if kinds.is_empty() {
        (text, Vec::new())
    } else {
        crate::redact::redact(&text, kinds)
    }
}

/// token 缩减率（%）：输入估算为 0 时视为 0（passthrough/fail-open 恒为 0）。
fn token_cut(tin: f64, tout: f64) -> f64 {
    if tin <= 0.0 {
        0.0
    } else {
        (tin - tout) * 100.0 / tin
    }
}

/// 千分位格式化（u64，如 3318 -> "3,318"）。
fn sep(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// 长文本截断（按字符数，超长加省略号）。
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}…")
    }
}

/// 字段名补宽到 8 列（CJK 按 2 列），保证效果报告各行左对齐。
fn pad_field(field: &str) -> String {
    let w = field
        .chars()
        .map(|c| if crate::terms::full(c) { 2 } else { 1 })
        .sum::<usize>();
    format!("{field}{}", " ".repeat(8usize.saturating_sub(w)))
}

/// 效果报告行：`{icon} {field:8}  {value}`。
fn row(icon: &str, field: &str, value: &str) -> String {
    format!("{icon} {}  {value}\n", pad_field(field))
}

/// 风险等级中文（效果报告用）。
fn risk_zh(r: crate::Risk) -> &'static str {
    match r {
        crate::Risk::Low => "低",
        crate::Risk::Medium => "中",
        crate::Risk::High => "高",
    }
}

/// 模式中文标签与一句话说明（效果报告用）。
fn mode_label(m: Mode) -> (&'static str, &'static str) {
    match m {
        Mode::Sieve => ("sieve 智能筛选", "按问题逐行召回证据"),
        Mode::PassSmall => ("小输入透传", "无需压缩"),
        Mode::PassLongline => ("超长行透传", "单行过长，原样保留"),
        Mode::PassTooManyLines => ("行数超限透传", "行数过多，原样保留避免内存放大"),
        Mode::PassNoKept => ("预算过紧回退", "未选出有效行，原文保留"),
        Mode::FailOpenZeroSignal => ("无命中保护", "零丢失"),
        Mode::OverflowEvidence => ("证据优先保底", "超过 token 上限，证据全保留"),
        Mode::OverflowFill => ("上限裁剪", "超上限，保留证据并裁剪可选内容"),
    }
}

/// 压缩召回效果报告（v0.6.4）：stderr 人工可读块，stdout 契约零变化。
/// 三模式各有结论卡；数字口径与 `render()` JSON 完全一致（`terms::estimate`）。
pub fn recall_report(r: &Outcome, query: &str) -> String {
    let mut s = String::from("✦ SCX 压缩召回 ✦\n\n");
    s.push_str(&row("📌", "查询", &clip(query, 40)));
    let (label, desc) = mode_label(r.mode);
    s.push_str(&row("⚙️", "模式", &format!("{label} · {desc}")));
    match r.mode {
        Mode::Sieve | Mode::OverflowEvidence | Mode::OverflowFill => {
            let tin = r.tokens_in;
            let tout = crate::terms::estimate(&r.text);
            s.push_str(&row(
                "📥",
                "原始",
                &format!(
                    "{} 行 · {} 字符 · 约 {} token",
                    sep(r.total as u64),
                    sep(r.input as u64),
                    sep(tin as u64)
                ),
            ));
            s.push_str(&row(
                "📤",
                "压缩后",
                &format!(
                    "{} 行 · {} 字符 · 约 {} token",
                    sep(r.kept as u64),
                    sep(r.output as u64),
                    sep(tout as u64)
                ),
            ));
            let d_rows = r.total.saturating_sub(r.kept);
            let d_chars = r.input.saturating_sub(r.output);
            s.push_str(&row(
                "💾",
                "省下",
                &format!(
                    "{} 行 · {} 字符 · 约 {} token（-{:.1}%）",
                    sep(d_rows as u64),
                    sep(d_chars as u64),
                    sep((tin - tout) as u64),
                    token_cut(tin, tout)
                ),
            ));
            s.push_str(&row(
                "🎯",
                "证据召回",
                &format!("{} 条关键证据全部保留 · 0 丢失", r.evidence),
            ));
            s.push_str(&row("🔁", "去重", &format!("{} 行重复内容已折叠", r.dup)));
            s.push_str(&row(
                "✂️",
                "裁剪",
                &format!("{} 段无关内容已移除", r.dropped.len()),
            ));
            s.push_str(&row(
                "🛡️",
                "风险",
                &format!("{} · {} 条额外守护", risk_zh(r.risk), r.guarded),
            ));
        }
        Mode::PassSmall | Mode::PassLongline | Mode::PassTooManyLines | Mode::PassNoKept => {
            s.push_str(&row(
                "📥",
                "原始",
                &format!("{} 行 · {} 字符", sep(r.total as u64), sep(r.input as u64)),
            ));
            s.push_str(&row("📤", "压缩后", "原样保留，一字未动"));
            let save = match r.mode {
                Mode::PassSmall => "0 —— 内容本就很少",
                Mode::PassLongline => "0 —— 无损失保留原文",
                Mode::PassTooManyLines => "0 —— 无损失保留原文",
                _ => "0 —— 无损失保留原文",
            };
            let why = match r.mode {
                Mode::PassSmall => format!(
                    "内容很少（<{} 行且不足 {} token），压缩反而可能丢信息，自动跳过",
                    crate::policy::SMALL_LINES,
                    crate::policy::SMALL_TOK as u64
                ),
                Mode::PassLongline => "单行过长无法安全压缩，原样返回，不冒险删减".to_string(),
                Mode::PassTooManyLines => format!(
                    "行数超过 {}，筛选会按行数放大内存，原样返回避免内存耗尽",
                    crate::policy::MAX_LINES
                ),
                _ => "预算过紧未选出有效行，宁可原样返回也不静默删减".to_string(),
            };
            s.push_str(&row("💾", "省下", save));
            s.push_str(&row("💡", "说明", &why));
        }
        Mode::FailOpenZeroSignal => {
            s.push_str(&row(
                "📥",
                "原始",
                &format!("{} 行 · {} 字符", sep(r.total as u64), sep(r.input as u64)),
            ));
            s.push_str(&row("📤", "压缩后", "原样保留，一字未动"));
            s.push_str(&row("💾", "省下", "0 —— 无相关内容可压缩"));
            s.push_str(&row(
                "💡",
                "说明",
                "没找到与问题相关的证据行，压缩会误删信息，完整返回",
            ));
        }
    }
    s
}

/// 输出召回段 `mem: ...`（旁路，绝不影响正文/统计/退出码）。
/// case 只显示指纹段（`YYYY-MM-DD_<hash16>` 的 hash16），保证输出不随日期漂移。
fn lines(
    out: &mut dyn Write,
    hits: &[Hit],
    root: &str,
    detail: bool,
    tolerate_unreadable: bool,
) -> io::Result<()> {
    for h in hits {
        let short = h.id.rsplit('_').next().unwrap_or(&h.id);
        // 索引路径的 id 未过白名单，可能含控制字符；转义后回显防终端注入。
        writeln!(
            out,
            "mem: sim={:.2} score={:.2} case={}",
            h.sim,
            h.score,
            escape(short)
        )?;
        let c = match crate::memory::load(root, &h.id) {
            Ok(c) => c,
            Err(e) if tolerate_unreadable => {
                eprintln!("scx: warning: recall case {short} unreadable: {e}");
                continue;
            }
            Err(e) => return Err(e),
        };
        if detail {
            writeln!(out, "  summary: {}", escape(&summary(&c)))?;
            for (i, &lno) in c.evidence_lno.iter().enumerate() {
                writeln!(
                    out,
                    "  evidence: {} {}",
                    lno,
                    escape(c.evidence.get(i).map(String::as_str).unwrap_or(""))
                )?;
            }
        }
        if !c.fix.is_empty() {
            writeln!(
                out,
                "  fix: {} (candidate, sim={:.2})",
                escape(&c.fix),
                h.sim
            )?;
        }
    }
    Ok(())
}

/// 召回详情摘要：字段 + 首个错误/非空行，最多 300 字符。
fn summary(c: &Case) -> String {
    let mut parts: Vec<&str> = c
        .fields
        .error_code
        .iter()
        .chain(&c.fields.file_loc)
        .map(String::as_str)
        .collect();
    // 单趟找“首个错误行”，找不到再用“首个非空行”；避免对整篇正文扫两遍。
    if let Some(line) = first(&c.text) {
        parts.push(line);
    }
    let s = parts.join(" | ");
    // 只数到截断点：超长日志行不应为“是否加省略号”再扫一遍剩余字符。
    truncate(&s)
}

/// 单趟找“首个错误行”，找不到再用“首个非空行”。
fn first(text: &str) -> Option<&str> {
    let mut first_nonempty = None;
    for line in text.lines().map(str::trim).filter(|t| !t.is_empty()) {
        if first_nonempty.is_none() {
            first_nonempty = Some(line);
        }
        if error(line) {
            return Some(line);
        }
    }
    first_nonempty
}

/// 最多 300 字符，截断处补省略号。
fn truncate(s: &str) -> String {
    let mut out = String::with_capacity(s.len().min(900));
    for (i, ch) in s.chars().enumerate() {
        if i == 300 {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

/// 错误/警告行词面判定（summary 的谓词，V(G)=6）。
fn error(t: &str) -> bool {
    t.contains("error")
        || t.contains("ERROR")
        || t.contains("错误")
        || t.contains("ERR")
        || t.contains("warn")
        || t.contains("WARN")
        || t.contains("警告")
}

/// 召回回放输出转义控制字符（ANSI ESC / C0），防恶意日志注入终端。
fn escape(s: &str) -> String {
    crate::memory::escape(s)
}

fn write(result: io::Result<()>) -> Reply {
    match result {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(Failure::message(format!("cannot write output: {e}"))),
    }
}

fn emit(result: io::Result<Vec<String>>, action: &str) -> Reply {
    let lines = result.map_err(|e| Failure::message(format!("{action} failed: {e}")))?;
    for line in lines {
        println!("{line}");
    }
    Ok(())
}

fn eval(result: io::Result<(Vec<String>, bool)>) -> Reply {
    let (lines, gate_failed) = result.map_err(|e| Failure::message(format!("eval failed: {e}")))?;
    for line in lines {
        println!("{line}");
    }
    if gate_failed {
        Err(Failure::GateFailed)
    } else {
        Ok(())
    }
}

fn annotate(job: &Annotate) -> Reply {
    let path = job.annotate_out.clone().unwrap_or_else(|| {
        std::path::Path::new(&job.mem_dir)
            .join(if job.annotate_raw {
                "baseline-raw.json"
            } else {
                "baseline-auto.json"
            })
            .to_string_lossy()
            .into_owned()
    });
    let lines = crate::memory::annotate(
        &job.mem_dir,
        job.seed,
        job.annotate_max,
        job.annotate_raw,
        &path,
    )
    .map_err(|e| Failure::message(format!("annotate failed: {e}")))?;
    for line in lines {
        println!("{line}");
    }
    let out_name = std::path::Path::new(&path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.clone());
    println!("  out: {out_name}");
    Ok(())
}

/// 可插拔相似度后端：在 ops 层（I/O 层）预计算 query×每行的相似度，
/// 作为数据传入纯核心 `compress`，不破坏其零 I/O 契约。默认 `None` 不触发。
fn embed_sims(job: &Compress, text: &str) -> Result<Option<Vec<f64>>, Failure> {
    let Some(cmd) = &job.embed_cmd else {
        return Ok(None);
    };
    let mut buf: Option<String> = None;
    let lines = crate::compress::lines(text, &mut buf);
    let sims = crate::sim::compute(Some(cmd), &job.query, &lines).map_err(Failure::message)?;
    Ok(Some(sims))
}

fn filter(job: &Compress) -> Reply {
    let text = input(&job.files).map_err(Failure::message)?;
    // 压缩前脱敏（P7）：脱敏后正文参与压缩与记忆入库，原文不落盘。
    let (text, redact_hits) = redact_input(text, &job.redact);
    let mut config = job.config.clone();
    config.sims = embed_sims(job, &text)?;
    let outcome = compress(&text, &job.query, &config);
    let stdout = io::stdout();
    let mut out = stdout.lock();
    output(&mut out, job, &outcome)?;
    let mut store_rx: Option<Receiver> = None;
    if job.store {
        store_rx = prepare(job, &text, &outcome)?;
    }
    memory(&mut out, job)?;
    if !job.no_stats {
        write(writeln!(out, "{}", stats(&outcome)))?;
        eprintln!("{}", recall_report(&outcome, &job.query));
    }
    diagnostics(job, &outcome, &redact_hits);
    if let Some(rx) = store_rx {
        let result = rx
            .recv()
            .map_err(|_| Failure::message("memory store thread lost"))?;
        report(result)?;
    }
    Ok(())
}

/// 正文输出：`--stat` 跳过；末尾补换行。
fn output(out: &mut dyn Write, job: &Compress, outcome: &Outcome) -> Reply {
    if job.stat {
        return Ok(());
    }
    write(out.write_all(outcome.text.as_bytes()))?;
    if !outcome.text.is_empty() && !outcome.text.ends_with('\n') {
        write(out.write_all(b"\n"))?;
    }
    Ok(())
}

/// 召回旁路输出；失败只告警，不改变正文/统计/退出码。
fn memory(out: &mut dyn Write, job: &Compress) -> Reply {
    if job.recall == 0 {
        return Ok(());
    }
    match crate::memory::search_fused(
        &job.mem_dir,
        &job.query,
        job.recall,
        false,
        job.embed_cmd.as_deref(),
        job.config.sim_weight,
        job.minhash_weight,
    ) {
        Ok(hits) => {
            let qualified = crate::memory::hits(&job.mem_dir, hits);
            write(lines(
                out,
                &qualified,
                &job.mem_dir,
                job.recall_detail,
                true,
            ))
        }
        Err(e) => {
            eprintln!("scx: warning: --recall skipped: {e}");
            Ok(())
        }
    }
}

fn prepare(job: &Compress, input: &str, outcome: &Outcome) -> Reply<Option<Receiver>> {
    let fix = if let Some(path) = &job.fix_file {
        let text = fix(path, crate::memory::MAX_FIX_BYTES as u64).map_err(Failure::message)?;
        let text = text.trim_end().to_string();
        if text.is_empty() {
            eprintln!("scx: note: --fix-file {path} is empty; case stored without fix");
        }
        text
    } else {
        job.fix.clone()
    };
    if fix.is_empty() {
        eprintln!(
            "scx: note: --store without --fix; L1 cannot answer 'how was it solved' - \
pass --fix \"...\" after a verified fix (HARD discipline)"
        );
    }
    if matches!(outcome.mode, Mode::FailOpenZeroSignal) {
        eprintln!(
            "scx: note: query had no hits; --store will persist the full input \
(low-signal memory). inspect with --recall-only first"
        );
    }
    let stats = Stats {
        kept: outcome.kept,
        total: outcome.total,
        cut: outcome.cut,
        risk: outcome.risk.to_string(),
    };
    let evidence_lno = outcome.numbers.clone();
    let evidence = evidence(input, &evidence_lno);
    if job.recall > 0 {
        let mut store_input = Input::new(&job.mem_dir, &job.query, &outcome.text, &stats);
        store_input.source = &job.files;
        store_input.forces = &job.config.forces;
        store_input.fix = &fix;
        store_input.evidence_lno = &evidence_lno;
        store_input.evidence = &evidence;
        store_input.max_bytes = job.max_bytes;
        report(crate::memory::save(store_input))?;
        return Ok(None);
    }

    let (tx, rx) = mpsc::channel();
    let mem_dir = job.mem_dir.clone();
    let query = job.query.clone();
    let text = outcome.text.clone();
    let files = job.files.clone();
    let forces = job.config.forces.clone();
    let max_bytes = job.max_bytes;
    std::thread::spawn(move || {
        let mut store_input = Input::new(&mem_dir, &query, &text, &stats);
        store_input.source = &files;
        store_input.forces = &forces;
        store_input.fix = &fix;
        store_input.evidence_lno = &evidence_lno;
        store_input.evidence = &evidence;
        store_input.max_bytes = max_bytes;
        let _ = tx.send(crate::memory::save(store_input));
    });
    Ok(Some(rx))
}

fn report(result: Stored) -> Reply {
    let (added, id) = result.map_err(|e| Failure::message(format!("memory store failed: {e}")))?;
    eprintln!(
        "scx: stored case={} added={}",
        id,
        if added { "yes" } else { "dedup" }
    );
    Ok(())
}

/// 按保留行原始行号从输入提取证据原文；行号越界时保留空串，不 panic。
fn evidence(input: &str, lno: &[usize]) -> Vec<String> {
    if lno.is_empty() {
        return Vec::new();
    }
    // 常见 LF 输入零拷贝；只有真正含 CR 时才做整文归一化。
    let normalized;
    let input = if input.contains('\r') {
        normalized = input.replace("\r\n", "\n");
        normalized.as_str()
    } else {
        input
    };
    let mut lines: Vec<&str> = input.split('\n').collect();
    if lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lno.iter()
        .map(|&n| {
            lines
                .get(n.saturating_sub(1))
                .copied()
                .unwrap_or("")
                .to_string()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_truncates_to_300_chars() {
        let mut c = Case::new("id".into(), "q".into(), vec![], String::new());
        c.text = format!("ERROR {}", "x".repeat(400));
        let got = summary(&c);
        assert_eq!(got.chars().count(), 301, "300 字符 + 省略号");
        assert!(got.ends_with('…'));
    }

    #[test]
    fn render_json_keeps_legacy_fields_and_appends_max_tokens() {
        let cfg = crate::Config::default();
        let o = compress("a\nb\nc\n", "a", &cfg);
        let legacy = render(&o);
        assert!(legacy.starts_with("{\"kept\":"));
        assert!(!legacy.contains("max_tokens"), "默认路径不得带 max_tokens");
        let mut cfg2 = cfg;
        cfg2.limit = Some(100);
        let o2 = compress("a\nb\nc\n", "a", &cfg2);
        assert!(render(&o2).contains("\"max_tokens\":100"));
    }

    /// 60 行合成日志 + 命中查询 → sieve 卡：标题/查询/模式/证据/风险齐备，
    /// token 数字与估算口径一致。
    #[test]
    fn recall_report_sieve_card_is_complete() {
        let text = (0..60)
            .map(|i| {
                if i % 5 == 0 {
                    format!(
                        "[12:00] ERROR failed to connect to database: connection refused attempt={i}"
                    )
                } else if i % 3 == 0 {
                    format!("[12:00] WARN retrying in 5s... attempt={i}")
                } else {
                    format!("[12:00] INFO cache warmup complete ({i} entries)")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let o = compress(
            &text,
            "database connection failed",
            &crate::Config::default(),
        );
        assert_eq!(o.mode, Mode::Sieve, "60 行命中查询应走 sieve");
        assert!(
            (o.tokens_in - crate::terms::estimate(&text)).abs() < 1e-9,
            "tokens_in 恒等于输入估算"
        );
        let rep = recall_report(&o, "database connection failed");
        assert!(rep.contains("✦ SCX 压缩召回 ✦"));
        assert!(rep.contains("📌 查询"));
        assert!(rep.contains("database connection failed"));
        assert!(rep.contains("sieve 智能筛选"));
        assert!(rep.contains("📥 原始"));
        assert!(rep.contains("📤 压缩后"));
        assert!(rep.contains("💾 省下"));
        assert!(rep.contains(&format!("约 {} token", o.tokens_in as u64)));
        assert!(rep.contains("证据召回"));
        assert!(rep.contains("🛡️ 风险"));
    }

    /// 小输入（<40 行）→ passthrough 卡：说明行解释为什么不压缩。
    #[test]
    fn recall_report_passthrough_card_explains() {
        let o = compress("a\nb\nc\nd\ne\nf\ng\n", "a", &crate::Config::default());
        assert_eq!(o.mode, Mode::PassSmall);
        let rep = recall_report(&o, "a");
        assert!(rep.contains("✦ SCX 压缩召回 ✦"));
        assert!(rep.contains("小输入透传"));
        assert!(rep.contains("原样保留，一字未动"));
        assert!(rep.contains("💡 说明"));
        assert!(rep.contains("自动跳过"));
    }

    /// 查询零命中 → fail-open 卡：零丢失语义。
    #[test]
    fn recall_report_failopen_card_preserves() {
        let text = (0..60)
            .map(|i| format!("[12:00] INFO warmup entry {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let o = compress(&text, "zzz_no_such_term", &crate::Config::default());
        assert_eq!(o.mode, Mode::FailOpenZeroSignal);
        let rep = recall_report(&o, "zzz_no_such_term");
        assert!(rep.contains("无命中保护"));
        assert!(rep.contains("原样保留，一字未动"));
        assert!(rep.contains("完整返回"));
    }

    /// render() v0.6.4 新字段：tokens_in/out/cut 全模式填充；passthrough cut=0。
    #[test]
    fn render_adds_token_effect_fields() {
        let cfg = crate::Config::default();
        let small = compress("a\nb\nc\n", "a", &cfg);
        let j = render(&small);
        assert!(j.contains("\"tokens_in\":"), "默认路径也带 tokens_in");
        assert!(j.contains("\"tokens_out\":"));
        assert!(j.contains("\"token_cut_pct\":0.0"), "passthrough cut 为 0");

        let big = (0..60)
            .map(|i| format!("line {i} ERROR boom"))
            .collect::<Vec<_>>()
            .join("\n");
        let o = compress(&big, "boom", &cfg);
        assert!(
            o.mode == Mode::Sieve || o.mode == Mode::OverflowEvidence,
            "60 行命中走 sieve/overflow"
        );
        let tin = o.tokens_in;
        let tout = crate::terms::estimate(&o.text);
        let j2 = render(&o);
        assert!(j2.contains(&format!("\"tokens_in\":{:.0}", tin)));
        assert!(j2.contains(&format!("\"tokens_out\":{:.0}", tout)));
        assert!(j2.contains(&format!("\"token_cut_pct\":{:.1}", token_cut(tin, tout))));
    }

    /// token_cut 边界：输入 0 安全返回 0；正常削减百分比正确。
    #[test]
    fn token_cut_is_safe_and_exact() {
        assert_eq!(token_cut(0.0, 999.0), 0.0);
        assert!((token_cut(100.0, 50.0) - 50.0).abs() < 1e-9);
        assert!((token_cut(200.0, 180.0) - 10.0).abs() < 1e-9);
    }
}
