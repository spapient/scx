#![forbid(unsafe_code)]

//! scx 操作编排层（ops）：把「命令动作」从 CLI 外壳中独立出来。
//!
//! - [`Op`]：所有 CLI 一等操作的类型化表示（压缩 / 记忆库 / 评估 …）。
//! - [`from_opts`]：`args::Opts` → `Op` 的映射（优先级与原 `main::run` 分派顺序一致）。
//! - [`run`]：执行一个 `Op` 并渲染输出；不直接终止进程，失败收敛到 [`Failure`]。
//! - [`render_stats`] / [`render_json`]：纯渲染函数（无 I/O），`render_json` 返回
//!   无前缀纯 JSON，可被 MCP 等集成层直接复用；CLI 输出前缀由调用方拼接。
//!
//! 设计约束：本层只依赖 `crate::compress` / `crate::memory` 与 `args::Opts`，
//! 不依赖任何第三方 crate；任何程序可直接构造 `Op::Compress` 复用压缩内核。

use crate::args::Opts;
use crate::input::{read_fix_file, read_input};
use crate::memory::{recall_cases, Case, CaseStats, EvalMode, RecallHit, StoreInput};
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

pub type CliResult<T = ()> = Result<T, Failure>;
type StoreResult = io::Result<(bool, String)>;
type StoreReceiver = mpsc::Receiver<StoreResult>;

/// 压缩动作的独立入参（与 CLI 解析解耦，MCP 等集成层直接构造）。
#[derive(Clone)]
pub struct CompressJob {
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
    pub max_bytes: Option<u64>,
    pub mem_dir: String,
}

/// 标注动作入参。
pub struct AnnotateJob {
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
    },
    Eval {
        mem_dir: String,
        path: String,
        mode: EvalMode,
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
    Annotate(AnnotateJob),
    Judge {
        mem_dir: String,
        path: String,
        by: String,
    },
    Compress(Box<CompressJob>),
}

/// `Opts` → `Op`：分派顺序与原 `main::run` 完全一致（recall-only → eval →
/// l1-hist → stats → rate → rebuild → gc → delete → annotate → judge →
/// compress 兜底），保证 CLI 行为逐字节不变。
pub fn from_opts(opts: &Opts) -> Op {
    if let Some(op) = memory_op_a(opts) {
        op
    } else if let Some(op) = memory_op_b(opts) {
        op
    } else {
        Op::Compress(Box::new(compress_job(opts)))
    }
}

/// 前五个记忆/评估动作（判定点 ≤ 8）。
fn memory_op_a(opts: &Opts) -> Option<Op> {
    if opts.recall_only {
        return Some(Op::RecallOnly {
            mem_dir: opts.mem_dir.clone(),
            query: opts.query.clone(),
            k: opts.recall,
            detail: opts.recall_detail,
        });
    }
    if let Some(path) = &opts.eval {
        let mode = if opts.eval_gate_human {
            EvalMode::Human
        } else if opts.eval_gate_items {
            EvalMode::Items
        } else if opts.eval_gate {
            EvalMode::Mechanical
        } else {
            EvalMode::Report
        };
        return Some(Op::Eval {
            mem_dir: opts.mem_dir.clone(),
            path: path.clone(),
            mode,
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

/// 后五个维护/标注动作（判定点 ≤ 6）。
fn memory_op_b(opts: &Opts) -> Option<Op> {
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
        return Some(Op::Annotate(AnnotateJob {
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

/// `Opts` → `CompressJob`（字段搬运，无判定）。
fn compress_job(opts: &Opts) -> CompressJob {
    CompressJob {
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
        max_bytes: opts.max_bytes,
        mem_dir: opts.mem_dir.clone(),
    }
}

/// 执行一个操作（3-arm 分派，V(G)=3；具体执行在 run_special_a/b 与 run_compress）。
pub fn run(op: Op) -> CliResult {
    match op {
        Op::Compress(job) => run_compress(&job),
        Op::RecallOnly { .. }
        | Op::Eval { .. }
        | Op::L1Hist { .. }
        | Op::Stats { .. }
        | Op::Rate { .. } => run_special_a(op),
        Op::Rebuild { .. }
        | Op::Gc { .. }
        | Op::Delete { .. }
        | Op::Annotate(_)
        | Op::Judge { .. } => run_special_b(op),
    }
}

/// 记忆查询/评估类动作（5-arm match，V(G)=5）。
fn run_special_a(op: Op) -> CliResult {
    match op {
        Op::RecallOnly {
            mem_dir,
            query,
            k,
            detail,
        } => match run_recall(&mem_dir, &query, k, detail) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
            Err(e) => Err(Failure::message(format!("--recall-only failed: {e}"))),
        },
        Op::Eval {
            mem_dir,
            path,
            mode,
            verbose,
        } => emit_eval(crate::memory::eval_cases(&mem_dir, &path, mode, verbose)),
        Op::L1Hist { mem_dir } => emit(crate::memory::l1_hist_table(&mem_dir), "l1-hist"),
        Op::Stats { mem_dir } => emit(crate::memory::stats_table(&mem_dir), "stats"),
        Op::Rate { mem_dir, id, good } => {
            let case = crate::memory::rate_case(&mem_dir, &id, good)
                .map_err(|e| Failure::message(format!("rate failed: {e}")))?;
            println!(
                "rate: good={} bad={} quality={:.2} status={} case={}",
                case.feedback.good, case.feedback.bad, case.quality, case.status, id
            );
            if good {
                let (base, _) = crate::memory::l1_params(&mem_dir);
                println!("l1_base: {base:.3} (自适应)");
            }
            Ok(())
        }
        Op::Compress(_)
        | Op::Rebuild { .. }
        | Op::Gc { .. }
        | Op::Delete { .. }
        | Op::Annotate(_)
        | Op::Judge { .. } => {
            unreachable!("run_special_a called with non-special-a op")
        }
    }
}

/// 维护/标注类动作（5-arm match，V(G)=5）。
fn run_special_b(op: Op) -> CliResult {
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
        Op::Delete { mem_dir, id } => match crate::memory::delete_case(&mem_dir, &id) {
            Ok(true) => {
                println!("delete: removed case={id}");
                Ok(())
            }
            Ok(false) => Err(Failure::message(format!("no such case: {id}"))),
            Err(e) => Err(Failure::message(format!("delete failed: {e}"))),
        },
        Op::Annotate(job) => run_annotate(&job),
        Op::Judge {
            mem_dir: _,
            path,
            by,
        } => emit(crate::memory::judge_baseline(&path, &by), "judge"),
        Op::RecallOnly { .. }
        | Op::Eval { .. }
        | Op::L1Hist { .. }
        | Op::Stats { .. }
        | Op::Rate { .. }
        | Op::Compress(_) => unreachable!("run_special_b called with non-special-b op"),
    }
}

/// `--recall-only` 查看模式：只读召回、零库状态写入，库缺失时不创建目录。
fn run_recall(mem_dir: &str, query: &str, k: usize, detail: bool) -> io::Result<()> {
    let hits = recall_cases(mem_dir, query, k, true)?;
    let stdout = io::stdout();
    let mut out = stdout.lock();
    write_mem_lines(&mut out, &hits, mem_dir, detail, false)
}

/// 压缩统计行渲染（纯文本，无 I/O；格式与 v0.4.1 逐字节一致）。
fn render_stats(r: &Outcome) -> String {
    let base = format!(
        "compressed: kept={}/{} lines chars={}->{} ({:.1}% cut) dup={} seeds={} evidence={} risk={} guarded={} regions={} window={} terms={} forced={} mode={}",
        r.kept, r.total, r.chars_in, r.chars_out, r.cut_pct, r.dup, r.seeds, r.evidence, r.risk, r.guarded, r.regions, r.window, r.terms, r.forced, r.mode
    );
    // 硬上限溢出时追加可解析的记账字段；正常模式统计行格式保持不变（golden 零漂移）。
    match r.mode {
        Mode::OverflowEvidence | Mode::OverflowFill => format!(
            "{base} max_tokens={} tok_est={:.0}",
            r.max_tokens.unwrap_or(0),
            r.tok_est
        ),
        _ => base,
    }
}

/// 压缩结果 JSON 渲染（无前缀纯 JSON，MCP 等集成层可直接复用）。
pub fn render_json(r: &Outcome) -> String {
    let mut out = format!(
        "{{\"kept\":{},\"total\":{},\"chars_in\":{},\"chars_out\":{},\"cut_pct\":{:.1},\"dup\":{},\"seeds\":{},\"evidence\":{},\"risk\":\"{}\",\"guarded\":{},\"regions\":{},\"window\":{},\"terms\":{},\"forced\":{},\"mode\":\"{}\"",
        r.kept, r.total, r.chars_in, r.chars_out, r.cut_pct, r.dup, r.seeds, r.evidence, r.risk, r.guarded, r.regions, r.window, r.terms, r.forced, r.mode
    );
    // --json 新增字段向后兼容：仅启用 --max-tokens 时附加，默认输出逐字节不变。
    if let Some(m) = r.max_tokens {
        out.push_str(&format!(
            ",\"max_tokens\":{},\"tok_est\":{:.0}",
            m, r.tok_est
        ));
    }
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
    out.push('}');
    out
}

/// 输出召回段 `mem: ...`（旁路，绝不影响正文/统计/退出码）。
/// case 只显示指纹段（`YYYY-MM-DD_<hash16>` 的 hash16），保证输出不随日期漂移。
fn write_mem_lines(
    out: &mut dyn Write,
    hits: &[RecallHit],
    root: &str,
    detail: bool,
    tolerate_unreadable: bool,
) -> io::Result<()> {
    for h in hits {
        let short = h.id.rsplit('_').next().unwrap_or(&h.id);
        writeln!(
            out,
            "mem: sim={:.2} score={:.2} case={}",
            h.sim, h.score, short
        )?;
        let c = match crate::memory::read_case(root, &h.id) {
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
    let mut first_nonempty = None;
    let mut first_error = None;
    for line in c.text.lines().map(str::trim).filter(|t| !t.is_empty()) {
        if first_nonempty.is_none() {
            first_nonempty = Some(line);
        }
        if is_err_text(line) {
            first_error = Some(line);
            break;
        }
    }
    let line = first_error.or(first_nonempty);
    if let Some(line) = line {
        parts.push(line);
    }
    let s = parts.join(" | ");
    // 只数到截断点：超长日志行不应为“是否加省略号”再扫一遍剩余字符。
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
fn is_err_text(t: &str) -> bool {
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
    crate::memory::escape_control(s)
}

fn checked_write(result: io::Result<()>) -> CliResult {
    match result {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(Failure::message(format!("cannot write output: {e}"))),
    }
}

fn emit(result: io::Result<Vec<String>>, action: &str) -> CliResult {
    let lines = result.map_err(|e| Failure::message(format!("{action} failed: {e}")))?;
    for line in lines {
        println!("{line}");
    }
    Ok(())
}

fn emit_eval(result: io::Result<(Vec<String>, bool)>) -> CliResult {
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

fn run_annotate(job: &AnnotateJob) -> CliResult {
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
    let lines = crate::memory::annotate_to_file(
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

fn run_compress(job: &CompressJob) -> CliResult {
    let text = read_input(&job.files).map_err(Failure::message)?;
    let outcome = compress(&text, &job.query, &job.config);
    let stdout = io::stdout();
    let mut out = stdout.lock();
    if !job.stat {
        checked_write(out.write_all(outcome.text.as_bytes()))?;
        if !outcome.text.is_empty() && !outcome.text.ends_with('\n') {
            checked_write(out.write_all(b"\n"))?;
        }
    }

    let mut store_rx: Option<StoreReceiver> = None;
    if job.store {
        store_rx = prepare_store(job, &text, &outcome)?;
    }
    if job.recall > 0 {
        match crate::memory::recall_cases(&job.mem_dir, &job.query, job.recall, false) {
            Ok(hits) => {
                let qualified = crate::memory::l1_hits(&job.mem_dir, hits);
                checked_write(write_mem_lines(
                    &mut out,
                    &qualified,
                    &job.mem_dir,
                    job.recall_detail,
                    true,
                ))?;
            }
            Err(e) => eprintln!("scx: warning: --recall skipped: {e}"),
        }
    }
    if !job.no_stats {
        checked_write(writeln!(out, "{}", render_stats(&outcome)))?;
    }
    if job.json {
        eprintln!("scxjson: {}", render_json(&outcome));
    }
    if let Some(rx) = store_rx {
        let result = rx
            .recv()
            .map_err(|_| Failure::message("memory store thread lost"))?;
        report_store(result)?;
    }
    Ok(())
}

fn prepare_store(
    job: &CompressJob,
    input: &str,
    outcome: &Outcome,
) -> CliResult<Option<StoreReceiver>> {
    let fix = if let Some(path) = &job.fix_file {
        let text =
            read_fix_file(path, crate::memory::MAX_FIX_BYTES as u64).map_err(Failure::message)?;
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
    let stats = CaseStats {
        kept: outcome.kept,
        total: outcome.total,
        cut_pct: outcome.cut_pct,
        risk: outcome.risk.to_string(),
    };
    let evidence_lno = outcome.kept_lno.clone();
    let evidence = extract_evidence(input, &evidence_lno);
    if job.recall > 0 {
        let mut store_input = StoreInput::new(&job.mem_dir, &job.query, &outcome.text, &stats);
        store_input.source = &job.files;
        store_input.forces = &job.config.forces;
        store_input.fix = &fix;
        store_input.evidence_lno = &evidence_lno;
        store_input.evidence = &evidence;
        store_input.max_bytes = job.max_bytes;
        report_store(crate::memory::store_case(store_input))?;
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
        let mut store_input = StoreInput::new(&mem_dir, &query, &text, &stats);
        store_input.source = &files;
        store_input.forces = &forces;
        store_input.fix = &fix;
        store_input.evidence_lno = &evidence_lno;
        store_input.evidence = &evidence;
        store_input.max_bytes = max_bytes;
        let _ = tx.send(crate::memory::store_case(store_input));
    });
    Ok(Some(rx))
}

fn report_store(result: StoreResult) -> CliResult {
    let (added, id) = result.map_err(|e| Failure::message(format!("memory store failed: {e}")))?;
    eprintln!(
        "scx: stored case={} added={}",
        id,
        if added { "yes" } else { "dedup" }
    );
    Ok(())
}

/// 按保留行原始行号从输入提取证据原文；行号越界时保留空串，不 panic。
fn extract_evidence(input: &str, lno: &[usize]) -> Vec<String> {
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
        let legacy = render_json(&o);
        assert!(legacy.starts_with("{\"kept\":"));
        assert!(!legacy.contains("max_tokens"), "默认路径不得带 max_tokens");
        let mut cfg2 = cfg;
        cfg2.max_tokens = Some(100);
        let o2 = compress("a\nb\nc\n", "a", &cfg2);
        assert!(render_json(&o2).contains("\"max_tokens\":100"));
    }
}
