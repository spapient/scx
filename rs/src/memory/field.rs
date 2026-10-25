//! 结构化字段抽取（O1）：从压缩证据块中抽四类判别字段。
//!
//! 复用 classify.rs 的行分类器（is_err / is_frame / is_lineref）与 norm.rs
//! 的 norm_into，只读证据块、无任何 I/O；摄取路径（memory 侧）调用，
//! compress 零触碰。
//!
//! 字段含义（与设计 v0.2 §2.2 一致）：
//! - error_code：错误行内 `key=val` 邻近 token（conn_id=9f2a、errno=28）
//! - file_loc：堆栈帧/行引用中的 `path:line`
//! - symbol：帧行内的方法/标识符词面
//! - num_fp：数字折叠后的行指纹（norm_into 后含 `#` 的稳定串，跨日志去噪声）

use crate::classify::{is_err, is_frame, is_lineref};
use crate::memory::store::Fields;
use crate::normalize::{norm_into, push_analysis_lower};

pub(crate) const CAP: usize = 16;

/// 四类结构化字段的稳定实体键（排序去重，供倒排与实体边共用）。
pub(crate) fn entity_keys(f: &Fields) -> Vec<String> {
    let mut keys =
        Vec::with_capacity(f.error_code.len() + f.file_loc.len() + f.symbol.len() + f.num_fp.len());
    keys.extend(f.error_code.iter().cloned());
    keys.extend(f.file_loc.iter().cloned());
    keys.extend(f.symbol.iter().cloned());
    keys.extend(f.num_fp.iter().cloned());
    keys.sort();
    keys.dedup();
    keys
}

/// 从证据块抽取四类字段。逐行扫描，去重、截断、封顶。
///
/// 判定用归一化行（norm_into，数字折叠），**提取用原始行**：norm 会把
/// `conn_id=9f2a` 折叠成 `conn_id=#f#a`、`Pool.java:214` 折叠成 `pool.java:#`，
/// 破坏 token/位置结构，故 token、位置、符号都从原始行取并统一小写。
pub(crate) fn extract(text: &str) -> Fields {
    let mut f = Fields::default();
    let mut seen: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut run = String::new();
    let mut ll = String::new();
    for raw in text.lines() {
        buf.clear();
        run.clear();
        norm_into(raw, &mut buf, &mut run);
        ll.clear();
        push_analysis_lower(raw, &mut ll);
        if ll.trim().is_empty() {
            continue;
        }
        if is_err(&ll) {
            collect_err_tokens(raw, &mut f.error_code, &mut seen);
        }
        if is_frame(raw) || is_lineref(&ll) {
            collect_file_loc(raw, &mut f.file_loc, &mut seen);
            collect_symbols(raw, &mut f.symbol, &mut seen);
        }
        if buf.contains('#') {
            push_capped(&mut f.num_fp, &buf, &mut seen);
        }
    }
    f
}

/// 收集错误行中 `ident=num|hex` 形式的判别 token（统一小写）。
fn collect_err_tokens(line: &str, out: &mut Vec<String>, seen: &mut Vec<String>) {
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if is_ident_start(bytes[i]) {
            let s = i;
            while i < bytes.len() && is_ident_char(bytes[i]) {
                i += 1;
            }
            let key = &line[s..i];
            if i < bytes.len() && bytes[i] == b'=' {
                let v = i + 1;
                let mut j = v;
                while j < bytes.len() && is_val_char(bytes[j]) {
                    j += 1;
                }
                if j > v && bytes[v..j].iter().any(|&b| b.is_ascii_digit()) {
                    push_capped(out, &format!("{key}={}", &line[v..j]).to_lowercase(), seen);
                }
            }
        } else {
            i += 1;
        }
    }
}

/// 收集 `path.ext:line` 形式的位置（统一小写）。
fn collect_file_loc(line: &str, out: &mut Vec<String>, seen: &mut Vec<String>) {
    // 从行尾向前找 `:digits` 之后逆推 `path.ext`。
    let bytes = line.as_bytes();
    let mut i = bytes.len();
    while i > 0 {
        i -= 1;
        if bytes[i] == b':' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            let mut j = i;
            while j > 0 && is_path_char(bytes[j - 1]) {
                j -= 1;
            }
            let loc = &line[j..(i + 1 + digit_run(line, i + 1))];
            if loc.contains('.') {
                push_capped(out, &loc.to_lowercase(), seen);
            }
        }
    }
}

/// 收集帧行内标识符（至少 3 字符，非纯路径段；统一小写）。
fn collect_symbols(line: &str, out: &mut Vec<String>, seen: &mut Vec<String>) {
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if is_ident_start(bytes[i]) {
            let s = i;
            while i < bytes.len() && is_ident_char(bytes[i]) {
                i += 1;
            }
            let w = &line[s..i];
            if w.len() >= 3 && !w.contains('.') && !looks_pathy(w) {
                push_capped(out, &w.to_lowercase(), seen);
            }
        } else {
            i += 1;
        }
    }
}

fn digit_run(line: &str, start: usize) -> usize {
    line.as_bytes()[start..]
        .iter()
        .take_while(|&&b| b.is_ascii_digit())
        .count()
}

fn push_capped(out: &mut Vec<String>, v: &str, seen: &mut Vec<String>) {
    if out.len() >= CAP {
        return;
    }
    if v.len() > 128 {
        return;
    }
    if !seen.contains(&v.to_string()) {
        seen.push(v.to_string());
        out.push(v.to_string());
    }
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}
fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}
fn is_val_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_' || b == b':'
}
fn is_path_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'.' || b == b'/' || b == b'\\' || b == b'-' || b == b'_'
}
fn looks_pathy(w: &str) -> bool {
    w.contains('/') || w.ends_with(".rs") || w.ends_with(".java") || w == "at"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_extract_smoke() {
        let text = concat!(
            "ERROR db timeout conn_id=9f2a errno=28\n",
            "    at Pool.java:214 (getConnection)\n",
            "    at rs/lib.rs:88 connect_timeout\n",
            "Root cause: pool exhausted\n",
        );
        let f = extract(text);
        assert!(
            f.error_code.iter().any(|t| t == "conn_id=9f2a"),
            "错误行应抽取 key=val：{:?}",
            f.error_code
        );
        assert!(
            f.error_code.iter().any(|t| t == "errno=28"),
            "errno 应入错误码：{:?}",
            f.error_code
        );
        assert!(
            f.file_loc.iter().any(|t| t == "pool.java:214"),
            "应抽取文件位置：{:?}",
            f.file_loc
        );
        assert!(
            f.file_loc.iter().any(|t| t == "rs/lib.rs:88"),
            "应抽取相对路径位置：{:?}",
            f.file_loc
        );
        assert!(
            f.symbol.iter().any(|t| t == "getconnection"),
            "帧内标识符应入 symbol：{:?}",
            f.symbol
        );
        assert!(
            f.symbol.iter().any(|t| t == "connect_timeout"),
            "帧内方法名应入 symbol：{:?}",
            f.symbol
        );
        assert!(
            f.num_fp.iter().any(|t| t.contains('#')),
            "数字折叠指纹应含 #：{:?}",
            f.num_fp
        );
        // 封顶与去重
        let big = "ERROR x=1\n".repeat(50);
        let fb = extract(&big);
        assert!(fb.error_code.len() <= CAP);
        assert_eq!(fb.error_code.len(), 1, "重复 token 必须去重");
    }
}
