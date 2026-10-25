#![forbid(unsafe_code)]
#![warn(clippy::cognitive_complexity)]

// scx CLI：参数解析在 scx::args，输入读取在 scx::input，操作编排在 scx::ops
// （Op 枚举 + run + 纯渲染）；筛选逻辑全在库 crate（scx::compress）。
// 本文件只做进程入口：帮助/版本短路、解析、分派与失败出口。

use scx::args::{self, parse_args};
use scx::ops::{self, Failure};

/// 帮助/版本短路：`--` 终止符之后不再解释为选项。
fn early_exit(args: &[String]) -> bool {
    let mut after_terminator = false;
    for a in args {
        if a == "--" {
            after_terminator = true;
            continue;
        }
        if after_terminator {
            continue;
        }
        if is_help(a) {
            println!("{}", args::usage());
            return true;
        }
        if is_version(a) {
            println!("scx {}", scx::VERSION);
            return true;
        }
    }
    false
}

fn is_help(a: &str) -> bool {
    a == "--help" || a == "-h"
}

fn is_version(a: &str) -> bool {
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

fn run(args: &[String]) -> ops::CliResult {
    if early_exit(args) {
        return Ok(());
    }
    let opts = parse_args(args).map_err(Failure::Usage)?;
    ops::run(ops::from_opts(&opts))
}
