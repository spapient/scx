// scx CLI：参数解析在 args，输入读取在 input，记忆接线在 memory_cli；
// 筛选逻辑全在库 crate（scx::compress）。本文件只做命令编排与输出。
mod args;
mod input;
mod memory_cli;

use args::{parse_args, Opts};
use input::{read_fix_file, read_input};
use memory_cli::{run_recall, write_mem_lines};
use scx::memory::{CaseStats, EvalMode, StoreInput};
use scx::{compress, Outcome};
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

fn stats_line(r: &Outcome) -> String {
    format!(
        "compressed: kept={}/{} lines chars={}->{} ({:.1}% cut) dup={} seeds={} evidence={} risk={} guarded={} regions={} window={} terms={} forced={} mode={}",
        r.kept, r.total, r.chars_in, r.chars_out, r.cut_pct, r.dup, r.seeds, r.evidence, r.risk, r.guarded, r.regions, r.window, r.terms, r.forced, r.mode
    )
}

fn json_line(r: &Outcome) {
    eprintln!(
        "scxjson: {{\"kept\":{},\"total\":{},\"chars_in\":{},\"chars_out\":{},\"cut_pct\":{:.1},\"dup\":{},\"seeds\":{},\"evidence\":{},\"risk\":\"{}\",\"guarded\":{},\"regions\":{},\"window\":{},\"terms\":{},\"forced\":{},\"mode\":\"{}\"}}",
        r.kept, r.total, r.chars_in, r.chars_out, r.cut_pct, r.dup, r.seeds, r.evidence, r.risk, r.guarded, r.regions, r.window, r.terms, r.forced, r.mode
    );
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
    if outcome.mode.starts_with("failopen") {
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
    let norm = input.replace("\r\n", "\n");
    let mut lines: Vec<&str> = norm.split('\n').collect();
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
