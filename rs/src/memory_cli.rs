//! CLI 记忆接线：`--recall`/`--recall-only` 旁路召回与记忆结果输出。
//!
//! 红线：记忆只做旁路（mem 段）或评分先验，绝不注入压缩正文；本模块不触碰
//! compress 引擎（N1/N2/N3）。库路径与参数已在 Opts 解析，此处只负责调用。

use scx::memory::{recall_cases, Case, RecallHit};
use std::io::{self, Write};

/// 输出召回段 `mem: ...`（旁路，绝不影响正文/统计/退出码）。
/// case 只显示指纹段（`YYYY-MM-DD_<hash16>` 的 hash16），保证输出不随日期漂移。
/// `detail` 时（P4.2 --recall-detail）每条追加一行 `summary:`（error_code+file_loc+证据行）；
/// P4.4+ 优化：case 带修复结论（--store 配 --fix）时追加 `fix:` 行——
/// **定位为"候选答案 + 证据"而非无条件事实**（P4.4+ 优化⑩：相似症状可能不同根因，
/// 复用 fix 有风险）——fix 行恒带 `(sim=..)` 相似度标注：主流程 L1 置信直答时
/// 该 sim 即直答依据；查看/候选场景下使用者据 sim 判断可复用性。
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
            writeln!(out, "  summary: {}", escape(&summary(&c)))?;
            for (i, &lno) in c.evidence_lno.iter().enumerate() {
                writeln!(
                    out,
                    "  evidence: {} {}",
                    lno,
                    escape(c.evidence.get(i).map(String::as_str).unwrap_or(""))
                )?;
            }
            let fix = c.fix.clone();
            if !fix.is_empty() {
                writeln!(out, "  fix: {} (candidate, sim={:.2})", escape(&fix), h.sim)?;
            }
        } else {
            let fix = scx::memory::read_case(root, &h.id)?.fix;
            if !fix.is_empty() {
                writeln!(out, "  fix: {} (candidate, sim={:.2})", escape(&fix), h.sim)?;
            }
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
    let line = c
        .text
        .lines()
        .map(str::trim)
        .find(|t| !t.is_empty() && is_err_line(t))
        .or_else(|| c.text.lines().map(str::trim).find(|t| !t.is_empty()));
    if let Some(line) = line {
        parts.push(line);
    }
    let s = parts.join(" | ");
    let mut out: String = s.chars().take(300).collect();
    if s.chars().count() > 300 {
        out.push('…');
    }
    out
}

/// P4.4+ 优化⑫（P1-4）：召回回放输出转义控制字符（ANSI ESC / C0），
/// 防恶意日志在终端注入转义序列。
fn escape(s: &str) -> String {
    scx::memory::escape_control(s)
}

/// 召回路径：先按 query 召回（线性扫描 BM25+字段加权），只输出 mem 段。
/// `--recall-only` 是查看模式：输出真实 top-k（不做 L1 过滤，方便调参/标注/演示）；
/// 主流程 `--recall` 的 mem 段仍走 L1 直答过滤（见 main.rs，同 §5.1 口径）。
pub(crate) fn run_recall(mem_dir: &str, query: &str, k: usize, detail: bool) -> io::Result<()> {
    // 查看模式：只读召回，零库状态写入（qmap/会话缓存/L1 历史）。
    // 也不调用初始化入口：库不存在时直接报错，避免只读查询产生目录副作用。
    let hits = recall_cases(mem_dir, query, k, true)?;
    let stdout = io::stdout();
    let mut out = stdout.lock();
    write_mem_lines(&mut out, &hits, mem_dir, detail)?;
    Ok(())
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
