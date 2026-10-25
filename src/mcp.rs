//! scx-mcp：MCP stdio 服务器（零第三方 crate）。
//!
//! - 传输：stdio（逐行 JSON-RPC 2.0，符合 MCP stdio transport）。
//! - 工具：`compress_context` / `connect_account` / `usage_summary`。
//! - 压缩走 `scx::compress`（纯内核）+ `scx::ops::render_json`（json renderer），
//!   **绝不触碰 CLI 渲染**（正文写 stdout / 统计行 / `scxjson:` 前缀），
//!   stdio 通道不被污染。
//!
//! 本文件既是 MCP 服务器实现，也是 `scx-mcp` 二进制的入口（`main` 在文末）。
//! 二进制仅在 `--features mcp` 下构建（Cargo.toml `required-features`）；
//! 默认构建不含 MCP 代码，压缩主路径与 v0.4.1 逐字节一致。

#![forbid(unsafe_code)]
#![warn(clippy::cognitive_complexity)]

use scx::json::{parse_json, Json};

/// 进程内用量记账（单线程 stdio 循环，无需原子）。
#[derive(Default)]
struct Usage {
    compressed: u64,
    chars_in: u64,
    chars_out: u64,
}

/// 工具定义（tools/list 静态表）。
const TOOLS: &[(&str, &str)] = &[
    (
        "compress_context",
        "query-aware extractive compression of a text blob. args: query (str), text (str), optional ratio/context/window/max_tokens (num), structure (bool or 0/1)",
    ),
    (
        "connect_account",
        "no-op for the local build: returns the local provider identity (stdio transport, no cloud account)",
    ),
    (
        "usage_summary",
        "process-lifetime counts: compressed calls, chars in/out",
    ),
];

/// 单条消息的响应体（`None` = 无响应，如 notifications/initialized）。
fn handle(line: &str, usage: &mut Usage) -> Option<String> {
    let msg = match parse_msg(line) {
        Ok(v) => v,
        Err(frame) => return frame,
    };
    let id = msg.get("id").cloned();
    let Some(method) = msg.get("method").and_then(Json::str) else {
        return Some(error_frame(id, -32600, "invalid request: missing method"));
    };
    dispatch_method(method, id, &msg, usage)
}

/// 解析单条 JSON 消息；语法错误直接产出错误响应帧。
fn parse_msg(line: &str) -> Result<Json, Option<String>> {
    parse_json(line).map_err(|e| Some(error_frame(None, -32700, &format!("parse error: {e}"))))
}

/// 方法级分派：每种方法一个独立小函数，保持本函数单一职责。
fn dispatch_method(
    method: &str,
    id: Option<Json>,
    msg: &Json,
    usage: &mut Usage,
) -> Option<String> {
    let id_str = id_string(&id);
    match method {
        "initialize" => Some(init_frame(&id_str)),
        "notifications/initialized" | "notifications/cancelled" => None,
        "ping" => Some(ping_frame(&id_str)),
        "tools/list" => Some(tools_list(&id_str)),
        "tools/call" => tools_call(id, msg, usage),
        _ => Some(error_frame(
            id,
            -32601,
            &format!("method not found: {method}"),
        )),
    }
}

fn init_frame(id_str: &str) -> String {
    format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{id_str},\"result\":{{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{{\"tools\":{{}}}},\"serverInfo\":{{\"name\":\"scx-mcp\",\"version\":\"{}\"}}}}}}",
            env!("CARGO_PKG_VERSION")
        )
}

fn ping_frame(id_str: &str) -> String {
    format!("{{\"jsonrpc\":\"2.0\",\"id\":{id_str},\"result\":{{}}}}")
}

fn tools_list(id_str: &str) -> String {
    let mut items = String::new();
    for (name, desc) in TOOLS {
        if !items.is_empty() {
            items.push(',');
        }
        items.push_str(&tool_item(name, desc));
    }
    format!("{{\"jsonrpc\":\"2.0\",\"id\":{id_str},\"result\":{{\"tools\":[{items}]}}}}")
}

fn tool_item(name: &str, desc: &str) -> String {
    format!(
        "{{\"name\":\"{name}\",\"description\":\"{}\"}}",
        desc.replace('"', "\\\"")
    )
}

fn tools_call(id: Option<Json>, msg: &Json, usage: &mut Usage) -> Option<String> {
    let name = msg
        .get("params")
        .and_then(|p| p.get("name"))
        .and_then(Json::str)
        .unwrap_or("")
        .to_string();
    let args = msg.get("params").and_then(|p| p.get("arguments"));
    Some(match name.as_str() {
        "compress_context" => call_compress(id, args, usage),
        "connect_account" => call_account(id),
        "usage_summary" => call_usage(id, usage),
        _ => error_frame(id, -32602, &format!("unknown tool: {name}")),
    })
}

/// `id` 序列化（数字/字符串/空）。消息里 id 多为数字，宽容处理。
fn id_string(id: &Option<Json>) -> String {
    match id {
        Some(Json::Num(n)) => format!("{n}"),
        Some(Json::Str(s)) => format!("\"{s}\""),
        Some(Json::Null) | None => "null".to_string(),
        Some(_) => "null".to_string(),
    }
}

fn error_frame(id: Option<Json>, code: i64, message: &str) -> String {
    format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":{},\"error\":{{\"code\":{code},\"message\":\"{}\"}}}}",
        id_string(&id),
        message.replace('"', "\\\"")
    )
}

fn tool_result_frame(id: Option<Json>, text: &str) -> String {
    format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{\"content\":[{{\"type\":\"text\",\"text\":\"{}\"}}],\"isError\":false}}}}",
        id_string(&id),
        text.replace('"', "\\\"")
    )
}

/// 从 JSON 值取数值参数（整数语义：f64 → usize，负值钳 0）。
fn num_arg(v: &Json, key: &str) -> Option<usize> {
    v.get(key).and_then(Json::num).map(|n| n.max(0.0) as usize)
}

/// bool 参数：接受 JSON true/false 或 0/1。
fn bool_arg(v: &Json, key: &str) -> bool {
    match v.get(key) {
        Some(Json::Bool(b)) => *b,
        Some(Json::Num(n)) => *n != 0.0,
        _ => false,
    }
}

fn call_compress(id: Option<Json>, args: Option<&Json>, usage: &mut Usage) -> String {
    let args = match args {
        Some(a) => a,
        None => return error_frame(id, -32602, "compress_context: missing arguments"),
    };
    let query = match args.get("query").and_then(Json::str) {
        Some(q) if !q.is_empty() => q.to_string(),
        _ => return error_frame(id, -32602, "compress_context: missing query"),
    };
    let text = match args.get("text").and_then(Json::str) {
        Some(t) => t.to_string(),
        _ => return error_frame(id, -32602, "compress_context: missing text"),
    };
    let mut cfg = scx::Config::default();
    apply_num_cfg(&mut cfg, args);
    let outcome = scx::compress(&text, &query, &cfg);
    usage.compressed += 1;
    usage.chars_in += text.len() as u64;
    usage.chars_out += outcome.text.len() as u64;
    tool_result_frame(id, &scx::ops::render_json(&outcome))
}

/// 数值与开关参数落到压缩配置（缺省即保持 `Config::default()`）。
fn apply_num_cfg(cfg: &mut scx::Config, args: &Json) {
    if let Some(r) = num_arg(args, "ratio") {
        cfg.ratio = r.min(1000) as f64 / 1000.0;
    }
    if let Some(c) = num_arg(args, "context") {
        cfg.context = c.min(scx::Config::MAX_CONTEXT as usize) as i64;
    }
    if let Some(w) = num_arg(args, "window") {
        cfg.window = w.min(scx::Config::MAX_WINDOW as usize) as i64;
    }
    if let Some(m) = num_arg(args, "max_tokens") {
        cfg.max_tokens = Some(m);
    }
    cfg.structure = bool_arg(args, "structure");
}

fn call_account(id: Option<Json>) -> String {
    tool_result_frame(
        id,
        "{\"connected\":true,\"provider\":\"scx-local\",\"transport\":\"stdio\"}",
    )
}

fn call_usage(id: Option<Json>, usage: &Usage) -> String {
    tool_result_frame(
        id,
        &format!(
            "{{\"compressed\":{},\"chars_in\":{},\"chars_out\":{}}}",
            usage.compressed, usage.chars_in, usage.chars_out
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
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        if let Some(resp) = handle(line.trim_end(), &mut usage) {
            if writer.write_all(resp.as_bytes()).is_err() {
                break;
            }
            if writer.write_all(b"\n").is_err() {
                break;
            }
            if writer.flush().is_err() {
                break;
            }
        }
    }
    0
}

use std::io::{self, BufRead, Write};

/// 二进制入口：stdio 服务循环，退出码 0 = 正常。
fn main() {
    std::process::exit(serve());
}
