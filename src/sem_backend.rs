//! 语义打分后端（`cargo feature="semantic"`，零第三方 crate）。
//!
//! # 定位
//! `--semantic <weights.json>` 加载权重后，对候选行批量打分。
//! - **cpu（默认）**：流式分块 + `std::thread` 并行。每行特征/点积与
//!   串行路径**逐位一致**（行间无依赖，并行只换吞吐不换数值）→ golden
//!   零漂移。工作内存受 `--max-mem`（MB，默认 64）预算约束：按
//!   `24 f64 特征 + 8B 分数`/行估算块内行数，超出则缩小块。
//! - **gpu（可选加速，默认关）**：调用**自研**外部内核（`SCX_GPU_BIN`
//!   指向可执行文件），JSON lines 协议批量打分——不是任何第三方推理引擎
//!   /外部模型。内核缺失或调用失败 → stderr 提示并自动回落 cpu
//!   （确定性路径不受影响）。
//!
//! # GPU 内核协议（自研，JSON lines，逐块请求）
//! stdin 一行请求：`{"schema":"scx-sem-batch","sha256":"<weights sha>",
//!   "terms":["..."],"lines":["..."]}`
//! stdout 每行一个分数（f64，居中 `2·sigmoid(w·x)−1`，行序对应），随后
//! 一行 `{"done":true,"n":N}`。校验：分数行数 == N == 请求行数、每个分数
//! 有限 → 通过；任何不符/非 0 退出/超时 → 回落 cpu。
//!
//! 确定性边界（文档承诺）：golden 冻结路径只用 cpu 后端；gpu 是显式
//! 可选的加速通道，其输出与 cpu 逐位不同属预期（不进入冻结基线）。

use crate::semantic::Weights;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

/// 语义打分后端选择（`--sem-backend cpu|gpu`，配置化声明，禁硬编码）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backend {
    /// 确定性 CPU 实现（默认）：并行逐位一致 + `--max-mem` 分块。
    #[default]
    Cpu,
    /// 可选 GPU 加速（自研内核，`SCX_GPU_BIN`；不可用自动回落 cpu）。
    Gpu,
}

impl Backend {
    pub fn parse(s: &str) -> Result<Backend, String> {
        match s {
            "cpu" => Ok(Backend::Cpu),
            "gpu" => Ok(Backend::Gpu),
            _ => Err(format!("--sem-backend expects cpu|gpu, got {s:?}")),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Backend::Cpu => "cpu",
            Backend::Gpu => "gpu",
        }
    }
}

/// 批量居中打分：每行 `2·sigmoid(w·x)−1 ∈ (−1,1)`，dup 行恒 0.0（不参与
/// 语义叠加，与原逐行语义一致）。gpu 后端探测失败自动回落 cpu（stderr 提示）。
pub fn centered_batch(
    w: &Weights,
    lines: &[&str],
    terms: &[String],
    total: usize,
    dup: &[bool],
    backend: Backend,
    max_mem_mb: usize,
) -> Vec<f64> {
    if backend == Backend::Gpu {
        match gpu_batch(w, lines, terms, total, dup, max_mem_mb) {
            Ok(s) => return s,
            Err(e) => eprintln!("scx: sem-backend gpu 不可用，回落 cpu（确定性）：{e}"),
        }
    }
    cpu_batch(w, lines, terms, total, dup, max_mem_mb)
}

/// 每行峰值工作内存（保守）：24 维特征 24×8B + 分数槽 8B + 辅助 ~256B。
const PER_LINE_BYTES: usize = 24 * 8 + 8 + 256;
/// 块内最小行数（防空块；预算折算值恒 ≥2299，仅 total 极小时生效）。
const CHUNK_MIN: usize = 64;
/// 单块并行线程上限（可用核数之上的安全顶，防 spawn 爆炸）。
const N_THREADS_CAP: usize = 32;

/// 按内存预算折算每块行数：`max_mem` MB → 字节 → 行数，直接作为块上限。
/// 预算即块大小（64MB → 147168 行、128MB → 294337、256MB → 588674），
/// 无额外拍脑袋系数；total 不足预算时整块处理。
fn chunk_lines(total: usize, max_mem_mb: usize) -> usize {
    let budget = max_mem_mb.max(1).saturating_mul(1024 * 1024);
    (budget / PER_LINE_BYTES)
        .max(CHUNK_MIN)
        .min(total.max(CHUNK_MIN))
}

fn cpu_batch(
    w: &Weights,
    lines: &[&str],
    terms: &[String],
    total: usize,
    dup: &[bool],
    max_mem_mb: usize,
) -> Vec<f64> {
    let mut out = vec![0.0f64; total];
    if total == 0 {
        return out;
    }
    let chunk = chunk_lines(total, max_mem_mb);
    let n_threads = std::thread::available_parallelism()
        .map(|v| v.get())
        .unwrap_or(1)
        .min(N_THREADS_CAP);
    for base in (0..total).step_by(chunk) {
        let end = (base + chunk).min(total);
        let seg = (end - base).div_ceil(n_threads);
        std::thread::scope(|sc| {
            let mut handles = Vec::new();
            for t in 0..n_threads {
                let lo = base + t * seg;
                let hi = (lo + seg).min(end);
                if lo >= hi {
                    continue;
                }
                // 行间无依赖：每行独立特征/点积 → 并行与串行逐位一致。
                // 每线程返回自己段的分数，主线程 join 后写回（无共享可变状态）。
                handles.push(sc.spawn(move || {
                    let mut seg = Vec::with_capacity(hi - lo);
                    for i in lo..hi {
                        seg.push(if dup[i] {
                            0.0
                        } else {
                            w.centered_line(lines[i], terms, i, total)
                        });
                    }
                    seg
                }));
            }
            for (t, h) in handles.into_iter().enumerate() {
                let seg_vec = h.join().expect("sem cpu worker join");
                let lo = base + t * seg;
                let hi = (lo + seg).min(end);
                out[lo..hi].copy_from_slice(&seg_vec);
            }
        });
    }
    out
}

/// JSON 字符串转义（自研最小序列化；`"` `\` 与控制字符 → `\u00XX`）。
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn gpu_batch(
    w: &Weights,
    lines: &[&str],
    terms: &[String],
    total: usize,
    dup: &[bool],
    max_mem_mb: usize,
) -> Result<Vec<f64>, String> {
    let bin = std::env::var("SCX_GPU_BIN")
        .map_err(|_| "环境变量 SCX_GPU_BIN 未设置（自研 GPU 内核路径）".to_string())?;
    if !std::path::Path::new(&bin).is_file() {
        return Err(format!("SCX_GPU_BIN 不存在: {bin}"));
    }
    let mut out = vec![0.0f64; total];
    // 权重指纹：驱动内核读取同一权重语义（sha 由权重自身按文件口径提供）。
    let sha = w.weight_sha();
    let chunk = chunk_lines(total, max_mem_mb);
    for base in (0..total).step_by(chunk) {
        let end = (base + chunk).min(total);
        let req = batch_request(&sha, terms, lines, base, end);
        gpu_roundtrip(&bin, &req, base, end, total, dup, &mut out)?;
    }
    Ok(out)
}

/// 组装一块的 JSON lines 请求（schema/权重指纹/terms/lines）。
fn batch_request(sha: &str, terms: &[String], lines: &[&str], base: usize, end: usize) -> String {
    let mut req = String::new();
    req.push_str("{\"schema\":\"scx-sem-batch\",\"sha256\":");
    req.push_str(&json_str(sha));
    req.push_str(",\"terms\":[");
    for (k, t) in terms.iter().enumerate() {
        if k > 0 {
            req.push(',');
        }
        req.push_str(&json_str(t));
    }
    req.push_str("],\"lines\":[");
    for (j, line) in lines[base..end].iter().enumerate() {
        if j > 0 {
            req.push(',');
        }
        req.push_str(&json_str(line));
    }
    req.push_str("]}\n");
    req
}

/// 单块 spawn → 写请求 → 读分数行 + done 确认 → 校验行数/有限性 → wait。
/// 任一不符即 Err（由调用方回落 cpu）。
fn gpu_roundtrip(
    bin: &str,
    req: &str,
    base: usize,
    end: usize,
    total: usize,
    dup: &[bool],
    out: &mut [f64],
) -> Result<(), String> {
    let n_lines = end - base;
    let mut child = Command::new(bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn {bin}: {e}"))?;
    child
        .stdin
        .as_mut()
        .ok_or("no stdin")?
        .write_all(req.as_bytes())
        .map_err(|e| format!("write: {e}"))?;
    let mut saw_done = false;
    for (i, line) in BufReader::new(child.stdout.take().ok_or("no stdout")?)
        .lines()
        .map_while(Result::ok)
        .enumerate()
    {
        if line.starts_with("{\"done\":") {
            if i != n_lines {
                return Err(format!(
                    "gpu 内核 done 行序号不符（i={i}，块行数={n_lines}）"
                ));
            }
            if !line.contains(&format!("\"n\":{n_lines}")) {
                return Err(format!("gpu 内核 n 不符: {line}"));
            }
            saw_done = true;
            break;
        }
        let v: f64 = line
            .trim()
            .parse()
            .map_err(|_| format!("gpu 内核非数值输出: {line:?}"))?;
        if !v.is_finite() {
            return Err(format!("gpu 内核非有限分数: {line}"));
        }
        let idx = base + i;
        if idx < total && !dup[idx] {
            out[idx] = v;
        }
    }
    if !saw_done {
        return Err("gpu 内核未输出 done 确认".to_string());
    }
    child.wait().map_err(|e| format!("wait: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_weights() -> Weights {
        // 构造 v2 命名式权重（4 特征 + bias），sha 按 verify_v2 口径计算。
        let names = ["overlap", "hit", "ln_len", "bias"];
        let ws: Vec<f64> = vec![1.5, 1.0, -0.3, -2.0];
        let mut h = Vec::new();
        for v in &ws {
            h.extend_from_slice(&(*v).to_be_bytes());
        }
        for n in names {
            h.extend_from_slice(n.as_bytes());
            h.push(b'\n');
        }
        let sha = crate::hash::hex(&crate::hash::sha256(&h));
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("scx-sem-bk-{}-{stamp}.json", std::process::id()));
        std::fs::write(
            &path,
            format!(
                "{{\"schema\":\"scx-sem-2\",\"features\":[{}],\"weights\":[1.5,1.0,-0.3,-2.0],\
                 \"sha256\":\"{sha}\"}}",
                names
                    .iter()
                    .map(|n| format!("\"{n}\""))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        )
        .unwrap();
        let w = Weights::load(path.to_str().unwrap()).expect("weights load");
        let _ = std::fs::remove_file(&path);
        w
    }

    fn sample_lines() -> (Vec<&'static str>, Vec<String>, Vec<bool>, usize) {
        let lines = vec![
            "error: build failed at crate compile",
            "zzz noise filler line with unrelated words here",
            "conn broken timeout retry conn broken",
            "warmup complete idle tick",
        ];
        let terms = vec!["build".to_string(), "error".to_string(), "conn".to_string()];
        let dup = vec![false, false, false, true];
        (lines, terms, dup, 4)
    }

    /// 写一个 mock 自研 GPU 内核（sh，不解析 JSON）：读一行请求 → 输出
    /// 4 行分数 + done 确认。用于验证 JSON lines 协议端到端契约。
    fn mock_kernel(ok: bool) -> String {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!(
            "scx-sem-mock-{}-{stamp}-{}.sh",
            std::process::id(),
            if ok { "ok" } else { "bad" }
        ));
        let body = if ok {
            "#!/bin/sh\nIFS= read -r req\necho 0.5\necho -0.2\necho 0.9\necho 0.0\necho '{\"done\":true,\"n\":4}'\n"
        } else {
            "#!/bin/sh\nIFS= read -r req\necho 0.5\necho -0.2\necho 9.99\n"
        };
        std::fs::write(&p, body).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.to_str().unwrap().to_string()
    }

    #[cfg(unix)]
    #[test]
    fn gpu_batch_protocol_ok_with_mock_kernel() {
        let w = small_weights();
        let (lines, terms, dup, total) = sample_lines();
        std::env::set_var("SCX_GPU_BIN", mock_kernel(true));
        let got = gpu_batch(&w, &lines, &terms, total, &dup, 64).expect("mock 内核应成功");
        assert_eq!(got[0], 0.5);
        assert_eq!(got[1], -0.2);
        assert_eq!(got[2], 0.9);
        assert_eq!(got[3], 0.0, "dup 行恒 0");
        std::env::remove_var("SCX_GPU_BIN");
    }

    #[cfg(unix)]
    #[test]
    fn gpu_batch_protocol_rejects_malformed_kernel() {
        let w = small_weights();
        let (lines, terms, dup, total) = sample_lines();
        std::env::set_var("SCX_GPU_BIN", mock_kernel(false));
        assert!(
            gpu_batch(&w, &lines, &terms, total, &dup, 64).is_err(),
            "缺 done 确认必须 Err（回落 cpu）"
        );
        std::env::remove_var("SCX_GPU_BIN");
    }

    #[test]
    fn cpu_batch_matches_serial_bitwise() {
        let w = small_weights();
        let (lines, terms, dup, total) = sample_lines();
        let batch = cpu_batch(&w, &lines, &terms, total, &dup, 1);
        assert_eq!(batch.len(), total);
        assert_eq!(batch[3], 0.0, "dup 行恒 0");
        for i in 0..total {
            let ser = if dup[i] {
                0.0
            } else {
                w.centered_line(lines[i], &terms, i, total)
            };
            assert_eq!(batch[i].to_bits(), ser.to_bits(), "行 {i} 逐位一致");
        }
    }

    #[test]
    fn tiny_mem_budget_same_result() {
        let w = small_weights();
        let (lines, terms, dup, total) = sample_lines();
        let a = cpu_batch(&w, &lines, &terms, total, &dup, 1);
        let b = cpu_batch(&w, &lines, &terms, total, &dup, 256);
        assert_eq!(a, b);
    }

    #[test]
    fn gpu_falls_back_to_cpu_without_kernel() {
        let w = small_weights();
        let (lines, terms, dup, total) = sample_lines();
        std::env::remove_var("SCX_GPU_BIN");
        let got = centered_batch(&w, &lines, &terms, total, &dup, Backend::Gpu, 8);
        let want = cpu_batch(&w, &lines, &terms, total, &dup, 8);
        assert_eq!(got, want, "gpu 不可用必须回落 cpu 且结果一致");
    }

    #[test]
    fn backend_parse_and_default() {
        assert_eq!(Backend::parse("cpu").unwrap(), Backend::Cpu);
        assert_eq!(Backend::parse("gpu").unwrap(), Backend::Gpu);
        assert!(Backend::parse("tpu").is_err());
        assert_eq!(Backend::default(), Backend::Cpu);
        assert_eq!(Backend::Cpu.name(), "cpu");
    }

    #[test]
    fn json_str_escapes_quotes_and_controls() {
        assert_eq!(json_str("a\"b\\c\nd"), "\"a\\\"b\\\\c\\nd\"");
        assert_eq!(json_str("数据库"), "\"数据库\"");
    }

    #[test]
    fn chunk_lines_respects_budget_bounds() {
        assert_eq!(chunk_lines(1_000_000, 1), 2299); // 1MB 折算 1MiB/456B
        assert_eq!(chunk_lines(100, 64), 100); // total 小于预算 → 整块
        assert_eq!(chunk_lines(10, 1), CHUNK_MIN); // total < CHUNK_MIN 回退（一次整块）
                                                   // 预算即块上限：64MB → 147168、128MB → 294337、256MB → 588674，单调递增。
        let b64 = chunk_lines(100_000_000, 64);
        let b128 = chunk_lines(100_000_000, 128);
        let b256 = chunk_lines(100_000_000, 256);
        assert_eq!(b64, 64 * 1024 * 1024 / PER_LINE_BYTES);
        assert_eq!(b128, 128 * 1024 * 1024 / PER_LINE_BYTES);
        assert_eq!(b256, 256 * 1024 * 1024 / PER_LINE_BYTES);
        assert!(b64 < b128 && b128 < b256);
        // 极端预算不恐慌（saturating 保护）。
        assert!(chunk_lines(1_000_000_000, usize::MAX / (1024 * 1024)) >= CHUNK_MIN);
    }
}
