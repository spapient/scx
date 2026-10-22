//! CLI 记忆接线：`--store 1` 摄取与 `--recall`/`--recall-only` 旁路召回。
//!
//! 红线：记忆只做旁路（mem 段）或评分先验，绝不注入压缩正文；本模块不触碰
//! compress 引擎（N1/N2/N3）。库路径与参数已在 Opts 解析，此处只负责调用。

use scx::memory::{
    l1_params, l1_qualified, recall_cases, store_case, CaseStats, Memory, RecallHit,
};
use std::io::{self, Write};

/// 输出召回段 `mem: ...`（旁路，绝不影响正文/统计/退出码）。
/// case 只显示指纹段（`YYYY-MM-DD_<hash8>` 的 hash8），保证输出不随日期漂移。
/// `detail` 时（P4.2 --recall-detail）每条追加一行 `summary:`（error_code+file_loc+证据行）。
pub(crate) fn write_mem_lines(
    out: &mut dyn Write,
    hits: &[RecallHit],
    root: &str,
    detail: bool,
) -> io::Result<()> {
    for h in hits {
        let short = h.id.rsplit('_').next().unwrap_or(&h.id);
        writeln!(
            out,
            "mem: sim={:.2} score={:.2} case={}",
            h.sim, h.score, short
        )?;
        if detail {
            let s = scx::memory::case_summary(root, &h.id).unwrap_or_default();
            writeln!(out, "  summary: {}", s)?;
        }
    }
    Ok(())
}

/// 召回路径：先按 query 召回（线性扫描 BM25+字段加权），只输出 mem 段。
pub(crate) fn run_recall(mem_dir: &str, query: &str, k: usize, detail: bool) {
    let m = match Memory::open(mem_dir) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("scx: cannot open memory at {}: {}", mem_dir, e);
            std::process::exit(2);
        }
    };
    let hits = match recall_cases(&m.root, query, k) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("scx: memory recall failed: {}", e);
            std::process::exit(2);
        }
    };
    // P2.6：L1 自适应过滤（§5.1），与主流程同口径。
    let (base, margin) = l1_params(&m.root);
    let top1 = hits.first().map(|h| h.sim).unwrap_or(0.0);
    let top2 = hits.get(1).map(|h| h.sim).unwrap_or(0.0);
    let qualified: Vec<_> = if l1_qualified(top1, top2, base, margin, 1.0) {
        hits
    } else {
        Vec::new()
    };
    let stdout = io::stdout();
    let mut out = stdout.lock();
    if let Err(e) = write_mem_lines(&mut out, &qualified, &m.root, detail) {
        report_write_error(e);
    }
}

/// 摄取：把本次压缩产物沉淀为记忆 case（旁路，失败即退出；由调用方决定是否致命）。
pub(crate) fn store(
    mem_dir: &str,
    query: &str,
    text: &str,
    files: &[String],
    stats: &CaseStats,
    forces: &[String],
) {
    if let Err(e) = store_case(mem_dir, query, text, files.to_vec(), stats, forces.to_vec()) {
        eprintln!("scx: memory store failed: {}", e);
        std::process::exit(2);
    }
}

fn report_write_error(e: io::Error) {
    if e.kind() != io::ErrorKind::BrokenPipe {
        eprintln!("scx: cannot write output: {}", e);
        std::process::exit(2);
    }
}
