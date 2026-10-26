//! 类型路由 + 计量桶 + 前后快照（P1 治理层）。
//!
//! 把全量上下文载荷按 `ContentType` 分桶，逐桶复用 `compress` 内核压缩，
//! 聚合成全局与分桶维度的 token 缩减快照；状态可序列化为标准化 JSON（P3 消费）。
//! 本层是薄壳，不复制裁剪逻辑；价值梯度预算分配（P2）在此之上单独叠加。

#![forbid(unsafe_code)]

use crate::{compress, Config, Mode};

/// 内容类型，按价值梯度排列：静态契约 > 系统定义 > 会话消息 > 临时工具输出。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    StaticContract,
    SystemDefinition,
    SessionMessage,
    ToolOutput,
}

impl ContentType {
    pub const fn as_str(self) -> &'static str {
        match self {
            ContentType::StaticContract => "static-contract",
            ContentType::SystemDefinition => "system-definition",
            ContentType::SessionMessage => "session-message",
            ContentType::ToolOutput => "tool-output",
        }
    }

    /// 价值梯度默认预算（ratio，越大保留越多可选内容）：静态契约最保守、工具输出最激进。
    pub const fn default_ratio(self) -> f64 {
        match self {
            ContentType::StaticContract => 1.0,
            ContentType::SystemDefinition => 0.8,
            ContentType::SessionMessage => 0.5,
            ContentType::ToolOutput => 0.2,
        }
    }
}

impl std::str::FromStr for ContentType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "static-contract" => Ok(ContentType::StaticContract),
            "system-definition" => Ok(ContentType::SystemDefinition),
            "session-message" => Ok(ContentType::SessionMessage),
            "tool-output" => Ok(ContentType::ToolOutput),
            _ => Err(format!("unknown content type: {s}")),
        }
    }
}

/// 单桶计量。
#[derive(Debug, Clone, Copy)]
pub struct Bucket {
    pub ty: ContentType,
    pub tokens_in: f64,
    pub tokens_out: f64,
    pub cut_pct: f64,
    pub kept: usize,
    pub total: usize,
    pub mode: Mode,
}

/// 前后双快照：全局 + 分桶。
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub buckets: Vec<Bucket>,
    pub tokens_in: f64,
    pub tokens_out: f64,
    pub cut_pct: f64,
}

fn pct(tin: f64, tout: f64) -> f64 {
    if tin > 0.0 {
        (tin - tout) * 100.0 / tin
    } else {
        0.0
    }
}

/// 分桶压缩：每桶独立 `compress`，聚合全局与分桶计量。
pub fn compress_parts(parts: &[(ContentType, &str)], query: &str, cfg: &Config) -> Snapshot {
    let mut buckets = Vec::with_capacity(parts.len());
    let (mut tin, mut tout) = (0.0, 0.0);
    for (ty, text) in parts {
        let mut c = cfg.clone();
        c.ratio = ty.default_ratio();
        let o = compress(text, query, &c);
        // passthrough/fail-open 时 `o.tokens` 不保证，重新估算输出（与 ops 渲染同口径）。
        let tout_part = crate::terms::estimate(&o.text);
        tin += o.tokens_in;
        tout += tout_part;
        buckets.push(Bucket {
            ty: *ty,
            tokens_in: o.tokens_in,
            tokens_out: tout_part,
            cut_pct: pct(o.tokens_in, tout_part),
            kept: o.kept,
            total: o.total,
            mode: o.mode,
        });
    }
    Snapshot {
        buckets,
        tokens_in: tin,
        tokens_out: tout,
        cut_pct: pct(tin, tout),
    }
}

/// 标准化 JSON 输出（P3 可视化/链路监控消费）。
pub fn render(s: &Snapshot) -> String {
    let mut out = format!(
        "{{\"tokens_in\":{:.0},\"tokens_out\":{:.0},\"cut_pct\":{:.1},\"buckets\":[",
        s.tokens_in, s.tokens_out, s.cut_pct
    );
    for (i, b) in s.buckets.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"type\":\"{}\",\"tokens_in\":{:.0},\"tokens_out\":{:.0},\"cut_pct\":{:.1},\"kept\":{},\"total\":{},\"mode\":\"{}\"}}",
            b.ty.as_str(),
            b.tokens_in,
            b.tokens_out,
            b.cut_pct,
            b.kept,
            b.total,
            b.mode
        ));
    }
    out.push_str("]}");
    out
}

/// `scx bucket` 入口：读 stdin 类型化载荷 JSON，压缩输出快照；`--record` 追加收益记录。
pub fn run(args: &[String]) -> i32 {
    let mut record_path: Option<&str> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--record" => {
                i += 1;
                record_path = args.get(i).map(|s| s.as_str());
            }
            other => {
                eprintln!("scx bucket: unknown argument {other}");
                return 2;
            }
        }
        i += 1;
    }
    let mut input = String::new();
    if std::io::Read::read_to_string(&mut std::io::stdin(), &mut input).is_err() {
        eprintln!("scx bucket: read stdin failed");
        return 2;
    }
    let (query, parts) = match parse_manifest(&input) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("scx bucket: {e}");
            return 2;
        }
    };
    let refs: Vec<(ContentType, &str)> = parts.iter().map(|(t, s)| (*t, s.as_str())).collect();
    let snap = compress_parts(&refs, &query, &crate::Config::default());
    println!("{}", render(&snap));
    if let Some(path) = record_path {
        append_record(path, &query, &snap);
    }
    0
}

/// 解析 stdin 类型化载荷 JSON：`{query, parts:[{type,text}]}`。
fn parse_manifest(input: &str) -> Result<(String, Vec<(ContentType, String)>), String> {
    let v = crate::json::parse(input).map_err(|e| format!("parse: {e}"))?;
    let query = v
        .get("query")
        .and_then(crate::json::Json::str)
        .unwrap_or("")
        .to_string();
    let arr = v.get("parts").ok_or("missing parts array")?;
    let crate::json::Json::Arr(arr) = arr else {
        return Err("missing parts array".to_string());
    };
    let mut parts = Vec::with_capacity(arr.len());
    for p in arr {
        let ty = p
            .get("type")
            .and_then(crate::json::Json::str)
            .and_then(|s| s.parse().ok())
            .ok_or("bad part type")?;
        let text = p.get("text").and_then(crate::json::Json::str).unwrap_or("");
        parts.push((ty, text.to_string()));
    }
    Ok((query, parts))
}

/// 追加一条收益记录（JSONL：`{ts, query, snapshot}`），失败静默（账本是旁路，不影响压缩）。
fn append_record(path: &str, query: &str, snap: &Snapshot) {
    use std::io::Write;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!(
        "{{\"ts\":{},\"query\":{},\"snapshot\":{}}}\n",
        ts,
        crate::json::string(query),
        render(snap)
    );
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text() -> String {
        let mut lines: Vec<String> = (0..200)
            .map(|i| format!("filler alpha beta line {i}"))
            .collect();
        lines[100] = "ERROR db timeout conn_id=9f2a".to_string();
        lines.join("\n")
    }

    #[test]
    fn compress_parts_aggregates_and_buckets() {
        let t = text();
        let parts = [
            (ContentType::StaticContract, t.as_str()),
            (ContentType::ToolOutput, t.as_str()),
        ];
        let s = compress_parts(&parts, "db timeout", &Config::default());
        assert_eq!(s.buckets.len(), 2);
        assert_eq!(s.buckets[0].ty, ContentType::StaticContract);
        assert_eq!(s.buckets[1].ty, ContentType::ToolOutput);
        assert!(s.tokens_in > 0.0);
        assert!(s.tokens_out < s.tokens_in);
        assert!(s.cut_pct > 0.0);
    }

    #[test]
    fn render_is_valid_json_shape() {
        let t = text();
        let s = compress_parts(
            &[(ContentType::ToolOutput, t.as_str())],
            "db timeout",
            &Config::default(),
        );
        let j = render(&s);
        assert!(j.starts_with("{\"tokens_in\":"));
        assert!(j.contains("\"buckets\":[{\"type\":\"tool-output\""));
        assert!(j.ends_with("]}"));
    }
}
