//! 测试专用工具（仅 `#[cfg(test)]` 编译，生产代码不引用本模块）。
//!
//! 收拢各测试模块重复的"唯一临时路径"模式：`<temp>/scx-<tag>-<pid>-<nanos>`，
//! 避免并发测试互相覆盖临时文件/目录。

use std::time::{SystemTime, UNIX_EPOCH};

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
