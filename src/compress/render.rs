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
    let mut out_text = String::with_capacity(lines.len() / 2);
    let mut numbers = Vec::with_capacity(keep.len());
    let mut in_gap = false;
    for (i, &k) in keep.iter().enumerate() {
        if k {
            if in_gap && cfg.elide {
                out_text.push_str("...\n");
            }
            in_gap = false;
            emit_line(
                &mut out_text,
                &mut numbers,
                i,
                lines[i],
                cfg,
                raw,
                query_terms,
            );
        } else {
            in_gap = true;
        }
    }
    if in_gap && cfg.elide {
        out_text.push_str("...\n");
    }
    (out_text, numbers)
}

/// 单行落地：行号前缀、行内 trim（raw 行跳过）、追加换行并记录序号。
fn emit_line(
    out: &mut String,
    numbers: &mut Vec<usize>,
    i: usize,
    line: &str,
    cfg: &Config,
    raw: &[bool],
    query_terms: &[String],
) {
    if cfg.lineno {
        prefix(out, i + 1);
    }
    let text = if cfg.trim && !raw[i] {
        crate::trim::trim_line(line, query_terms, cfg.trim_ratio, cfg.trim_op_keep)
    } else {
        line.to_string()
    };
    out.push_str(&text);
    out.push('\n');
    numbers.push(i + 1);
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
