//! 健壮性测试：对确定性伪随机生成的畸形输入做长时间对拍。
//!
//! 目标是"解析器与选行逻辑在任何输入上都不 panic，且输出保持基本不变量"。
//! 输入覆盖：控制字符、NUL、BOM、混合换行、超长单行、极深括号、
//! Unicode 边界、重复到极致的行、空行密集、超长分词。
//!
//! 随机序列由固定种子驱动，因此失败可复现（同一 case 编号重跑结果一致）。

use scx::{compress, Config, Mode};

/// xorshift64*：确定性、无外部依赖，仅用于生成输入。
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % (n as u64 + 1)) as usize
    }

    fn pick<'a>(&mut self, items: &'a [&'a str]) -> &'a str {
        items[self.below(items.len() - 1)]
    }
}

const NOISE: &[&str] = &[
    "INFO tick=1",
    "\u{0}NUL 开头",
    "ERROR boom: \u{1b}[31mcolor\u{1b}[0m",
    "    ",
    "\t\t\t",
    "\u{feff}带 BOM 的行",
    "很长很长很长的行",
    "}}}]]])) 只由闭合符号组成",
    "{\"json\": {\"nested\": {\"deep\": [[[[[]]]]]}}",
    "dup-dup-dup-dup-dup-dup",
    "🦀 emoji 与 中文 混排 with ascii",
    "\r\n CRLF 行尾",
    "ENDING WITH BACKSLASH \\",
    "WARN latency spike p99=1200ms",
    "ERROR auth failed at handle_login: token expired",
];

fn build_input(rng: &mut Rng, lines: usize) -> String {
    let mut out = String::new();
    for _ in 0..lines {
        let kind = rng.below(9);
        match kind {
            0 => out.push('\n'),
            1 => {
                // 超长单行：最多约 40k 字符
                let n = rng.below(40_000);
                for _ in 0..n {
                    out.push((b'a' + (rng.below(25) as u8)) as char);
                }
                out.push('\n');
            }
            2 => {
                // 深括号嵌套
                let n = rng.below(2_000);
                out.push_str(&"{[(".repeat(n));
                out.push_str(&"}])".repeat(n));
                out.push('\n');
            }
            3 => {
                // 控制字符
                for _ in 0..rng.below(40) {
                    out.push(rng.below(31) as u8 as char);
                }
                out.push('\n');
            }
            4 => {
                // 超长分词（无分隔符号）
                let n = rng.below(20_000);
                out.push_str(&"x".repeat(n));
                out.push('\n');
            }
            _ => {
                out.push_str(rng.pick(NOISE));
                out.push('\n');
            }
        }
    }
    out
}

fn check(text: &str, query: &str, cfg: &Config, case: usize) {
    let out = compress(text, query, cfg);
    let lines = text.lines().count();
    assert!(
        out.kept <= out.total,
        "case {}: kept {} > total {}",
        case,
        out.kept,
        out.total
    );
    assert!(
        out.total <= lines,
        "case {}: total {} > 输入行数 {}",
        case,
        out.total,
        lines
    );
    assert!(out.cut <= 100.0, "case {}: cut {} 越界", case, out.cut);
    if !text.is_empty() {
        assert!(!out.text.is_empty(), "case {}: 非空输入产生了空输出", case);
    }
    // 只删不改：筛选模式下，每个被保留的原始行都必须原样出现在输出里。
    // 输出尾部附带的统计卡片（`compressed: ...`）与直通模式不计入行号序列，
    // 因此这里以 `numbers` 非空为前提，并要求「原文行内容出现在输出中」。
    let mut prev = 0usize;
    for &n in &out.numbers {
        assert!(n >= 1 && n <= lines, "case {}: 行号 {} 越界", case, n);
        assert!(n > prev, "case {}: 行号 {} 未严格递增", case, n);
        prev = n;
    }
    if !out.rewritten {
        for &n in &out.numbers {
            let want = text.lines().nth(n - 1).unwrap_or("");
            if want.is_empty() {
                continue;
            }
            assert!(
                out.text.contains(want),
                "case {}: 原文第 {} 行未出现在输出中（只有删除、不得改写）",
                case,
                n
            );
        }
    }
    // 无信号时必须回退原文，不能静默返回空。
    if matches!(
        out.mode,
        Mode::PassSmall
            | Mode::PassLongline
            | Mode::PassTooManyLines
            | Mode::FailOpenZeroSignal
            | Mode::PassNoKept
    ) {
        assert_eq!(out.text, text, "case {}: 回退模式必须原样返回", case);
    }
}

#[test]
fn random_inputs_never_panic_and_keep_invariants() {
    let mut rng = Rng::new(0x5c1_2026_1110);
    let mut cfg = Config::default();
    for case in 0..2_000usize {
        let count = rng.below(60) + 1;
        let text = build_input(&mut rng, count);
        let query = rng.pick(&[
            "",
            "ERROR auth",
            "latency",
            "认证失败",
            "完全不存在的词",
            "🦀",
            "\u{0}",
        ]);
        cfg.ratio = match rng.below(4) {
            0 => 0.05,
            1 => 0.3,
            2 => 0.7,
            _ => 1.0,
        };
        cfg.window = rng.below(8) as i64;
        check(&text, query, &cfg, case);
    }
}

#[test]
fn extreme_sizes_are_bounded() {
    let mut cfg = Config::default();
    let huge = "z".repeat(1_000_000);
    check(&huge, "z", &cfg, 9001);
    let blanks = "\n".repeat(200_000);
    check(&blanks, "ERROR", &cfg, 9002);
    let dup = "INFO same line\n".repeat(50_000);
    check(&dup, "INFO", &cfg, 9003);
    let mixed = format!("{huge}\n{dup}\n{blanks}");
    check(&mixed, "same", &cfg, 9004);
    cfg.ratio = 0.0;
    check(&mixed, "", &cfg, 9005);
}

#[test]
fn invalid_configs_are_normalized_not_crashing() {
    let text = "ERROR a\nINFO b\nERROR c\n";
    for ratio in [f64::NAN, -5.0, 99.0] {
        let cfg = Config {
            ratio,
            ..Config::default()
        };
        let out = compress(text, "ERROR", &cfg);
        assert!(out.cut <= 100.0, "ratio={ratio} 时 cut 异常");
    }
    let cfg = Config {
        window: i64::MAX,
        ..Config::default()
    };
    let out = compress(text, "ERROR", &cfg);
    assert!(out.window >= 0, "window 未归一化");
    let cfg = Config {
        sem_weight: f64::INFINITY,
        ..Config::default()
    };
    let out = compress(text, "ERROR", &cfg);
    assert!(out.cut <= 100.0, "sem_weight 无穷时 cut 异常");
}

#[test]
fn determinism_repeats_bitwise() {
    let mut rng = Rng::new(0xdead_beef_2026);
    let mut cfg = Config::default();
    for case in 0..50usize {
        let count = rng.below(40) + 1;
        let text = build_input(&mut rng, count);
        let query = rng.pick(&["ERROR", "latency", "中文", ""]);
        let a = compress(&text, query, &cfg);
        let b = compress(&text, query, &cfg);
        assert_eq!(a.text, b.text, "case {}: 同输入两次输出不一致", case);
        assert_eq!(a.kept, b.kept, "case {}: kept 不稳定", case);
        assert_eq!(a.total, b.total, "case {}: total 不稳定", case);
    }
    cfg.ratio = 0.4;
    let text = build_input(&mut rng, 30);
    let a = compress(&text, "ERROR", &cfg);
    let b = compress(&text, "ERROR", &cfg);
    assert_eq!(a.text, b.text, "非默认 ratio 下输出必须稳定");
}
