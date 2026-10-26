//! MCP stdio 冒烟集成测试（mcp 已并入默认轨，主二进制 `scx mcp` 承担 stdio）。
//! 通过 CARGO_BIN_EXE_scx 拉起真实二进制，走 stdio 往返。

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn spawn() -> (
    std::process::Child,
    std::process::ChildStdin,
    BufReader<std::process::ChildStdout>,
) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_scx"))
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn scx mcp");
    let stdin = child.stdin.take().expect("stdin");
    let stdout = BufReader::new(child.stdout.take().expect("stdout"));
    (child, stdin, stdout)
}

fn roundtrip(
    stdin: &mut std::process::ChildStdin,
    stdout: &mut BufReader<std::process::ChildStdout>,
    msg: &str,
) -> String {
    stdin.write_all(msg.as_bytes()).expect("write msg");
    stdin.write_all(b"\n").expect("write nl");
    stdin.flush().expect("flush");
    let mut line = String::new();
    stdout.read_line(&mut line).expect("read response");
    line
}

#[test]
fn mcp_stdio_roundtrip() {
    let (mut child, mut stdin, mut stdout) = spawn();

    let init = roundtrip(
        &mut stdin,
        &mut stdout,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
    );
    assert!(init.contains("\"serverInfo\""), "initialize: {init}");

    let list = roundtrip(
        &mut stdin,
        &mut stdout,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
    );
    assert!(list.contains("compress_context"), "tools/list: {list}");
    assert!(list.contains("usage_summary"), "tools/list: {list}");

    let text = (0..300)
        .map(|i| {
            if i == 150 {
                "conn 9f2a broken db crash".to_string()
            } else {
                format!("INFO heartbeat seq={i} ok pool=3 latency=1ms")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let escaped = text
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    let call = format!(
        r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"compress_context","arguments":{{"query":"conn broken","ratio":0.35,"text":"{escaped}"}}}}}}"#
    );
    let out = roundtrip(&mut stdin, &mut stdout, &call);
    assert!(
        out.contains("sieve") && out.contains("kept") && out.contains("dropped"),
        "compress_context: {out}"
    );
    // v0.6.5：返回为「效果卡 + JSON」双 content，首块人类可读、次块机器字段。
    assert!(
        out.contains("SCX 压缩召回") && out.contains("tokens_in") && out.contains("token_cut_pct"),
        "compress_context effect card missing: {out}"
    );

    let usage = roundtrip(
        &mut stdin,
        &mut stdout,
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"usage_summary"}}"#,
    );
    assert!(
        usage.contains("compressed") && usage.contains("chars_in"),
        "usage_summary: {usage}"
    );

    let ping = roundtrip(
        &mut stdin,
        &mut stdout,
        r#"{"jsonrpc":"2.0","id":5,"method":"ping"}"#,
    );
    assert!(ping.contains("result"), "ping: {ping}");

    drop(stdin);
    let status = child.wait().expect("wait scx mcp");
    assert!(status.success(), "exit status: {status:?}");
}
