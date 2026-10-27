//! 零依赖行式配置解析（TOML 子集）：`[section]` + `key = value` 行。
//!
//! 记忆库 `meta.toml` 与内核策略文件（P3 `--policy-file`）共用同一解析器；
//! 只做解析与类型转换，不持有状态、不含 I/O。语义与 meta.rs 既有行式解析
//! 逐字节一致（trim、跳过空行/注释、节追踪、`=` 拆分）。

/// 一条解析结果：所属节（"" 为根节）、键、值（均已 trim）。
pub struct Entry<'a> {
    pub section: &'a str,
    pub key: &'a str,
    pub value: &'a str,
}

/// 逐行解析：空行与 `#` 注释跳过；`[name]` 切换节；无 `=` 的行忽略。
pub fn parse<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<Entry<'a>> {
    let mut out = Vec::new();
    let mut section = "";
    for raw in lines {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = &line[1..line.len() - 1];
            continue;
        }
        let Some(eq) = line.find('=') else {
            continue;
        };
        let key = line[..eq].trim();
        let value = line[eq + 1..].trim();
        out.push(Entry {
            section,
            key,
            value,
        });
    }
    out
}

/// 带行号的解析结果（P3 `--policy-file` 报错定位用）。
pub struct Line<'a> {
    /// 与 `parse` 相同的解析结果。
    pub entry: Entry<'a>,
    /// 原文 1-based 行号。
    pub line: usize,
}

/// 逐行解析并保留行号（语义与 `parse` 完全一致）。
pub fn numbered<'a>(lines: impl Iterator<Item = (usize, &'a str)>) -> Vec<Line<'a>> {
    let mut out = Vec::new();
    let mut section = "";
    for (line_no, raw) in lines {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = &line[1..line.len() - 1];
            continue;
        }
        let Some(eq) = line.find('=') else {
            continue;
        };
        let key = line[..eq].trim();
        let value = line[eq + 1..].trim();
        out.push(Line {
            entry: Entry {
                section,
                key,
                value,
            },
            line: line_no,
        });
    }
    out
}

/// f64 读取：必须可解析且有限（NaN/inf 拒绝）。
pub fn float(v: &str) -> Option<f64> {
    v.parse::<f64>().ok().filter(|f| f.is_finite())
}

/// usize 读取（df 快照等）。
pub fn size(v: &str) -> Option<usize> {
    v.parse().ok()
}

/// u64 读取（gen 代际等）。
pub fn integer(v: &str) -> Option<u64> {
    v.parse().ok()
}

/// 去掉首尾空白与一对双引号（TOML 字符串值的朴素处理）。
pub fn strip(v: &str) -> &str {
    v.trim().trim_matches('"')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 确定性模糊：策略文件与 meta.toml 都是外部可写文本，任意行都不得 panic，
    /// 且返回的切片必须仍属于输入（不越界、不破坏 UTF-8）。
    #[test]
    fn parse_never_panics_on_arbitrary_lines() {
        let pool: Vec<char> = "[]=# \t\n\"abc012中🚀".chars().collect();
        let mut state = 0xcafe_babeu32;
        let mut next = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            state
        };
        for _ in 0..1500 {
            let n = next() as usize % 5;
            let mut lines: Vec<String> = Vec::new();
            for _ in 0..n {
                let m = next() as usize % 16;
                let mut s = String::new();
                for _ in 0..m {
                    s.push(pool[next() as usize % pool.len()]);
                }
                lines.push(s);
            }
            for e in parse(lines.iter().map(String::as_str)) {
                let _ = (e.section, e.key, e.value);
            }
            for l in numbered(lines.iter().enumerate().map(|(i, s)| (i + 1, s.as_str()))) {
                let _ = (l.line, l.entry.section, l.entry.key, l.entry.value);
            }
        }
    }

    #[test]
    fn parses_sections_and_values() {
        let src =
            "# comment\nschema = 2\n\n[params]\nbm25_k1 = 1.2\nbad = nan\n[word_w]\nfoo = 0.5\n";
        let entries = parse(src.lines());
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].section, "");
        assert_eq!(entries[0].key, "schema");
        assert_eq!(entries[0].value, "2");
        assert_eq!(entries[1].section, "params");
        assert_eq!(entries[1].key, "bm25_k1");
        assert_eq!(float(entries[1].value), Some(1.2));
        assert!(float(entries[2].value).is_none(), "nan 必须拒绝");
        assert_eq!(entries[3].section, "word_w");
    }

    #[test]
    fn unquotes_and_typed_reads() {
        assert_eq!(strip("  \"abc\"  "), "abc");
        assert_eq!(size("42"), Some(42));
        assert_eq!(integer("7"), Some(7));
        assert_eq!(size("x"), None);
        assert_eq!(integer("-1"), None);
    }
}
