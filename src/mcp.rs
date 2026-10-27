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

/// 进程内状态（单线程 stdio 循环，无需原子）：用量计数 + 快照账本 + 轮次。
#[derive(Default)]
struct Usage {
    compressed: u64,
    input: u64,
    output: u64,
    round: u64,
    ledger: crate::meter::Ledger,
}

/// 工具定义（tools/list 静态表）：名称、描述、JSON Schema。
/// `inputSchema` 为 MCP 客户端（尤其 Claude Code）严格校验的必填对象，
/// 缺失会被判定为 invalid result；无参工具使用空 schema `{}`。
const TOOLS: &[(&str, &str, &str)] = &[
    (
        "compress_context",
        "query-aware extractive compression of a text blob. args: query (str), text (str), optional ratio (0..=1 fraction of optional-content budget) / context / window / max_tokens (num), structure (bool or 0/1), scoped (bool or 0/1), strict (bool or 0/1), kind (str: static-contract/system-definition/session-message/tool-output/auto)",
        "{\"type\":\"object\",\"properties\":{\"query\":{\"type\":\"string\"},\"text\":{\"type\":\"string\"},\"ratio\":{\"type\":\"number\"},\"context\":{\"type\":\"number\"},\"window\":{\"type\":\"number\"},\"max_tokens\":{\"type\":\"number\"},\"structure\":{\"type\":\"boolean\"},\"scoped\":{\"type\":\"boolean\"},\"strict\":{\"type\":\"boolean\"},\"kind\":{\"type\":\"string\"},\"evidence\":{\"type\":\"array\",\"items\":{\"type\":[\"string\",\"number\"]}}},\"required\":[\"query\",\"text\"]}",
    ),
    (
        "context_snapshot",
        "meter a text blob without compressing (pre-snapshot): returns token count and stores the snapshot in the process ledger. args: text (str), optional kind (str)",
        "{\"type\":\"object\",\"properties\":{\"text\":{\"type\":\"string\"},\"kind\":{\"type\":\"string\"}},\"required\":[\"text\"]}",
    ),
    (
        "context_diff",
        "diff the two most recent snapshots in the process ledger (token deltas). no args",
        "{\"type\":\"object\",\"properties\":{}}",
    ),
    (
        "ledger_summary",
        "aggregate the process ledger by kind: tokens in/out/saved, ratio, calls. no args",
        "{\"type\":\"object\",\"properties\":{}}",
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
        // JSON-RPC 2.0：notification（无 id）不得回复，即使 method 未知。
        _ => id.map(|id| error(Some(id), -32601, &format!("method not found: {method}"))),
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
        "context_snapshot" => snapshot(id, args, usage),
        "context_diff" => context_diff(id, usage),
        "ledger_summary" => ledger_summary(id, usage),
        "connect_account" => account(id),
        "usage_summary" => summary(id, usage),
        _ => {
            // 无 id 即 notification：按 JSON-RPC 2.0 不得回复。
            id.as_ref()?;
            error(id, -32602, &format!("unknown tool: {name}"))
        }
    })
}

/// `id` 序列化（数字/字符串/空）。消息里 id 多为数字，宽容处理。
fn wire(id: &Option<Json>) -> String {
    match id {
        Some(Json::Num(n)) => format!("{n}"),
        Some(Json::Str(s)) => format!("\"{}\"", jesc(s)),
        Some(Json::Null) | None => "null".to_string(),
        Some(_) => "null".to_string(),
    }
}

fn error(id: Option<Json>, code: i64, message: &str) -> String {
    format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":{},\"error\":{{\"code\":{code},\"message\":\"{}\"}}}}",
        wire(&id),
        jesc(message)
    )
}

fn result(id: Option<Json>, text: &str) -> String {
    format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{\"content\":[{{\"type\":\"text\",\"text\":\"{}\"}}],\"isError\":false}}}}",
        wire(&id),
        jesc(text)
    )
}

/// JSON 字符串转义：反斜杠、引号、换行、回车、制表符与全部 C0 控制字符。
/// 效果卡为多行文本，缺少换行转义会使返回帧非法 JSON；query/text 可能携带
/// 任意控制字符，未转义会产出非法 JSON 帧。
fn jesc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            _ => out.push(c),
        }
    }
    out
}

/// 从 JSON 值取数值参数（整数语义：f64 → usize，负值钳 0）。
fn number(v: &Json, key: &str) -> Option<usize> {
    v.get(key).and_then(Json::num).map(|n| n.max(0.0) as usize)
}

/// 从 JSON 值取浮点参数（f64，不截断；`ratio` 等连续参数用）。
fn float(v: &Json, key: &str) -> Option<f64> {
    v.get(key).and_then(Json::num)
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
    apply_kind(&mut cfg, args, &text);
    let outcome = crate::compress(&text, &query, &cfg);
    usage.compressed += 1;
    usage.input += text.len() as u64;
    usage.output += outcome.text.len() as u64;
    let card = crate::ops::recall_report(&outcome, &query);
    let json = crate::ops::render(&outcome);
    // v0.6.6：单块 text（效果卡在上、机器 JSON 在下）。实测豆包对多 content 只渲染
    // JSON 转述、效果卡不可见；合并为一块后任意客户端对话均原样显示效果卡，模型仍可读 JSON。
    let combined = format!("{card}\n{json}");
    result(id, &combined)
}

/// 数值与开关参数落到压缩配置（缺省即保持 `Config::default()`）。
fn apply(cfg: &mut crate::Config, args: &Json) {
    // ratio 与 CLI 语义一致：0..=1 小数（可选内容预算比例），不再是千分比。
    if let Some(r) = float(args, "ratio") {
        cfg.ratio = r.clamp(0.0, 1.0);
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
    cfg.scoped = flag(args, "scoped");
    cfg.strict = flag(args, "strict");
    evidence(cfg, args);
}

/// 解析可选 `kind`（内容类型标注；`auto` = 按正文隐式推断）。
/// 缺省 ratio 用该类型价值梯度默认；显式 `ratio` 参数优先（与 CLI `--kind` 语义一致）。
fn apply_kind(cfg: &mut crate::Config, args: &Json, text: &str) {
    let Some(v) = args.get("kind").and_then(Json::str) else {
        return;
    };
    let kind = if v == "auto" {
        crate::ContentType::infer(text)
    } else {
        match v.parse::<crate::ContentType>() {
            Ok(k) => k,
            Err(_) => return, // 非法 kind 忽略（fail-open 到现状单桶 ToolOutput）
        }
    };
    cfg.kind = kind;
    if args.get("ratio").is_none() {
        cfg.ratio = kind.default_ratio();
    }
}

/// 解析可选 `evidence` 数组（元素：正整数 = 原始行号，字符串 = 必须出现的子串）。
fn evidence(cfg: &mut crate::Config, args: &Json) {
    if let Some(crate::json::Json::Arr(evs)) = args.get("evidence") {
        for e in evs {
            match e {
                crate::json::Json::Num(n) if *n >= 1.0 => {
                    cfg.evidence.push(crate::Evidence::Line(*n as usize));
                }
                crate::json::Json::Str(s) => {
                    cfg.evidence.push(crate::Evidence::Substr(s.clone()));
                }
                _ => {}
            }
        }
    }
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

/// 计量（不压缩）：计数 token，写入账本并返回快照 JSON。
fn snapshot(id: Option<Json>, args: Option<&Json>, usage: &mut Usage) -> String {
    let args = match args {
        Some(a) => a,
        None => return error(id, -32602, "context_snapshot: missing arguments"),
    };
    let text = match args.get("text").and_then(Json::str) {
        Some(t) => t,
        _ => return error(id, -32602, "context_snapshot: missing text"),
    };
    let kind = args
        .get("kind")
        .and_then(Json::str)
        .and_then(|s| s.parse().ok())
        .unwrap_or(crate::ContentType::ToolOutput);
    let snap = crate::meter::snapshot(usage.round, kind, text);
    usage.round += 1;
    let json = crate::meter::render(&snap);
    usage.ledger.push(snap);
    result(id, &json)
}

/// 差分最近两次快照（token 增减）。
fn context_diff(id: Option<Json>, usage: &Usage) -> String {
    let n = usage.ledger.snaps.len();
    if n < 2 {
        return error(id, -32602, "context_diff: need at least two snapshots");
    }
    let a = &usage.ledger.snaps[n - 2];
    let b = &usage.ledger.snaps[n - 1];
    match usage.ledger.diff(&a.id, &b.id) {
        Some(d) => result(
            id,
            &format!(
                "{{\"from\":{},\"to\":{},\"delta_in\":{:.2},\"delta_out\":{:.2},\"delta_saved\":{:.4},\"delta_ratio\":{:.4}}}",
                crate::json::string(&a.id),
                crate::json::string(&b.id),
                d.delta_in,
                d.delta_out,
                d.delta_saved,
                d.delta_ratio
            ),
        ),
        None => error(id, -32602, "context_diff: snapshots not found"),
    }
}

/// 账本聚合（按 kind）。
fn ledger_summary(id: Option<Json>, usage: &Usage) -> String {
    result(id, &crate::meter::summarize(&usage.ledger))
}

/// stdio 服务循环：逐行读 stdin，逐行回 stdout，EOF 结束。
/// 返回进程退出码（0 = 正常）。
pub fn serve() -> i32 {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut usage = Usage::default();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();
    loop {
        let resp = match read(&mut reader, MAX_FRAME) {
            Frame::Eof => break,
            Frame::Line(line) => handle(line.trim_end(), &mut usage),
            // 超长/非 UTF-8 帧：回 -32700 后继续下一帧（保持行同步），不静默退出。
            Frame::Invalid(msg) => Some(error(None, -32700, &msg)),
        };
        if let Some(resp) = resp {
            if !send(&mut writer, &resp) {
                break;
            }
        }
    }
    0
}

/// 单帧读取结果。
enum Frame {
    Line(String),
    Invalid(String),
    Eof,
}

/// 单帧上限：与 CLI 输入上限（64 MiB）一致，防 MCP 客户端用超长单行放大内存。
const MAX_FRAME: usize = 64 * 1024 * 1024;

fn read(reader: &mut impl BufRead, max: usize) -> Frame {
    read_bounded(reader, max)
}

/// 以 `dyn BufRead` 接收：`&mut dyn BufRead: Read` 走 `take`，避免把具体类型 move 出去。
fn read_bounded(reader: &mut dyn BufRead, max: usize) -> Frame {
    let mut buf = Vec::new();
    let n = {
        // UFCS：`take` 要求 Sized 接收者，`&mut dyn BufRead` 通过 `Read for &mut R`
        // 的转发实现生效。
        let mut limited = std::io::Read::take(&mut *reader, max as u64 + 1);
        match limited.read_until(b'\n', &mut buf) {
            Ok(n) => n,
            Err(_) => return Frame::Eof,
        }
    };
    if n == 0 {
        return Frame::Eof;
    }
    // 上限按「帧内容字节数」计，不含换行符：恰好 max 字节 + '\n' 是合法帧
    // （总数 max+1）。只有读满上限且未见到换行时，才需要丢弃本行剩余字节。
    let nl = buf.last() == Some(&b'\n');
    let content = n - usize::from(nl);
    if content > max {
        if !nl {
            discard(reader);
        }
        return Frame::Invalid(format!("request exceeds {max} bytes"));
    }
    match String::from_utf8(buf) {
        Ok(s) => Frame::Line(s),
        Err(_) => Frame::Invalid("request is not valid UTF-8".to_string()),
    }
}

/// 丢弃当前帧剩余字节直到换行，保证超限帧之后仍按行同步。
/// 必须用 `read_until` 精确停在第一个换行：`read` 一次可能带回后续多行，
/// 会把下一帧一起吞掉。
fn discard(reader: &mut dyn BufRead) {
    let mut sink = Vec::new();
    loop {
        sink.clear();
        match reader.read_until(b'\n', &mut sink) {
            Ok(0) => return,
            Ok(_) if sink.last() == Some(&b'\n') => return,
            Ok(_) => {}
            Err(_) => return,
        }
    }
}

fn send(writer: &mut impl Write, response: &str) -> bool {
    writer.write_all(response.as_bytes()).is_ok()
        && writer.write_all(b"\n").is_ok()
        && writer.flush().is_ok()
}

use std::io::{self, BufRead, Write};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_with_unknown_method_gets_no_reply() {
        // JSON-RPC 2.0：notification（无 id）不得回复。
        let mut usage = Usage::default();
        assert!(handle(r#"{"jsonrpc":"2.0","method":"nope"}"#, &mut usage).is_none());
        assert!(handle(
            r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"nope"}}"#,
            &mut usage
        )
        .is_none());
        // 带 id 的未知 method 仍需回错误。
        assert!(
            handle(r#"{"jsonrpc":"2.0","id":7,"method":"nope"}"#, &mut usage)
                .is_some_and(|r| r.contains("-32601"))
        );
    }

    #[test]
    fn boundary_frame_keeps_next_line() {
        use std::io::Cursor;
        // 恰好 max 字节内容 + 换行是合法帧，且不得吞掉下一帧。
        let mut src = Cursor::new(b"12345678\nnext\n".to_vec());
        match read(&mut src, 8) {
            Frame::Line(l) => assert_eq!(l, "12345678\n"),
            _ => panic!("exact-max frame must be a Line"),
        }
        match read(&mut src, 8) {
            Frame::Line(l) => assert_eq!(l, "next\n"),
            _ => panic!("next frame must survive an exact-max frame"),
        }
        // 超限帧（无换行）丢弃到行尾后，下一帧仍然完整。
        let mut src = Cursor::new(b"123456789\nok\n".to_vec());
        assert!(matches!(read(&mut src, 8), Frame::Invalid(_)));
        match read(&mut src, 8) {
            Frame::Line(l) => assert_eq!(l, "ok\n"),
            _ => panic!("stream must stay line-synced after an oversize frame"),
        }
    }

    #[test]
    fn frames_are_bounded_and_utf8_checked() {
        use std::io::Cursor;
        // 超限帧：回 Invalid，且后续帧仍能按行同步读取。
        let mut src = Cursor::new(b"12345678901234567890\nok\n".to_vec());
        match read(&mut src, 8) {
            Frame::Invalid(msg) => assert!(msg.contains("exceeds"), "{msg}"),
            _ => panic!("oversized frame must be Invalid"),
        }
        match read(&mut src, 8) {
            Frame::Line(l) => assert_eq!(l, "ok\n"),
            _ => panic!("stream must stay line-synced after oversize"),
        }
        // 非法 UTF-8：Invalid 而非静默 EOF。
        let mut bad = Cursor::new(vec![0xff, 0xfe, b'\n']);
        assert!(matches!(read(&mut bad, 64), Frame::Invalid(_)));
        // 空输入：EOF。
        assert!(matches!(read(&mut Cursor::new(Vec::new()), 64), Frame::Eof));
    }

    #[test]
    fn apply_ratio_is_a_fraction_not_permille() {
        // ratio 与 CLI 语义一致：0..=1 小数，不再按千分比缩放。
        let mut cfg = crate::Config::default();
        apply(&mut cfg, &parse(r#"{"ratio":0.5}"#).unwrap());
        assert!(
            (cfg.ratio - 0.5).abs() < 1e-9,
            "ratio=0.5 应作为小数生效，而非被截断成 0: {}",
            cfg.ratio
        );

        let mut low = crate::Config::default();
        apply(&mut low, &parse(r#"{"ratio":0.02}"#).unwrap());
        assert!((low.ratio - 0.02).abs() < 1e-9, "ratio=0.02: {}", low.ratio);

        // 越界钳制：>1 → 1.0，负值 → 0.0。
        let mut over = crate::Config::default();
        apply(&mut over, &parse(r#"{"ratio":1.5}"#).unwrap());
        assert_eq!(over.ratio, 1.0, "ratio>1 应钳到 1.0");
        let mut neg = crate::Config::default();
        apply(&mut neg, &parse(r#"{"ratio":-0.3}"#).unwrap());
        assert_eq!(neg.ratio, 0.0, "负 ratio 应钳到 0.0");
    }

    #[test]
    fn apply_scoped_flag() {
        // scoped 与 structure 同为 bool/0/1 开关；缺省 false，显式落位。
        let mut cfg = crate::Config::default();
        apply(&mut cfg, &parse(r#"{"scoped":1}"#).unwrap());
        assert!(cfg.scoped, "scoped=1 应落位");

        let mut off = crate::Config::default();
        apply(&mut off, &parse(r#"{"scoped":0}"#).unwrap());
        assert!(!off.scoped, "scoped=0 应关闭");

        let mut absent = crate::Config::default();
        apply(&mut absent, &parse(r#"{"query":"q"}"#).unwrap());
        assert!(!absent.scoped, "缺省应为 false");

        let mut bool_form = crate::Config::default();
        apply(&mut bool_form, &parse(r#"{"scoped":true}"#).unwrap());
        assert!(bool_form.scoped, "布尔 true 也应落位");
    }

    #[test]
    fn apply_kind_tags_and_defaults_ratio() {
        // 显式类型：kind 落位，缺省 ratio 用该类型价值梯度默认。
        let mut cfg = crate::Config::default();
        apply_kind(
            &mut cfg,
            &parse(r#"{"kind":"static-contract"}"#).unwrap(),
            "x",
        );
        assert_eq!(cfg.kind, crate::ContentType::StaticContract);
        assert_eq!(
            cfg.ratio,
            crate::ContentType::StaticContract.default_ratio()
        );

        // 显式 ratio 优先于类型默认（真实流程：apply 先落 ratio，apply_kind 不覆盖）。
        let mut cfg = crate::Config::default();
        let args = parse(r#"{"kind":"tool-output","ratio":0.5}"#).unwrap();
        apply(&mut cfg, &args);
        apply_kind(&mut cfg, &args, "x");
        assert_eq!(cfg.kind, crate::ContentType::ToolOutput);
        assert_eq!(cfg.ratio, 0.5);

        // auto：按正文隐式推断。
        let mut cfg = crate::Config::default();
        apply_kind(
            &mut cfg,
            &parse(r#"{"kind":"auto"}"#).unwrap(),
            "{\"type\":\"object\"}",
        );
        assert_eq!(cfg.kind, crate::ContentType::StaticContract);

        // 非法 kind 忽略（fail-open 到现状单桶 ToolOutput）。
        let mut cfg = crate::Config::default();
        apply_kind(&mut cfg, &parse(r#"{"kind":"bogus"}"#).unwrap(), "x");
        assert_eq!(cfg.kind, crate::ContentType::ToolOutput);
        assert_eq!(cfg.ratio, crate::Config::DEFAULT_RATIO);

        // 无 kind：不改任何字段。
        let mut cfg = crate::Config::default();
        apply_kind(&mut cfg, &parse(r#"{}"#).unwrap(), "x");
        assert_eq!(cfg.kind, crate::ContentType::ToolOutput);
        assert_eq!(cfg.ratio, crate::Config::DEFAULT_RATIO);
    }

    #[test]
    fn jesc_escapes_all_c0_control_chars() {
        assert_eq!(jesc("a\x01b"), r#"a\u0001b"#, "C0 控制字符须转义");
        assert_eq!(jesc("a\x1bb"), r#"a\u001bb"#, "ESC 须转义");
        assert_eq!(jesc("a\nb"), r#"a\nb"#);
        assert_eq!(jesc("a\"b\\c"), r#"a\"b\\c"#);
    }

    #[test]
    fn wire_escapes_string_id() {
        assert_eq!(wire(&Some(Json::Str("a\"b".into()))), r#""a\"b""#);
        assert_eq!(wire(&Some(Json::Str("a\\b".into()))), r#""a\\b""#);
    }

    #[test]
    fn error_escapes_message_backslash() {
        let e = error(None, -32601, "method not found: x\\y");
        assert!(e.contains(r#"x\\y"#), "message 反斜杠应转义: {e}");
    }
}
