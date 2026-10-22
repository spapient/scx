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
    "failed",
    "failure",
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
// 守卫专用错误判据:失败级(与 ERR_NEEDLES 同表,唯去 warn/警告)。warn 级行在噪声日志中常成批出现,
// 风险守卫不应整批保留;行内另有失败级词面者仍受守护。评分/折叠仍用含 warn 的 ERR_NEEDLES。
pub(crate) const GUARD_NEEDLES: &[&str] = &[
    "error",
    "fail",
    "failed",
    "failure",
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
];

pub(crate) fn is_err(ll: &str) -> bool {
    ERR_NEEDLES.iter().any(|n| ll.contains(n))
}
// 事故诊断标记行(行首、大小写不敏感):risk HIGH 且 risk_gate 时与错误行一同受守卫;不参与种子判定。
pub(crate) fn is_incident_marker(ll: &str) -> bool {
    let t = ll.trim_start();
    INCIDENT_MARKERS.iter().any(|m| t.starts_with(m))
}
pub(crate) fn is_guardable(ll: &str) -> bool {
    GUARD_NEEDLES.iter().any(|n| ll.contains(n))
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
    let cs: Vec<char> = t.chars().collect();
    for i in 1..cs.len() {
        if cs[i] == '.' && !cs[i - 1].is_whitespace() {
            let mut j = i + 1;
            let limit = (i + 1 + MAX_FRAME_EXT_CHARS).min(cs.len());
            while j < limit && !cs[j].is_whitespace() && cs[j] != ':' {
                j += 1;
            }
            if j < cs.len() && cs[j] == ':' {
                let ext = &cs[i + 1..j];
                if EXTS.iter().any(|e| e.chars().eq(ext.iter().copied()))
                    && j + 1 < cs.len()
                    && cs[j + 1].is_ascii_digit()
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
