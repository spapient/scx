//! 文本分析层：归一化、ANSI/CR 清理、错误/事故标记、标题、diff、堆栈帧与行引用判定。
//!
//! 全部为纯函数，写入调用方缓冲或无分配判定，供压缩引擎和记忆库共享；输出仍
//! 使用原文行，分析结果只影响匹配、去重和排序。

pub(crate) const MAX_FRAME_EXT_CHARS: usize = 8;
pub(crate) const EXTS: &[&str] = &[
    "c", "cc", "cpp", "h", "hpp", "cu", "cuh", "rs", "py", "js", "ts", "go", "java", "mm", "m",
    "swift", "zig",
];
pub(crate) const ERR_NEEDLES: &[&str] = &[
    "error",
    "fail",
    "exception",
    "panic",
    "assert",
    "denied",
    "refused",
    "timeout",
    "timed out",
    "cannot",
    "unable",
    "undefined",
    "segfault",
    "fatal",
    "warn",
    "traceback",
    "npm err",
    "✗",
    "❌",
    "错误",
    "失败",
    "报错",
    "异常",
    "超时",
    "崩溃",
    "警告",
];
pub(crate) const INCIDENT_MARKERS: &[&str] = &[
    "root cause:",
    "fix hint:",
    "mitigation:",
    "recommendation:",
    "action:",
    "assertionerror:",
];
/// 一次扫描同时得到“是否错误行”和“是否失败级（可受风险守卫）”。
/// warn/警告只参与错误判定，不成批进入守卫；遇到失败级词即可提前返回。
pub(crate) fn error_flags(ll: &str) -> (bool, bool) {
    let mut err = false;
    for n in ERR_NEEDLES {
        if ll.contains(n) {
            err = true;
            if !matches!(*n, "warn" | "警告") {
                return (true, true);
            }
        }
    }
    (err, false)
}

pub(crate) fn is_err(ll: &str) -> bool {
    error_flags(ll).0
}
// 事故诊断标记行(行首、大小写不敏感):risk HIGH 且 risk_gate 时与错误行一同受守卫;不参与种子判定。
pub(crate) fn is_incident_marker(ll: &str) -> bool {
    let t = ll.trim_start();
    INCIDENT_MARKERS.iter().any(|m| t.starts_with(m))
}
pub(crate) fn is_heading(s: &str) -> bool {
    let t = s.trim_start();
    if t.is_empty() {
        return false;
    }
    let hashes = t.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) && t.chars().nth(hashes).is_some_and(|c| c.is_whitespace()) {
        return true;
    }
    if t.starts_with("===") || t.starts_with("---") || t.starts_with("***") {
        return true;
    }
    t.starts_with('>') && t.chars().nth(1).is_some_and(|c| c.is_whitespace())
}
pub(crate) fn is_diffmeta(s: &str) -> bool {
    s.starts_with("@@")
        || s.starts_with("+++")
        || s.starts_with("---")
        || s.starts_with("diff --git")
        || s.starts_with("index ")
}
pub(crate) fn is_diffline(s: &str) -> bool {
    // 统一 diff 的正文行是 "+code" / "-code"，而不是只有第二个字符为空白才成立；
    // 三个相同前缀分别是文件头，交给 is_diffmeta 处理。
    (s.starts_with('+') && !s.starts_with("+++")) || (s.starts_with('-') && !s.starts_with("---"))
}
// 带前导空白的行且形如堆栈帧：at ...／File "／#N ／含 path.ext:line。
pub(crate) fn is_frame(s: &str) -> bool {
    let t = s.trim_start();
    if t.len() == s.len() {
        return false;
    }
    if t.starts_with("at ") || t.starts_with("File \"") {
        return true;
    }
    let hashes = t.chars().take_while(|&c| c == '#').count();
    if hashes >= 1
        && t.chars().nth(hashes).is_some_and(|c| c.is_ascii_digit())
        && t.chars().skip(hashes).any(|c| c.is_whitespace())
    {
        return true;
    }
    if !t.as_bytes().contains(&b'.') {
        return false;
    }
    // 仅为 "." 开头的 ASCII 扩展名扫描。按字节定位 "."，但扩展名用
    // char_indices 截取，避免在多字节 UTF-8 字符中间切片触发 panic。
    let bytes = t.as_bytes();
    for i in 1..bytes.len() {
        let prev_is_space =
            bytes[i] == b'.' && t[..i].chars().next_back().is_some_and(char::is_whitespace);
        if bytes[i] == b'.' && !prev_is_space {
            let rest = &t[i + 1..];
            let mut colon = None;
            for (off, ch) in rest.char_indices().take(MAX_FRAME_EXT_CHARS + 1) {
                if ch == ':' {
                    colon = Some(off);
                    break;
                }
                if ch.is_whitespace() {
                    break;
                }
            }
            if let Some(off) = colon {
                let ext = &rest[..off];
                if EXTS.contains(&ext)
                    && rest[off + 1..]
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_digit())
                {
                    return true;
                }
            }
        }
    }
    false
}
pub(crate) fn is_lineref(s: &str) -> bool {
    let mut it = s.chars().rev();
    let mut cur = it.next();
    if cur == Some(')') {
        cur = it.next();
    }
    let mut digits = 0usize;
    while cur.is_some_and(|c| c.is_ascii_digit()) {
        digits += 1;
        cur = it.next();
    }
    digits > 0 && cur == Some(':')
}

// 单趟归一化：8+ 位十六进制串折叠为 '#'；其余数字段折叠为 '#'；空白段折叠为单空格；首尾去空。
// run 为复用的十六进制串中转缓冲。
pub(crate) fn norm_into(s: &str, out: &mut String, run: &mut String) {
    if s.is_ascii() {
        norm_ascii(s.as_bytes(), out);
        return;
    }
    norm_generic(s, out, run);
}

/// 通用 Unicode 归一化；`norm_ascii` 的语义基准。
fn norm_generic(s: &str, out: &mut String, run: &mut String) {
    let mut pending_space = false;
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c.is_ascii_hexdigit() {
            run.clear();
            run.push(c);
            while it.peek().is_some_and(|&n| n.is_ascii_hexdigit()) {
                run.push(it.next().unwrap());
            }
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            if run.len() >= 8 {
                out.push('#');
            } else {
                let mut ri = run.chars().peekable();
                while let Some(rc) = ri.next() {
                    if rc.is_ascii_digit() {
                        while ri.peek().is_some_and(|&n| n.is_ascii_digit()) {
                            ri.next();
                        }
                        out.push('#');
                    } else {
                        out.push(rc);
                    }
                }
            }
        } else if c.is_ascii_digit() {
            while it.peek().is_some_and(|&n| n.is_ascii_digit()) {
                it.next();
            }
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push('#');
        } else if c.is_whitespace() {
            while it.peek().is_some_and(|&n| n.is_whitespace()) {
                it.next();
            }
            pending_space = true;
        } else {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push(c);
        }
    }
}

/// ASCII 快路径：与通用路径语义一致，跳过 Unicode 解码与临时十六进制缓冲。
fn norm_ascii(s: &[u8], out: &mut String) {
    let mut i = 0usize;
    let mut pending_space = false;
    while i < s.len() {
        let c = s[i];
        if c.is_ascii_hexdigit() {
            let start = i;
            while i < s.len() && s[i].is_ascii_hexdigit() {
                i += 1;
            }
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            let run = &s[start..i];
            if run.len() >= 8 {
                out.push('#');
            } else {
                let mut j = 0usize;
                while j < run.len() {
                    if run[j].is_ascii_digit() {
                        while j < run.len() && run[j].is_ascii_digit() {
                            j += 1;
                        }
                        out.push('#');
                    } else {
                        out.push(run[j] as char);
                        j += 1;
                    }
                }
            }
        } else if is_space_byte(c) {
            while i < s.len() && is_space_byte(s[i]) {
                i += 1;
            }
            pending_space = true;
        } else {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push(c as char);
            i += 1;
        }
    }
}

/// `u8::is_ascii_whitespace` 不含垂直制表 U+000B，而 `char::is_whitespace` 含；
/// 快路径显式补齐，保证与通用路径逐字节一致。
fn is_space_byte(b: u8) -> bool {
    b == b'\x0b' || b.is_ascii_whitespace()
}

/// 身份归一化：保留数字段，仅折叠空白并做 Unicode 小写。
///
/// 用于 case 去重 / qmap / 会话缓存这类“精确键”。`norm_into` 会把 8080/9090、
/// errno=28/29 都折叠成 `#`，适合日志词面匹配，但不适合判断“是不是同一条记忆”。
pub(crate) fn norm_identity_into(s: &str, out: &mut String) {
    if s.is_ascii() {
        norm_identity_ascii(s, out);
        return;
    }
    norm_identity_generic(s, out);
}

fn norm_identity_ascii(s: &str, out: &mut String) {
    let mut pending_space = false;
    for &b in s.as_bytes() {
        if is_space_byte(b) {
            pending_space = true;
            continue;
        }
        if pending_space && !out.is_empty() {
            out.push(' ');
        }
        pending_space = false;
        out.push(b.to_ascii_lowercase() as char);
    }
}

fn norm_identity_generic(s: &str, out: &mut String) {
    let mut pending_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space && !out.is_empty() {
            out.push(' ');
        }
        pending_space = false;
        for lc in c.to_lowercase() {
            out.push(lc);
        }
    }
}

// 分析用小写缓冲：剥离 ANSI 转义并折叠裸 CR。输出仍保留原始行，因此只影响匹配/排序，
// 不影响"保留行逐字原文"的承诺。进度条常用裸 \r 回写，按空白处理避免污染词面。
pub(crate) fn push_analysis_lower(s: &str, out: &mut String) {
    // 日志分析最常见的是无控制符 ASCII 行。按字节小写可跳过 Unicode
    // 大小写迭代与 ESC/CR 状态机；输出语义与通用路径完全一致。
    if s.is_ascii() && !s.as_bytes().iter().any(|b| matches!(*b, b'\x1b' | b'\r')) {
        out.reserve(s.len());
        out.extend(s.bytes().map(|b| b.to_ascii_lowercase() as char));
        return;
    }
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' {
            match it.peek().copied() {
                Some('[') => {
                    it.next();
                    for esc in it.by_ref() {
                        if ('@'..='~').contains(&esc) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    it.next();
                    while let Some(esc) = it.next() {
                        if esc == '\u{7}' {
                            break;
                        }
                        if esc == '\u{1b}' && it.peek() == Some(&'\\') {
                            it.next();
                            break;
                        }
                    }
                }
                Some(_) => {
                    it.next();
                }
                None => {}
            }
            continue;
        }
        if c == '\r' {
            out.push(' ');
            continue;
        }
        for lc in c.to_lowercase() {
            out.push(lc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_flags_split_warn_from_failure() {
        assert_eq!(error_flags("error: boom"), (true, true));
        assert_eq!(error_flags("warning only"), (true, false));
        assert_eq!(error_flags("failed to connect"), (true, true));
        assert_eq!(error_flags("info heartbeat"), (false, false));
    }

    #[test]
    fn frame_scan_handles_non_ascii_without_slicing_panic() {
        assert!(!is_frame("  错误.中文:42"));
        assert!(!is_frame("  错误.abcdefghi:42"));
        assert!(is_frame("  错误 at main.rs:42"));
    }

    #[test]
    fn ascii_normalization_matches_generic_path() {
        let cases = [
            "",
            "abc",
            "  abc  ",
            "a\t\nb",
            "a\x0bb",
            "ABC 0xDEADBEEF",
            "deadbeef deadbee abc123",
            "8080/9090 errno=28",
            "错误 42 naïve 123456789",
            "服务器\t超时  42",
            "ＡＢＣ　１２３",
        ];
        for input in cases {
            let mut generic = String::new();
            norm_generic(input, &mut generic, &mut String::new());
            let mut actual = String::new();
            norm_into(input, &mut actual, &mut String::new());
            assert_eq!(actual, generic, "norm_into mismatch for {input:?}");
            let mut generic_id = String::new();
            norm_identity_generic(input, &mut generic_id);
            let mut actual_id = String::new();
            norm_identity_into(input, &mut actual_id);
            assert_eq!(actual_id, generic_id, "identity mismatch for {input:?}");
            if input.is_ascii() {
                let mut fast = String::new();
                norm_ascii(input.as_bytes(), &mut fast);
                assert_eq!(fast, generic, "ASCII fast path mismatch for {input:?}");
                let mut fast_id = String::new();
                norm_identity_ascii(input, &mut fast_id);
                assert_eq!(
                    fast_id, generic_id,
                    "ASCII identity fast path mismatch for {input:?}"
                );
            }
        }
    }

    #[test]
    fn normalization_folds_numbers_but_identity_keeps_them() {
        let mut norm = String::new();
        norm_into(
            " cafe 1234 deadbeef 42 错误\t运行 ",
            &mut norm,
            &mut String::new(),
        );
        assert_eq!(norm, "cafe # # # 错误 运行");

        let mut identity = String::new();
        norm_identity_into("  Hello\t WORLD 1234 ", &mut identity);
        assert_eq!(identity, "hello world 1234");
    }
}
