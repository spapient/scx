//! 纯函数工具：FNV-1a 64 哈希、UTC 时间自算、case id 生成。
//!
//! 零依赖、零 I/O、确定性（无随机源、无环境读取）。

/// FNV-1a 64 位哈希（确定性、无随机源）。
pub(crate) fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// case id：`<YYYY-MM-DD>_<hash16>`，hash = FNV-1a(query‖\0‖text) 全 64 位 hex。
/// 64 位避免持久化资产的生日碰撞（32 位在 1 万条时碰撞率 ≈1.2%）。
pub(crate) fn case_id(now: &str, query: &str, text: &str) -> String {
    let day = &now[0..10]; // YYYY-MM-DD
    let h = fnv1a(&format!("{query}\u{0}{text}"));
    format!("{day}_{h:016x}")
}

/// UTC 时间戳（秒精度，ISO-8601 `YYYY-MM-DDTHH:MM:SSZ`，零依赖自算日历）。
pub(crate) fn utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (y, mo, d) = civil_from_days(days);
    let (hh, mi, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    format!("{y:04}-{mo:02}-{d:02}T{hh:02}:{mi:02}:{ss:02}Z")
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
}
