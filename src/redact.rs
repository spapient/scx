//! 压缩前脱敏（P7）：手写确定性匹配器，零第三方 crate。
//!
//! 高置信模式（密钥/连接串/Bearer）复用 `crate::memory::sanitize`；身份类
//! （身份证/手机/邮箱）用手写匹配器（长度/边界规则）。内置匹配器仅做 Mask
//! （替换为 `<redacted>`）；`Replace`/`Hash` 动作与正则 span 替换留待后续
//! （现有 `regex.rs` 仅 `matches(bool)`，无匹配位置，无法做子串替换）。

#![forbid(unsafe_code)]

/// 内置脱敏种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedactKind {
    /// 密钥/连接串/Bearer（复用 `memory::sanitize`）。
    Secret,
    /// 18 位身份证号（17 位数字 + 数字/X 校验位）。
    IdCard,
    /// 11 位手机号（1 开头）。
    Phone,
    /// 邮箱 `local@domain.tld`。
    Email,
}

impl RedactKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            RedactKind::Secret => "secret",
            RedactKind::IdCard => "id-card",
            RedactKind::Phone => "phone",
            RedactKind::Email => "email",
        }
    }

    pub fn parse(s: &str) -> Option<RedactKind> {
        match s {
            "secret" => Some(RedactKind::Secret),
            "id-card" => Some(RedactKind::IdCard),
            "phone" => Some(RedactKind::Phone),
            "email" => Some(RedactKind::Email),
            _ => None,
        }
    }
}

/// 单种脱敏命中统计。
#[derive(Debug, Clone, Copy)]
pub struct Hit {
    pub kind: &'static str,
    pub count: usize,
}

const MASK: &str = "<redacted>";

/// 压缩前脱敏：逐 kind 应用内置匹配器，返回脱敏文本 + 命中统计。
/// 命中数按该 kind 新增的 `<redacted>` 数量计（各规则幂等、互不覆盖）。
pub fn redact(text: &str, kinds: &[RedactKind]) -> (String, Vec<Hit>) {
    let mut out = text.to_string();
    let mut hits = Vec::new();
    for &k in kinds {
        let before = out.matches(MASK).count();
        out = match k {
            // Secret 脱敏复用 memory 的幂等 sanitize（默认启用）；无 memory
            // 的最小内核构建下该模式降级为原样输出（feature opt-out 语义）。
            #[cfg(feature = "memory")]
            RedactKind::Secret => crate::memory::sanitize(&out),
            #[cfg(not(feature = "memory"))]
            RedactKind::Secret => out,
            RedactKind::IdCard => mask_id_card(&out),
            RedactKind::Phone => mask_phone(&out),
            RedactKind::Email => mask_email(&out),
        };
        let count = out.matches(MASK).count() - before;
        if count > 0 {
            hits.push(Hit {
                kind: k.as_str(),
                count,
            });
        }
    }
    (out, hits)
}

/// 连续 ASCII 数字段长度（从 i 起）。
fn digit_run(b: &[u8], i: usize) -> usize {
    let mut j = i;
    while j < b.len() && b[j].is_ascii_digit() {
        j += 1;
    }
    j - i
}

/// 对数字段应用命中规则：整段匹配则整体掩码，否则原样复制（避免长数字的子段误命中）。
fn mask_digit_runs(s: &str, pred: impl Fn(&[u8], usize, usize) -> Option<usize>) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let run = digit_run(b, i);
            match pred(b, i, run) {
                Some(len) => {
                    out.push_str(MASK);
                    i += len;
                }
                None => {
                    out.push_str(&s[i..i + run]);
                    i += run;
                }
            }
            continue;
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// 身份证：恰 18 位数字，或 17 位数字 + `X`。
fn mask_id_card(s: &str) -> String {
    mask_digit_runs(s, |b, i, run| {
        let check_x = run == 17 && b.get(i + 17).is_some_and(|&c| c == b'X' || c == b'x');
        (run == 18 || check_x).then_some(18)
    })
}

/// 手机号：恰 11 位数字且以 `1` 开头。
fn mask_phone(s: &str) -> String {
    mask_digit_runs(s, |b, i, run| (b[i] == b'1' && run == 11).then_some(11))
}

/// 邮箱：从 `@` 向两侧按合法字符集扩展，domain 须含 `.`。
fn mask_email(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'@' {
            if let Some(end) = email_span(b, i) {
                out.push_str(MASK);
                i = end;
                continue;
            }
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// 邮箱 span：`@` 两侧合法字符扩展，返回 domain 结束位置（须含点）；非邮箱返回 None。
fn email_span(b: &[u8], at: usize) -> Option<usize> {
    let mut start = at;
    while start > 0 && email_local(b[start - 1]) {
        start -= 1;
    }
    let mut end = at + 1;
    let mut has_dot = false;
    while end < b.len() && (email_domain(b[end]) || b[end] == b'.') {
        if b[end] == b'.' {
            has_dot = true;
        }
        end += 1;
    }
    (start < at && end > at + 1 && has_dot).then_some(end)
}

fn email_local(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'%' | b'+' | b'-')
}

fn email_domain(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "memory")]
    fn secret_redacts_keys_and_conn() {
        let (out, hits) = redact(
            "password=hunter2 token=abc postgres://u:pw@db/x",
            &[RedactKind::Secret],
        );
        assert!(!out.contains("hunter2"));
        assert!(out.contains("password=<redacted>"));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, "secret");
    }

    #[test]
    fn id_card_redacts_18_digits() {
        let (out, _) = redact("id=11010519491231002X", &[RedactKind::IdCard]);
        assert!(!out.contains("11010519491231002X"));
        assert!(out.contains("<redacted>"));
        // 19 位数字不误命中（整段非 18 位）。
        let (o2, h2) = redact("n=1234567890123456789", &[RedactKind::IdCard]);
        assert!(!o2.contains("<redacted>"), "19 位长数字不掩码: {o2}");
        assert!(h2.is_empty());
    }

    #[test]
    fn phone_redacts_11_digit_mobile() {
        let (out, hits) = redact("tel=13812345678", &[RedactKind::Phone]);
        assert!(!out.contains("13812345678"));
        assert_eq!(hits[0].count, 1);
        // 非 1 开头的 11 位数字不误命中。
        let (o2, h2) = redact("n=23812345678", &[RedactKind::Phone]);
        assert!(!o2.contains("<redacted>"), "{o2}");
        assert!(h2.is_empty());
    }

    #[test]
    fn email_redacts_address() {
        let (out, hits) = redact("mail alice@example.com ok", &[RedactKind::Email]);
        assert!(!out.contains("alice@example.com"));
        assert!(out.contains("<redacted>"));
        assert_eq!(hits[0].count, 1);
    }

    #[test]
    fn empty_kinds_is_identity() {
        let (out, hits) = redact("password=abc", &[]);
        assert_eq!(out, "password=abc");
        assert!(hits.is_empty());
    }
}
