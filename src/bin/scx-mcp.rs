//! MCP stdio 服务器入口（`cargo build --features mcp`）。
//! 薄壳：所有逻辑在 `scx::mcp`（feature-gated lib 模块）。

fn main() {
    std::process::exit(scx::mcp::serve());
}
