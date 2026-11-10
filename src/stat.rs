//! 统一统计层：机器契约 `scx-stat-1` 与人读卡片的单一来源。
//!
//! CLI / MCP / hook 三处的统计都从 `Outcome` 构造同一个 `Stat`：
//! - `Stat::to_json` 输出版本化机器字段，恒等路径填恒等值，不省略；
//! - `Stat::card` 输出对齐的人读块，样式只在此处定义。
//!
//! 纯函数、无 I/O、零第三方 crate。

use crate::{Mode, Outcome, Risk};

/// token 缩减率（%）：输入估算为 0 时视为 0（恒等路径恒为 0）。
pub(crate) fn token_cut(tin: f64, tout: f64) -> f64 {
    if tin <= 0.0 {
        0.0
    } else {
        (tin - tout) * 100.0 / tin
    }
}

/// 千分位格式化（u64，如 3318 -> "3,318"）。
pub(crate) fn sep(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// 发生筛选的模式族。
fn sieve_family(m: Mode) -> bool {
    matches!(m, Mode::Sieve | Mode::OverflowEvidence | Mode::OverflowFill)
}

/// 模式对应的稳定原因码（机器字段，不随 UI 文案变化）。
fn reason_code(m: Mode) -> &'static str {
    match m {
        Mode::Sieve => "sieve",
        Mode::PassSmall => "small-input",
        Mode::PassLongline => "long-line",
        Mode::PassTooManyLines => "too-many-lines",
        Mode::PassNoKept => "no-kept",
        Mode::FailOpenZeroSignal => "zero-signal",
        Mode::OverflowEvidence => "overflow-evidence",
        Mode::OverflowFill => "overflow-fill",
    }
}

/// 统一统计模型。
pub(crate) struct Stat {
    decision: &'static str,
    mode: Mode,
    risk: Risk,
    lines_in: usize,
    lines_out: usize,
    chars_in: usize,
    chars_out: usize,
    tokens_in: f64,
    tokens_out: f64,
    delta_lines: usize,
    delta_chars: usize,
    delta_tokens: f64,
    cut_pct: f64,
    ev_total: usize,
    ev_retention: f64,
    ser_b: usize,
    reason: &'static str,
    rewritten: bool,
}

impl Stat {
    /// 从结果构造统计：始终按最终文本重估 token，保证与 CLI 机器 JSON 同口径。
    pub(crate) fn from_outcome(r: &Outcome) -> Stat {
        let sieve = sieve_family(r.mode);
        // 统一口径：token 一律按最终文本重估，避免筛选期中间值与重写后文本漂移。
        let tokens_out = crate::terms::estimate(&r.text);
        let lines_out = if sieve { r.kept } else { r.total };
        let chars_out = if sieve { r.output } else { r.input };
        Stat {
            decision: if sieve { "compress" } else { "passthrough" },
            mode: r.mode,
            risk: r.risk,
            lines_in: r.total,
            lines_out,
            chars_in: r.input,
            chars_out,
            tokens_in: r.tokens_in,
            tokens_out,
            delta_lines: r.total.saturating_sub(lines_out),
            delta_chars: r.input.saturating_sub(chars_out),
            delta_tokens: (r.tokens_in - tokens_out).max(0.0),
            cut_pct: token_cut(r.tokens_in, tokens_out),
            ev_total: r.ev_total,
            ev_retention: r.ev_retention,
            ser_b: r.text.len(),
            reason: reason_code(r.mode),
            rewritten: r.rewritten,
        }
    }

    /// 机器契约：`scx-stat-1` 全字段 JSON。
    pub(crate) fn to_json(&self) -> String {
        format!(
            "{{\"schema\":\"scx-stat-1\",\"decision\":\"{}\",\"mode\":\"{}\",\"risk\":\"{}\",\
\"lines_in\":{},\"lines_out\":{},\"chars_in\":{},\"chars_out\":{},\
\"tokens_in\":{:.4},\"tokens_out\":{:.4},\"delta_lines\":{},\"delta_chars\":{},\
\"delta_tokens\":{:.4},\"cut_pct\":{:.4},\"ev_total\":{},\"ev_retention\":{:.4},\
\"ser_b\":{},\"reason\":\"{}\",\"rewritten\":{}}}",
            self.decision,
            self.mode.as_str(),
            self.risk.as_str(),
            self.lines_in,
            self.lines_out,
            self.chars_in,
            self.chars_out,
            self.tokens_in,
            self.tokens_out,
            self.delta_lines,
            self.delta_chars,
            self.delta_tokens,
            self.cut_pct,
            self.ev_total,
            self.ev_retention,
            self.ser_b,
            self.reason,
            self.rewritten
        )
    }

    /// 人读卡片：样式与栅格的唯一来源。
    pub(crate) fn card(&self, r: &Outcome, query: &str) -> String {
        let mut s = String::from("✦ SCX 压缩召回 ✦\n\n");
        s.push_str(&row("📌", "查询", &clip(query, 40)));
        let (label, desc) = title(self.mode);
        s.push_str(&row("⚙️", "模式", &format!("{label} · {desc}")));
        if self.rewritten {
            s.push_str(&row(
                "✍️",
                "改写",
                "保留行已按 --trim/--fold 重写或折叠，非逐字原文",
            ));
        }
        if sieve_family(self.mode) {
            s.push_str(&row(
                "📥",
                "原始",
                &format!(
                    "{} 行 · {} 字符 · 约 {} token",
                    sep(self.lines_in as u64),
                    sep(self.chars_in as u64),
                    sep(self.tokens_in as u64)
                ),
            ));
            s.push_str(&row(
                "📤",
                "压缩后",
                &format!(
                    "{} 行 · {} 字符 · 约 {} token",
                    sep(self.lines_out as u64),
                    sep(self.chars_out as u64),
                    sep(self.tokens_out as u64)
                ),
            ));
            s.push_str(&row(
                "💾",
                "省下",
                &format!(
                    "{} 行 · {} 字符 · 约 {} token（-{:.1}%）",
                    sep(self.delta_lines as u64),
                    sep(self.delta_chars as u64),
                    sep(self.delta_tokens as u64),
                    self.cut_pct
                ),
            ));
            if self.ev_total > 0 {
                let ev_kept = (self.ev_retention * self.ev_total as f64).round() as usize;
                let ev_loss = self.ev_total.saturating_sub(ev_kept);
                s.push_str(&row(
                    "🎯",
                    "证据召回",
                    &format!("{} 条中 {ev_kept} 条保留 · {ev_loss} 丢失", self.ev_total),
                ));
            } else {
                s.push_str(&row("🎯", "证据召回", "未声明证据"));
            }
            s.push_str(&row("🔁", "去重", &format!("{} 行重复内容已折叠", r.dup)));
            s.push_str(&row(
                "✂️",
                "裁剪",
                &format!("{} 段无关内容已移除", r.dropped.len()),
            ));
            s.push_str(&row("🛡️", "风险", &guard_note(self.risk, r.guarded)));
        } else {
            s.push_str(&row(
                "📥",
                "原始",
                &format!(
                    "{} 行 · {} 字符 · 约 {} token",
                    sep(self.lines_in as u64),
                    sep(self.chars_in as u64),
                    sep(self.tokens_in as u64)
                ),
            ));
            s.push_str(&row(
                "📤",
                "压缩后",
                &format!(
                    "原样保留 · {} 行 · 约 {} token",
                    sep(self.lines_out as u64),
                    sep(self.tokens_out as u64)
                ),
            ));
            s.push_str(&row("💾", "省下", "0 —— 一字未动"));
            s.push_str(&row("💡", "说明", &why(self.mode)));
        }
        s
    }
}

/// 长文本截断（按字符数，超长加省略号）。
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}…")
    }
}

/// 字段名补宽到 8 列（CJK 按 2 列），保证卡片各行左对齐。
fn pad_field(field: &str) -> String {
    let w = field
        .chars()
        .map(|c| if crate::terms::full(c) { 2 } else { 1 })
        .sum::<usize>();
    format!("{field}{}", " ".repeat(8usize.saturating_sub(w)))
}

/// 卡片行：`{icon} {field:8}  {value}`。
fn row(icon: &str, field: &str, value: &str) -> String {
    format!("{icon} {}  {value}\n", pad_field(field))
}

fn risk_zh(r: Risk) -> &'static str {
    match r {
        Risk::Low => "低",
        Risk::Medium => "中",
        Risk::High => "高",
    }
}

/// 风险行文案：`guarded` 是 0/1 标志位，不是被守卫行数。
fn guard_note(risk: Risk, guarded: u8) -> String {
    let risk = risk_zh(risk);
    if guarded > 0 {
        format!("{risk} · 已启用额外守护")
    } else {
        risk.to_string()
    }
}

/// 模式中文标签与一句话说明。
fn title(m: Mode) -> (&'static str, &'static str) {
    match m {
        Mode::Sieve => ("sieve 智能筛选", "按问题逐行召回证据"),
        Mode::PassSmall => ("小输入透传", "无需压缩"),
        Mode::PassLongline => ("超长行透传", "单行过长，原样保留"),
        Mode::PassTooManyLines => ("行数超限透传", "行数过多，原样保留避免内存放大"),
        Mode::PassNoKept => ("预算过紧回退", "未选出有效行，原文保留"),
        Mode::FailOpenZeroSignal => ("无命中保护", "零丢失"),
        Mode::OverflowEvidence => ("证据优先保底", "超过 token 上限，证据全保留"),
        Mode::OverflowFill => ("上限裁剪", "超上限，保留证据并裁剪可选内容"),
    }
}

/// 恒等路径的原因说明。
fn why(m: Mode) -> String {
    match m {
        Mode::PassSmall => format!(
            "内容很少（<{} 行且不足 {} token），压缩反而可能丢信息，自动跳过",
            crate::policy::SMALL_LINES,
            crate::policy::SMALL_TOK as u64
        ),
        Mode::PassLongline => "单行过长无法安全压缩，原样返回，不冒险删减".to_string(),
        Mode::PassTooManyLines => format!(
            "行数超过 {}，筛选会按行数放大内存，原样返回避免内存耗尽",
            crate::policy::MAX_LINES
        ),
        Mode::PassNoKept => "预算过紧未选出有效行，宁可原样返回也不静默删减".to_string(),
        Mode::FailOpenZeroSignal => {
            "没找到与问题相关的证据行，压缩会误删信息，完整返回".to_string()
        }
        _ => "当前模式无需压缩，原样返回".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_stat_fills_equal_values() {
        let r = crate::compress("hi\nthere", "zzz_no_such_term", &crate::Config::default());
        let stat = Stat::from_outcome(&r);
        assert_eq!(stat.decision, "passthrough");
        assert_eq!(stat.lines_in, stat.lines_out);
        assert_eq!(stat.chars_in, stat.chars_out);
        assert_eq!(stat.delta_lines, 0);
        assert_eq!(stat.cut_pct, 0.0);
    }

    #[test]
    fn json_contains_version_and_reason() {
        let r = crate::compress("hello", "x", &crate::Config::default());
        let j = Stat::from_outcome(&r).to_json();
        assert!(j.contains("\"schema\":\"scx-stat-1\""));
        assert!(j.contains("\"reason\":\"small-input\""));
        assert!(j.contains("\"tokens_out\":"));
    }

    #[test]
    fn card_uses_one_render_style() {
        let r = crate::compress("hello", "x", &crate::Config::default());
        let card = Stat::from_outcome(&r).card(&r, "x");
        assert!(card.contains("✦ SCX 压缩召回 ✦"));
        assert!(card.contains("一字未动"));
    }

    #[test]
    fn token_cut_is_safe_and_exact() {
        assert_eq!(token_cut(0.0, 999.0), 0.0);
        assert!((token_cut(100.0, 50.0) - 50.0).abs() < 1e-9);
        assert!((token_cut(200.0, 180.0) - 10.0).abs() < 1e-9);
    }
}
