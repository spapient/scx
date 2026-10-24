//! 结构分类层：错误/事故标记/守卫词面、标题、diff、堆栈帧、行引用判定。
//!
//! 全部为纯函数行分类器（输入小写归一化行或原文行，输出 bool），供引擎打分、
//! 守卫与种子区域使用；未来记忆库摄取可直接复用 is_err / is_frame 做结构化加权。

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
}
