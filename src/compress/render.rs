//! 压缩管线·渲染阶段（compress/render）：保留/裁剪区间落地、原因注释、
//! 输出截断与效果说明。纯函数、无 I/O；全部 `pub(crate)`。

use super::*;
use crate::{Config, Mode, Risk};

pub(crate) fn render(
    lines: &[&str],
    keep: &[bool],
    cfg: &Config,
    raw: &[bool],
    query_terms: &[String],
) -> (String, Vec<usize>) {
    let total = keep.len();
    // trim 结果预计算（并行）：trim 行间相互独立，emit 阶段按保留行顺序取用，
    // 消除 render 主循环里逐行 trim 的串行成本。
    let trimmed: Vec<Option<String>> = if cfg.trim {
        compute_trimmed(lines, keep, raw, cfg, query_terms)
    } else {
        vec![None; total]
    };
    let mut sink = Sink {
        out: String::with_capacity(lines.len() / 2),
        numbers: Vec::with_capacity(keep.len()),
    };
    let mut in_gap = false;
    for (i, &k) in keep.iter().enumerate() {
        if k {
            emit_kept(&mut sink, &mut in_gap, i, lines[i], cfg, raw, &trimmed);
        } else {
            in_gap = true;
        }
    }
    flush_gap(&mut sink, in_gap, cfg);
    // perf 诊断：输出逐行 trim 的累计分段耗时（仅 feature="perf"，默认零开销）。
    crate::perf::T::start().dump_agg("trim_line");
    (sink.out, sink.numbers)
}

/// 保留行落地：先行写省略标记（若前置非保留行段），再输出该行。
fn emit_kept(
    sink: &mut Sink,
    in_gap: &mut bool,
    i: usize,
    line: &str,
    cfg: &Config,
    raw: &[bool],
    trimmed: &[Option<String>],
) {
    if *in_gap && cfg.elide {
        sink.out.push_str("...\n");
    }
    *in_gap = false;
    let trim_line = if cfg.trim && !raw[i] {
        trimmed[i].as_deref()
    } else {
        None
    };
    emit_line(sink, i, line, cfg, trim_line);
}

/// 尾部省略标记（elide 且末尾存在被裁段）。
fn flush_gap(sink: &mut Sink, in_gap: bool, cfg: &Config) {
    if in_gap && cfg.elide {
        sink.out.push_str("...\n");
    }
}

/// 并行计算所有 trim 行的裁剪结果（`keep[i] && !raw[i]`），其余为 None。
/// 行间独立；线程各自产出 `(保留行索引, 裁剪串)`，串行回填保序。
fn compute_trimmed(
    lines: &[&str],
    keep: &[bool],
    raw: &[bool],
    cfg: &Config,
    query_terms: &[String],
) -> Vec<Option<String>> {
    let total = keep.len();
    let mut trimmed = vec![None; total];
    let idxs: Vec<usize> = (0..total).filter(|&i| keep[i] && !raw[i]).collect();
    if idxs.len() < crate::compress::plan::PAR_MIN_REF {
        for &i in &idxs {
            let mut buf = String::new();
            crate::trim::trim_line_into(
                lines[i],
                query_terms,
                cfg.trim_ratio,
                cfg.trim_op_keep,
                &mut buf,
            );
            trimmed[i] = Some(buf);
        }
        return trimmed;
    }
    let n_t = 6;
    let part = idxs.len().div_ceil(n_t);
    let rows: Vec<Vec<(usize, String)>> = std::thread::scope(|s| {
        let mut handles = Vec::with_capacity(n_t);
        for chunk in idxs.chunks(part) {
            handles.push(s.spawn(move || {
                let mut local = Vec::with_capacity(chunk.len());
                for &i in chunk {
                    let mut buf = String::new();
                    crate::trim::trim_line_into(
                        lines[i],
                        query_terms,
                        cfg.trim_ratio,
                        cfg.trim_op_keep,
                        &mut buf,
                    );
                    local.push((i, buf));
                }
                local
            }));
        }
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for row in rows {
        for (idx, s) in row {
            trimmed[idx] = Some(s);
        }
    }
    trimmed
}

/// 渲染期输出槽：`out` 主文本、`numbers` 行号。
struct Sink {
    out: String,
    numbers: Vec<usize>,
}

/// 单行落地：行号前缀、行内 trim 结果（预计算）或原行、追加换行并记录序号。
fn emit_line(sink: &mut Sink, i: usize, line: &str, cfg: &Config, trim_line: Option<&str>) {
    if cfg.lineno {
        prefix(&mut sink.out, i + 1);
    }
    match trim_line {
        Some(s) => sink.out.push_str(s),
        None => sink.out.push_str(line),
    }
    sink.out.push('\n');
    sink.numbers.push(i + 1);
}

pub(crate) fn spans(
    cfg: &Config,
    seed: &[bool],
    window: usize,
    total: usize,
    lines: &[&str],
) -> Region {
    if cfg.structure {
        let map = structures(lines);
        expand(seed, window, total, Some(&map))
    } else {
        expand(seed, window, total, None)
    }
}

#[cfg(feature = "semantic")]
pub(crate) fn semantic(
    cfg: &Config,
    lines: &[&str],
    uniq_terms: &[String],
    total: usize,
    dup: &[bool],
    scores: &mut [f64],
) {
    if let Some(w) = &cfg.semantic {
        for i in 0..total {
            if dup[i] {
                continue;
            }
            scores[i] += w.score(&crate::semantic::feats(lines[i], uniq_terms, w.dim));
        }
    }
}

#[cfg(not(feature = "semantic"))]
pub(crate) fn semantic(
    _cfg: &Config,
    _lines: &[&str],
    _uniq_terms: &[String],
    _total: usize,
    _dup: &[bool],
    _scores: &mut [f64],
) {
}

pub(crate) fn optional(
    sel: &Select,
    cfg: &Fill,
    context: i64,
    overflow: Option<Mode>,
    keep: &mut [bool],
    kept: &mut usize,
    reason: &mut [u8],
) -> f64 {
    if context == 0 || overflow.is_some() {
        return 0.0;
    }
    fill(sel, cfg, keep, kept, reason)
}

pub(crate) fn active(risk: Risk, cfg: &Config) -> bool {
    risk == Risk::High && cfg.gate
}

pub(crate) fn runs(keep: &[bool], dup: &[bool], overflow: bool) -> Vec<crate::Dropped> {
    let mut out: Vec<crate::Dropped> = Vec::with_capacity(keep.len() / 2);
    let mut start: Option<usize> = None;
    let mut cur = crate::Reason::Budget;
    for (i, &k) in keep.iter().enumerate() {
        if k {
            if let Some(s) = start.take() {
                run(&mut out, s, i, cur);
            }
            continue;
        }
        let reason = drop_reason(dup[i], overflow);
        advance(&mut out, &mut start, &mut cur, i, reason);
    }
    if let Some(s) = start {
        run(&mut out, s, keep.len(), cur);
    }
    out
}

pub(crate) fn drop_reason(dup: bool, overflow: bool) -> crate::Reason {
    if dup {
        crate::Reason::Dedup
    } else if overflow {
        crate::Reason::MaxTokens
    } else {
        crate::Reason::Budget
    }
}

pub(crate) fn advance(
    out: &mut Vec<crate::Dropped>,
    start: &mut Option<usize>,
    cur: &mut crate::Reason,
    i: usize,
    reason: crate::Reason,
) {
    match *start {
        None => {
            *start = Some(i);
            *cur = reason;
        }
        Some(s) if reason != *cur => {
            run(out, s, i, *cur);
            *start = Some(i);
            *cur = reason;
        }
        Some(_) => {}
    }
}

pub(crate) fn run(out: &mut Vec<crate::Dropped>, start: usize, end: usize, reason: crate::Reason) {
    out.push(crate::Dropped {
        start: start + 1,
        end,
        reason,
    });
}

pub(crate) fn explain(reason: &[u8], dup: &[bool]) -> Vec<crate::Explain> {
    reason
        .iter()
        .enumerate()
        .map(|(i, &r)| match r {
            R_SEED => crate::Explain::Seed,
            R_CONTEXT => crate::Explain::Context,
            R_FILL => crate::Explain::Fill,
            R_GUARD => crate::Explain::Guard,
            R_ANCHOR => crate::Explain::Anchor,
            R_NUM_LINE => crate::Explain::NumLine,
            R_SYM_LINE => crate::Explain::SymLine,
            R_ZH_LINE => crate::Explain::ZhLine,
            _ => {
                if dup[i] {
                    crate::Explain::Dedup
                } else {
                    crate::Explain::Dropped
                }
            }
        })
        .collect()
}

pub(crate) fn cut(out_text: &str, chars_a: usize) -> (usize, f64) {
    let chars_b = if out_text.ends_with('\n') {
        chars(out_text).saturating_sub(1)
    } else {
        chars(out_text)
    };
    let cut = if chars_a > 0 {
        100.0 * (1.0 - chars_b as f64 / chars_a as f64)
    } else {
        0.0
    };
    (chars_b, cut)
}
