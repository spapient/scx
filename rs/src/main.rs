// scx — CLI：主流程编排。参数解析在 cli_args，输入读取在 cli_io，
// 记忆接线在 cli_mem；筛选逻辑全在库 crate（scx::compress）。本文件只做编排。
mod cli_args;
mod cli_io;
mod cli_mem;

use cli_args::{parse_args, Opts};
use cli_io::read_input;
use cli_mem::{run_recall, store, write_mem_lines};
use scx::memory::CaseStats;
use scx::{compress, Config, Outcome};
use std::io::{self, Write};
use std::sync::mpsc;

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

fn report_write_error(e: io::Error) {
    if e.kind() != io::ErrorKind::BrokenPipe {
        eprintln!("scx: cannot write output: {}", e);
        std::process::exit(2);
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
            println!("{}", cli_args::USAGE);
            return true;
        }
        if !after_terminator && (a == "--version" || a == "-V") {
            println!("scx {}", scx::VERSION);
            return true;
        }
    }
    false
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if early_exit(&args) {
        std::process::exit(0);
    }
    let opts: Opts = parse_args(&args);
    if opts.recall_only {
        run_recall(&opts.mem_dir, &opts.query, opts.recall, opts.recall_detail);
        return;
    }
    // P4.1 评估系统：--eval <baseline.json> 冻结重放，A/B 分层对拍（独立于压缩主流程）。
    if let Some(path) = &opts.eval {
        match scx::memory::eval_cases(&opts.mem_dir, path) {
            Ok(lines) => {
                for l in lines {
                    println!("{l}");
                }
            }
            Err(e) => {
                eprintln!("scx: eval failed: {}", e);
                std::process::exit(2);
            }
        }
        return;
    }
    // P2 阶段门残余：--l1-hist 运行期 [l1] 直答命中分布出表（读 meta.toml [l1] hist）。
    if opts.l1_hist {
        match scx::memory::l1_hist_table(&opts.mem_dir) {
            Ok(lines) => {
                for l in lines {
                    println!("{l}");
                }
            }
            Err(e) => {
                eprintln!("scx: l1-hist failed: {}", e);
                std::process::exit(2);
            }
        }
        return;
    }
    // O4 反馈标注：--rate good|bad <case-id>，独立于压缩主流程。
    if let Some((good, id)) = &opts.rate {
        match scx::memory::rate_case(&opts.mem_dir, id, *good) {
            Ok(c) => {
                println!(
                    "rate: good={} bad={} quality={:.2} status={} case={}",
                    c.feedback.good, c.feedback.bad, c.quality, c.status, id
                );
                if *good {
                    let (base, _) = scx::memory::l1_params(&opts.mem_dir);
                    println!("l1_base: {base:.3} (自适应)");
                }
            }
            Err(e) => {
                eprintln!("scx: rate failed: {}", e);
                std::process::exit(2);
            }
        }
        return;
    }
    // P3.3 库维护模式：重建索引 / 过期清理 / 删除 case（均独立于压缩主流程）。
    if opts.rebuild {
        match scx::memory::rebuild(&opts.mem_dir) {
            Ok((n_docs, n_terms)) => println!("rebuild: docs={n_docs} terms={n_terms}"),
            Err(e) => {
                eprintln!("scx: rebuild failed: {}", e);
                std::process::exit(2);
            }
        }
        return;
    }
    if opts.gc {
        match scx::memory::gc(&opts.mem_dir) {
            Ok(removed) => println!("gc: removed={removed}"),
            Err(e) => {
                eprintln!("scx: gc failed: {}", e);
                std::process::exit(2);
            }
        }
        return;
    }
    if let Some(id) = &opts.delete {
        match scx::memory::delete_case(&opts.mem_dir, id) {
            Ok(true) => println!("delete: removed case={id}"),
            Ok(false) => {
                eprintln!("scx: no such case: {id}");
                std::process::exit(2);
            }
            Err(e) => {
                eprintln!("scx: delete failed: {}", e);
                std::process::exit(2);
            }
        }
        return;
    }
    let text = match read_input(&opts.files) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("scx: {}", e);
            std::process::exit(2);
        }
    };
    let cfg = Config {
        ratio: opts.ratio,
        context: opts.context,
        window: opts.window,
        elide: opts.elide,
        risk_gate: opts.risk_gate,
        lineno: opts.lineno,
        forces: opts.forces.clone(),
    };
    let o = compress(&text, &opts.query, &cfg);
    let stdout = io::stdout();
    let mut out = stdout.lock();
    if !opts.stat {
        if let Err(e) = out.write_all(o.text.as_bytes()) {
            report_write_error(e);
            return;
        }
        if !o.text.is_empty() && !o.text.ends_with('\n') {
            if let Err(e) = out.write_all(b"\n") {
                report_write_error(e);
                return;
            }
        }
    }
    // P3.4 写后台化：--store 时摄取入队后台线程（std::thread + mpsc，零依赖），
    // 正文先回显、进程退出前 join 落盘确认；与 --recall 组合时保持同步
    // （recall 需看到新 case，顺序依赖）。
    let mut store_rx: Option<mpsc::Receiver<std::io::Result<()>>> = None;
    if opts.store {
        let stats = CaseStats {
            kept: o.kept,
            total: o.total,
            cut_pct: o.cut_pct,
            risk: o.risk.to_string(),
        };
        if opts.recall > 0 {
            store(
                &opts.mem_dir,
                &opts.query,
                &o.text,
                &opts.files,
                &stats,
                &opts.forces,
            );
        } else {
            let (tx, rx) = mpsc::channel();
            let mem_dir = opts.mem_dir.clone();
            let query = opts.query.clone();
            let text = o.text.clone();
            let files = opts.files.clone();
            let forces = opts.forces.clone();
            std::thread::spawn(move || {
                let r = scx::memory::store_case(&mem_dir, &query, &text, files, &stats, forces);
                let _ = tx.send(r.map(|_| ()));
            });
            store_rx = Some(rx);
        }
    }
    // 召回：--recall K 时把本地记忆命中段追加到 stdout（旁路，L1 自适应置信过滤）。
    // 主流程是"学习路径"：召回回流 qmap/缓存/L1 历史（read_only=false）。
    if opts.recall > 0 {
        if let Ok(hits) = scx::memory::recall_cases(&opts.mem_dir, &opts.query, opts.recall, false)
        {
            let (base, margin) = scx::memory::l1_params(&opts.mem_dir);
            let top1 = hits.first().map(|h| h.sim).unwrap_or(0.0);
            let top2 = hits.get(1).map(|h| h.sim).unwrap_or(0.0);
            // §5.1：sim_top1 ≥ l1_base 且 (top1−top2) ≥ margin 才输出 mem 段。
            let qualified: Vec<_> = if scx::memory::l1_qualified(top1, top2, base, margin, 1.0) {
                hits.into_iter().collect()
            } else {
                Vec::new()
            };
            if let Err(e) = write_mem_lines(&mut out, &qualified, &opts.mem_dir, opts.recall_detail)
            {
                report_write_error(e);
                return;
            }
        }
    }
    if !opts.no_stats {
        if let Err(e) = writeln!(out, "{}", stats_line(&o)) {
            report_write_error(e);
            return;
        }
    }
    if opts.json {
        json_line(&o);
    }
    // 后台摄取落盘确认（失败不阻塞正文/统计输出，进程退出前报错退出）。
    if let Some(rx) = store_rx {
        match rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                eprintln!("scx: memory store failed: {}", e);
                std::process::exit(2);
            }
            Err(_) => {
                eprintln!("scx: memory store thread lost");
                std::process::exit(2);
            }
        }
    }
}
