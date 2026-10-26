//! PostToolUse 自动拦截（P0）：压缩 Bash 大输出，减少进上下文的 token。
//!
//! 宿主 hook 协议（Claude Code）：stdin 收 PostToolUse JSON，stdout 写
//! `{"hookSpecificOutput":{"updatedToolOutput":{...}}}` 替换 tool_response
//! （须匹配工具输出结构 `{stdout,stderr,interrupted,isImage}`，否则被忽略）。
//! 小输入 / 无命中 / 非 Bash 一律透传（`{"continue":true}`），零副作用。

#![forbid(unsafe_code)]

use crate::json::{parse, Json};
use crate::{compress, Config, Mode};

/// 核心变换：PostToolUse JSON → hook 输出 JSON；`None` = 透传。
fn transform(input: &str) -> Option<String> {
    let v = parse(input).ok()?;
    if v.get("tool_name").and_then(Json::str).unwrap_or("") != "Bash" {
        return None;
    }
    let resp = v.get("tool_response")?;
    let stdout = resp.get("stdout").and_then(Json::str).unwrap_or("");
    let cmd = v
        .get("tool_input")
        .and_then(|t| t.get("command"))
        .and_then(Json::str)
        .unwrap_or("");
    let query = if cmd.trim().is_empty() {
        "error timeout failed"
    } else {
        cmd
    };
    let o = compress(stdout, query, &Config::default());
    // 只有真正筛选（sieve/overflow）才替换；小输入/无命中透传保留原文。
    if !matches!(
        o.mode,
        Mode::Sieve | Mode::OverflowEvidence | Mode::OverflowFill
    ) {
        return None;
    }
    Some(render_hook(resp, &o.text))
}

/// 组装 `updatedToolOutput`，保留 stderr/interrupted/isImage 结构。
fn render_hook(resp: &Json, stdout: &str) -> String {
    let stderr = resp.get("stderr").and_then(Json::str).unwrap_or("");
    let interrupted = matches!(resp.get("interrupted"), Some(Json::Bool(true)));
    let is_image = matches!(resp.get("isImage"), Some(Json::Bool(true)));
    format!(
        "{{\"hookSpecificOutput\":{{\"updatedToolOutput\":{{\"stdout\":{},\"stderr\":{},\"interrupted\":{},\"isImage\":{}}}}}}}",
        esc(stdout),
        esc(stderr),
        interrupted,
        is_image
    )
}

fn passthrough() -> i32 {
    println!("{{\"continue\":true}}");
    0
}

/// `scx hook` 入口：读 stdin，变换，写 stdout。
pub fn run() -> i32 {
    let mut input = String::new();
    if std::io::Read::read_to_string(&mut std::io::stdin(), &mut input).is_err() {
        return passthrough();
    }
    match transform(&input) {
        Some(out) => {
            println!("{out}");
            0
        }
        None => passthrough(),
    }
}

/// 最小 JSON 字符串转义（stdout/stderr 可能含控制字符与引号）。
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bash_json(stdout: &str) -> String {
        let s = stdout
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n");
        format!(
            "{{\"tool_name\":\"Bash\",\"tool_input\":{{\"command\":\"db timeout\"}},\
\"tool_response\":{{\"stdout\":\"{s}\",\"stderr\":\"\"}}}}"
        )
    }

    #[test]
    fn small_output_passthrough() {
        assert_eq!(transform(&bash_json("hello\nworld\n")), None);
    }

    #[test]
    fn large_output_compressed_and_replaces() {
        let mut lines: Vec<String> = (0..200)
            .map(|i| format!("filler alpha beta line {i}"))
            .collect();
        lines[100] = "ERROR db timeout conn_id=9f2a".to_string();
        let j = bash_json(&lines.join("\n"));
        let out = transform(&j).expect("large output must be compressed");
        assert!(out.contains("hookSpecificOutput"));
        assert!(out.contains("updatedToolOutput"));
        assert!(
            !out.contains("filler alpha beta line 50"),
            "中间噪声应被裁剪"
        );
    }

    #[test]
    fn non_bash_passthrough() {
        let j = "{\"tool_name\":\"Read\",\"tool_response\":{\"stdout\":\"x\"}}";
        assert_eq!(transform(j), None);
    }

    #[test]
    fn esc_round_trips_control_chars() {
        assert_eq!(esc("a\"b\nc"), "\"a\\\"b\\nc\"");
        assert_eq!(esc("tab\tend"), "\"tab\\tend\"");
    }
}
