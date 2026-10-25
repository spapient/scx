#![forbid(unsafe_code)]
#![warn(clippy::cognitive_complexity)]

// scx CLI：参数解析在 scx::args，输入读取在 scx::input，操作编排在 scx::ops
// （Op 枚举 + run + 纯渲染）；筛选逻辑全在库 crate（scx::compress）。
// 本文件只做进程入口：帮助/版本短路、解析、分派与失败出口。

use scx::args::{self, parse};
use scx::ops::{self, Failure};

/// 帮助/版本短路：`--` 终止符之后不再解释为选项。
fn short(args: &[String]) -> bool {
    let mut past = false;
    for a in args {
        if a == "--" {
            past = true;
            continue;
        }
        if past {
            continue;
        }
        if help(a) {
            println!("{}", args::usage());
            return true;
        }
        if version(a) {
            println!("scx {}", scx::VERSION);
            return true;
        }
    }
    false
}

fn help(a: &str) -> bool {
    a == "--help" || a == "-h"
}

fn version(a: &str) -> bool {
    a == "--version" || a == "-V"
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let failure = match run(&args) {
        Ok(()) => return,
        Err(failure) => failure,
    };
    match failure {
        Failure::Usage(message) => {
            eprintln!("scx: {message}");
            eprintln!("{}", args::usage());
        }
        Failure::Message(message) => eprintln!("scx: {message}"),
        Failure::GateFailed => {}
    }
    std::process::exit(2);
}

fn run(args: &[String]) -> ops::Reply {
    if short(args) {
        return Ok(());
    }
    let env_mcp = std::env::var("SCX_MCP").is_ok_and(|v| v == "1" || v == "true");
    if wants_mcp(args, env_mcp) {
        std::process::exit(scx::mcp::serve());
    }
    if let Some(sub) = args.first() {
        if sub == "setup" {
            std::process::exit(scx::setup::run(&args[1..]));
        }
    }
    let opts = parse(args).map_err(Failure::Usage)?;
    ops::run(ops::parse(&opts))
}

/// 是否进入 MCP server 模式：显式子命令 `mcp`，或环境变量 `SCX_MCP=1`
/// 且无命令行参数（供「命令字段只保存单 token」的平台以裸命令启动）。
/// 有显式参数时参数优先，环境变量不劫持正常调用。
fn wants_mcp(args: &[String], env_mcp: bool) -> bool {
    (env_mcp && args.is_empty()) || args.first().is_some_and(|s| s == "mcp")
}

#[cfg(test)]
mod tests {
    use super::wants_mcp;

    #[test]
    fn env_mcp_without_args_serves() {
        assert!(wants_mcp(&[], true));
    }

    #[test]
    fn env_mcp_with_args_deferred() {
        assert!(!wants_mcp(
            &["--ratio".into(), "0.2".into(), "f.txt".into()],
            true
        ));
        assert!(!wants_mcp(&["--help".into()], true));
    }

    #[test]
    fn explicit_mcp_subcommand_serves() {
        assert!(wants_mcp(&["mcp".into()], false));
        assert!(wants_mcp(&["mcp".into()], true));
    }

    #[test]
    fn no_env_no_mcp() {
        assert!(!wants_mcp(&[], false));
        assert!(!wants_mcp(&["setup".into()], false));
        assert!(!wants_mcp(&["compress".into(), "f.txt".into()], false));
    }
}
