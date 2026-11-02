//! 集成测试共享设施：spawn 真实 scx 二进制，stdin/stdout/stderr 往返。
//! `tests/common/` 子目录不会被 Cargo 自动注册为集成测试目标，
//! 由各测试文件 `mod common;` 引入。

use std::io::Write;
use std::process::{Command, Stdio};

pub fn scx() -> &'static str {
    env!("CARGO_BIN_EXE_scx")
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
