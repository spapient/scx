//! MCP stdio 服务器（零第三方 crate，无条件编译进 scx 主二进制）。
//!
//! - 传输：stdio（逐行 JSON-RPC 2.0，符合 MCP stdio transport）。
//! - 工具：`compress_context` / `connect_account` / `usage_summary`。
//! - 压缩走 `scx::compress`（纯内核）+ `scx::ops::render`（json renderer），
//!   **绝不触碰 CLI 渲染**（正文写 stdout / 统计行 / `scxjson:` 前缀），
//!   stdio 通道不被污染。
//!
//! 入口：`scx mcp` 子命令（main.rs 分派）调用 `serve()`；`serverInfo.name`
//! 为 `"scx"`。压缩主路径与 v0.4.1 逐字节一致。

use crate::json::{parse, Json};

/// 进程内用量记账（单线程 stdio 循环，无需原子）。
#[derive(Default)]
struct Usage {
    compressed: u64,
    input: u64,
    output: u64,
}

/// 工具定义（tools/list 静态表）：名称、描述、JSON Schema。
/// `inputSchema` 为 MCP 客户端（尤其 Claude Code）严格校验的必填对象，
/// 缺失会被判定为 invalid result；无参工具使用空 schema `{}`。
const TOOLS: &[(&str, &str, &str)] = &[
    (
        "compress_context",
        "query-aware extractive compression of a text blob. args: query (str), text (str), optional ratio/context/window/max_tokens (num), structure (bool or 0/1)",
        "{\"type\":\"object\",\"properties\":{\"query\":{\"type\":\"string\"},\"text\":{\"type\":\"string\"},\"ratio\":{\"type\":\"number\"},\"context\":{\"type\":\"number\"},\"window\":{\"type\":\"number\"},\"max_tokens\":{\"type\":\"number\"},\"structure\":{\"type\":\"boolean\"}},\"required\":[\"query\",\"text\"]}",
    ),
    (
        "connect_account",
        "no-op for the local build: returns the local provider identity (stdio transport, no cloud account)",
        "{\"type\":\"object\",\"properties\":{}}",
    ),
    (
        "usage_summary",
        "process-lifetime counts: compressed calls, chars in/out",
        "{\"type\":\"object\",\"properties\":{}}",
    ),
];

/// 单条消息的响应体（`None` = 无响应，如 notifications/initialized）。
fn handle(line: &str, usage: &mut Usage) -> Option<String> {
    let msg = match message(line) {
        Ok(v) => v,
        Err(frame) => return frame,
    };
    let id = msg.get("id").cloned();
    let Some(method) = msg.get("method").and_then(Json::str) else {
        return Some(error(id, -32600, "invalid request: missing method"));
    };
    dispatch(method, id, &msg, usage)
}

/// 解析单条 JSON 消息；语法错误直接产出错误响应帧。
fn message(line: &str) -> Result<Json, Option<String>> {
    parse(line).map_err(|e| Some(error(None, -32700, &format!("parse error: {e}"))))
}

/// 方法级分派：每种方法一个独立小函数，保持本函数单一职责。
fn dispatch(method: &str, id: Option<Json>, msg: &Json, usage: &mut Usage) -> Option<String> {
    let key = wire(&id);
    match method {
        "initialize" => Some(init(&key)),
        "notifications/initialized" | "notifications/cancelled" => None,
        "ping" => Some(ping(&key)),
        "tools/list" => Some(tools(&key)),
        "tools/call" => call(id, msg, usage),
        _ => Some(error(id, -32601, &format!("method not found: {method}"))),
    }
}

fn init(key: &str) -> String {
    format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{key},\"result\":{{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{{\"tools\":{{}}}},\"serverInfo\":{{\"name\":\"scx\",\"version\":\"{}\"}}}}}}",
            env!("CARGO_PKG_VERSION")
        )
}

fn ping(key: &str) -> String {
    format!("{{\"jsonrpc\":\"2.0\",\"id\":{key},\"result\":{{}}}}")
}

fn tools(key: &str) -> String {
    let mut items = String::new();
    for (name, desc, schema) in TOOLS {
        if !items.is_empty() {
            items.push(',');
        }
        items.push_str(&item(name, desc, schema));
    }
    format!("{{\"jsonrpc\":\"2.0\",\"id\":{key},\"result\":{{\"tools\":[{items}]}}}}")
}

fn item(name: &str, desc: &str, schema: &str) -> String {
    format!(
        "{{\"name\":\"{name}\",\"description\":\"{}\",\"inputSchema\":{schema}}}",
        desc.replace('"', "\\\"")
    )
}

fn call(id: Option<Json>, msg: &Json, usage: &mut Usage) -> Option<String> {
    let name = msg
        .get("params")
        .and_then(|p| p.get("name"))
        .and_then(Json::str)
        .unwrap_or("")
        .to_string();
    let args = msg.get("params").and_then(|p| p.get("arguments"));
    Some(match name.as_str() {
        "compress_context" => compress(id, args, usage),
        "connect_account" => account(id),
        "usage_summary" => summary(id, usage),
        _ => error(id, -32602, &format!("unknown tool: {name}")),
    })
}

/// `id` 序列化（数字/字符串/空）。消息里 id 多为数字，宽容处理。
fn wire(id: &Option<Json>) -> String {
    match id {
        Some(Json::Num(n)) => format!("{n}"),
        Some(Json::Str(s)) => format!("\"{s}\""),
        Some(Json::Null) | None => "null".to_string(),
        Some(_) => "null".to_string(),
    }
}

fn error(id: Option<Json>, code: i64, message: &str) -> String {
    format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":{},\"error\":{{\"code\":{code},\"message\":\"{}\"}}}}",
        wire(&id),
        message.replace('"', "\\\"")
    )
}

fn result(id: Option<Json>, text: &str) -> String {
    format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{\"content\":[{{\"type\":\"text\",\"text\":\"{}\"}}],\"isError\":false}}}}",
        wire(&id),
        text.replace('"', "\\\"")
    )
}

/// 从 JSON 值取数值参数（整数语义：f64 → usize，负值钳 0）。
fn number(v: &Json, key: &str) -> Option<usize> {
    v.get(key).and_then(Json::num).map(|n| n.max(0.0) as usize)
}

/// bool 参数：接受 JSON true/false 或 0/1。
fn flag(v: &Json, key: &str) -> bool {
    match v.get(key) {
        Some(Json::Bool(b)) => *b,
        Some(Json::Num(n)) => *n != 0.0,
        _ => false,
    }
}

fn compress(id: Option<Json>, args: Option<&Json>, usage: &mut Usage) -> String {
    let args = match args {
        Some(a) => a,
        None => return error(id, -32602, "compress_context: missing arguments"),
    };
    let query = match args.get("query").and_then(Json::str) {
        Some(q) if !q.is_empty() => q.to_string(),
        _ => return error(id, -32602, "compress_context: missing query"),
    };
    let text = match args.get("text").and_then(Json::str) {
        Some(t) => t.to_string(),
        _ => return error(id, -32602, "compress_context: missing text"),
    };
    let mut cfg = crate::Config::default();
    apply(&mut cfg, args);
    let outcome = crate::compress(&text, &query, &cfg);
    usage.compressed += 1;
    usage.input += text.len() as u64;
    usage.output += outcome.text.len() as u64;
    result(id, &crate::ops::render(&outcome))
}

/// 数值与开关参数落到压缩配置（缺省即保持 `Config::default()`）。
fn apply(cfg: &mut crate::Config, args: &Json) {
    if let Some(r) = number(args, "ratio") {
        cfg.ratio = r.min(1000) as f64 / 1000.0;
    }
    if let Some(c) = number(args, "context") {
        cfg.context = c.min(crate::Config::MAX_CONTEXT as usize) as i64;
    }
    if let Some(w) = number(args, "window") {
        cfg.window = w.min(crate::Config::MAX_WINDOW as usize) as i64;
    }
    if let Some(m) = number(args, "max_tokens") {
        cfg.limit = Some(m);
    }
    cfg.structure = flag(args, "structure");
}

fn account(id: Option<Json>) -> String {
    result(
        id,
        "{\"connected\":true,\"provider\":\"scx-local\",\"transport\":\"stdio\"}",
    )
}

fn summary(id: Option<Json>, usage: &Usage) -> String {
    result(
        id,
        &format!(
            "{{\"compressed\":{},\"chars_in\":{},\"chars_out\":{}}}",
            usage.compressed, usage.input, usage.output
        ),
    )
}

/// stdio 服务循环：逐行读 stdin，逐行回 stdout，EOF 结束。
/// 返回进程退出码（0 = 正常）。
pub fn serve() -> i32 {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut usage = Usage::default();
    let mut line = String::new();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();
    while read(&mut reader, &mut line) {
        if let Some(resp) = handle(line.trim_end(), &mut usage) {
            if !send(&mut writer, &resp) {
                break;
            }
        }
    }
    0
}

fn read(reader: &mut impl BufRead, line: &mut String) -> bool {
    line.clear();
    matches!(reader.read_line(line), Ok(n) if n > 0)
}

fn send(writer: &mut impl Write, response: &str) -> bool {
    writer.write_all(response.as_bytes()).is_ok()
        && writer.write_all(b"\n").is_ok()
        && writer.flush().is_ok()
}

use std::io::{self, BufRead, Write};
