//! MCP stdio 服务器（零第三方 crate，无条件编译进 scx 主二进制）。
//!
//! - 传输：stdio（逐行 JSON-RPC 2.0，符合 MCP stdio transport）。
//! - 工具：`compress_context` / `connect_account` / `usage_summary`。
//! - 压缩走 `scx::compress`（纯内核）+ `scx::ops::render`（json renderer），
//!   **绝不触碰 CLI 渲染**（正文写 stdout / 统计行 / `scxjson:` 前缀），
//!   stdio 通道不被污染。
//!
//! 入口：`scx mcp` 子命令（main.rs 分派）调用 `serve()`；`serverInfo.name`
//! 为 `"scx {version}"`（客户端列表可见版本，随 CARGO_PKG_VERSION 自动变化）。
//! 压缩主路径与 v0.4.1 逐字节一致。

use crate::json::{parse, Json};

/// 进程内状态（单线程 stdio 循环，无需原子）：用量计数 + 快照账本 + 轮次。
#[derive(Default)]
struct Usage {
    compressed: u64,
    input: u64,
    output: u64,
    round: u64,
    ledger: crate::meter::Ledger,
    /// 账本落盘文件；`None` = 纯内存（写失败时静默降级，不阻断主链路）。
    file: Option<String>,
}

impl Usage {
    /// 启动态：账本路径取 `SCX_LEDGER_FILE`，否则 `<SCX_MEM_DIR|~/.scx-memory>/ledger.jsonl`。
    /// 已有文件则恢复快照与轮次，使统计跨进程累积，而不是每次重启清零。
    fn open() -> Usage {
        let path = ledger_path();
        let mut u = Usage::default();
        if let Ok(ledger) = crate::meter::Ledger::load_jsonl(&path) {
            u.round = ledger.snaps.len() as u64;
            u.ledger = ledger;
        }
        u.file = Some(path);
        u
    }

    /// 记账并落盘（追加一行）。写失败静默：账本是旁路，不能拖垮压缩。
    fn record(&mut self, snap: crate::meter::Snapshot) {
        if let Some(path) = self.file.as_deref() {
            let _ = append(path, &crate::meter::render(&snap));
        }
        self.round += 1;
        self.ledger.push(snap);
    }
}

/// 记忆库根目录：`SCX_MEM_DIR` > `~/.scx-memory`。
fn mem_dir() -> String {
    if let Ok(d) = std::env::var("SCX_MEM_DIR") {
        if !d.trim().is_empty() {
            return d;
        }
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    if home.is_empty() {
        ".scx-memory".to_string()
    } else {
        format!("{home}/.scx-memory")
    }
}

/// 账本文件路径：`SCX_LEDGER_FILE` > `<记忆库根>/ledger.jsonl`。
fn ledger_path() -> String {
    if let Ok(p) = std::env::var("SCX_LEDGER_FILE") {
        if !p.trim().is_empty() {
            return p;
        }
    }
    format!("{}/ledger.jsonl", mem_dir())
}

/// 追加一行到账本文件（父目录按需创建）。
fn append(path: &str, line: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    if let Some(dir) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(line.as_bytes())?;
    f.write_all(b"\n")
}

/// 从原始输入按 1-based 行号取证据行（与 CLI `--store` 同一口径）。
#[cfg(feature = "memory")]
fn evidence_of(text: &str, lno: &[usize]) -> Vec<String> {
    if lno.is_empty() {
        return Vec::new();
    }
    let normalized;
    let text = if text.contains('\r') {
        normalized = text.replace("\r\n", "\n");
        normalized.as_str()
    } else {
        text
    };
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lno.iter()
        .map(|&n| {
            lines
                .get(n.saturating_sub(1))
                .copied()
                .unwrap_or("")
                .to_string()
        })
        .collect()
}

/// `store: true` 时把压缩结果写入记忆库（与 CLI `--store` 同一摄取入口）。
/// 失败只回错误字段，不影响压缩结果返回。
#[cfg(feature = "memory")]
fn store_case(args: &Json, query: &str, text: &str, outcome: &crate::Outcome) -> String {
    let root = args
        .get("mem_dir")
        .and_then(Json::str)
        .map(str::to_string)
        .unwrap_or_else(mem_dir);
    let stats = crate::memory::Stats {
        kept: outcome.kept,
        total: outcome.total,
        cut: outcome.cut,
        risk: outcome.risk.to_string(),
    };
    let evidence = evidence_of(text, &outcome.numbers);
    let mut input = crate::memory::Input::new(&root, query, &outcome.text, &stats);
    input.evidence_lno = &outcome.numbers;
    input.evidence = &evidence;
    match crate::memory::save(input) {
        Ok((_added, id)) => format!("{{\"ok\":true,\"case\":{}}}", crate::json::string(&id)),
        Err(e) => format!(
            "{{\"ok\":false,\"error\":{}}}",
            crate::json::string(&e.to_string())
        ),
    }
}

#[cfg(not(feature = "memory"))]
fn store_case(_args: &Json, _query: &str, _text: &str, _outcome: &crate::Outcome) -> String {
    "{\"ok\":false,\"error\":\"memory feature disabled\"}".to_string()
}

/// 工具定义（tools/list 静态表）：名称、描述、JSON Schema。
/// `inputSchema` 为 MCP 客户端（尤其 Claude Code）严格校验的必填对象，
/// 缺失会被判定为 invalid result；无参工具使用空 schema `{}`。
const TOOLS: &[(&str, &str, &str)] = &[
    (
        "compress_context",
        "query-aware extractive compression of a text blob. args: query (str), text (str), optional ratio (0..=1 fraction of optional-content budget) / context / window / max_tokens (num), structure (bool or 0/1), scoped (bool or 0/1), strict (bool or 0/1), kind (str: static-contract/system-definition/session-message/tool-output/auto), store (bool: also persist the result into the local memory bank, same ingest as CLI --store), mem_dir (str: memory root override)",
        "{\"type\":\"object\",\"properties\":{\"query\":{\"type\":\"string\"},\"text\":{\"type\":\"string\"},\"ratio\":{\"type\":\"number\"},\"context\":{\"type\":\"number\"},\"window\":{\"type\":\"number\"},\"max_tokens\":{\"type\":\"number\"},\"structure\":{\"type\":\"boolean\"},\"scoped\":{\"type\":\"boolean\"},\"strict\":{\"type\":\"boolean\"},\"kind\":{\"type\":\"string\"},\"store\":{\"type\":\"boolean\"},\"mem_dir\":{\"type\":\"string\"},\"evidence\":{\"type\":\"array\",\"items\":{\"type\":[\"string\",\"number\"]}}},\"required\":[\"query\",\"text\"]}",
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
    (
        "vec",
        "encode texts to raw vectors (f32 components). args: texts (array of str), optional provider (str: encoder command; default built-in lexical encoder, env SCX_EMBED_VEC also honored); returns one vector per text in order",
        "{\"type\":\"object\",\"properties\":{\"texts\":{\"type\":\"array\",\"items\":{\"type\":\"string\"}},\"provider\":{\"type\":\"string\"}},\"required\":[\"texts\"]}",
    ),
    (
        "vec_sim",
        "cosine similarity of a query against candidate texts (normalized in scx, deterministic). args: query (str), texts (array of str), optional cache (str dir) / provider (str: encoder command; default built-in lexical encoder, env SCX_EMBED_VEC also honored)",
        "{\"type\":\"object\",\"properties\":{\"query\":{\"type\":\"string\"},\"texts\":{\"type\":\"array\",\"items\":{\"type\":\"string\"}},\"cache\":{\"type\":\"string\"},\"provider\":{\"type\":\"string\"}},\"required\":[\"query\",\"texts\"]}",
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
            "{{\"jsonrpc\":\"2.0\",\"id\":{key},\"result\":{{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{{\"tools\":{{}}}},\"serverInfo\":{{\"name\":\"scx {}\",\"version\":\"{}\"}}}}}}",
            env!("CARGO_PKG_VERSION"),
            env!("CARGO_PKG_VERSION"),
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
        "vec" => vec(id, args),
        "vec_sim" => vec_sim(id, args),
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
        Some(Json::Str(s)) => crate::json::string(s),
        Some(Json::Null) | None => "null".to_string(),
        Some(_) => "null".to_string(),
    }
}

fn error(id: Option<Json>, code: i64, message: &str) -> String {
    format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":{},\"error\":{{\"code\":{code},\"message\":\"{}\"}}}}",
        wire(&id),
        crate::json::escape(message)
    )
}

fn result(id: Option<Json>, text: &str) -> String {
    format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{\"content\":[{{\"type\":\"text\",\"text\":\"{}\"}}],\"isError\":false}}}}",
        wire(&id),
        crate::json::escape(text)
    )
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
    // 未显式指定权重时默认加载随仓语义模型，与 CLI 同口径（feature=semantic）。
    #[cfg(feature = "semantic")]
    cfg.ensure_semantic();
    let outcome = crate::compress(&text, &query, &cfg);
    usage.compressed += 1;
    usage.input += text.len() as u64;
    usage.output += outcome.text.len() as u64;
    usage.record(crate::meter::measure_of(
        usage.round,
        cfg.kind,
        &text,
        &outcome,
    ));
    let st = crate::stat::Stat::from_outcome(&outcome);
    let card = st.card(&outcome, &query);
    let json = crate::ops::render(&outcome);
    // v0.6.6：单块 text（效果卡在上、机器 JSON 在下）。实测豆包对多 content 只渲染
    // JSON 转述、效果卡不可见；合并为一块后任意客户端对话均原样显示效果卡，模型仍可读 JSON。
    // 修复：render 仅输出元数据 JSON，不含压缩正文；把 outcome.text 作为 "text"
    // 字段嵌入 JSON，使 MCP 调用方能拿到压缩产物（契约不变：效果卡在上、机器 JSON 在下）。
    let stored = if flag(args, "store") {
        store_case(args, &query, &text, &outcome)
    } else {
        "null".to_string()
    };
    let mut body = json;
    body.pop();
    body.push_str(",\"kind\":\"");
    body.push_str(cfg.kind.as_str());
    body.push_str("\",\"text\":\"");
    body.push_str(&crate::json::escape(&outcome.text));
    body.push_str("\",\"stored\":");
    body.push_str(&stored);
    body.push('}');
    let combined = format!("{card}\n{body}");
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
    cfg.word_boundary = flag(args, "word_boundary");
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
    if let Some(Json::Arr(evs)) = args.get("evidence") {
        for e in evs {
            match e {
                Json::Num(n) if *n >= 1.0 => {
                    cfg.evidence.push(crate::Evidence::Line(*n as usize));
                }
                Json::Str(s) => {
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

/// 内置编码器维度（零配置默认后端；外部 provider 的维度由对方决定）。
const DIM: usize = 256;

/// 向量后端：显式 `provider` 参数 > 环境变量 `SCX_EMBED_VEC` > 内置词法编码器。
enum Backend {
    Cmd(String),
    Builtin,
}

/// 后端选择与标签（`builtin` 表示零配置路径，无需模型即可用）。
fn backend(args: &Json) -> (Backend, &'static str) {
    if let Some(cmd) = args.get("provider").and_then(Json::str) {
        if !cmd.trim().is_empty() {
            return (Backend::Cmd(cmd.to_string()), "cmd");
        }
    }
    match std::env::var("SCX_EMBED_VEC") {
        Ok(cmd) if !cmd.trim().is_empty() => (Backend::Cmd(cmd), "cmd"),
        _ => (Backend::Builtin, "builtin"),
    }
}

/// 取字符串数组参数；非字符串元素跳过，缺失或为空返回错误。
fn str_array(v: &Json, key: &str) -> Result<Vec<String>, String> {
    let arr = match v.get(key) {
        Some(Json::Arr(items)) => items
            .iter()
            .filter_map(|it| it.str().map(str::to_string))
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    if arr.is_empty() {
        Err(format!("{key}: expected a non-empty array of strings"))
    } else {
        Ok(arr)
    }
}

/// `vec`：编码文本为原始向量，按输入顺序返回（JSON 数组的数组）。
fn vec(id: Option<Json>, args: Option<&Json>) -> String {
    let Some(a) = args else {
        return error(id, -32602, "vec: missing arguments");
    };
    let body: Result<String, String> = (|| {
        let texts = str_array(a, "texts")?;
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let (be, tag) = backend(a);
        let rows = match be {
            Backend::Cmd(cmd) => crate::sim::encode(&cmd, &refs)?,
            Backend::Builtin => refs.iter().map(|t| crate::sim::hash(t, DIM)).collect(),
        };
        Ok(render_vectors(&rows, tag))
    })();
    match body {
        Ok(text) => result(id, &text),
        Err(e) => error(id, -32602, &e),
    }
}

/// `vec_sim`：query 与候选的余弦相似度，可选缓存目录。
fn vec_sim(id: Option<Json>, args: Option<&Json>) -> String {
    let Some(a) = args else {
        return error(id, -32602, "vec_sim: missing arguments");
    };
    let body: Result<String, String> = (|| {
        let query = a
            .get("query")
            .and_then(Json::str)
            .filter(|s| !s.is_empty())
            .ok_or("missing query")?;
        let texts = str_array(a, "texts")?;
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let (be, tag) = backend(a);
        let sims = match be {
            Backend::Cmd(cmd) => {
                let cache = a.get("cache").and_then(Json::str).map(std::path::Path::new);
                crate::sim::compute_vec(Some(&cmd), cache, query, &refs)?
            }
            Backend::Builtin => {
                let q = crate::sim::hash(query, DIM);
                refs.iter()
                    .map(|t| crate::sim::cos(&q, &crate::sim::hash(t, DIM)))
                    .collect()
            }
        };
        Ok(render_numbers(&sims, tag))
    })();
    match body {
        Ok(text) => result(id, &text),
        Err(e) => error(id, -32602, &e),
    }
}

/// 向量序列化为 JSON 二维数组（f32 原样数值，空格分隔由 JSON 承载）。
fn render_vectors(rows: &[Vec<f32>], tag: &str) -> String {
    let inner: Vec<String> = rows
        .iter()
        .map(|v| {
            let cells: Vec<String> = v.iter().map(|x| format!("{x}")).collect();
            format!("[{}]", cells.join(","))
        })
        .collect();
    format!(
        "{{\"backend\":\"{tag}\",\"vectors\":[{}]}}",
        inner.join(",")
    )
}

/// 一维数值序列化为 JSON 数组。
fn render_numbers(values: &[f64], tag: &str) -> String {
    let cells: Vec<String> = values.iter().map(|x| format!("{x}")).collect();
    format!("{{\"backend\":\"{tag}\",\"sim\":[{}]}}", cells.join(","))
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
    let json = crate::meter::render(&snap);
    usage.record(snap);
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
    let mut usage = Usage::open();
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
        let mut limited = io::Read::take(&mut *reader, max as u64 + 1);
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
    fn compress_json_exposes_kind_and_store() {
        // 机器 JSON 必须自描述 kind 与 stored，客户端才能判断走了哪个桶、有没有落库。
        let mut usage = Usage::default();
        let msg = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"compress_context","arguments":{"query":"q","text":"fn a(b: u8) -> u8 { b }\nfn c(d: u8) -> u8 { d }"}}}"#;
        let resp = handle(msg, &mut usage).expect("有 id 的请求必须回复");
        assert!(
            resp.contains("\\\"kind\\\":\\\"tool-output\\\""),
            "缺少 kind: {resp}"
        );
        assert!(resp.contains("\\\"stored\\\":null"), "缺少 stored: {resp}");
    }

    #[test]
    fn vec_without_provider_uses_builtin_encoder() {
        // 无 SCX_EMBED_VEC / provider 时不得报错，须回落内置词法编码器。
        let mut usage = Usage::default();
        let msg = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"vec","arguments":{"texts":["Rust 内存安全","Rust 内存安全"]}}}"#;
        let resp = handle(msg, &mut usage).expect("vec 须回复");
        assert!(
            resp.contains("\\\"backend\\\":\\\"builtin\\\""),
            "未走内置后端: {resp}"
        );
        assert!(resp.contains("\\\"vectors\\\""), "缺少向量: {resp}");
    }

    #[test]
    fn vec_sim_without_provider_returns_finite_cosines() {
        let mut usage = Usage::default();
        let msg = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"vec_sim","arguments":{"query":"Rust 内存安全","texts":["Rust 内存安全","无关天气"]}}}"#;
        let resp = handle(msg, &mut usage).expect("vec_sim 须回复");
        assert!(
            resp.contains("\\\"backend\\\":\\\"builtin\\\""),
            "未走内置后端: {resp}"
        );
        assert!(resp.contains("\\\"sim\\\""), "缺少分数: {resp}");
    }

    #[test]
    fn docs_list_every_advertised_tool() {
        // tools/list 是公开契约；README/SKILL 的工具清单必须覆盖静态表，
        // 防止新增工具后文档停留在旧的三项列表。
        let readme = include_str!("../README.md");
        let skill = include_str!("../SKILL.md");
        for (name, _, _) in TOOLS {
            assert!(readme.contains(name), "README 缺少 MCP 工具 {name}");
            assert!(skill.contains(name), "SKILL.md 缺少 MCP 工具 {name}");
        }
    }

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
    fn escape_escapes_all_c0_control_chars() {
        assert_eq!(
            crate::json::escape("a\x01b"),
            r#"a\u0001b"#,
            "C0 控制字符须转义"
        );
        assert_eq!(crate::json::escape("a\x1bb"), r#"a\u001bb"#, "ESC 须转义");
        assert_eq!(crate::json::escape("a\nb"), r#"a\nb"#);
        assert_eq!(crate::json::escape("a\"b\\c"), r#"a\"b\\c"#);
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
