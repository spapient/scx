//! 开发/CI 评估工具入口（`cargo run --features eval --bin xtask -- ...`）。
//! 不进发布二进制：Cargo.toml 以 `required-features = ["eval"]` 排除默认构建。

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(scx::evaltool::run(&args));
}
