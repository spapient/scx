//! CLI 记忆接线：`--store 1` 摄取与 `--recall`/`--recall-only` 旁路召回。
//!
//! 红线：记忆只做旁路（mem 段）或评分先验，绝不注入压缩正文；本模块不触碰
//! compress 引擎（N1/N2/N3）。库路径与参数已在 Opts 解析，此处只负责调用。

use scx::memory::{recall_cases, store_case, CaseStats, Memory, RecallHit};
use std::io::{self, Write};

/// 输出召回段 `mem: ...`（旁路，绝不影响正文/统计/退出码）。
/// case 只显示指纹段（`YYYY-MM-DD_<hash16>` 的 hash16），保证输出不随日期漂移。
/// `detail` 时（P4.2 --recall-detail）每条追加一行 `summary:`（error_code+file_loc+证据行）；
/// P4.4+ 优化：case 带修复结论（--store 配 --fix）时**无条件**追加 `fix:` 行——
/// L1 本地直答的"上次怎么解决"在主流程/查看模式均直接可见（不依赖 --recall-detail）。
/// P4.4+ 优化⑤：case 带原始证据（evidence_lno/evidence）时 detail 追加 `  evidence: <lno> <原文>` 行
/// （证据行区间 + 原始证据回放，旧 case 无此键则跳过）。
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
            let c = scx::memory::read_case(root, &h.id)?;
            writeln!(
                out,
                "  summary: {}",
                scx::memory::case_summary(root, &h.id)?
            )?;
            for (i, &lno) in c.evidence_lno.iter().enumerate() {
                writeln!(
                    out,
                    "  evidence: {} {}",
                    lno,
                    c.evidence.get(i).map(String::as_str).unwrap_or("")
                )?;
            }
            let fix = c.fix.clone();
            if !fix.is_empty() {
                writeln!(out, "  fix: {}", fix)?;
            }
        } else {
            let fix = scx::memory::case_fix(root, &h.id).unwrap_or_default();
            if !fix.is_empty() {
                writeln!(out, "  fix: {}", fix)?;
            }
        }
    }
    Ok(())
}

/// 召回路径：先按 query 召回（线性扫描 BM25+字段加权），只输出 mem 段。
/// `--recall-only` 是查看模式：输出真实 top-k（不做 L1 过滤，方便调参/标注/演示）；
/// 主流程 `--recall` 的 mem 段仍走 L1 直答过滤（见 main.rs，同 §5.1 口径）。
pub(crate) fn run_recall(mem_dir: &str, query: &str, k: usize, detail: bool) {
    let m = match Memory::open(mem_dir) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("scx: cannot open memory at {}: {}", mem_dir, e);
            std::process::exit(2);
        }
    };
    // 查看模式：只读召回，零库状态写入（qmap/会话缓存/L1 历史）。
    let hits = match recall_cases(&m.root, query, k, true) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("scx: memory recall failed: {}", e);
            std::process::exit(2);
        }
    };
    let stdout = io::stdout();
    let mut out = stdout.lock();
    if let Err(e) = write_mem_lines(&mut out, &hits, &m.root, detail) {
        report_write_error(e);
    }
}

/// 摄取：把本次压缩产物沉淀为记忆 case（旁路，失败即退出；由调用方决定是否致命）。
/// `fix` 为修复结论（--fix，可空）：非空时随 case 入库，recall-detail 直答输出。
/// `evidence_lno`/`evidence`（P4.4+ 优化⑤）：原始证据行区间+原文，空数组等价旧行为。
#[allow(clippy::too_many_arguments)]
pub(crate) fn store(
    mem_dir: &str,
    query: &str,
    text: &str,
    files: &[String],
    stats: &CaseStats,
    forces: &[String],
    fix: &str,
    evidence_lno: &[usize],
    evidence: &[String],
) {
    let r = if evidence_lno.is_empty() {
        if fix.is_empty() {
            store_case(mem_dir, query, text, files.to_vec(), stats, forces.to_vec())
        } else {
            scx::memory::store_case_fix(
                mem_dir,
                query,
                text,
                files.to_vec(),
                stats,
                forces.to_vec(),
                fix,
            )
        }
    } else {
        scx::memory::store_case_full(
            mem_dir,
            query,
            text,
            files.to_vec(),
            stats,
            forces.to_vec(),
            fix,
            evidence_lno,
            evidence,
        )
    };
    match r {
        Ok((added, id)) => {
            // P4.4+ 优化③：存储结果报告（stderr 旁路，不碰正文）——用户拿到完整
            // case id 可直接 --rate/--delete 闭环；added=false 为指纹去重（刷新时间戳）。
            eprintln!(
                "scx: stored case={} added={}",
                id,
                if added { "yes" } else { "dedup" }
            );
        }
        Err(e) => {
            eprintln!("scx: memory store failed: {}", e);
            std::process::exit(2);
        }
    }
}

fn report_write_error(e: io::Error) {
    if e.kind() != io::ErrorKind::BrokenPipe {
        eprintln!("scx: cannot write output: {}", e);
        std::process::exit(2);
    }
}
