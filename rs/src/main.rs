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
    // P4.4+ 优化④：--eval-verbose 时追加逐 case A/B top-10 候选诊断（服务 M4 标注校准）。
    if let Some(path) = &opts.eval {
        let res = if opts.eval_verbose {
            scx::memory::eval_cases_verbose(&opts.mem_dir, path)
        } else {
            scx::memory::eval_cases(&opts.mem_dir, path)
        };
        match res {
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
    // P4.4+ 优化：--stats 库盘点（case/桶/状态/fix 覆盖率/质量/磁盘，豁免 --query）。
    if opts.stats {
        match scx::memory::stats_table(&opts.mem_dir) {
            Ok(lines) => {
                for l in lines {
                    println!("{l}");
                }
            }
            Err(e) => {
                eprintln!("scx: stats failed: {}", e);
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
    // P4.4+ 优化⑥：智能标注器（--annotate，豁免 --query）——生成 M4 证据门标注集。
    // P4.4+ 优化⑧：--annotate-raw 盲测模式——不过自检的全变体集（人工裁决用，选择偏差消除）。
    if opts.annotate || opts.annotate_raw {
        let report = match if opts.annotate_raw {
            scx::memory::annotate_raw_report(&opts.mem_dir, opts.seed, opts.annotate_max)
        } else {
            scx::memory::annotate_report(&opts.mem_dir, opts.seed, opts.annotate_max)
        } {
            Ok(c) => c,
            Err(e) => {
                eprintln!("scx: annotate failed: {}", e);
                std::process::exit(2);
            }
        };
        let cases = &report.cases;
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
        let bytes = {
            let mut buf: Vec<u8> = Vec::new();
            if let Err(e) = scx::memory::annotate::write_baseline(&mut buf, cases) {
                eprintln!("scx: annotate serialize failed: {}", e);
                std::process::exit(2);
            }
            buf
        };
        if let Err(e) = std::fs::write(&path, bytes) {
            eprintln!("scx: annotate write {} failed: {}", path, e);
            std::process::exit(2);
        }
        for line in scx::memory::annotate::stats_report(&report) {
            println!("{line}");
        }
        let out_name = std::path::Path::new(&path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        println!("  out: {out_name}");
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
    let mut store_rx: Option<mpsc::Receiver<std::io::Result<(bool, String)>>> = None;
    if opts.store {
        // P4.4+ 优化③：query 零命中（failopen）时压缩结果=全文，store 会持久化
        // 低信号记忆——stderr 警告（不拒绝：用户可能刻意沉淀原文作底料）。
        if o.mode.starts_with("failopen") {
            eprintln!(
                "scx: note: query had no hits; --store will persist the full input \
(low-signal memory). inspect with --recall-only first"
            );
        }
        let stats = CaseStats {
            kept: o.kept,
            total: o.total,
            cut_pct: o.cut_pct,
            risk: o.risk.to_string(),
        };
        // P4.4+ 优化⑤：证据行区间 + 原始证据（评审缺口 B）——按 kept_lno 从输入提取
        // 保留行原文（与 compress 内部同一归一化口径：\r\n→\n、末行空行剔除）。
        // failopen/passthrough 的 kept_lno 为空 → evidence 为空（全文仅存 text 快照，
        // 低信号记忆语义不变）。
        let evidence_lno = o.kept_lno.clone();
        let evidence = extract_evidence(&text, &evidence_lno);
        if opts.recall > 0 {
            store(
                &opts.mem_dir,
                &opts.query,
                &o.text,
                &opts.files,
                &stats,
                &opts.forces,
                &opts.fix,
                &evidence_lno,
                &evidence,
            );
        } else {
            let (tx, rx) = mpsc::channel();
            let mem_dir = opts.mem_dir.clone();
            let query = opts.query.clone();
            let text = o.text.clone();
            let files = opts.files.clone();
            let forces = opts.forces.clone();
            let fix = opts.fix.clone();
            let ev_lno = evidence_lno.clone();
            let ev = evidence.clone();
            std::thread::spawn(move || {
                // P4.4+ 优化：--fix 非空走带修复结论的摄取（L1 本地直答）。
                let r = if ev_lno.is_empty() {
                    if fix.is_empty() {
                        scx::memory::store_case(&mem_dir, &query, &text, files, &stats, forces)
                    } else {
                        scx::memory::store_case_fix(
                            &mem_dir, &query, &text, files, &stats, forces, &fix,
                        )
                    }
                } else {
                    scx::memory::store_case_full(
                        &mem_dir, &query, &text, files, &stats, forces, &fix, &ev_lno, &ev,
                    )
                };
                let _ = tx.send(r);
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
    // P4.4+ 优化③：成功时 stderr 报告 stored 行（完整 case id，可立即 --rate/--delete）。
    if let Some(rx) = store_rx {
        match rx.recv() {
            Ok(Ok((added, id))) => {
                eprintln!(
                    "scx: stored case={} added={}",
                    id,
                    if added { "yes" } else { "dedup" }
                );
            }
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

/// P4.4+ 优化⑤：按保留行原始行号（1-based）从输入提取证据行原文（评审缺口 B）。
/// 与 compress 内部同一归一化口径（\r\n→\n、末行空行剔除），保证 evidence 行与
/// 压缩保留行逐字一致；行号越界（异常输入）跳过该行，不 panic。
fn extract_evidence(input: &str, lno: &[usize]) -> Vec<String> {
    if lno.is_empty() {
        return Vec::new();
    }
    let norm = input.replace("\r\n", "\n");
    let mut lines: Vec<&str> = norm.split('\n').collect();
    if lines.last().is_some_and(|l| l.is_empty()) {
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
