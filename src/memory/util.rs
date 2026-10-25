//! 纯函数工具：FNV-1a 64 哈希、UTC 时间自算、case id 生成。
//!
//! 零依赖、零 I/O、确定性（无随机源、无环境读取）。

/// FNV-1a 64 位哈希（确定性、无随机源）。
pub(crate) fn fnv1a(s: &str) -> u64 {
    fnv1a_parts(&[s.as_bytes()])
}

/// 分片 FNV-1a：避免为复合身份键拼接临时字符串。
pub(crate) fn fnv1a_parts(parts: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for &b in *part {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    }
    h
}

/// case id：`<YYYY-MM-DD>_<hash16>`，hash = FNV-1a(query‖\0‖text) 全 64 位 hex。
/// 64 位避免持久化资产的生日碰撞（32 位在 1 万条时碰撞率 ≈1.2%）。
pub(crate) fn case_id(now: &str, query: &str, text: &str) -> String {
    let day = &now[0..10]; // YYYY-MM-DD
    let h = fnv1a_parts(&[query.as_bytes(), b"\0", text.as_bytes()]);
    format!("{day}_{h:016x}")
}

/// UTC 时间戳（秒精度，ISO-8601 `YYYY-MM-DDTHH:MM:SSZ`，零依赖自算日历）。
pub(crate) fn utc_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub(crate) fn utc_now() -> String {
    let secs = utc_now_secs();
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (y, mo, d) = civil_from_days(days);
    let (hh, mi, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    format!("{y:04}-{mo:02}-{d:02}T{hh:02}:{mi:02}:{ss:02}Z")
}

/// 入库前模式脱敏：token/密码/连接串/Bearer 等敏感值
/// 在落盘前替换为 `<redacted>`。幂等（脱敏结果不再含可脱敏模式）、确定性、
/// 不破坏行号与结构（键名保留、仅值替换）。朴素字节扫描（零依赖）。
pub(crate) fn sanitize(s: &str) -> String {
    // 多轮直到稳定：嵌套形式（如 `Authorization: Bearer token=..`）逐层收敛。
    let Some(mut out) = sanitize_once(s) else {
        return s.to_string();
    };
    for _ in 1..3 {
        match sanitize_once(&out) {
            Some(next) => out = next,
            None => break,
        }
    }
    out
}

/// 敏感键（大小写不敏感匹配，后随 `=`/`:` + 可选空白 + 值到边界）。
const SENSITIVE_KEYS: &[&str] = &[
    "password",
    "passwd",
    "pwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "access_token",
    "access-token",
    "authorization",
    "client_secret",
    "private_key",
    "auth",
];

fn sanitize_once(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    let mut changed = false;
    while i < b.len() {
        if let Some((key_len, sep_len)) = match_key_value(b, i) {
            // key + 分隔符（保留键名与分隔符原样）。
            let key_end = i + key_len + sep_len;
            out.push_str(&s[i..key_end]);
            // 值：到空白/逗号/分号/引号/尖括号边界；空值不替换。
            // Bearer/Basic 前缀的整个 `Bearer <token>` 一并替换（Authorization 一行收敛）。
            let mut v = key_end;
            if let Some(tok_end) = match_bearer_value(b, v) {
                v = tok_end;
            } else {
                while v < b.len() && !is_val_boundary(b[v]) {
                    v += 1;
                }
            }
            if v > key_end {
                out.push_str("<redacted>");
                changed = true;
                i = v;
            } else {
                i = key_end;
            }
            continue;
        }
        if let Some((pw_start, at_pos)) = match_url_userinfo(b, i) {
            out.push_str(&s[i..pw_start]);
            out.push_str("<redacted>");
            changed = true;
            i = at_pos; // @ 由下一轮原样输出（保留结构）。
            continue;
        }
        if let Some(len) = match_bearer(b, i) {
            out.push_str(&s[i..i + len]);
            out.push_str("<redacted>");
            changed = true;
            i += len;
            continue;
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    changed.then_some(out)
}

/// 位置 i 是否以敏感键开头（大小写不敏感）且键后紧跟 `=`/`:`（可带空白）。
/// 返回 (键字节长, 分隔符字节长含键后空白)。
fn match_key_value(b: &[u8], i: usize) -> Option<(usize, usize)> {
    for key in SENSITIVE_KEYS {
        let k = key.as_bytes();
        if b.len() - i < k.len() + 1 {
            continue;
        }
        if !b[i..i + k.len()].eq_ignore_ascii_case(k) {
            continue;
        }
        let mut j = i + k.len();
        let mut sep_len = 0usize;
        while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
            j += 1;
            sep_len += 1;
        }
        if j >= b.len() || (b[j] != b'=' && b[j] != b':') {
            continue;
        }
        sep_len += 1; // '=' 或 ':'
                      // 分隔符后的前导空白也并入分隔符（`passwd: hunter2` → sep=": "）。
        j += 1;
        while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
            j += 1;
            sep_len += 1;
        }
        return Some((k.len(), sep_len));
    }
    None
}

/// 值边界：空白、逗号、分号、引号、尖括号。
fn is_val_boundary(c: u8) -> bool {
    matches!(
        c,
        b' ' | b'\t' | b'\r' | b'\n' | b',' | b';' | b'"' | b'\'' | b'<' | b'>'
    )
}

/// URL 用户信息：`://user:pass@` → 保留到密码起点，密码替换。
/// 返回 (密码起点, @ 位置)——上层输出 s[i..pw_start] + `<redacted>` 后从 @ 续扫。
fn match_url_userinfo(b: &[u8], i: usize) -> Option<(usize, usize)> {
    if !b[i..].starts_with(b"://") {
        return None;
    }
    let mut j = i + 3;
    while j < b.len() && !matches!(b[j], b'/' | b'@' | b' ' | b'\t' | b'\r' | b'\n') {
        if b[j] == b':' {
            // 找到 user:pass@ 形态。
            let mut k = j + 1;
            while k < b.len() && !matches!(b[k], b'@' | b'/' | b' ' | b'\t' | b'\r' | b'\n') {
                k += 1;
            }
            if k < b.len() && b[k] == b'@' {
                return Some((j + 1, k));
            }
        }
        j += 1;
    }
    None
}

/// `Bearer <token>` / `Basic <token>`：键 + 空白 + 8+ 字符值到空白边界。
fn match_bearer(b: &[u8], i: usize) -> Option<usize> {
    for kw in ["Bearer", "Basic"] {
        let k = kw.as_bytes();
        if b.len() - i < k.len() + 2 {
            continue;
        }
        if !b[i..i + k.len()].eq_ignore_ascii_case(k) {
            continue;
        }
        let mut j = i + k.len();
        if b[j] != b' ' && b[j] != b'\t' {
            continue;
        }
        j += 1;
        let mut n = j;
        while n < b.len() && !is_val_boundary(b[n]) {
            n += 1;
        }
        if n - j >= 8 {
            return Some(n - j);
        }
    }
    None
}

/// 从位置 i 起，若值以 `Bearer `/`Basic ` 开头且 token ≥8 字符，返回 token 末尾
/// （把整个 `Bearer <token>` 交给上层一并替换；否则返回 None 走普通值扫描）。
fn match_bearer_value(b: &[u8], i: usize) -> Option<usize> {
    for kw in ["Bearer", "Basic"] {
        let k = kw.as_bytes();
        if b.len() - i < k.len() + 2 {
            continue;
        }
        if !b[i..i + k.len()].eq_ignore_ascii_case(k) {
            continue;
        }
        let mut j = i + k.len();
        if b[j] != b' ' && b[j] != b'\t' {
            continue;
        }
        j += 1;
        let mut n = j;
        while n < b.len() && !is_val_boundary(b[n]) {
            n += 1;
        }
        if n - j >= 8 {
            return Some(n);
        }
    }
    None
}

/// 终端输出安全转义：C0 控制字符与 ESC/ANSI 序列
/// 转成可见字面（`\x1b`、`\xNN`），防 --recall-detail 回放证据时的转义注入。
pub fn escape_ctrl(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        let u = ch as u32;
        if ch == '\n' {
            out.push_str("\\n");
        } else if ch == '\t' {
            out.push_str("\\t");
        } else if ch == '\r' {
            out.push_str("\\r");
        } else if u == 0x1b {
            out.push_str("\\x1b");
        } else if u < 0x20 || u == 0x7f {
            out.push_str(&format!("\\x{u:02x}"));
        } else {
            out.push(ch);
        }
    }
    out
}

/// Howard Hinnant 的 civil_from_days：days-since-epoch → (年, 月, 日)。
fn civil_from_days(z: u64) -> (u64, u64, u64) {
    let z = z + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 解析本模块自产的 UTC 串（`YYYY-MM-DDTHH:MM:SSZ`）为 Unix 秒。
/// 失败/非自产格式返回 0（与"未知年龄按全新"口径一致）。
pub(crate) fn parse_utc_secs(s: &str) -> u64 {
    let s = s.trim();
    let s = s.strip_suffix('Z').unwrap_or(s);
    let (date, time) = match s.split_once('T') {
        Some((d, t)) => (d, t),
        None => return 0,
    };
    let p = |v: &str| v.parse::<u64>().ok();
    let (y, mo, d) = match (date.get(0..4), date.get(5..7), date.get(8..10)) {
        (Some(y), Some(m), Some(d)) => match (p(y), p(m), p(d)) {
            (Some(y), Some(m), Some(d)) => (y, m, d),
            _ => return 0,
        },
        _ => return 0,
    };
    let (hh, mi, ss) = match (time.get(0..2), time.get(3..5), time.get(6..8)) {
        (Some(h), Some(m), Some(s)) => match (p(h), p(m), p(s)) {
            (Some(h), Some(m), Some(s)) => (h, m, s),
            _ => return 0,
        },
        _ => return 0,
    };
    if mo == 0 || mo > 12 || d == 0 || d > 31 || hh > 23 || mi > 59 || ss > 60 {
        return 0;
    }
    // days_since_epoch = civil_to_days(y,m,d)，逆算法。
    let y_adj = if mo <= 2 { y - 1 } else { y };
    let era = y_adj / 400;
    let yoe = y_adj - era * 400;
    let mp = if mo > 2 { mo - 3 } else { mo + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 86_400 + hh * 3600 + mi * 60 + ss
}

/// 编辑距离（Levenshtein），band 限制：超过 `max` 直接剪枝返回 None。
/// 用于 qmap 变体识别（≤2）。滚动两行数组，O(min(m,n)) 空间。
pub(crate) fn edit_dist(a: &str, b: &str, max: usize) -> Option<usize> {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > max {
        return None;
    }
    let (s, t) = if a.len() <= b.len() {
        (&a, &b)
    } else {
        (&b, &a)
    };
    let mut prev: Vec<usize> = (0..=s.len()).collect();
    let mut cur: Vec<usize> = vec![0; s.len() + 1];
    for (j, cj) in t.iter().enumerate() {
        cur[0] = j + 1;
        for (i, ci) in s.iter().enumerate() {
            let cost = if ci == cj { 0 } else { 1 };
            cur[i + 1] = (prev[i + 1] + 1).min(cur[i] + 1).min(prev[i] + cost);
        }
        // 当前前缀最小距离 > max 则不可能到达 ≤max，剪枝。
        if cur.iter().min().unwrap() > &max {
            return None;
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    let d = prev[s.len()];
    if d > max {
        None
    } else {
        Some(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_dist_band() {
        assert_eq!(edit_dist("abc", "abc", 2), Some(0));
        assert_eq!(edit_dist("abc", "abd", 2), Some(1));
        assert_eq!(edit_dist("abc", "abcd", 2), Some(1));
        assert_eq!(edit_dist("连接池", "连接池爆", 2), Some(1));
        assert_eq!(edit_dist("abc", "xyz", 2), None);
        assert_eq!(edit_dist("abcdef", "ab", 2), None);
        assert_eq!(edit_dist("连接池爆了", "连接池满", 3), Some(2));
    }

    #[test]
    fn fnv1a_deterministic_and_sensitive() {
        assert_eq!(fnv1a("abc"), fnv1a("abc"));
        assert_ne!(fnv1a("abc"), fnv1a("abd"));
    }

    #[test]
    fn civil_date_algorithm() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        // 2024-01-01 起第 79 天 = 3 月 20 日（1 月 31 + 2 月 29 + 19）。
        assert_eq!(civil_from_days(19_802), (2024, 3, 20));
    }

    #[test]
    fn case_id_shape() {
        let id = case_id("2026-10-22T00:00:00Z", "q", "t");
        assert!(id.starts_with("2026-10-22_"));
        assert_eq!(id.len(), 11 + 16, "64 位 hash = 16 hex");
        assert_eq!(id, case_id("2026-10-22T00:00:00Z", "q", "t"));
    }

    #[test]
    fn parse_utc_secs_roundtrip() {
        // 1970-01-01T00:00:00Z = 0
        assert_eq!(parse_utc_secs("1970-01-01T00:00:00Z"), 0);
        // 2024-01-01T00:00:00Z = civil 19_723 天
        assert_eq!(parse_utc_secs("2024-01-01T00:00:00Z"), 19_723 * 86_400);
        // 闰日：2024-02-29T00:00:00Z = 19723+59 天
        assert_eq!(
            parse_utc_secs("2024-02-29T00:00:00Z"),
            (19_723 + 59) * 86_400
        );
        // 时间分量
        assert_eq!(
            parse_utc_secs("2024-01-01T01:02:03Z"),
            19_723 * 86_400 + 3723
        );
        // 非法输入 → 0
        assert_eq!(parse_utc_secs("garbage"), 0);
        assert_eq!(parse_utc_secs("2024-13-01T00:00:00Z"), 0);
    }

    /// 脱敏覆盖全部模式、幂等且不误伤明文。
    #[test]
    fn sanitize_redacts_secrets_and_is_idempotent() {
        let cases: &[(&str, &str)] = &[
            ("password=abc123", "password=<redacted>"),
            ("passwd: hunter2", "passwd: <redacted>"),
            (
                "Authorization: Bearer abcd.efgh.ijkl",
                "Authorization: <redacted>",
            ),
            ("token=eyJhbGciOiJIUzI1NiJ9.abc", "token=<redacted>"),
            ("api_key=sk-live-abc", "api_key=<redacted>"),
            (
                "postgres://user:pw@db:5432/x",
                "postgres://user:<redacted>@db:5432/x",
            ),
            ("client_secret = 9f8e7d6c5b4a", "client_secret = <redacted>"),
            ("ERROR plain log line", "ERROR plain log line"),
            ("conn timeout pool", "conn timeout pool"),
        ];
        for (input, want) in cases {
            assert_eq!(sanitize(input), *want, "input={input}");
        }
        // 幂等：再脱敏结果不变。
        let once = sanitize("Authorization: Bearer xyz.123 token=abc");
        assert_eq!(sanitize(&once), once);
        // 嵌套收敛：Bearer 内嵌 token=。
        let nested = sanitize("Authorization: Bearer aaaaaaaaaaaaaaaaaa token=zzz");
        assert!(!nested.contains("aaaaaaaaaaaaaaaaaa"), "{nested}");
        assert!(!nested.contains("zzz"), "{nested}");
        assert!(nested.contains("<redacted>"));
        assert!(nested.contains("Authorization:"));
    }

    /// 控制字符与 ANSI 序列必须可见化。
    #[test]
    fn escape_ctrl_renders_controls_visibly() {
        assert_eq!(escape_ctrl("a\x1b[31mb"), "a\\x1b[31mb");
        assert_eq!(escape_ctrl("x\ny"), "x\\ny");
        assert_eq!(escape_ctrl("plain"), "plain");
        assert_eq!(escape_ctrl("\x00\x07\x7f"), "\\x00\\x07\\x7f");
    }
}
