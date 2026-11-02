//! `xtask extract`：Claude Code 会话 JSONL -> 压缩语料（feature = "eval"）。
//!
//! 迁移自 `tools/scx.py extract`：扫描 `*.jsonl`，统计 tool_use / tool_result
//! 文本量，并可把 result 字符数最大的 N 个会话导出为 `.dump.txt`。

// JSONL 解析分支线性；认知复杂度门禁针对压缩内核，不适用于迁移工具。
#![allow(clippy::cognitive_complexity)]

use super::io;
use super::{fail, Fail};
use scx::json::Json;
use std::fs;
use std::path::{Path, PathBuf};

struct Opts {
    dir: String,
    out: String,
    extract: Option<usize>,
}

struct Stat {
    path: PathBuf,
    use_n: usize,
    res_n: usize,
    use_c: usize,
    res_c: usize,
}

pub(crate) fn run(args: &[String]) -> Result<(), Fail> {
    let opts = parse(args)?;
    let mut files: Vec<PathBuf> = fs::read_dir(&opts.dir)
        .map_err(|e| fail(format!("{}: {e}", opts.dir)))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    files.sort();
    let mut stats = Vec::new();
    for path in files {
        let blocks = blocks(&path)?;
        let mut use_n = 0usize;
        let mut res_n = 0usize;
        let mut use_c = 0usize;
        let mut res_c = 0usize;
        for (kind, text) in &blocks {
            if kind == "use" {
                use_n += 1;
                use_c += text.chars().count();
            } else {
                res_n += 1;
                res_c += text.chars().count();
            }
        }
        stats.push(Stat {
            path,
            use_n,
            res_n,
            use_c,
            res_c,
        });
    }
    stats.sort_by_key(|stat| std::cmp::Reverse(stat.res_c));
    for stat in &stats {
        let base: String = basename(&stat.path).chars().take(12).collect();
        println!(
            "{base:<14} tool_use={:>4} tool_result={:>4} use_chars={:>9} result_chars={:>9}",
            stat.use_n, stat.res_n, stat.use_c, stat.res_c
        );
    }
    if let Some(count) = opts.extract.filter(|count| *count > 0) {
        fs::create_dir_all(&opts.out).map_err(|e| fail(format!("{}: {e}", opts.out)))?;
        for stat in stats.iter().take(count) {
            let base = stat
                .path
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
                .unwrap_or_default();
            let out = Path::new(&opts.out).join(format!("{base}.dump.txt"));
            let mut text = String::new();
            for (_, block) in blocks(&stat.path)? {
                text.push_str(&block);
                if !block.is_empty() && !block.ends_with('\n') {
                    text.push('\n');
                }
            }
            fs::write(&out, &text).map_err(|e| fail(format!("{}: {e}", out.display())))?;
            let lines = fs::read_to_string(&out)
                .map(|text| text.lines().count())
                .unwrap_or(0);
            println!("  -> {} ({lines} lines)", out.display());
        }
    }
    Ok(())
}

fn parse(args: &[String]) -> Result<Opts, Fail> {
    let mut opts = Opts {
        dir: home()
            .join(".claude/projects/-Users-fei-code-scx")
            .to_string_lossy()
            .to_string(),
        out: std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string()),
        extract: None,
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dir" | "--out" | "--extract" => {
                let flag = args[i].as_str();
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| usage(format!("{flag} expects a value")))?;
                match flag {
                    "--dir" => opts.dir = value.clone(),
                    "--out" => opts.out = value.clone(),
                    "--extract" => {
                        opts.extract = Some(
                            value
                                .parse()
                                .map_err(|_| usage(format!("invalid --extract: {value}")))?,
                        )
                    }
                    _ => {}
                }
            }
            "--scan" => {}
            "-h" | "--help" => return Err(usage(String::new())),
            other => return Err(usage(format!("unrecognized argument: {other}"))),
        }
        i += 1;
    }
    Ok(opts)
}

fn usage(message: impl Into<String>) -> Fail {
    let body = "usage: xtask extract [--dir DIR] [--scan] [--extract N] [--out DIR]\n";
    let message = message.into();
    Fail {
        msg: if message.is_empty() {
            body.to_string()
        } else {
            format!("{message}\n\n{body}")
        },
        code: 2,
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn basename(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn blocks(path: &Path) -> Result<Vec<(String, String)>, Fail> {
    let text = fs::read_to_string(path).map_err(|e| fail(format!("{}: {e}", path.display())))?;
    let mut out = Vec::new();
    for line in text.lines() {
        let Ok(value) = scx::json::parse(line) else {
            continue;
        };
        let Json::Obj(object) = &value else {
            continue;
        };
        let kind = field(object, "type");
        if !matches!(kind.as_str(), "user" | "assistant") {
            continue;
        }
        let message = match object.iter().find(|(key, _)| key == "message") {
            Some((_, Json::Obj(message))) => message,
            _ => continue,
        };
        let content = match message.iter().find(|(key, _)| key == "content") {
            Some((_, Json::Arr(content))) => content,
            _ => continue,
        };
        for block in content {
            let Json::Obj(block) = block else {
                continue;
            };
            match field(block, "type").as_str() {
                "tool_use" => {
                    let Some(input) = block
                        .iter()
                        .find(|(key, _)| key == "input")
                        .map(|(_, value)| value)
                    else {
                        continue;
                    };
                    match input {
                        Json::Obj(entries) => {
                            if let Some(command) = find_str(entries, "command") {
                                out.push(("use".to_string(), format!("command: {command}")));
                            } else if let Some(content) = find_str(entries, "content") {
                                out.push(("use".to_string(), content.to_string()));
                            } else {
                                out.push(("use".to_string(), dumps(input)));
                            }
                        }
                        Json::Str(text) => out.push(("use".to_string(), text.clone())),
                        _ => {}
                    }
                }
                "tool_result" => {
                    let Some(inner) = block
                        .iter()
                        .find(|(key, _)| key == "content")
                        .map(|(_, value)| value)
                    else {
                        continue;
                    };
                    match inner {
                        Json::Arr(items) => {
                            let mut parts = Vec::new();
                            for sub in items {
                                match sub {
                                    Json::Obj(entries)
                                        if find_str(entries, "type") == Some("text") =>
                                    {
                                        parts.push(
                                            find_str(entries, "text").unwrap_or("").to_string(),
                                        );
                                    }
                                    Json::Str(text) => parts.push(text.clone()),
                                    _ => {}
                                }
                            }
                            out.push(("result".to_string(), parts.join("\n")));
                        }
                        Json::Str(text) => out.push(("result".to_string(), text.clone())),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
    Ok(out)
}

fn field(object: &[(String, Json)], key: &str) -> String {
    find_str(object, key).unwrap_or("").to_string()
}

fn find_str<'a>(object: &'a [(String, Json)], key: &str) -> Option<&'a str> {
    object
        .iter()
        .find(|(name, _)| name == key)
        .and_then(|(_, value)| value.str())
}

/// Python `json.dumps(..., ensure_ascii=False)` 的紧凑格式。
fn dumps(value: &Json) -> String {
    match value {
        Json::Null => "null".to_string(),
        Json::Bool(flag) => flag.to_string(),
        Json::Num(number) if number.fract() == 0.0 && number.abs() < 9e15 => {
            (*number as i64).to_string()
        }
        Json::Num(number) => io::repr(*number),
        Json::Str(text) => io::json_string(text),
        Json::Arr(items) => {
            let body = items.iter().map(dumps).collect::<Vec<_>>().join(", ");
            format!("[{body}]")
        }
        Json::Obj(entries) => {
            let body = entries
                .iter()
                .map(|(key, value)| format!("{}: {}", io::json_string(key), dumps(value)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{{body}}}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tool_blocks_and_compact_json() {
        let dir = crate::testsupport::tmpdir("extract");
        let path = PathBuf::from(&dir).with_extension("jsonl");
        fs::write(
            &path,
            concat!(
                "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"input\":{\"command\":\"ls -la\"}}]}}\n",
                "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\",\"content\":[{\"type\":\"text\",\"text\":\"a\"},{\"type\":\"text\",\"text\":\"b\"}]}]}}\n",
                "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"input\":{\"foo\":\"bar\",\"n\":2}}]}}\n",
                "not json\n"
            ),
        )
        .unwrap();
        let blocks = blocks(&path).unwrap();
        assert_eq!(
            blocks[0],
            ("use".to_string(), "command: ls -la".to_string())
        );
        assert_eq!(blocks[1], ("result".to_string(), "a\nb".to_string()));
        assert_eq!(
            blocks[2],
            (
                "use".to_string(),
                "{\"foo\": \"bar\", \"n\": 2}".to_string()
            )
        );
        assert_eq!(
            dumps(&scx::json::parse("{\"a\":[1,2]}").unwrap()),
            "{\"a\": [1, 2]}"
        );
        let _ = fs::remove_file(path);
    }
}
