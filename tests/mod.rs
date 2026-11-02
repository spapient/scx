//! 测试共享设施：spawn 真实 scx 二进制，stdin/stdout/stderr 往返，以及
//! 库内 `#[cfg(test)]` 单元测试复用的唯一临时路径 helper。
//! 集成测试通过 `#[path = "mod.rs"] mod common;` 引入；Cargo 也会把本文件
//! 注册为一个 0 用例的集成测试目标（`mod`），属预期的轻微开销。
//!
//! `scx()` 用 `option_env!` 而非 `env!`：本文件被 `src/lib.rs` 以 `#[path]`
//! 编入库内测试时不存在 `CARGO_BIN_EXE_scx`，该函数在库内测试中不会被调用。

#![allow(dead_code)]

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

#[allow(clippy::option_env_unwrap)] // 库内测试构建无此变量；本函数仅在集成测试中调用
pub fn scx() -> &'static str {
    option_env!("CARGO_BIN_EXE_scx").expect("集成测试目标应设置 CARGO_BIN_EXE_scx")
}

/// 唯一临时目录路径（`tag` 为前缀防碰撞）。
///
/// 返回 `<temp>/scx-<tag>-<pid>-<nanos>`；非 UTF-8 路径经 lossy 转换（测试环境
/// 极少出现，不 panic 优先）。
pub fn tmpdir(tag: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir()
        .join(format!("scx-{tag}-{}-{nanos}", std::process::id()))
        .to_string_lossy()
        .to_string()
}

/// 运行 scx：`args` + `stdin_txt` → `(成功?, stdout, stderr)`。
/// stdin 写入容错 BrokenPipe（非法参数时子进程可能先退出，属预期竞态）。
pub fn run(args: &[&str], stdin_txt: &str) -> (bool, String, String) {
    let mut child = Command::new(scx())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn scx");
    let mut stdin = child.stdin.take().unwrap();
    if let Err(e) = stdin.write_all(stdin_txt.as_bytes()) {
        assert_eq!(e.kind(), std::io::ErrorKind::BrokenPipe, "write stdin: {e}");
    }
    drop(stdin);
    let out = child.wait_with_output().expect("wait");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}
