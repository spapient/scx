//! 可插拔相似度后端（默认关闭，`--embed-cmd` 启用）。
//!
//! 设计原则：相似度是**传入纯核心的数据**，不在 `compress` 内做 I/O。
//! 本模块只提供一个批量函数 `compute`：`ops` 层在进入 `compress` 前调用它，
//! 得到 `query` 与每行的相似度 `Vec<f64>`，经 `Config::sims` 传入纯核心。
//! 默认 `None` 返回全 0，`compress` 行为与 v0.6.7 逐字节一致。
//!
//! 子进程协议（批量、文本行）：
//! - stdin：首行 `Q\t<query>`，随后每行 `L\t<text>`，写完关闭 stdin（EOF）。
//! - stdout：与 `L` 行数相同的浮点相似度，每行一个（顺序对应）。
//! - 约定：子进程**先读完 stdin 再写 stdout**（批式），避免管道死锁。
//!   确定性只保证在确定性 provider 下成立（固定模型 + 固定 seed）。

#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

/// 计算 `query` 与每个候选文本的相似度。
///
/// `cmd: None` → 返回全 0（默认路径，无副作用、无 I/O）。
/// `cmd: Some(program)` → 以 `sh -c <program>` 启动 provider，按上述协议批式计算。
pub fn compute(cmd: Option<&str>, query: &str, texts: &[&str]) -> Result<Vec<f64>, String> {
    let Some(program) = cmd else {
        return Ok(vec![0.0; texts.len()]);
    };
    let program = program.trim();
    if program.is_empty() {
        return Err("--embed-cmd requires a non-empty command".to_string());
    }

    let mut child = Command::new("sh")
        .arg("-c")
        .arg(program)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("--embed-cmd spawn: {e}"))?;

    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "--embed-cmd: provider stdin unavailable".to_string())?;
        let safe_query = sanitize(query);
        writeln!(stdin, "Q\t{safe_query}").map_err(|e| format!("--embed-cmd write: {e}"))?;
        for t in texts {
            let safe = sanitize(t);
            writeln!(stdin, "L\t{safe}").map_err(|e| format!("--embed-cmd write: {e}"))?;
        }
    } // stdin 在此关闭，子进程读到 EOF。

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "--embed-cmd: provider stdout unavailable".to_string())?;
    let mut reader = BufReader::new(stdout);
    let mut sims = Vec::with_capacity(texts.len());
    for _ in 0..texts.len() {
        let mut line = String::new();
        let n = reader
            .read_line(&mut line)
            .map_err(|e| format!("--embed-cmd read: {e}"))?;
        if n == 0 {
            return Err(
                "--embed-cmd: provider closed stdout before emitting all scores".to_string(),
            );
        }
        let v: f64 = line
            .trim()
            .parse()
            .map_err(|e| format!("--embed-cmd: bad score {:?}: {e}", line.trim()))?;
        if !v.is_finite() {
            return Err(format!("--embed-cmd: non-finite score {v}"));
        }
        sims.push(v);
    }

    let status = child.wait().map_err(|e| format!("--embed-cmd wait: {e}"))?;
    if !status.success() {
        return Err(format!("--embed-cmd: provider exited with {status}"));
    }
    Ok(sims)
}

/// 去除换行/回车，避免破坏 `Q\t`/`L\t` 单行协议（text 为已切行切片，通常无需处理）。
fn sanitize(s: &str) -> String {
    if s.contains('\n') || s.contains('\r') {
        s.replace(['\n', '\r'], " ")
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_backend_returns_zeros() {
        assert_eq!(compute(None, "q", &["a", "b"]).unwrap(), vec![0.0, 0.0]);
    }

    #[test]
    fn empty_command_is_rejected() {
        assert!(compute(Some("   "), "q", &["a"]).is_err());
    }

    #[test]
    fn subprocess_scores_are_parsed_in_order() {
        // 用 printf 模拟 provider：读入后输出固定两个分数（批式，先读后写）。
        let cmd = "printf '0.25\\n0.75\\n'";
        let got = compute(Some(cmd), "query", &["line a", "line b"]).unwrap();
        assert_eq!(got.len(), 2);
        assert!((got[0] - 0.25).abs() < 1e-9);
        assert!((got[1] - 0.75).abs() < 1e-9);
    }

    #[test]
    fn short_output_is_rejected() {
        let cmd = "printf '0.5\\n'"; // 只输出 1 个，但请求 2 个
        assert!(compute(Some(cmd), "q", &["a", "b"]).is_err());
    }
}
