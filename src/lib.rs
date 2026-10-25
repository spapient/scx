#![forbid(unsafe_code)]

//! scx 引擎：本地查询感知证据筛选压缩（纯函数、无 I/O、零依赖、确定性）。
//!
//! 语义（v0.4.1）：种子不变量（证据行与 ±context（默认 1，可设 0）最小邻域无条件保留，`ratio` 只删可选）；证据仅
//! 来自 IDF 加权查询命中与 `forces`；种子区域门控（可选预算只填种子邻域区域：±window
//! 扩展，间隔 <= window*3 合并；种子为零回退全图）；邻域权重只排序；context=0 完全跳过可选填充（针尖模式）；无信号 fail-open；
//! `risk_gate` 在风险 HIGH 时保护失败级错误行(warn/警告 级除外)与事故标记行(root cause / fix hint / mitigation / recommendation / action / assertionerror)。只删不改、保留行逐字原文；省略以单行 "..." 标记；`lineno` 可为保留行附加原文行号前缀。
//!
//! 模块划分：`terms` 负责词面与估算，`analysis` 负责归一化与结构分类，
//! `compress` 是算法主体。三者均为纯函数层；本文件只保留公共类型、模块声明和
//! 跨模块行为测试。
//!
//! 压缩模式与风险等级以 `Mode` / `Risk` 枚举表达（`Display` 输出保持 v0.4.1
//! 字符串格式：`passthrough:small` / `failopen:zero-signal` / `LOW` / `HIGH` 等）。

use std::fmt;
mod analysis;
mod compress;
mod conf;
pub mod contain;
mod policy;
mod terms;

/// 记忆知识库（旁路，默认不启用）：I/O 侧模块，与纯函数 `compress` 完全隔离。
pub mod memory;

pub use compress::compress;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 引擎配置。`ratio` 为可选内容预算占输入 token 估算的比例；入口会做边界规范化。
pub struct Config {
    pub ratio: f64,
    pub context: i64,
    pub window: i64,
    pub elide: bool,
    pub risk_gate: bool,
    /// 保留行输出时附加原文行号前缀（grep -n 风格 `N: `）；passthrough/fail-open 返回原文时不附加。
    pub lineno: bool,
    /// `--force` 值按 `|` 拆分后的原始子串（大小写不敏感字面匹配）。
    pub forces: Vec<String>,
    /// 绝对 token 硬上限（估算口径，非真实 tokenizer；`None` = 现状行为不变）。
    /// 证据地板最高优先：证据行与最小邻域无条件保留，超限时输出
    /// `mode=overflow:evidence` / `overflow:fill` 并如实报告，绝不静默丢弃证据。
    pub max_tokens: Option<usize>,
}

impl Config {
    pub const DEFAULT_RATIO: f64 = 0.35;
    pub const MIN_RATIO: f64 = 0.02;
    pub const DEFAULT_CONTEXT: i64 = 1;
    pub const MAX_CONTEXT: i64 = 4;
    pub const DEFAULT_WINDOW: i64 = 2;
    pub const MIN_WINDOW: i64 = 1;
    pub const MAX_WINDOW: i64 = 8;
}

impl Default for Config {
    fn default() -> Self {
        Config {
            ratio: Self::DEFAULT_RATIO,
            context: Self::DEFAULT_CONTEXT,
            window: Self::DEFAULT_WINDOW,
            elide: true,
            risk_gate: true,
            lineno: false,
            forces: Vec::new(),
            max_tokens: None,
        }
    }
}

/// 压缩模式。`as_str` / `Display` 输出与 v0.4.1 统计行字符串逐字一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 小输入直通（< `policy::SMALL_LINES` 行且 token 估算 < `policy::SMALL_TOK`）。
    PassSmall,
    /// 超长行直通（单行字节与字符均超过 `policy::LONGLINE_*`）。
    PassLongline,
    /// 无查询信号 fail-open（零命中且无 `--force` 命中），返回输入原文。
    FailOpenZeroSignal,
    /// 预算过紧未选出任何行，回退原文。
    PassNoKept,
    /// 正常筛选（sieve）。
    Sieve,
    /// 证据地板（证据行 + 最小邻域）已超 `--max-tokens` 上限；证据仍全保留，
    /// 如实报告（绝不静默丢弃证据）。
    OverflowEvidence,
    /// 削减可选填充后输出仍超 `--max-tokens` 上限（如风险守卫追加行）；
    /// 如实报告，不丢弃已保留的证据性内容。
    OverflowFill,
}

impl Mode {
    /// v0.4.1 统计行字符串（向后兼容；新增变体须保持冒号分段格式）。
    pub const fn as_str(self) -> &'static str {
        match self {
            Mode::PassSmall => "passthrough:small",
            Mode::PassLongline => "passthrough:longline",
            Mode::FailOpenZeroSignal => "failopen:zero-signal",
            Mode::PassNoKept => "passthrough:no-kept",
            Mode::Sieve => "sieve",
            Mode::OverflowEvidence => "overflow:evidence",
            Mode::OverflowFill => "overflow:fill",
        }
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 压缩风险等级（风险审计输出）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    Low,
    Medium,
    High,
}

impl Risk {
    /// v0.4.1 统计行字符串（`LOW` / `MED` / `HIGH`）。
    pub const fn as_str(self) -> &'static str {
        match self {
            Risk::Low => "LOW",
            Risk::Medium => "MED",
            Risk::High => "HIGH",
        }
    }
}

impl fmt::Display for Risk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 压缩结果与统计。`seeds == evidence` 恒成立（结构性不变量，可断言）。
pub struct Outcome {
    /// 压缩后的文本；passthrough / fail-open 时为输入原文。
    pub text: String,
    /// 保留行的原始输入行号（1-based，按原顺序；仅筛选模式有值，
    /// passthrough/fail-open 为空）。记忆库 `--store` 据此从输入提取原始证据行，
    /// L1 直答可回放原文。
    pub kept_lno: Vec<usize>,
    pub kept: usize,
    pub total: usize,
    pub chars_in: usize,
    pub chars_out: usize,
    pub cut_pct: f64,
    pub dup: usize,
    pub seeds: usize,
    pub evidence: usize,
    pub risk: Risk,
    pub guarded: u8,
    pub regions: usize,
    pub window: i64,
    pub terms: usize,
    pub forced: usize,
    pub mode: Mode,
    /// 生效的 `--max-tokens` 上限（`None` = 未启用）。
    pub max_tokens: Option<usize>,
    /// 输出文本的 token 估算（`terms::token_est` 口径）；passthrough/fail-open 不保证。
    pub tok_est: f64,
}

// 零分配 u64→十进制行号写入（`N: ` 前缀，grep -n 风格；供 --lineno 使用）。
pub(crate) fn push_lineno(out: &mut String, n: usize) {
    let mut buf = [0u8; 20];
    let mut m = n;
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (m % 10) as u8;
        m /= 10;
        if m == 0 {
            break;
        }
    }
    out.push_str(std::str::from_utf8(&buf[i..]).unwrap());
    out.push_str(": ");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{is_diffline, is_frame, is_incident_marker, is_lineref};
    use crate::analysis::{norm_into, push_analysis_lower};
    use crate::terms::build_terms;

    fn cfg() -> Config {
        Config {
            ratio: 0.35,
            ..Config::default()
        }
    }
    fn filler() -> String {
        "// filler line with enough tokens to pass the small guard".to_string()
    }

    #[test]
    fn invariant_seeds_kept() {
        let mut lines: Vec<String> = (0..200).map(|_| filler()).collect();
        lines[100] = "fn target() { return 1; }".to_string();
        let text = lines.join("\n");
        let o = compress(&text, "target", &cfg());
        assert_eq!(o.seeds, o.evidence);
        assert!(o.seeds >= 1);
        assert!(o.text.contains("fn target"));
    }

    #[test]
    fn sieve_kept_lno_aligns_with_output_lines() {
        // 保留行原始行号（1-based）与输出行一一对应、严格递增。
        let mut lines: Vec<String> = (0..200)
            .map(|i| format!("pad alpha beta line {}", i))
            .collect();
        lines[50] = "ERROR db timeout conn_id=9f2a".to_string();
        lines[51] = "Root cause: pool exhausted".to_string();
        lines[150] = "at Pool.java:214".to_string();
        let text = lines.join("\n");
        let o = compress(&text, "db timeout conn_id pool", &cfg());
        assert_eq!(o.mode, Mode::Sieve);
        assert_eq!(o.kept_lno.len(), o.kept, "kept_lno 数量必须等于 kept");
        assert!(!o.kept_lno.is_empty(), "筛选模式必须给出证据行号");
        for w in o.kept_lno.windows(2) {
            assert!(w[0] < w[1], "证据行号必须严格递增（原顺序）");
        }
        assert!(o.kept_lno.contains(&51), "保留行含 Root cause 行");
        // 输出的保留行内容与输入行一致（按行号回查）。
        let out_lines: Vec<&str> = o
            .text
            .split('\n')
            .filter(|l| !l.is_empty() && *l != "...")
            .collect();
        assert_eq!(out_lines.len(), o.kept, "输出非省略行数必须等于 kept");
    }

    #[test]
    fn passthrough_kept_lno_empty() {
        // passthrough/fail-open 不产生证据行号（记忆库只对筛选结果提取原始证据）。
        let text: String = (0..50)
            .map(|i| format!("line {} with padding text alpha beta gamma", i))
            .collect::<Vec<_>>()
            .join("\n");
        let o = compress(&text, "绝对不存在的词zz", &cfg());
        assert!(o.kept_lno.is_empty(), "failopen 无证据行号");
        let o2 = compress("small", "q", &cfg());
        assert!(o2.kept_lno.is_empty(), "passthrough:small 无证据行号");
    }

    #[test]
    fn zero_signal_failopen() {
        let text: String = (0..200)
            .map(|i| format!("line {} with padding text alpha beta gamma", i))
            .collect::<Vec<_>>()
            .join("\n");
        let o = compress(&text, "绝对不存在的词zz", &cfg());
        assert_eq!(o.mode, Mode::FailOpenZeroSignal);
        assert_eq!(o.text, text);
    }

    #[test]
    fn warn_mass_not_guarded_in_needle() {
        let mut lines: Vec<String> = (0..300)
            .map(|i| format!("WARN queue depth={} consumer_lag=2s pool=3", i))
            .collect();
        lines[150] = "ERROR db timeout at 12:01 pool exhausted (conn_id=9f2a)".to_string();
        lines[151] = "Root cause: connection pool size 4 < peak concurrency 11".to_string();
        let text = lines.join(&char::from(10u8).to_string());
        let needle = Config {
            ratio: 0.35,
            context: 0,
            ..Config::default()
        };
        let o = compress(&text, "what failed db timeout pool conn_id", &needle);
        assert!(o.text.contains("conn_id=9f2a"));
        assert!(o.text.contains("Root cause:"));
        assert!(
            !o.text.contains("WARN queue depth"),
            "warn 级行不应被风险守卫成批保留"
        );
        assert!(o.kept <= 4);
    }

    #[test]
    fn marker_guard_keeps_incident_marker_in_needle() {
        let mut lines: Vec<String> = (0..300)
            .map(|i| format!("INFO heartbeat seq={} ok pool=3 latency=1ms", i))
            .collect();
        lines[150] = "ERROR db timeout at 12:01 pool exhausted (conn_id=9f2a)".to_string();
        lines[151] = "at com.ingest.Pool.acquire(Pool.java:214)".to_string();
        lines[152] = "Root cause: connection pool size 4 < peak concurrency 11".to_string();
        let text = lines.join(&char::from(10u8).to_string());
        let q = "what failed db timeout pool conn_id";
        let needle = Config {
            ratio: 0.35,
            context: 0,
            ..Config::default()
        };
        let o = compress(&text, q, &needle);
        assert_eq!(o.risk, Risk::High);
        assert_eq!(o.guarded, 1);
        assert!(
            o.text.contains("Root cause:"),
            "needle 档必须在 HIGH 风险下保留事故标记行"
        );
        assert!(o.text.contains("conn_id=9f2a"));
        let gated_off = Config {
            ratio: 0.35,
            context: 0,
            risk_gate: false,
            ..Config::default()
        };
        let o2 = compress(&text, q, &gated_off);
        assert!(
            !o2.text.contains("Root cause:"),
            "risk_gate=0 时标记行不被强制保留"
        );
        assert_eq!(o2.guarded, 0);
    }

    #[test]
    fn lineref_semantics() {
        assert!(is_lineref("foo.rs:10"));
        assert!(is_lineref("(bar.c:3)"));
        assert!(is_lineref(":123"));
        assert!(is_lineref("y:3)"));
        assert!(!is_lineref("abc:"));
        assert!(!is_lineref("12"));
        assert!(!is_lineref(":"));
        assert!(!is_lineref("x)y"));
    }

    #[test]
    fn unified_diff_content_lines_are_recognized() {
        assert!(is_diffline("+let value = 1;"));
        assert!(is_diffline("-old_value"));
        assert!(is_diffline("+"));
        assert!(is_diffline("-"));
        assert!(!is_diffline("+++ b/src/lib.rs"));
        assert!(!is_diffline("--- a/src/lib.rs"));
        assert!(!is_diffline("diff --git a/x b/x"));
    }

    #[test]
    fn analysis_strips_ansi_and_bare_carriage_returns() {
        let mut low = String::new();
        push_analysis_lower("\u{1b}[31mRoot cause:\u{1b}[0m 10%\rnext", &mut low);
        assert_eq!(low, "root cause: 10% next");
        assert!(is_incident_marker(&low));
        low.clear();
        push_analysis_lower("\u{1b}]8;;https://example.com\u{7}link", &mut low);
        assert_eq!(low, "link");
    }

    #[test]
    fn norm_into_semantics() {
        let n = |t: &str| {
            let mut o = String::new();
            let mut r = String::new();
            norm_into(t, &mut o, &mut r);
            o
        };
        assert_eq!(n("  a   b  "), "a b");
        assert_eq!(n("xyz123def"), "xyz#def");
        assert_eq!(n("abcdefab"), "#");
        assert_eq!(n("0xDEADBEEF1234"), "#x#");
        assert_eq!(n("a1b2c3"), "a#b#c#");
        assert_eq!(n("seq=42 ok"), "seq=# ok");
        assert_eq!(n(""), "");
        assert_eq!(n("汉字 42"), "汉字 #");
    }

    #[test]
    fn crlf_equivalence() {
        let lf = char::from(10u8).to_string();
        let crlf_sep = String::from(char::from(13u8)) + lf.as_str();
        let base: String = (0..200)
            .map(|i| format!("line {} alpha kvwrite beta gamma delta", i))
            .collect::<Vec<_>>()
            .join(lf.as_str());
        let crlf = base.replace(lf.as_str(), crlf_sep.as_str());
        let a = compress(&base, "kvwrite", &cfg());
        let b = compress(&crlf, "kvwrite", &cfg());
        assert_eq!(a.text, b.text);
        assert_eq!(a.kept, b.kept);
        assert_eq!(a.chars_out, b.chars_out);
        assert!(b.chars_in > a.chars_in, "CRLF 输入的原始字符数应包含 CR");
    }

    #[test]
    fn passthrough_modes() {
        let lf = char::from(10u8).to_string();
        let small: String = (0..10)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join(lf.as_str());
        let o = compress(&small, "line", &cfg());
        assert_eq!(o.mode, Mode::PassSmall);
        assert_eq!(o.text, small);
        let mut lines = vec!["x".repeat(150000)];
        lines.extend((0..60).map(|i| format!("filler error kvwrite line {}", i)));
        let text = lines.join(lf.as_str());
        let o2 = compress(&text, "kvwrite", &cfg());
        assert_eq!(o2.mode, Mode::PassLongline);
        assert_eq!(o2.text, text);
    }

    #[test]
    fn cjk_error_words_guard_and_class() {
        let mut lines: Vec<String> = (0..300)
            .map(|i| format!("INFO 心跳 seq={} 状态=正常 latency=1ms", i))
            .collect();
        lines[150] = "错误：数据库连接超时 (conn_id=9f2a)".to_string();
        lines[151] = "错误：磁盘已满 errno=28".to_string();
        lines[152] = "警告：队列深度=40".to_string();
        let text = lines.join(&char::from(10u8).to_string());
        let needle = Config {
            ratio: 0.35,
            context: 0,
            ..Config::default()
        };
        let o = compress(&text, "数据库 超时 conn_id", &needle);
        assert!(o.text.contains("conn_id=9f2a"), "命中行作为种子保留");
        assert!(
            o.text.contains("磁盘已满"),
            "失败级 CJK 错误行在 HIGH 风险下受守卫"
        );
        assert!(!o.text.contains("队列深度"), "警告级 CJK 行不成批保留");
        assert_eq!(o.guarded, 1);
    }

    #[test]
    fn deterministic_output() {
        let text: String = (0..300)
            .map(|i| format!("l{} error alpha beta kvwrite", i % 7))
            .collect::<Vec<_>>()
            .join("\n");
        let a = compress(&text, "kvwrite", &cfg());
        let b = compress(&text, "kvwrite", &cfg());
        assert_eq!(a.text, b.text);
        assert_eq!(a.cut_pct.to_bits(), b.cut_pct.to_bits());
    }

    #[test]
    fn force_duplicates_are_all_kept() {
        let mut lines: Vec<String> = (0..200)
            .map(|i| format!("INFO filler {} alpha beta", i))
            .collect();
        for idx in [150usize, 151, 152, 153] {
            lines[idx] = "SPECIAL_FORCE_LINE".to_string();
        }
        let text = lines.join("\n");
        let needle = Config {
            ratio: 0.02,
            context: 0,
            forces: vec!["SPECIAL_FORCE_LINE".to_string()],
            ..Config::default()
        };
        let o = compress(&text, "zzzznomatch", &needle);
        assert_eq!(o.text.matches("SPECIAL_FORCE_LINE").count(), 4);
        assert_eq!(o.seeds, 4);
    }

    #[test]
    fn no_kept_falls_back_to_passthrough() {
        let line = format!("alpha {}", "x".repeat(5000));
        let text: String = (0..40).map(|_| line.clone()).collect::<Vec<_>>().join("\n");
        let needle = Config {
            ratio: 0.02,
            context: 1,
            ..Config::default()
        };
        let o = compress(&text, "alpha", &needle);
        assert_eq!(o.mode, Mode::PassNoKept);
        assert_eq!(o.text, text);
        assert_eq!(o.kept, o.total);
        assert_eq!(o.chars_out, o.chars_in);
        assert_eq!(o.cut_pct, 0.0);
    }

    #[test]
    fn config_values_are_normalized() {
        let mut lines: Vec<String> = (0..200).map(|_| filler()).collect();
        lines[100] = "fn target() { return 1; }".to_string();
        let text = lines.join("\n");
        let weird = Config {
            ratio: f64::NAN,
            context: -9,
            window: -1,
            ..Config::default()
        };
        let o = compress(&text, "target", &weird);
        assert_eq!(o.window, 1);
        assert!(o.text.contains("fn target"));

        let huge = Config {
            window: i64::MAX,
            context: i64::MAX,
            ..Config::default()
        };
        let o2 = compress(&text, "target", &huge);
        assert_eq!(o2.window, 8);
        assert!(o2.text.contains("fn target"));
    }

    #[test]
    fn small_guard_requires_few_tokens() {
        let line = format!("alpha {}", "x".repeat(5000));
        let text: String = (0..39).map(|_| line.clone()).collect::<Vec<_>>().join("\n");
        let o = compress(&text, "alpha", &cfg());
        assert_ne!(o.mode, Mode::PassSmall);
        assert!(o.seeds == 0);
    }

    #[test]
    fn budget_is_optional_only() {
        let mut lines: Vec<String> = (0..99)
            .map(|i| format!("alpha filler {} {}", i, "x".repeat(100)))
            .collect();
        lines.insert(50, format!("SPECIAL_FORCE {}", "x".repeat(10000)));
        let text = lines.join("\n");
        let needle = Config {
            ratio: 0.02,
            context: 1,
            forces: vec!["SPECIAL_FORCE".to_string()],
            ..Config::default()
        };
        let o = compress(&text, "alpha", &needle);
        assert_eq!(o.seeds, 1);
        assert!(o.kept > 3, "种子与上下文不应挤占可选内容预算");
    }

    #[test]
    fn cjk_stop_and_tail_terms() {
        assert!(build_terms("这个").is_empty());
        assert!(build_terms("数据库连接超时").contains(&"超时".to_string()));
        assert!(build_terms("エラー").contains(&"エラー".to_string()));
    }

    #[test]
    fn runaway_token_is_dropped() {
        let long = "x".repeat(crate::terms::MAX_TERM_BYTES + 1);
        assert!(
            build_terms(&long).is_empty(),
            "超长 token 不得进入索引词表（u8 长度字段会截断）"
        );
        let max = "x".repeat(crate::terms::MAX_TERM_BYTES);
        assert_eq!(build_terms(&max), vec![max], "上限内 token 保持可用");
    }

    #[test]
    fn crlf_failopen_preserves_original() {
        let lines: Vec<String> = (0..200)
            .map(|i| format!("line {} alpha beta gamma delta", i))
            .collect();
        let text = lines.join("\r\n");
        let o = compress(&text, "zzzznomatch", &cfg());
        assert_eq!(o.mode, Mode::FailOpenZeroSignal);
        assert_eq!(o.text, text);
        assert_eq!(o.chars_in, text.chars().count());
    }

    #[test]
    fn frame_extension_scan_is_bounded() {
        assert!(is_frame("  foo.rs:10"));
        assert!(!is_frame(&format!("  foo.{}:10", "a".repeat(50))));
    }

    #[test]
    fn lineno_prefixes_kept_lines() {
        let mut lines: Vec<String> = (0..200).map(|_| filler()).collect();
        lines[99] = "fn target() { return 1; }".to_string();
        let text = lines.join("\n");
        let cfg = Config {
            ratio: 0.35,
            context: 0,
            lineno: true,
            ..Config::default()
        };
        let o = compress(&text, "target", &cfg);
        assert!(
            o.text.contains("100: fn target() { return 1; }"),
            "种子行应带原文行号"
        );
        // passthrough / fail-open 返回原文，不加行号
        let small = Config {
            lineno: true,
            ..Config::default()
        };
        let s: String = (0..10)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let o2 = compress(&s, "line", &small);
        assert_eq!(o2.mode, Mode::PassSmall);
        assert_eq!(o2.text, s);
        assert!(!o2.text.contains("1: line 0"), "passthrough 不附加行号");
    }

    #[test]
    fn max_tokens_overflow_evidence_keeps_evidence() {
        // 证据地板（证据 + 最小邻域）已超 --max-tokens：证据仍全保留，如实报告。
        let mut lines: Vec<String> = (0..200).map(|_| filler()).collect();
        lines[100] = "conn 9f2a broken db crash".to_string();
        let text = lines.join("\n");
        let cfg = Config {
            max_tokens: Some(3),
            ..cfg()
        };
        let o = compress(&text, "alpha conn broken", &cfg);
        assert_eq!(o.mode, Mode::OverflowEvidence);
        assert_eq!(o.max_tokens, Some(3));
        assert!(
            o.text.contains("conn 9f2a"),
            "证据行必须保留（绝不静默丢证据）"
        );
        assert!(o.tok_est > 3.0, "tok_est 应如实报告超限后的估算");
    }

    #[test]
    fn max_tokens_overflow_fill_after_guard() {
        // 证据地板未超限，但风险守卫追加证据性错误行后总估算超限：OverflowFill。
        let mut lines: Vec<String> = (0..200)
            .map(|i| format!("pad alpha line {} beta gamma", i))
            .collect();
        lines[5] = "conn 9f2a broken".to_string();
        lines[6] = "error alpha pipe burst 7a1c now".to_string();
        let text = lines.join("\n");
        let cfg = Config {
            ratio: 0.35,
            context: 0,
            max_tokens: Some(5),
            ..cfg()
        };
        let o = compress(&text, "alpha conn broken", &cfg);
        assert_eq!(o.mode, Mode::OverflowFill);
        assert_eq!(o.guarded, 1, "HIGH 风险守卫必须仍生效");
        assert!(o.text.contains("conn 9f2a"), "证据行保留");
        assert!(o.text.contains("error alpha pipe burst"), "守卫行保留");
        assert!(o.tok_est > 5.0, "tok_est 如实报告超限");
    }

    #[test]
    fn max_tokens_normal_sieve_respects_cap() {
        // 上限足够时：可选填充受 min(budget, max - floor) 约束，最终估算不超过上限。
        let mut lines: Vec<String> = (0..200).map(|_| filler()).collect();
        lines[100] = "conn 9f2a broken db crash".to_string();
        let text = lines.join("\n");
        let cfg = Config {
            max_tokens: Some(1000),
            ..cfg()
        };
        let o = compress(&text, "alpha conn broken", &cfg);
        assert_eq!(o.mode, Mode::Sieve);
        assert_eq!(o.max_tokens, Some(1000));
        assert!(o.tok_est <= 1000.0 + 1e-9, "正常模式下输出估算不得超上限");
        assert!(o.text.contains("conn 9f2a"));
    }

    #[test]
    fn max_tokens_default_none_keeps_legacy_shape() {
        // 默认 None = 现状行为不变：Outcome 不携带上限字段语义。
        let mut lines: Vec<String> = (0..200).map(|_| filler()).collect();
        lines[100] = "conn 9f2a broken db crash".to_string();
        let text = lines.join("\n");
        let o = compress(&text, "alpha conn broken", &cfg());
        assert_eq!(o.mode, Mode::Sieve);
        assert_eq!(o.max_tokens, None);
    }
}
