//! 宿主事件自动拦截：事件表驱动的通用 hook，覆盖会话与每轮消息。
//!
//! 支持的 `hook_event_name`：
//! - Claude Code / Codex（PascalCase）：`SessionStart` / `UserPromptSubmit` /
//!   `PostToolUse` / `Stop`；
//! - Cursor（camelCase，官方 hooks.json）：`sessionStart` / `beforeSubmitPrompt` /
//!   `postToolUse` / `stop`。
//!
//! 任何路径都不静默：判定结果与 token 口径以 `scx-stat-1` 机器字段返回；
//! 不识别的事件 / 非 UTF-8 / 超限载荷一律安全透传。

#![forbid(unsafe_code)]

use crate::json::{parse, string, Json};
use crate::{compress, Config, Mode, Outcome};
use std::io::Read;

/// 待判定正文与查询词来源。
struct Payload {
    text: String,
    query: String,
}

/// 事件名：缺省按 PostToolUse 兼容旧宿主。
fn event_name(v: &Json) -> &str {
    v.get("hook_event_name")
        .and_then(Json::str)
        .unwrap_or("PostToolUse")
}

/// 是否为 Cursor 宿主：camelCase 事件名，或 Cursor 专有的 `tool_output` 字段。
fn is_cursor(v: &Json) -> bool {
    let lower = event_name(v)
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase());
    lower || v.get("tool_output").is_some()
}

/// Cursor 事件名归一化到内部 PascalCase 口径。
fn norm_event(name: &str) -> &str {
    match name {
        "sessionStart" => "SessionStart",
        "beforeSubmitPrompt" => "UserPromptSubmit",
        "postToolUse" => "PostToolUse",
        "stop" => "Stop",
        other => other,
    }
}

/// 工具事件取数：正文取 stdout，查询词取命令文本，逐级回退。
///
/// 宿主口径有两种：Claude Code 的 `tool_response` 是对象（含 stdout 字段）；
/// Codex 的 `tool_response` 直接是字符串。两者都兼容。
fn tool_payload(v: &Json) -> Option<Payload> {
    let resp = v.get("tool_response")?;
    let text = match resp {
        Json::Str(s) => s.clone(),
        _ => resp
            .get("stdout")
            .and_then(Json::str)
            .unwrap_or("")
            .to_string(),
    };
    let tool = v.get("tool_name").and_then(Json::str).unwrap_or("");
    let cmd = v
        .get("tool_input")
        .and_then(|t| t.get("command"))
        .and_then(Json::str)
        .unwrap_or("");
    let query = match (cmd.trim(), tool.trim()) {
        ("", "") => "error timeout failed".to_string(),
        ("", t) => t.to_string(),
        (c, _) => c.to_string(),
    };
    Some(Payload { text, query })
}

/// 消息事件取数：prompt 同时作为正文与查询词。
fn prompt_payload(v: &Json) -> Payload {
    let text = v
        .get("prompt")
        .and_then(Json::str)
        .unwrap_or("")
        .to_string();
    let query = text.clone();
    Payload { text, query }
}

/// Cursor 工具事件取数：`tool_output` 是 JSON 字符串化的载荷，
/// 若为含 stdout 的对象则提取 stdout，否则按原文处理。
fn cursor_tool_payload(v: &Json) -> Payload {
    let raw = v.get("tool_output").and_then(Json::str).unwrap_or("");
    let text = match parse(raw) {
        Ok(j) => j
            .get("stdout")
            .and_then(Json::str)
            .map(str::to_string)
            .unwrap_or_else(|| raw.to_string()),
        Err(_) => raw.to_string(),
    };
    let tool = v.get("tool_name").and_then(Json::str).unwrap_or("");
    let cmd = v
        .get("tool_input")
        .and_then(|t| t.get("command"))
        .and_then(Json::str)
        .unwrap_or("");
    let query = match (cmd.trim(), tool.trim()) {
        ("", "") => "error timeout failed".to_string(),
        ("", t) => t.to_string(),
        (c, _) => c.to_string(),
    };
    Payload { text, query }
}

/// 真正发生筛选的模式。
fn is_sieve(o: &Outcome) -> bool {
    matches!(
        o.mode,
        Mode::Sieve | Mode::OverflowEvidence | Mode::OverflowFill
    )
}

/// 机器统计：统一走 `scx-stat-1` 模型，恒等路径字段由 stat 层回填。
fn stat_json(o: &Outcome) -> String {
    crate::stat::Stat::from_outcome(o).to_json()
}

/// 无正文事件（SessionStart/Stop）的恒等统计。
fn simple_stat(reason: &str) -> String {
    format!("{{\"schema\":\"scx-stat-1\",\"decision\":\"passthrough\",\"reason\":\"{reason}\"}}")
}

/// 包一层 systemMessage（UI 可见，不改变上下文）。
fn wrap_system(msg: &str) -> String {
    format!(
        "{{\"continue\":true,\"hookSpecificOutput\":{{\"systemMessage\":{}}}}}",
        string(msg)
    )
}

/// 包一层 additionalContext（作为上下文注入）。
fn wrap_context(msg: &str) -> String {
    format!(
        "{{\"continue\":true,\"hookSpecificOutput\":{{\"additionalContext\":{}}}}}",
        string(msg)
    )
}

/// 组装 `updatedToolOutput`，保留 stderr/interrupted/isImage 结构。
fn render_hook(resp: &Json, stdout: &str) -> String {
    let stderr = resp.get("stderr").and_then(Json::str).unwrap_or("");
    let interrupted = matches!(resp.get("interrupted"), Some(Json::Bool(true)));
    let is_image = matches!(resp.get("isImage"), Some(Json::Bool(true)));
    format!(
        "{{\"hookSpecificOutput\":{{\"updatedToolOutput\":{{\"stdout\":{},\"stderr\":{},\
\"interrupted\":{},\"isImage\":{}}}}}}}",
        string(stdout),
        string(stderr),
        interrupted,
        is_image
    )
}

/// 工具事件路由：Bash 且响应为对象时走替换（Claude Code 结构），
/// 其余情况（含 Codex 的字符串响应）只回报统计，不做未文档化的结构假设。
fn route_tool(v: &Json) -> String {
    let Some(p) = tool_payload(v) else {
        return wrap_system(&simple_stat("no-tool-response"));
    };
    let o = compress(&p.text, &p.query, &Config::default());
    let tool = v.get("tool_name").and_then(Json::str).unwrap_or("");
    let resp = v.get("tool_response").expect("tool_payload 已确认存在");
    if tool == "Bash" && is_sieve(&o) && matches!(resp, Json::Obj(_)) {
        render_hook(resp, &o.text)
    } else {
        wrap_system(&stat_json(&o))
    }
}

/// 消息事件路由：命中时以 additionalContext 注入筛选文本，否则回报统计。
fn route_prompt(v: &Json) -> String {
    let p = prompt_payload(v);
    let o = compress(&p.text, &p.query, &Config::default());
    if is_sieve(&o) {
        wrap_context(&o.text)
    } else {
        wrap_context(&stat_json(&o))
    }
}

// ---------- Cursor 原生契约（snake_case 字段） ----------

/// Cursor 上下文注入（sessionStart/postToolUse）。
fn cursor_context(msg: &str) -> String {
    format!("{{\"additional_context\":{}}}", string(msg))
}

/// Cursor postToolUse：命中压缩时注入筛选文本，否则注入统计。
fn route_cursor_tool(v: &Json) -> String {
    let p = cursor_tool_payload(v);
    let o = compress(&p.text, &p.query, &Config::default());
    if is_sieve(&o) {
        cursor_context(&o.text)
    } else {
        cursor_context(&stat_json(&o))
    }
}

/// Cursor beforeSubmitPrompt：官方仅文档化阻断语义，恒等放行。
fn route_cursor_prompt() -> String {
    "{\"continue\":true}".to_string()
}

/// 核心路由：事件 JSON → 宿主契约输出。
fn route(input: &str) -> Option<String> {
    let v = parse(input).ok()?;
    if is_cursor(&v) {
        let out = match norm_event(event_name(&v)) {
            "PostToolUse" => route_cursor_tool(&v),
            "UserPromptSubmit" => route_cursor_prompt(),
            "SessionStart" => cursor_context(&simple_stat("session-open")),
            "Stop" => "{}".to_string(),
            _ => return None,
        };
        return Some(out);
    }
    let out = match event_name(&v) {
        "PostToolUse" => route_tool(&v),
        "UserPromptSubmit" => route_prompt(&v),
        "SessionStart" => wrap_context(&simple_stat("session-open")),
        "Stop" => wrap_system(&simple_stat("turn-stop")),
        _ => return None,
    };
    Some(out)
}

/// 读完 stdin 的剩余字节，避免父进程写入时收到 EPIPE。
fn drain<R: Read>(reader: &mut R) {
    let mut buf = [0u8; 64 * 1024];
    while let Ok(n) = reader.read(&mut buf) {
        if n == 0 {
            break;
        }
    }
}

/// 读取 hook 载荷；超限先 drain 再返回 None（透传）。
fn read_hook_input<R: Read>(reader: &mut R, limit: u64) -> Option<Vec<u8>> {
    let bytes = crate::input::read(&mut *reader, limit).ok()?;
    if crate::input::large(bytes.len(), limit) {
        drain(reader);
        return None;
    }
    Some(bytes)
}

/// `scx hook` 入口：读 stdin，路由，写 stdout。
pub fn run() -> i32 {
    // 复用 CLI 的 64 MiB 上限；超限或非 UTF-8 时安全透传，不改变宿主行为。
    let limit = crate::input::MAX_INPUT_BYTES;
    let mut stdin = std::io::stdin().lock();
    let Some(bytes) = read_hook_input(&mut stdin, limit) else {
        let _ = crate::ops::out_line("{\"continue\":true}");
        return 0;
    };
    let input = match String::from_utf8(bytes) {
        Ok(input) => input,
        Err(_) => {
            let _ = crate::ops::out_line("{\"continue\":true}");
            return 0;
        }
    };
    let out = route(&input).unwrap_or_else(|| "{\"continue\":true}".to_string());
    let _ = crate::ops::out_line(&out);
    0
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
            "{{\"hook_event_name\":\"PostToolUse\",\"tool_name\":\"Bash\",\
\"tool_input\":{{\"command\":\"db timeout\"}},\
\"tool_response\":{{\"stdout\":\"{s}\",\"stderr\":\"\",\"interrupted\":false,\"isImage\":false}}}}"
        )
    }

    #[test]
    fn small_output_passthrough_with_stats() {
        let out = route(&bash_json("hello\nworld\n")).expect("必有结构化回执");
        assert!(out.contains("systemMessage"), "got: {out}");
        // 统计作为 JSON 字符串值嵌入，引号为转义形态。
        assert!(
            out.contains("\\\"decision\\\":\\\"passthrough\\\""),
            "got: {out}"
        );
    }

    #[test]
    fn large_bash_output_is_replaced() {
        let lines: Vec<String> = (0..200)
            .map(|i| format!("filler alpha beta line {i}"))
            .collect();
        let mut lines = lines;
        lines[100] = "ERROR db timeout conn_id=9f2a".to_string();
        let out = route(&bash_json(&lines.join("\n"))).expect("必有回执");
        assert!(out.contains("updatedToolOutput"));
        assert!(
            !out.contains("filler alpha beta line 50"),
            "中间噪声应被裁剪"
        );
    }

    #[test]
    fn non_bash_tool_never_replaces_but_reports() {
        let j = "{\"hook_event_name\":\"PostToolUse\",\"tool_name\":\"Read\",\
\"tool_response\":{\"stdout\":\"x\"}}";
        let out = route(j).expect("必有回执");
        assert!(out.contains("systemMessage"), "非 Bash 不应做结构替换");
        assert!(!out.contains("updatedToolOutput"));
    }

    #[test]
    fn prompt_submit_always_returns_context() {
        let j = "{\"hook_event_name\":\"UserPromptSubmit\",\"prompt\":\"你好\"}";
        let out = route(j).expect("必有回执");
        assert!(out.contains("additionalContext"));
        assert!(out.contains("scx-stat-1"));
    }

    #[test]
    fn session_start_and_stop_emit_identity_stats() {
        let start = route("{\"hook_event_name\":\"SessionStart\",\"source\":\"startup\"}")
            .expect("必有回执");
        assert!(start.contains("session-open"));
        let stop = route("{\"hook_event_name\":\"Stop\"}").expect("必有回执");
        assert!(stop.contains("turn-stop"));
    }

    #[test]
    fn unknown_event_is_plain_passthrough() {
        assert!(route("{\"hook_event_name\":\"SubagentStart\"}").is_none());
    }

    #[test]
    fn missing_event_name_defaults_to_post_tool_use() {
        let j = "{\"tool_name\":\"Bash\",\"tool_response\":{\"stdout\":\"x\"}}";
        let out = route(j).expect("缺省事件也应回执");
        assert!(out.contains("systemMessage"));
    }

    #[test]
    fn over_limit_drains_the_rest_of_input() {
        use std::io::Cursor;
        let mut reader = Cursor::new(vec![b'a'; 100]);
        assert!(read_hook_input(&mut reader, 10).is_none());
        assert_eq!(reader.position(), 100, "超限后必须读完 stdin，避免 EPIPE");
    }

    #[test]
    fn cursor_events_use_native_contract() {
        let start = route("{\"hook_event_name\":\"sessionStart\"}").expect("回执");
        assert!(start.contains("additional_context") && start.contains("session-open"));
        let prompt = route("{\"hook_event_name\":\"beforeSubmitPrompt\",\"prompt\":\"你好\"}")
            .expect("回执");
        assert!(prompt.contains("\"continue\":true"));
        let stop = route("{\"hook_event_name\":\"stop\"}").expect("回执");
        assert!(stop.contains("{}"));
    }

    #[test]
    fn cursor_post_tool_unwraps_json_string_output() {
        let lines: Vec<String> = (0..200)
            .map(|i| format!("filler alpha beta line {i}"))
            .collect();
        let mut lines = lines;
        lines[100] = "ERROR db timeout conn_id=9f2a".to_string();
        let inner = format!(
            "{{\"exitCode\":0,\"stdout\":\"{}\"}}",
            lines.join("\n").replace('\\', "\\\\").replace('"', "\\\"")
        );
        let j = format!(
            "{{\"hook_event_name\":\"postToolUse\",\"tool_name\":\"Shell\",\
\"tool_input\":{{\"command\":\"db timeout\"}},\"tool_output\":{}}}",
            string(&inner)
        );
        let out = route(&j).expect("回执");
        assert!(out.contains("additional_context"));
        assert!(out.contains("db timeout"), "应解包 stdout 后压缩: {out}");
        assert!(!out.contains("filler alpha beta line 50"));
    }
}
