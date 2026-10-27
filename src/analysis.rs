//! 文本分析层：归一化、ANSI/CR 清理、错误/事故标记、标题、diff、堆栈帧与行引用判定。
//!
//! 全部为纯函数，写入调用方缓冲或无分配判定，供压缩引擎和记忆库共享；输出仍
//! 使用原文行，分析结果只影响匹配、去重和排序。

pub(crate) const MAX_FRAME_EXT_CHARS: usize = 8;
pub(crate) const EXTS: &[&str] = &[
    "c", "cc", "cpp", "h", "hpp", "cu", "cuh", "rs", "py", "js", "ts", "go", "java", "mm", "m",
    "swift", "zig",
];
/// 错误词表（测试用单一事实源）：`fast_path_covers_all_needles` 逐项校验分派表
/// 与之完全一致（含反向检查），生产路径只读 `ERR_NEEDLES_BY_FIRST`。
#[cfg(test)]
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
pub(crate) fn flags(ll: &str) -> (bool, bool) {
    // 单趟字节扫描覆盖全部词表（含中文/符号）：此前非 ASCII 行退化为对每行
    // 做 20+ 次整串 contains，中文日志上是主要热点。
    fast(ll)
}

/// ASCII 快路径：按首字节分组做单趟扫描，避免对每行执行 20+ 次完整 contains。
///
/// 每个词（ASCII 与中文/符号）必须在这里有对应分支；`fast_path_covers_all_needles`
/// 测试会在新增词表项而忘记接线时失败。
fn fast(ll: &str) -> (bool, bool) {
    let bytes = ll.as_bytes();
    let mut err = false;
    for i in 0..bytes.len() {
        for &(needle, fatal) in ERR_NEEDLES_BY_FIRST[bytes[i] as usize] {
            if bytes[i..].starts_with(needle) {
                if fatal {
                    return (true, true);
                }
                err = true;
            }
        }
    }
    (err, false)
}

/// 首字节 → 该字节可能命中的 ASCII 错误词表；`fast` 的单趟分派表。
/// 词序与原 match 分支一致，保证 `warn` 仅置 err、其余命中即失败级返回。
const ERR_EMPTY: &[(&[u8], bool)] = &[];
const ERR_E: &[(&[u8], bool)] = &[(b"error", true), (b"exception", true)];
const ERR_P: &[(&[u8], bool)] = &[(b"panic", true)];
const ERR_A: &[(&[u8], bool)] = &[(b"assert", true)];
const ERR_D: &[(&[u8], bool)] = &[(b"denied", true)];
const ERR_R: &[(&[u8], bool)] = &[(b"refused", true)];
const ERR_T: &[(&[u8], bool)] = &[
    (b"timeout", true),
    (b"timed out", true),
    (b"traceback", true),
];
const ERR_C: &[(&[u8], bool)] = &[(b"cannot", true)];
const ERR_U: &[(&[u8], bool)] = &[(b"unable", true), (b"undefined", true)];
const ERR_S: &[(&[u8], bool)] = &[(b"segfault", true)];
const ERR_F: &[(&[u8], bool)] = &[(b"fail", true), (b"fatal", true)];
const ERR_N: &[(&[u8], bool)] = &[(b"npm err", true)];
const ERR_W: &[(&[u8], bool)] = &[(b"warn", false)];
// 非 ASCII 词按 UTF-8 首字节分组（✗/❌=E2，失败/异常/崩溃=E5，报错=E6，
// 超时/警告=E8，错误=E9）；`bytes.starts_with` 对多字节序列同样成立。
const ERR_E2: &[(&[u8], bool)] = &[("✗".as_bytes(), true), ("❌".as_bytes(), true)];
const ERR_E5: &[(&[u8], bool)] = &[
    ("失败".as_bytes(), true),
    ("异常".as_bytes(), true),
    ("崩溃".as_bytes(), true),
];
const ERR_E6: &[(&[u8], bool)] = &[("报错".as_bytes(), true)];
const ERR_E8: &[(&[u8], bool)] = &[("超时".as_bytes(), true), ("警告".as_bytes(), false)];
const ERR_E9: &[(&[u8], bool)] = &[("错误".as_bytes(), true)];
const ERR_NEEDLES_BY_FIRST: [&[(&[u8], bool)]; 256] = {
    let mut table = [ERR_EMPTY; 256];
    table[b'e' as usize] = ERR_E;
    table[b'p' as usize] = ERR_P;
    table[b'a' as usize] = ERR_A;
    table[b'd' as usize] = ERR_D;
    table[b'r' as usize] = ERR_R;
    table[b't' as usize] = ERR_T;
    table[b'c' as usize] = ERR_C;
    table[b'u' as usize] = ERR_U;
    table[b's' as usize] = ERR_S;
    table[b'f' as usize] = ERR_F;
    table[b'n' as usize] = ERR_N;
    table[b'w' as usize] = ERR_W;
    table[0xE2] = ERR_E2;
    table[0xE5] = ERR_E5;
    table[0xE6] = ERR_E6;
    table[0xE8] = ERR_E8;
    table[0xE9] = ERR_E9;
    table
};

pub(crate) fn fails(ll: &str) -> bool {
    flags(ll).0
}
// 事故诊断标记行(行首、大小写不敏感):risk HIGH 且 gate 时与错误行一同受守卫;不参与种子判定。
pub(crate) fn marker(ll: &str) -> bool {
    let t = ll.trim_start();
    INCIDENT_MARKERS.iter().any(|m| t.starts_with(m))
}
pub(crate) fn heading(s: &str) -> bool {
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
pub(crate) fn meta(s: &str) -> bool {
    s.starts_with("@@")
        || s.starts_with("+++")
        || s.starts_with("---")
        || s.starts_with("diff --git")
        || s.starts_with("index ")
}
pub(crate) fn diff(s: &str) -> bool {
    // 统一 diff 的正文行是 "+code" / "-code"，而不是只有第二个字符为空白才成立；
    // 三个相同前缀分别是文件头，交给 is_diffmeta 处理。
    (s.starts_with('+') && !s.starts_with("+++")) || (s.starts_with('-') && !s.starts_with("---"))
}
// 带前导空白的行且形如堆栈帧：at ...／File "／#N ／含 path.ext:line。
pub(crate) fn frame(s: &str) -> bool {
    let t = s.trim_start();
    if t.len() == s.len() {
        return false;
    }
    if prefix(t) {
        return true;
    }
    if !t.as_bytes().contains(&b'.') {
        return false;
    }
    dots(t)
}

/// `at `/`File "` 前缀或 `#N` 编号帧（如 `#3 foo`）。
fn prefix(t: &str) -> bool {
    if t.starts_with("at ") || t.starts_with("File \"") {
        return true;
    }
    let hashes = t.chars().take_while(|&c| c == '#').count();
    hashes >= 1
        && t.chars().nth(hashes).is_some_and(|c| c.is_ascii_digit())
        && t.chars().skip(hashes).any(|c| c.is_whitespace())
}

/// 仅为 "." 开头的 ASCII 扩展名扫描。按字节定位 "."，但扩展名用
/// char_indices 截取，避免在多字节 UTF-8 字符中间切片触发 panic。
fn dots(t: &str) -> bool {
    let bytes = t.as_bytes();
    for i in 1..bytes.len() {
        let prev_is_space =
            bytes[i] == b'.' && t[..i].chars().next_back().is_some_and(char::is_whitespace);
        if bytes[i] == b'.' && !prev_is_space && dot(t, i) {
            return true;
        }
    }
    false
}

/// 判定第 `i` 个字节处的 `.` 是否为 `path.ext:digits` 帧后缀。
fn dot(t: &str, i: usize) -> bool {
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
    match colon {
        Some(off) => {
            let ext = &rest[..off];
            EXTS.contains(&ext)
                && rest[off + 1..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit())
        }
        None => false,
    }
}
pub(crate) fn lineref(s: &str) -> bool {
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
pub(crate) fn normalize(s: &str, out: &mut String) {
    if s.is_ascii() {
        ascii(s, out);
        return;
    }
    unicode(s, out);
}

/// 通用 Unicode 归一化；`ascii` 的语义基准。
fn unicode(s: &str, out: &mut String) {
    let mut pending_space = false;
    let mut it: Cursor = s.char_indices().peekable();
    while let Some((start, c)) = it.next() {
        if c.is_ascii_hexdigit() {
            let end = hex_end(&mut it, start, c);
            space(&mut pending_space, out);
            let run = &s[start..end];
            // 无数字的 hex 段折叠后与原文一致，直接拷贝（省掉 fold 的逐字符处理）。
            if run.bytes().any(|b| b.is_ascii_digit()) {
                fold(run, out);
            } else {
                out.push_str(run);
            }
        } else if c.is_ascii_digit() {
            skip(&mut it, |c| c.is_ascii_digit());
            space(&mut pending_space, out);
            out.push('#');
        } else if c.is_whitespace() {
            skip(&mut it, |c| c.is_whitespace());
            pending_space = true;
        } else {
            space(&mut pending_space, out);
            // 整段吞并「普通字符」run 并一次性 push_str：中文日志每行只有少量
            // 数字/空白段，逐字符 push 是 normalize 的主要成本。
            let end = plain(&mut it, start, c);
            out.push_str(&s[start..end]);
        }
    }
}

/// 归一化扫描游标：字符 + 字节起点（普通 run 需要整段切片）。
type Cursor<'a> = std::iter::Peekable<std::str::CharIndices<'a>>;

/// 吞并「普通字符」run（非十六进制/数字/空白），返回切片终点（不含）。
fn plain(it: &mut Cursor<'_>, start: usize, first: char) -> usize {
    let mut end = start + first.len_utf8();
    while let Some(&(j, d)) = it.peek() {
        if d.is_ascii_hexdigit() || d.is_whitespace() {
            break;
        }
        end = j + d.len_utf8();
        it.next();
    }
    end
}

/// 连续十六进制 run 的终点（不含），首字符已消费。
fn hex_end(it: &mut Cursor<'_>, start: usize, first: char) -> usize {
    let mut end = start + first.len_utf8();
    while let Some(&(j, n)) = it.peek() {
        if !n.is_ascii_hexdigit() {
            break;
        }
        end = j + n.len_utf8();
        it.next();
    }
    end
}

/// 跳过满足谓词的连续字符。
fn skip(it: &mut Cursor<'_>, pred: impl Fn(char) -> bool) {
    while let Some(&(_, n)) = it.peek() {
        if !pred(n) {
            break;
        }
        it.next();
    }
}

/// 十六进制 run：>=8 位且含数字视为十六进制值/地址，整体折叠为 `#`；
/// 纯字母串（如 `deadbeef`/`cafebabe` 这类 a-f 魔数名）与 <8 位串按
/// 数字段折叠、字母保留，避免把字母标识符误折叠成数字。
fn fold(run: &str, out: &mut String) {
    if run.len() >= 8 && run.bytes().any(|b| b.is_ascii_digit()) {
        out.push('#');
        return;
    }
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

/// 在非空输出前补写悬空的单空格，并复位标志。
fn space(pending: &mut bool, out: &mut String) {
    if *pending && !out.is_empty() {
        out.push(' ');
    }
    *pending = false;
}

/// ASCII 快路径：与通用路径语义一致，跳过 Unicode 解码与临时十六进制缓冲。
fn ascii(s: &str, out: &mut String) {
    let s = s.as_bytes();
    out.reserve(s.len());
    let mut i = 0usize;
    let mut pending_space = false;
    while i < s.len() {
        let c = s[i];
        if c.is_ascii_hexdigit() {
            let start = i;
            while i < s.len() && s[i].is_ascii_hexdigit() {
                i += 1;
            }
            space(&mut pending_space, out);
            bytes(&s[start..i], out);
        } else if blank(c) {
            while i < s.len() && blank(s[i]) {
                i += 1;
            }
            pending_space = true;
        } else {
            space(&mut pending_space, out);
            out.push(c as char);
            i += 1;
        }
    }
}

/// 十六进制 run：>=8 位且含数字视为十六进制值，整体折叠；纯字母与 <8 位
/// 按字节折叠（数字→`#`，其余保留），与 `fold` 通用路径逐字节一致。
fn bytes(run: &[u8], out: &mut String) {
    if run.len() >= 8 && run.iter().any(|b| b.is_ascii_digit()) {
        out.push('#');
        return;
    }
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

/// `u8::is_ascii_whitespace` 不含垂直制表 U+000B，而 `char::is_whitespace` 含；
/// 快路径显式补齐，保证与通用路径逐字节一致。
fn blank(b: u8) -> bool {
    b == b'\x0b' || b.is_ascii_whitespace()
}

/// 身份归一化：保留数字段，仅折叠空白并做 Unicode 小写。
///
/// 用于 case 去重 / qmap / 会话缓存这类“精确键”。`normalize` 会把 8080/9090、
/// errno=28/29 都折叠成 `#`，适合日志词面匹配，但不适合判断“是不是同一条记忆”。
pub(crate) fn canonical(s: &str, out: &mut String) {
    if s.is_ascii() {
        simple(s, out);
        return;
    }
    wide(s, out);
}

fn simple(s: &str, out: &mut String) {
    let mut pending_space = false;
    for &b in s.as_bytes() {
        if blank(b) {
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

fn wide(s: &str, out: &mut String) {
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
/// 无大小写映射的常用非 ASCII 文字块（CJK/假名/谚文及其扩展）。
fn uncased(c: char) -> bool {
    matches!(
        c as u32,
        0x3040..=0x30FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xAC00..=0xD7AF
            | 0xF900..=0xFAFF
    )
}

pub(crate) fn lowercase(s: &str, out: &mut String) {
    // 日志分析最常见的是无控制符 ASCII 行。按字节小写可跳过 Unicode
    // 大小写迭代与 ESC/CR 状态机；输出语义与通用路径完全一致。
    // 单趟同时确认 ASCII 与无 ESC/CR；避免先 is_ascii 再 any 扫两遍。
    if s.as_bytes()
        .iter()
        .all(|&b| b.is_ascii() && b != b'\x1b' && b != b'\r')
    {
        out.reserve(s.len());
        out.extend(s.bytes().map(|b| b.to_ascii_lowercase() as char));
        return;
    }
    lower_general(s, out);
}

/// 通用路径：ESC 序列清理 + CR 折叠 + 逐字符小写（无大小写文字走快分支）。
fn lower_general(s: &str, out: &mut String) {
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' {
            escape(&mut it);
            continue;
        }
        if c == '\r' {
            out.push(' ');
            continue;
        }
        // 无大小写映射的文字块（CJK/假名/谚文）直接输出，省掉逐字 to_lowercase
        // 迭代器；这些区块不含大小写字符，输出与通用路径逐字节一致。
        if uncased(c) {
            out.push(c);
            continue;
        }
        for lc in c.to_lowercase() {
            out.push(lc);
        }
    }
}

/// 消费一个已见 `ESC` 的 ANSI 序列（CSI / OSC / 单字符）。
fn escape(it: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    match it.peek().copied() {
        Some('[') => csi(it),
        Some(']') => osc(it),
        Some(_) => {
            it.next();
        }
        None => {}
    }
}

/// CSI 序列：`ESC [ ... 终止字节 @..~`。
fn csi(it: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    it.next();
    for esc in it.by_ref() {
        if ('@'..='~').contains(&esc) {
            break;
        }
    }
}

/// OSC 序列：`ESC ] ... BEL` 或 `ESC ] ... ESC \`。
fn osc(it: &mut std::iter::Peekable<std::str::Chars<'_>>) {
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

/// markdown fence 开启行：trim 后以三个反引号/波浪号开头（`rust` 等后缀视为开启）。
pub(crate) fn fence(s: &str) -> bool {
    let t = s.trim_start();
    t.starts_with("```") || t.starts_with("~~~")
}

/// markdown fence 纯闭合行：trim 后恰为三个反引号/波浪号。
pub(crate) fn closes(s: &str) -> bool {
    let t = s.trim_start();
    t == "```" || t == "~~~"
}

/// import/use/#include 块行（连续此类行构成导入块区域；`use strict`/`use of` 除外）。
pub(crate) fn import(s: &str) -> bool {
    let t = s.trim_start();
    if t.starts_with("import ") || t.starts_with("#include") {
        return true;
    }
    t.starts_with("use ") && !t.starts_with("use strict") && !t.starts_with("use of")
}

/// 堆栈链行：at 帧或 `Caused by:` 延续（大小写不敏感）。
pub(crate) fn stack(s: &str) -> bool {
    frame(s) || s.to_ascii_lowercase().contains("caused by:")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 朴素参考实现（批量 run 优化前的逐字符语义），供差分测试。
    fn naive_unicode(s: &str) -> String {
        let mut out = String::new();
        let mut run = String::new();
        let mut pending = false;
        let mut it = s.chars().peekable();
        while let Some(c) = it.next() {
            if c.is_ascii_hexdigit() {
                run.clear();
                run.push(c);
                while let Some(&n) = it.peek() {
                    if !n.is_ascii_hexdigit() {
                        break;
                    }
                    run.push(n);
                    it.next();
                }
                space(&mut pending, &mut out);
                fold(&run, &mut out);
            } else if c.is_ascii_digit() {
                while it.peek().is_some_and(|&n| n.is_ascii_digit()) {
                    it.next();
                }
                space(&mut pending, &mut out);
                out.push('#');
            } else if c.is_whitespace() {
                while it.peek().is_some_and(|&n| n.is_whitespace()) {
                    it.next();
                }
                pending = true;
            } else {
                space(&mut pending, &mut out);
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn unicode_bulk_run_matches_naive() {
        let cases = [
            "服务器　超时  42",
            "a\tb\x0bc\r\nd",
            "deadbeef deadbee abc123 0xDEADBEEF",
            "错误 42 naïve 123456789",
            "conn 9f2a ",
            "纯中文无数字的普通文本",
            "🙂 emoji 混合 ＡＢＣ",
            "abc def",
        ];
        for input in cases {
            let mut got = String::new();
            normalize(input, &mut got);
            assert_eq!(got, naive_unicode(input), "normalize mismatch for {input:?}");
        }
    }

    #[test]
    fn error_flags_split_warn_from_failure() {
        assert_eq!(flags("error: boom"), (true, true));
        assert_eq!(flags("warning only"), (true, false));
        assert_eq!(flags("failed to connect"), (true, true));
        assert_eq!(flags("info heartbeat"), (false, false));
        assert_eq!(flags("warn then error"), (true, true));
    }

    #[test]
    fn fast_path_covers_all_needles() {
        // 反向一致性：分派表不得含词表之外的词（防止接线漂移）。
        let mut wired: Vec<&[u8]> = ERR_NEEDLES_BY_FIRST
            .iter()
            .flat_map(|g| g.iter().map(|(n, _)| *n))
            .collect();
        wired.sort_unstable();
        let mut listed: Vec<&[u8]> = ERR_NEEDLES.iter().map(|n| n.as_bytes()).collect();
        listed.sort_unstable();
        assert_eq!(wired, listed, "dispatch table drifted from ERR_NEEDLES");
        for needle in ERR_NEEDLES.iter() {
            let expected = (true, !matches!(*needle, "warn" | "警告"));
            assert_eq!(
                flags(needle),
                expected,
                "fast path mismatch for needle {needle:?}"
            );
            // 词出现在句中（前有中文/ASCII 前缀）时同样命中。
            let embedded = format!("中文前缀 log {needle} 后续");
            assert_eq!(
                flags(&embedded),
                expected,
                "embedded needle mismatch: {needle:?}"
            );
        }
    }

    #[test]
    fn frame_scan_handles_non_ascii_without_slicing_panic() {
        assert!(!frame("  错误.中文:42"));
        assert!(!frame("  错误.abcdefghi:42"));
        assert!(frame("  错误 at main.rs:42"));
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
            let mut expected = String::new();
            unicode(input, &mut expected);
            let mut actual = String::new();
            normalize(input, &mut actual);
            assert_eq!(actual, expected, "normalize mismatch for {input:?}");
            let mut generic_id = String::new();
            wide(input, &mut generic_id);
            let mut actual_id = String::new();
            canonical(input, &mut actual_id);
            assert_eq!(actual_id, generic_id, "canonical mismatch for {input:?}");
            if input.is_ascii() {
                let mut fast = String::new();
                ascii(input, &mut fast);
                assert_eq!(fast, expected, "ASCII fast path mismatch for {input:?}");
                let mut fast_id = String::new();
                simple(input, &mut fast_id);
                assert_eq!(
                    fast_id, generic_id,
                    "ASCII canonical fast path mismatch for {input:?}"
                );
            }
        }
    }

    #[test]
    fn ascii_chunked_normalization_matches_generic_on_generated_input() {
        // 确定性 LCG 覆盖数字/十六进制/空白/标点边界；分块拷贝不得改变折叠语义。
        let alphabet = b" abcdefABCDEF0123456789[]()=:/.-_\\\t\r\n\x0b";
        let mut state = 0x9e37_79b9u32;
        for _ in 0..4_000 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let len = (state as usize >> 8) % 64;
            let mut input = String::with_capacity(len);
            for _ in 0..len {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                input.push(alphabet[(state as usize >> 16) % alphabet.len()] as char);
            }
            let mut expected = String::new();
            unicode(&input, &mut expected);
            let mut actual = String::new();
            ascii(&input, &mut actual);
            assert_eq!(actual, expected, "chunked mismatch for {input:?}");
        }
    }

    #[test]
    fn normalization_folds_numbers_but_identity_keeps_them() {
        let mut norm = String::new();
        normalize(" cafe 1234 deadbeef 42 错误\t运行 ", &mut norm);
        // 数字段（1234/42）折叠为 #；纯字母 hex（deadbeef）保留词面，不误当数字。
        assert_eq!(norm, "cafe # deadbeef # 错误 运行");

        let mut out = String::new();
        canonical("  Hello\t WORLD 1234 ", &mut out);
        assert_eq!(out, "hello world 1234");
    }
}
