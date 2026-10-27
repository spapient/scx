//! 类型路由 + 分桶压缩（P1 治理层）。
//!
//! 把全量上下文载荷按 [`ContentType`] 分桶，复用 [`crate::compress_bucketed`]
//! 逐桶压缩（各桶按价值梯度 ratio），聚合到单一 [`Outcome`]（`buckets` 携带逐桶
//! 统计）。本层是薄壳，不复制裁剪逻辑；分桶类型缺失时走隐式推断。

#![forbid(unsafe_code)]

use crate::{compress_bucketed, ContentType};

/// `scx bucket` 入口：读 stdin 类型化载荷 JSON，分桶压缩输出 `Outcome` JSON；
/// `--record` 追加收益记录。
pub fn run(args: &[String]) -> i32 {
    let mut record_path: Option<&str> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--record" => {
                i += 1;
                let Some(p) = args.get(i) else {
                    eprintln!("scx bucket: --record requires a path");
                    return 2;
                };
                record_path = Some(p.as_str());
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
    let cfg = crate::Config::default();
    let outcome = compress_bucketed(&refs, &query, &cfg);
    if let Err(e) = crate::ops::out_line(&crate::ops::render(&outcome)) {
        eprintln!("scx bucket: write stdout failed: {e}");
        return 1;
    }
    if let Some(path) = record_path {
        // 收益记录写 Snapshot JSONL（与 `scx ledger --summarize` 同格式、确定性、无 wall-clock）。
        let snap = crate::meter::measure(0, &refs, &query, &cfg, None);
        record(path, &snap);
    }
    0
}

/// 解析 stdin 类型化载荷 JSON：`{query, parts:[{type?,text}]}`；`type` 缺失时隐式推断。
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
        let text = p
            .get("text")
            .and_then(crate::json::Json::str)
            .unwrap_or("")
            .to_string();
        let tag = p.get("type").and_then(crate::json::Json::str);
        let ty = ContentType::resolve(tag, &text);
        parts.push((ty, text));
    }
    Ok((query, parts))
}

/// 追加一条快照 JSONL 到账本文件（供 `scx ledger --summarize` 聚合），失败静默
/// （账本是旁路，不影响压缩正文/统计/退出码）。
fn record(path: &str, snap: &crate::meter::Snapshot) {
    use std::io::Write;
    let line = format!("{}\n", crate::meter::render(snap));
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
    fn compress_bucketed_aggregates_and_buckets() {
        let t = text();
        let parts = [
            (ContentType::StaticContract, t.as_str()),
            (ContentType::ToolOutput, t.as_str()),
        ];
        let o = compress_bucketed(&parts, "db timeout", &crate::Config::default());
        assert_eq!(o.buckets.len(), 2);
        assert_eq!(o.buckets[0].kind, ContentType::StaticContract);
        assert_eq!(o.buckets[1].kind, ContentType::ToolOutput);
        assert!(o.tokens_in > 0.0);
        assert!(o.tokens < o.tokens_in);
    }

    #[test]
    fn parse_manifest_infers_missing_type() {
        let input = r#"{"query":"db timeout","parts":[{"text":"{\"type\":\"object\"}"},{"text":"INFO heartbeat ok"}]}"#;
        let (_, parts) = parse_manifest(input).unwrap();
        assert_eq!(parts[0].0, ContentType::StaticContract);
        assert_eq!(parts[1].0, ContentType::ToolOutput);
    }
}
