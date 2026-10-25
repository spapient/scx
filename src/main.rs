#![forbid(unsafe_code)]

// scx CLI：参数解析在 args，输入读取在 input，记忆输出接线在本文件；
// 筛选逻辑全在库 crate（scx::compress）。本文件只做命令编排与输出。
mod args;
mod input;

use args::{parse_args, Opts};
use input::{read_fix_file, read_input};
use scx::memory::{recall_cases, Case, CaseStats, EvalMode, RecallHit, StoreInput};
use scx::{compress, Mode, Outcome};
use std::io::{self, Write};
use std::sync::mpsc;

/// CLI 失败必须收敛到单一出口；库与业务层不直接终止进程。
enum Failure {
    Usage(String),
    Message(String),
    GateFailed,
}

impl Failure {
    fn message(text: impl Into<String>) -> Self {
        Self::Message(text.into())
    }
}

type CliResult<T = ()> = Result<T, Failure>;
type StoreResult = io::Result<(bool, String)>;
type StoreReceiver = mpsc::Receiver<StoreResult>;

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
        let c = match scx::memory::read_case(root, &h.id) {
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
    let is_err_line = |t: &str| {
        t.contains("error")
            || t.contains("ERROR")
            || t.contains("错误")
            || t.contains("ERR")
            || t.contains("warn")
            || t.contains("WARN")
            || t.contains("警告")
    };
    // 单趟找“首个错误行”，找不到再用“首个非空行”；避免对整篇正文扫两遍。
    let mut first_nonempty = None;
    let mut first_error = None;
    for line in c.text.lines().map(str::trim).filter(|t| !t.is_empty()) {
        if first_nonempty.is_none() {
            first_nonempty = Some(line);
        }
        if is_err_line(line) {
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

/// 召回回放输出转义控制字符（ANSI ESC / C0），防恶意日志注入终端。
fn escape(s: &str) -> String {
    scx::memory::escape_control(s)
}

/// `--recall-only` 查看模式：只读召回、零库状态写入，库缺失时不创建目录。
fn run_recall(mem_dir: &str, query: &str, k: usize, detail: bool) -> io::Result<()> {
    let hits = recall_cases(mem_dir, query, k, true)?;
    let stdout = io::stdout();
    let mut out = stdout.lock();
    write_mem_lines(&mut out, &hits, mem_dir, detail, false)
}

fn stats_line(r: &Outcome) -> String {
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

fn json_line(r: &Outcome) {
    let mut out = format!(
        "scxjson: {{\"kept\":{},\"total\":{},\"chars_in\":{},\"chars_out\":{},\"cut_pct\":{:.1},\"dup\":{},\"seeds\":{},\"evidence\":{},\"risk\":\"{}\",\"guarded\":{},\"regions\":{},\"window\":{},\"terms\":{},\"forced\":{},\"mode\":\"{}\"",
        r.kept, r.total, r.chars_in, r.chars_out, r.cut_pct, r.dup, r.seeds, r.evidence, r.risk, r.guarded, r.regions, r.window, r.terms, r.forced, r.mode
    );
    // --json 新增字段向后兼容：仅启用 --max-tokens 时附加，默认输出逐字节不变。
    if let Some(m) = r.max_tokens {
        out.push_str(&format!(
            ",\"max_tokens\":{},\"tok_est\":{:.0}",
            m, r.tok_est
        ));
    }
    out.push('}');
    eprintln!("{out}");
}

fn checked_write(result: io::Result<()>) -> CliResult {
    match result {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(Failure::message(format!("cannot write output: {e}"))),
    }
}

/// 帮助/版本短路：`--` 终止符之后不再解释为选项。
fn early_exit(args: &[String]) -> bool {
    let mut after_terminator = false;
    for a in args {
        if a == "--" {
            after_terminator = true;
            continue;
        }
        if !after_terminator && (a == "--help" || a == "-h") {
            println!("{}", args::usage());
            return true;
        }
        if !after_terminator && (a == "--version" || a == "-V") {
            println!("scx {}", scx::VERSION);
            return true;
        }
    }
    false
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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let failure = match run(&args) {
        Ok(()) => return,
        Err(failure) => failure,
    };
    match failure {
        Failure::Usage(message) => {
            eprintln!("scx: {message}");
            eprintln!("{}", args::usage());
        }
        Failure::Message(message) => eprintln!("scx: {message}"),
        Failure::GateFailed => {}
    }
    std::process::exit(2);
}

fn run(args: &[String]) -> CliResult {
    if early_exit(args) {
        return Ok(());
    }
    let opts: Opts = parse_args(args).map_err(Failure::Usage)?;

    if opts.recall_only {
        return match run_recall(&opts.mem_dir, &opts.query, opts.recall, opts.recall_detail) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
            Err(e) => Err(Failure::message(format!("--recall-only failed: {e}"))),
        };
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
        return emit_eval(scx::memory::eval_cases(
            &opts.mem_dir,
            path,
            mode,
            opts.eval_verbose,
        ));
    }
    if opts.l1_hist {
        return emit(scx::memory::l1_hist_table(&opts.mem_dir), "l1-hist");
    }
    if opts.stats {
        return emit(scx::memory::stats_table(&opts.mem_dir), "stats");
    }
    if let Some((good, id)) = &opts.rate {
        let case = scx::memory::rate_case(&opts.mem_dir, id, *good)
            .map_err(|e| Failure::message(format!("rate failed: {e}")))?;
        println!(
            "rate: good={} bad={} quality={:.2} status={} case={}",
            case.feedback.good, case.feedback.bad, case.quality, case.status, id
        );
        if *good {
            let (base, _) = scx::memory::l1_params(&opts.mem_dir);
            println!("l1_base: {base:.3} (自适应)");
        }
        return Ok(());
    }
    if opts.rebuild {
        let (docs, terms) = scx::memory::rebuild(&opts.mem_dir)
            .map_err(|e| Failure::message(format!("rebuild failed: {e}")))?;
        println!("rebuild: docs={docs} terms={terms}");
        return Ok(());
    }
    if opts.gc {
        let removed = scx::memory::gc(&opts.mem_dir)
            .map_err(|e| Failure::message(format!("gc failed: {e}")))?;
        println!("gc: removed={removed}");
        return Ok(());
    }
    if let Some(id) = &opts.delete {
        return match scx::memory::delete_case(&opts.mem_dir, id) {
            Ok(true) => {
                println!("delete: removed case={id}");
                Ok(())
            }
            Ok(false) => Err(Failure::message(format!("no such case: {id}"))),
            Err(e) => Err(Failure::message(format!("delete failed: {e}"))),
        };
    }
    if opts.annotate || opts.annotate_raw {
        return run_annotate(&opts);
    }
    if let Some(path) = &opts.judge {
        let by = opts
            .judge_by
            .clone()
            .unwrap_or_else(|| std::env::var("USERNAME").unwrap_or_else(|_| "anonymous".into()));
        return emit(scx::memory::judge_baseline(path, &by), "judge");
    }
    run_compress(&opts)
}

fn run_annotate(opts: &Opts) -> CliResult {
    let path = opts.annotate_out.clone().unwrap_or_else(|| {
        std::path::Path::new(&opts.mem_dir)
            .join(if opts.annotate_raw {
                "baseline-raw.json"
            } else {
                "baseline-auto.json"
            })
            .to_string_lossy()
            .into_owned()
    });
    let lines = scx::memory::annotate_to_file(
        &opts.mem_dir,
        opts.seed,
        opts.annotate_max,
        opts.annotate_raw,
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

fn run_compress(opts: &Opts) -> CliResult {
    let text = read_input(&opts.files).map_err(Failure::message)?;
    let outcome = compress(&text, &opts.query, &opts.config);
    let stdout = io::stdout();
    let mut out = stdout.lock();
    if !opts.stat {
        checked_write(out.write_all(outcome.text.as_bytes()))?;
        if !outcome.text.is_empty() && !outcome.text.ends_with('\n') {
            checked_write(out.write_all(b"\n"))?;
        }
    }

    let mut store_rx: Option<StoreReceiver> = None;
    if opts.store {
        store_rx = prepare_store(opts, &text, &outcome)?;
    }
    if opts.recall > 0 {
        match scx::memory::recall_cases(&opts.mem_dir, &opts.query, opts.recall, false) {
            Ok(hits) => {
                let qualified = scx::memory::l1_hits(&opts.mem_dir, hits);
                checked_write(write_mem_lines(
                    &mut out,
                    &qualified,
                    &opts.mem_dir,
                    opts.recall_detail,
                    true,
                ))?;
            }
            Err(e) => eprintln!("scx: warning: --recall skipped: {e}"),
        }
    }
    if !opts.no_stats {
        checked_write(writeln!(out, "{}", stats_line(&outcome)))?;
    }
    if opts.json {
        json_line(&outcome);
    }
    if let Some(rx) = store_rx {
        let result = rx
            .recv()
            .map_err(|_| Failure::message("memory store thread lost"))?;
        report_store(result)?;
    }
    Ok(())
}

fn prepare_store(opts: &Opts, input: &str, outcome: &Outcome) -> CliResult<Option<StoreReceiver>> {
    let fix = if let Some(path) = &opts.fix_file {
        let text =
            read_fix_file(path, scx::memory::MAX_FIX_BYTES as u64).map_err(Failure::message)?;
        let text = text.trim_end().to_string();
        if text.is_empty() {
            eprintln!("scx: note: --fix-file {path} is empty; case stored without fix");
        }
        text
    } else {
        opts.fix.clone()
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
    if opts.recall > 0 {
        let mut store_input = StoreInput::new(&opts.mem_dir, &opts.query, &outcome.text, &stats);
        store_input.source = &opts.files;
        store_input.forces = &opts.config.forces;
        store_input.fix = &fix;
        store_input.evidence_lno = &evidence_lno;
        store_input.evidence = &evidence;
        store_input.max_bytes = opts.max_bytes;
        report_store(scx::memory::store_case(store_input))?;
        return Ok(None);
    }

    let (tx, rx) = mpsc::channel();
    let mem_dir = opts.mem_dir.clone();
    let query = opts.query.clone();
    let text = outcome.text.clone();
    let files = opts.files.clone();
    let forces = opts.config.forces.clone();
    let max_bytes = opts.max_bytes;
    std::thread::spawn(move || {
        let mut store_input = StoreInput::new(&mem_dir, &query, &text, &stats);
        store_input.source = &files;
        store_input.forces = &forces;
        store_input.fix = &fix;
        store_input.evidence_lno = &evidence_lno;
        store_input.evidence = &evidence;
        store_input.max_bytes = max_bytes;
        let _ = tx.send(scx::memory::store_case(store_input));
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
}
