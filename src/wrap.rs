//! `scx wrap`：包装一个命令，把其 stdout+stderr 合并后按需压缩输出。
//!
//! 用法：`scx wrap [--query Q] [--ratio R] [--context N] [--force P] [--preserve T] [--evidence N|substr] -- CMD...`
//!
//! 行为：执行 CMD，合并 stdout+stderr；小输出由 `compress` 内核自动 passthrough，
//! 大输出按 `--query`/`--force`/`--preserve` 压缩（无信号时 fail-open 保留原文）；
//! 正文写 stdout、效果卡写 stderr，退出码沿用 CMD。

use std::process::Command;

/// 主入口：解析 `--` 分隔的压缩选项与命令，执行后压缩输出。返回进程退出码。
pub fn run(args: &[String]) -> i32 {
    let Some(sep) = args.iter().position(|a| a == "--") else {
        eprintln!("scx wrap: usage: scx wrap [--query Q] [--ratio R] [--context N] [--force P] [--preserve T] [--evidence N|substr] -- CMD...");
        return 2;
    };
    let (opts, cmd) = (&args[..sep], &args[sep + 1..]);
    if cmd.is_empty() {
        eprintln!("scx wrap: missing command after --");
        return 2;
    }
    let (config, query) = match parse_opts(opts) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("scx wrap: {e}");
            return 2;
        }
    };

    // 执行命令，合并 stdout+stderr（命令的 stderr 也是排障证据，一并参与筛选）。
    let output = match Command::new(&cmd[0]).args(&cmd[1..]).output() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("scx wrap: cannot run {}: {e}", cmd[0]);
            return 1;
        }
    };
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));

    // 压缩（小输入 passthrough、无信号 fail-open 均由内核保证）。
    let outcome = crate::compress(&text, &query, &config);
    print!("{}", outcome.text);
    if !outcome.text.is_empty() && !outcome.text.ends_with('\n') {
        println!();
    }
    eprintln!("{}", crate::ops::recall_report(&outcome, &query));

    // 退出码沿用被包装命令。
    output.status.code().unwrap_or(1)
}

/// 解析 `--` 前的压缩选项（有限的声明式集合，与 args 表解耦）。
fn parse_opts(opts: &[String]) -> Result<(crate::Config, String), String> {
    let mut config = crate::Config::default();
    let mut query = String::new();
    let mut i = 0;
    while i < opts.len() {
        let name = opts[i].as_str();
        let next = |i: &mut usize| -> Result<&str, String> {
            *i += 1;
            opts.get(*i)
                .map(|s| s.as_str())
                .ok_or_else(|| format!("{name} requires a value"))
        };
        match name {
            "--query" => query = next(&mut i)?.to_string(),
            "--ratio" => {
                let r: f64 = next(&mut i)?
                    .parse()
                    .map_err(|_| "--ratio expects a number".to_string())?;
                config.ratio = r;
            }
            "--context" => {
                let c: i64 = next(&mut i)?
                    .parse()
                    .map_err(|_| "--context expects an integer".to_string())?;
                config.context = c;
            }
            "--force" => config.forces.push(next(&mut i)?.to_string()),
            "--preserve" => {
                let ty = next(&mut i)?;
                crate::args::preserve_into(&mut config, ty)?;
            }
            "--evidence" => config.evidence.push(evidence(next(&mut i)?)),
            other => return Err(format!("unknown option {other}")),
        }
        i += 1;
    }
    Ok((config, query))
}

/// 显式证据解析：正整数 = 原始行号（1-based），其他 = 输出必须出现的子串。
/// 与 `args.rs` 的 `--evidence` 同口径（wrap 与 args 表解耦，故独立实现）。
fn evidence(v: &str) -> crate::Evidence {
    let trimmed = v.trim();
    match trimmed.parse::<usize>() {
        Ok(n) if n >= 1 => crate::Evidence::Line(n),
        _ => crate::Evidence::Substr(trimmed.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_opts_reads_query_ratio_force_preserve() {
        let opts: Vec<String> = [
            "--query",
            "db timeout",
            "--ratio",
            "0.5",
            "--force",
            "SPECIAL",
            "--preserve",
            "url",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let (config, query) = parse_opts(&opts).unwrap();
        assert_eq!(query, "db timeout");
        assert!((config.ratio - 0.5).abs() < 1e-9);
        assert_eq!(
            config.forces,
            vec!["SPECIAL".to_string(), "re://".to_string()]
        );
    }

    #[test]
    fn parse_opts_rejects_unknown_and_missing_value() {
        assert!(parse_opts(&["--bogus".to_string()]).is_err());
        assert!(parse_opts(&["--query".to_string()]).is_err());
        assert!(parse_opts(&["--preserve".to_string(), "bogus".to_string()]).is_err());
    }
}
