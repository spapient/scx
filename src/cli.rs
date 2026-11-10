//! CLI 命令层：`scx wrap` / `scx bucket`（合并自原 wrap.rs/bucket.rs）。
//!
//! `wrap`：包装一个命令，把其 stdout+stderr 合并后按需压缩输出。
//! 用法：`scx wrap [--query Q] [--ratio R] [--context N] [--force P] [--preserve T] [--evidence N|substr] -- CMD...`
//!
//! `bucket`：把类型化载荷 JSON 分桶压缩（P1 治理层），`--record` 追加收益记录。

#![forbid(unsafe_code)]

use crate::{compress_bucketed, ContentType};
use std::io::Read;
use std::process::Command;

/// 主入口：解析 `--` 分隔的压缩选项与命令，执行后压缩输出。返回进程退出码。
/// 退出码沿用被包装命令；信号终止按 shell 惯例映射为 128+信号值。
fn exit_code(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128 + sig;
        }
    }
    1
}

pub fn wrap_run(args: &[String]) -> i32 {
    let Some(sep) = args.iter().position(|a| a == "--") else {
        eprintln!("scx wrap: usage: scx wrap [--query Q] [--ratio R] [--context N] [--force P] [--preserve T] [--evidence N|substr] -- CMD...");
        return 2;
    };
    let (opts, cmd) = (&args[..sep], &args[sep + 1..]);
    if cmd.is_empty() {
        eprintln!("scx wrap: missing command after --");
        return 2;
    }
    let (config, query) = match parse_opts(opts) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("scx wrap: {e}");
            return 2;
        }
    };

    // 执行命令，合并 stdout+stderr（命令的 stderr 也是排障证据，一并参与筛选）。
    // stdin 必须继承：`echo x | scx wrap -- cat` 的输入要交给被包装命令
    // （`Command::output()` 会把 stdin 置空，故手动 spawn + 管道读取）。
    let mut child = match Command::new(&cmd[0])
        .args(&cmd[1..])
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("scx wrap: cannot run {}: {e}", cmd[0]);
            return 1;
        }
    };
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        eprintln!("scx wrap: cannot capture {} output pipes", cmd[0]);
        return 1;
    };
    // 两个管道必须并发读取，否则任一管道写满都会让子进程阻塞；每路先按
    // 64 MiB 上限读取，合并后再做总上限检查（与 CLI/MCP 输入契约一致）。
    let limit = crate::input::MAX_INPUT_BYTES;
    let (out, err) = std::thread::scope(|scope| {
        let out = scope.spawn(move || read_capped(stdout, limit));
        let err = scope.spawn(move || read_capped(stderr, limit));
        (
            out.join().expect("stdout reader"),
            err.join().expect("stderr reader"),
        )
    });
    let (out, err) = match (out, err) {
        (Ok(out), Ok(err)) => (out, err),
        (Err(e), _) | (_, Err(e)) => {
            let _ = child.kill();
            let _ = child.wait();
            eprintln!("scx wrap: cannot read {} output: {e}", cmd[0]);
            return 1;
        }
    };
    if out.len() as u64 + err.len() as u64 > limit {
        let _ = child.kill();
        let _ = child.wait();
        eprintln!(
            "scx wrap: combined output exceeds the {} MiB limit; split the command output first",
            limit / (1024 * 1024)
        );
        return 1;
    }
    let status = match child.wait() {
        Ok(status) => status,
        Err(e) => {
            eprintln!("scx wrap: cannot wait for {}: {e}", cmd[0]);
            return 1;
        }
    };
    let mut text = String::from_utf8_lossy(&out).into_owned();
    text.push_str(&String::from_utf8_lossy(&err));

    // 压缩（小输入 passthrough、无信号 fail-open 均由内核保证）。
    let outcome = crate::compress(&text, &query, &config);
    if let Err(e) = crate::ops::out_text(&outcome.text) {
        eprintln!("scx wrap: write stdout failed: {e}");
    }
    if !outcome.text.is_empty() && !outcome.text.ends_with('\n') {
        let _ = crate::ops::out_text("\n");
    }
    let st = crate::stat::Stat::from_outcome(&outcome);
    eprintln!("{}", st.card(&outcome, &query));

    // 退出码沿用被包装命令；信号终止按 shell 惯例映射为 128+信号值。
    exit_code(status)
}

/// 读取单个管道至多 `limit + 1` 字节：多出 1 字节用于让调用方检测超限，
/// 不在超限后继续无界分配。
fn read_capped<R: Read>(reader: R, limit: u64) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    reader.take(limit.saturating_add(1)).read_to_end(&mut buf)?;
    Ok(buf)
}

/// 解析 `--` 前的压缩选项（有限的声明式集合，与 args 表解耦）。
fn parse_opts(opts: &[String]) -> Result<(crate::Config, String), String> {
    let mut config = crate::Config::default();
    let mut query = String::new();
    let mut i = 0;
    while i < opts.len() {
        let name = opts[i].as_str();
        let next = |i: &mut usize| -> Result<&str, String> {
            *i += 1;
            opts.get(*i)
                .map(|s| s.as_str())
                .ok_or_else(|| format!("{name} requires a value"))
        };
        match name {
            "--query" => query = next(&mut i)?.to_string(),
            "--ratio" => {
                let r: f64 = next(&mut i)?
                    .parse()
                    .map_err(|_| "--ratio expects a number".to_string())?;
                config.ratio = r;
            }
            "--context" => {
                let c: i64 = next(&mut i)?
                    .parse()
                    .map_err(|_| "--context expects an integer".to_string())?;
                config.context = c;
            }
            "--force" => config.forces.push(next(&mut i)?.to_string()),
            "--preserve" => {
                let ty = next(&mut i)?;
                crate::args::preserve_into(&mut config, ty)?;
            }
            "--evidence" => config.evidence.push(evidence(next(&mut i)?)),
            other => return Err(format!("unknown option {other}")),
        }
        i += 1;
    }
    Ok((config, query))
}

fn evidence(v: &str) -> crate::Evidence {
    crate::Evidence::parse(v).unwrap_or_else(|| crate::Evidence::Substr(v.trim().to_string()))
}

#[cfg(test)]
mod wrap_tests {
    use super::*;

    #[test]
    fn parse_opts_reads_query_ratio_force_preserve() {
        let opts: Vec<String> = [
            "--query",
            "db timeout",
            "--ratio",
            "0.5",
            "--force",
            "SPECIAL",
            "--preserve",
            "url",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let (config, query) = parse_opts(&opts).unwrap();
        assert_eq!(query, "db timeout");
        assert!((config.ratio - 0.5).abs() < 1e-9);
        assert_eq!(
            config.forces,
            vec!["SPECIAL".to_string(), "re://".to_string()]
        );
    }

    #[test]
    fn parse_opts_rejects_unknown_and_missing_value() {
        assert!(parse_opts(&["--bogus".to_string()]).is_err());
        assert!(parse_opts(&["--query".to_string()]).is_err());
        assert!(parse_opts(&["--preserve".to_string(), "bogus".to_string()]).is_err());
    }

    #[test]
    fn read_capped_reads_at_most_limit_plus_one() {
        let buf = read_capped(std::io::Cursor::new(b"123456789"), 8).unwrap();
        assert_eq!(buf, b"123456789");
        assert!(buf.len() as u64 > 8, "多读 1 字节用于检测超限");

        let buf = read_capped(std::io::Cursor::new(b"12345678"), 8).unwrap();
        assert_eq!(buf, b"12345678");
    }
}

/// `scx bucket` 入口：读 stdin 类型化载荷 JSON，分桶压缩输出 `Outcome` JSON；
/// `--record` 追加收益记录。
pub fn bucket_run(args: &[String]) -> i32 {
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
mod bucket_tests {
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
