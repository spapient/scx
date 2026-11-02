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
//!
//! 向量后端（`--embed-vec` + `--embed-cache`，仿 usearch 持久化思路）：
//! - 同一 stdin 协议，但 stdout **每个输入行输出一个原始向量**（首行 = query
//!   向量，随后每行 = 一个 text 向量，空格分隔 dim 个 f32），归一化与余弦在
//!   scx 内完成（f64，确定性）。
//! - `--embed-cache <dir>` 启用候选向量持久化：text 向量按内容 sha256 键落盘
//!   一次（`<dir>/<key16>.vec`），后续召回只在线编码 query、直接读盘取候选
//!   向量——省掉 provider 重复加载模型与重复编码全部候选的大头开销。

#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::hash::{hex, sha256};

/// 计算 `query` 与每个候选文本的相似度。
///
/// `cmd: None` → 返回全 0（默认路径，无副作用、无 I/O）。
/// `cmd: Some(program)` → Unix 用 `sh -c <program>`，Windows 用
/// `cmd /C <program>` 启动 provider，按上述协议批式计算。
pub fn compute(cmd: Option<&str>, query: &str, texts: &[&str]) -> Result<Vec<f64>, String> {
    let Some(program) = cmd else {
        return Ok(vec![0.0; texts.len()]);
    };
    let program = program.trim();
    if program.is_empty() {
        return Err("--embed-cmd requires a non-empty command".to_string());
    }

    let mut child = shell(program)
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

/// 按平台选择 shell；`program` 由调用方提供，语义与平台原生 shell 一致。
fn shell(program: &str) -> Command {
    #[cfg(windows)]
    {
        let mut command = Command::new("cmd");
        command.arg("/C").arg(program);
        command
    }
    #[cfg(not(windows))]
    {
        let mut command = Command::new("sh");
        command.arg("-c").arg(program);
        command
    }
}

/// 去除换行/回车，避免破坏 `Q\t`/`L\t` 单行协议（text 为已切行切片，通常无需处理）。
fn sanitize(s: &str) -> String {
    if s.contains('\n') || s.contains('\r') {
        s.replace(['\n', '\r'], " ")
    } else {
        s.to_string()
    }
}

/// 向量后端：`query` 与每个候选文本的余弦相似度（scx 内归一化 + 点积，f64 确定性）。
///
/// `cmd: None` → 全 0。`cache: Some(dir)` 时启用候选向量持久化：text 向量按
/// 内容 sha256 键落盘一次，后续召回只在线编码 query、候选向量直接读盘，
/// 避免 provider 重复加载模型与重复编码全部候选。
pub fn compute_vec(
    cmd: Option<&str>,
    cache: Option<&Path>,
    query: &str,
    texts: &[&str],
) -> Result<Vec<f64>, String> {
    let Some(program) = cmd else {
        return Ok(vec![0.0; texts.len()]);
    };
    let program = program.trim();
    if program.is_empty() {
        return Err("--embed-vec requires a non-empty command".to_string());
    }
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let dir = match cache {
        Some(d) => {
            std::fs::create_dir_all(d).map_err(|e| format!("--embed-cache mkdir: {e}"))?;
            Some(d.to_path_buf())
        }
        None => None,
    };

    // query 向量：每次在线编码（query 与候选不同，不可缓存）。
    let qv = spawn_vec(program, query, &[])?
        .into_iter()
        .next()
        .ok_or("--embed-vec: provider returned no query vector")?;

    // 候选向量：先读缓存，未命中者批量编码一次并写回。
    let mut tv: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
    let mut miss: Vec<(usize, String, &str)> = Vec::new();
    for (i, t) in texts.iter().enumerate() {
        let key = vec_key(t);
        let hit = dir.as_deref().and_then(|d| read_vec(&cache_path(d, &key)));
        match hit {
            Some(v) => tv.push(v),
            None => {
                tv.push(Vec::new());
                miss.push((i, key, *t));
            }
        }
    }
    if !miss.is_empty() {
        let miss_texts: Vec<&str> = miss.iter().map(|(_, _, t)| *t).collect();
        let out = spawn_vec(program, query, &miss_texts)?;
        if out.len() != 1 + miss.len() {
            return Err(format!(
                "--embed-vec: provider returned {} vectors for {} texts",
                out.len() - 1,
                miss.len()
            ));
        }
        for (j, (i, key, _)) in miss.iter().enumerate() {
            let v = &out[j + 1];
            tv[*i] = v.clone();
            if let Some(d) = &dir {
                write_vec(&cache_path(d, key), v)
                    .map_err(|e| format!("--embed-cache write: {e}"))?;
            }
        }
    }

    let qn = normalize(&qv);
    Ok(tv.iter().map(|v| dot(&qn, &normalize(v))).collect())
}

/// 启动向量 provider：stdin 写 `Q\t<query>` 与 `L\t<text>` 行，stdout 每个输入
/// 行一个向量（空格分隔 dim 个 f32，首行为 query 向量、随后为 text 向量）。
fn spawn_vec(program: &str, query: &str, texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
    let mut child = shell(program)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("--embed-vec spawn: {e}"))?;

    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "--embed-vec: provider stdin unavailable".to_string())?;
        writeln!(stdin, "Q\t{}", sanitize(query)).map_err(|e| format!("--embed-vec write: {e}"))?;
        for t in texts {
            writeln!(stdin, "L\t{}", sanitize(t)).map_err(|e| format!("--embed-vec write: {e}"))?;
        }
    } // stdin 关闭，子进程读到 EOF。

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "--embed-vec: provider stdout unavailable".to_string())?;
    let mut reader = BufReader::new(stdout);
    let mut vecs = Vec::with_capacity(1 + texts.len());
    for _ in 0..=texts.len() {
        let mut line = String::new();
        let n = reader
            .read_line(&mut line)
            .map_err(|e| format!("--embed-vec read: {e}"))?;
        if n == 0 {
            return Err(
                "--embed-vec: provider closed stdout before emitting all vectors".to_string(),
            );
        }
        let v: Vec<f32> = line
            .split_whitespace()
            .map(|s| {
                s.parse()
                    .map_err(|e| format!("--embed-vec: bad float {:?}: {e}", s))
            })
            .collect::<Result<_, _>>()?;
        if v.is_empty() {
            return Err("--embed-vec: empty vector line".to_string());
        }
        vecs.push(v);
    }

    let status = child.wait().map_err(|e| format!("--embed-vec wait: {e}"))?;
    if !status.success() {
        return Err(format!("--embed-vec: provider exited with {status}"));
    }
    Ok(vecs)
}

/// 候选向量缓存键：内容 sha256 前 16 hex（64 bit），同内容跨召回/跨会话复用。
fn vec_key(text: &str) -> String {
    hex(&sha256(text.as_bytes()))[..16].to_string()
}

fn cache_path(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{key}.vec"))
}

/// 读缓存向量：`u32 LE dim` + `f32 LE × dim`；缺失/损坏/长度不符 → None（按未命中重编码）。
fn read_vec(path: &Path) -> Option<Vec<f32>> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < 4 {
        return None;
    }
    let dim = u32::from_le_bytes(bytes[0..4].try_into().ok()?) as usize;
    if bytes.len() != 4 + dim * 4 {
        return None;
    }
    let mut v = Vec::with_capacity(dim);
    for i in 0..dim {
        let off = 4 + i * 4;
        v.push(f32::from_le_bytes(bytes[off..off + 4].try_into().ok()?));
    }
    Some(v)
}

/// 写缓存向量（原子：先写 `.tmp` 再 rename，避免并发读半成品）。
fn write_vec(path: &Path, v: &[f32]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("--embed-cache mkdir: {e}"))?;
    }
    let tmp = path.with_extension("vec.tmp");
    let mut buf = Vec::with_capacity(4 + v.len() * 4);
    buf.extend_from_slice(&(v.len() as u32).to_le_bytes());
    for x in v {
        buf.extend_from_slice(&x.to_le_bytes());
    }
    std::fs::write(&tmp, &buf).map_err(|e| format!("--embed-cache write: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("--embed-cache rename: {e}"))?;
    Ok(())
}

/// L2 归一化（f64），供余弦点积使用。
fn normalize(v: &[f32]) -> Vec<f64> {
    let mut norm = 0.0f64;
    for x in v {
        norm += (*x as f64) * (*x as f64);
    }
    if norm <= 0.0 {
        return vec![0.0; v.len()];
    }
    let inv = norm.sqrt().recip();
    v.iter().map(|x| (*x as f64) * inv).collect()
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    let mut acc = 0.0f64;
    for (x, y) in a.iter().zip(b.iter()) {
        acc += x * y;
    }
    acc
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
        // 模拟 provider：读入后输出固定两个分数（批式，先读后写）。
        let cmd = if cfg!(windows) {
            "echo 0.25& echo 0.75"
        } else {
            "printf '0.25\\n0.75\\n'"
        };
        let got = compute(Some(cmd), "query", &["line a", "line b"]).unwrap();
        assert_eq!(got.len(), 2);
        assert!((got[0] - 0.25).abs() < 1e-9);
        assert!((got[1] - 0.75).abs() < 1e-9);
    }

    #[test]
    fn short_output_is_rejected() {
        let cmd = if cfg!(windows) {
            "echo 0.5"
        } else {
            "printf '0.5\\n'"
        }; // 只输出 1 个，但请求 2 个
        assert!(compute(Some(cmd), "q", &["a", "b"]).is_err());
    }
}
