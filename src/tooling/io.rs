//! 评估工具的零依赖 I/O：CSV（RFC 4180 子集）与 Python `json.dump(indent=1)`
//! 兼容的 JSON 输出。仅 `scx-tooling` 使用。
//!
//! CSV 状态机与 JSON 缩进器的分支是线性映射；拆分反而降低可读性。
#![allow(clippy::cognitive_complexity)]

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use scx::json::Json;

type Csv = (Vec<String>, Vec<HashMap<String, String>>);

/// 读取 CSV：返回表头与按表头映射的行。UTF-8 BOM 会被剥离（对应 Python
/// `utf-8-sig`），CRLF/LF 均接受，引号内可含逗号、换行与转义双引号。
pub(crate) fn read_csv(path: &str) -> Result<Csv, String> {
    let records = read_records(path)?;
    csv_from_records(path, records)
}

/// 从内存文本读取 CSV；用于冻结数据集包，避免为只读访问落临时文件。
pub(crate) fn parse_csv(label: &str, text: &str) -> Result<Csv, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    csv_from_records(label, records(text)?)
}

fn csv_from_records(path: &str, records: Vec<Vec<String>>) -> Result<Csv, String> {
    let Some((head, body)) = records.split_first() else {
        return Ok((Vec::new(), Vec::new()));
    };
    let mut rows = Vec::with_capacity(body.len());
    for (index, record) in body.iter().enumerate() {
        if record.len() != head.len() {
            return Err(format!(
                "{path}: row {} has {} columns, expected {}",
                index + 2,
                record.len(),
                head.len()
            ));
        }
        let mut row = HashMap::new();
        for (key, value) in head.iter().zip(record) {
            row.insert(key.clone(), value.clone());
        }
        rows.push(row);
    }
    Ok((head.clone(), rows))
}

/// 读取原始 CSV 记录（含表头，不去重、不映射字段）。`perf` 取首列时使用。
pub(crate) fn read_records(path: &str) -> Result<Vec<Vec<String>>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    parse_records(path, &text)
}

/// 从内存文本读取原始 CSV 记录。
pub(crate) fn parse_records(_path: &str, text: &str) -> Result<Vec<Vec<String>>, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    records(text)
}

fn records(text: &str) -> Result<Vec<Vec<String>>, String> {
    let mut out = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            ',' => row.push(std::mem::take(&mut field)),
            '\r' | '\n' => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                // Python csv 对空行不产出记录；只有真实字段才结束一行。
                if !row.is_empty() || !field.is_empty() {
                    row.push(std::mem::take(&mut field));
                    out.push(std::mem::take(&mut row));
                }
            }
            _ => field.push(c),
        }
    }
    if quoted {
        return Err("CSV: unterminated quoted field".to_string());
    }
    if !row.is_empty() || !field.is_empty() {
        row.push(field);
        out.push(row);
    }
    Ok(out)
}

/// 写 CSV：字段含逗号/引号/换行时按 Excel 默认方言加引号；先写 `.tmp` 再原子替换。
pub(crate) fn write_csv(path: &str, fields: &[String], rows: &[Vec<String>]) -> Result<(), String> {
    write_csv_with(path, fields, rows, "\n")
}

/// 指定行终止符（Python `csv` 默认方言为 CRLF，部分命令未显式改 LF）。
pub(crate) fn write_csv_with(
    path: &str,
    fields: &[String],
    rows: &[Vec<String>],
    terminator: &str,
) -> Result<(), String> {
    write_atomic(path, &render_csv(fields, rows, terminator))
}

/// 渲染 CSV 文本，供文件输出与结果包更新共用。
pub(crate) fn render_csv(fields: &[String], rows: &[Vec<String>], terminator: &str) -> String {
    let mut out = String::new();
    out.push_str(&fields.iter().map(|f| cell(f)).collect::<Vec<_>>().join(","));
    out.push_str(terminator);
    for row in rows {
        out.push_str(&row.iter().map(|f| cell(f)).collect::<Vec<_>>().join(","));
        out.push_str(terminator);
    }
    out
}

/// 原子写文本；先写同目录临时文件，再替换目标。
pub(crate) fn write_text(path: &str, text: &str) -> Result<(), String> {
    write_atomic(path, text)
}

fn cell(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// 极简 JSON 值，只覆盖评估产出需要的类型。
pub(crate) enum J {
    Null,
    Bool(bool),
    Int(i64),
    Num(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

/// 可缩进 JSON 值转换：保留对象键序，整数型 `Num` 还原为整数。
pub(crate) fn to_j(value: &Json) -> J {
    match value {
        Json::Null => J::Null,
        Json::Bool(flag) => J::Bool(*flag),
        Json::Num(number) => {
            if number.fract() == 0.0 && number.abs() < 9e15 {
                J::Int(*number as i64)
            } else {
                J::Num(*number)
            }
        }
        Json::Str(text) => J::Str(text.clone()),
        Json::Arr(items) => J::Arr(items.iter().map(to_j).collect()),
        Json::Obj(entries) => J::Obj(
            entries
                .iter()
                .map(|(key, value)| (key.clone(), to_j(value)))
                .collect(),
        ),
    }
}

/// 读整数：接受整数型 JSON 数字或可解析的十进制字符串；其他类型返回 `None`。
pub(crate) fn as_i64(value: &Json) -> Option<i64> {
    match value {
        Json::Num(number) if number.fract() == 0.0 => Some(*number as i64),
        Json::Str(text) => text.parse().ok(),
        _ => None,
    }
}

/// 读字符串数组：非数组或缺键返回空表，非字符串元素忽略。
pub(crate) fn strings(value: &Json, key: &str) -> Vec<String> {
    match value.get(key) {
        Some(Json::Arr(items)) => items
            .iter()
            .filter_map(Json::str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// 写 JSON：与 Python `json.dump(ensure_ascii=False, indent=1)` 对齐。
pub(crate) fn write_json(path: &str, value: &J) -> Result<(), String> {
    write_json_indent(path, value, 1)
}

/// 指定缩进宽度（冻结语料可能用 indent=2）。
pub(crate) fn write_json_indent(path: &str, value: &J, width: usize) -> Result<(), String> {
    write_atomic(path, &render_json(value, width))
}

/// 与 Python `json.dumps(..., indent=width)` + 末尾换行一致。
pub(crate) fn render_json(value: &J, width: usize) -> String {
    let mut out = String::new();
    pretty(value, 0, width, &mut out);
    out.push('\n');
    out
}

/// 不带末尾换行的 JSON（Python `json.dump` 语义）。
pub(crate) fn render_json_raw(value: &J, width: usize) -> String {
    let mut out = String::new();
    pretty(value, 0, width, &mut out);
    out
}

pub(crate) fn write_json_raw(path: &str, value: &J, width: usize) -> Result<(), String> {
    write_atomic(path, &render_json_raw(value, width))
}

fn write_atomic(path: &str, text: &str) -> Result<(), String> {
    let target = Path::new(path);
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| format!("{path}: {e}"))?;
        }
    }
    let tmp = format!("{path}.tmp");
    fs::write(&tmp, text).map_err(|e| format!("{path}: {e}"))?;
    fs::rename(&tmp, target).map_err(|e| format!("{path}: {e}"))
}

fn pretty(value: &J, level: usize, width: usize, out: &mut String) {
    match value {
        J::Null => out.push_str("null"),
        J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        J::Int(i) => out.push_str(&i.to_string()),
        J::Num(n) => out.push_str(&repr(*n)),
        J::Str(s) => push_str(s, out),
        J::Arr(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                indent(level + 1, width, out);
                pretty(item, level + 1, width, out);
                out.push_str(if i + 1 == items.len() { "\n" } else { ",\n" });
            }
            indent(level, width, out);
            out.push(']');
        }
        J::Obj(entries) => {
            if entries.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{\n");
            for (i, (key, item)) in entries.iter().enumerate() {
                indent(level + 1, width, out);
                push_str(key, out);
                out.push_str(": ");
                pretty(item, level + 1, width, out);
                out.push_str(if i + 1 == entries.len() { "\n" } else { ",\n" });
            }
            indent(level, width, out);
            out.push('}');
        }
    }
}

fn indent(level: usize, width: usize, out: &mut String) {
    for _ in 0..level * width {
        out.push(' ');
    }
}

/// JSON 字符串字面量：与 Python `ensure_ascii=False` 的转义集合一致
/// （含 `\b`/`\f`，其余 C0 用 `\u00xx`）。
/// 完整 JSON 字符串字面量（Python `json.dumps(ensure_ascii=False)` 口径）。
pub(crate) fn json_string(s: &str) -> String {
    let mut out = String::new();
    push_str(s, &mut out);
    out
}

fn push_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Python `round(x, 4)` 的 4 位小数值（返回最接近该十进制值的 f64）。
pub(crate) fn round4(x: f64) -> f64 {
    format!("{x:.4}").parse().unwrap_or(x)
}

pub(crate) fn round6(x: f64) -> f64 {
    format!("{x:.6}").parse().unwrap_or(x)
}

/// Python `str(float)` / `repr(float)` 的近似：最短往返表示，整数补 `.0`。
pub(crate) fn repr(x: f64) -> String {
    if x.is_nan() {
        return "NaN".to_string();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0" } else { "0.0" }.to_string();
    }
    let abs = x.abs();
    if !(1e-4..1e16).contains(&abs) {
        return exponent(x);
    }
    let s = format!("{x}");
    if s.contains('.') || s.contains('e') {
        s
    } else {
        format!("{s}.0")
    }
}

fn exponent(x: f64) -> String {
    let s = format!("{x:e}");
    let Some((mantissa, exp)) = s.split_once('e') else {
        return s;
    };
    let exp: i32 = exp.parse().unwrap_or(0);
    let sign = if exp < 0 { '-' } else { '+' };
    format!("{mantissa}e{sign}{:02}", exp.abs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_roundtrip_quotes_and_newlines() {
        let dir = crate::testsupport::tmpdir("csv");
        let path = format!("{dir}.csv");
        let fields = vec!["a".to_string(), "b".to_string()];
        let rows = vec![
            vec!["1".to_string(), "x,y".to_string()],
            vec!["2".to_string(), "line\nbreak".to_string()],
            vec!["3".to_string(), "say \"hi\"".to_string()],
        ];
        write_csv(&path, &fields, &rows).unwrap();
        let (head, got) = read_csv(&path).unwrap();
        assert_eq!(head, fields);
        assert_eq!(got.len(), 3);
        assert_eq!(got[1]["b"], "line\nbreak");
        assert_eq!(got[2]["b"], "say \"hi\"");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn json_pretty_matches_python_indent_one() {
        let value = J::Obj(vec![
            ("cases".to_string(), J::Arr(vec![])),
            ("n".to_string(), J::Int(2)),
            ("x".to_string(), J::Num(0.5)),
            ("s".to_string(), J::Str("数据库".to_string())),
        ]);
        let mut out = String::new();
        pretty(&value, 0, 1, &mut out);
        assert_eq!(
            out,
            "{\n \"cases\": [],\n \"n\": 2,\n \"x\": 0.5,\n \"s\": \"数据库\"\n}"
        );
    }

    #[test]
    fn float_repr_matches_python_examples() {
        assert_eq!(repr(1.0), "1.0");
        assert_eq!(repr(0.0), "0.0");
        assert_eq!(repr(0.15), "0.15");
        assert_eq!(repr(1e-5), "1e-05");
        assert_eq!(repr(1e16), "1e+16");
    }
}
