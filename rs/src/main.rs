// scx — CLI：参数解析、输入读取、输出与统计格式化。筛选逻辑全在库 crate（scx::compress），
// 本文件不含引擎代码；任何程序都可直接复用库。
use scx::{compress, Config, Outcome, VERSION};
use std::io::{self, Read, Write};

const USAGE: &str = "usage: scx --query \"...\" [--ratio 0.35] [--window 2] [--context 1] [--elide 1] [--risk-gate 1] [--force SUBSTR]... [--json] [--stat|--no-stats] [file...]";
// 64 MiB：交互式上下文压缩的内存上限；更大的日志应先分段，避免一次读入和筛选向量挤爆内存。
const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024;

struct Opts {
    query: String,
    ratio: f64,
    context: i64,
    window: i64,
    elide: bool,
    risk_gate: bool,
    json: bool,
    stat: bool,
    no_stats: bool,
    forces: Vec<String>,
    files: Vec<String>,
}

fn option_value(
    args: &[String],
    i: &mut usize,
    name: &str,
    inline: Option<String>,
) -> Result<String, String> {
    if let Some(v) = inline {
        return Ok(v);
    }
    *i += 1;
    if *i >= args.len() {
        return Err(format!("{} requires a value", name));
    }
    let v = &args[*i];
    if v == "--" || v.starts_with("--") {
        return Err(format!(
            "{} requires a value (use {}=VALUE for values starting with --)",
            name, name
        ));
    }
    Ok(v.clone())
}

fn parse_bool01(value: &str, name: &str) -> Result<bool, String> {
    match value {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(format!("{} expects 0 or 1", name)),
    }
}

fn parse_args_from(args: Vec<String>) -> Result<Opts, String> {
    let mut query: Option<String> = None;
    let mut query_seen = false;
    let mut ratio = 0.35;
    let mut ratio_seen = false;
    let mut context = 1;
    let mut context_seen = false;
    let mut window = 2;
    let mut window_seen = false;
    let mut elide = true;
    let mut elide_seen = false;
    let mut risk_gate = true;
    let mut risk_gate_seen = false;
    let mut json = false;
    let mut stat = false;
    let mut no_stats = false;
    let mut forces = Vec::new();
    let mut files = Vec::new();

    let mut i = 0usize;
    while i < args.len() {
        let raw = &args[i];
        if raw == "--" {
            files.extend(args[i + 1..].iter().cloned());
            break;
        }
        let (name, inline) = match raw.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v.to_string())),
            _ => (raw.as_str(), None),
        };
        match name {
            "--query" => {
                if query_seen {
                    return Err("--query specified more than once".to_string());
                }
                query_seen = true;
                query = Some(option_value(&args, &mut i, name, inline)?);
            }
            "--ratio" => {
                if ratio_seen {
                    return Err("--ratio specified more than once".to_string());
                }
                ratio_seen = true;
                let v = option_value(&args, &mut i, name, inline)?;
                let parsed = v
                    .parse::<f64>()
                    .map_err(|_| format!("--ratio expects a number, got {:?}", v))?;
                if !parsed.is_finite() || !(0.02..=1.0).contains(&parsed) {
                    return Err("--ratio must be a finite number in 0.02..=1.0".to_string());
                }
                ratio = parsed;
            }
            "--context" => {
                if context_seen {
                    return Err("--context specified more than once".to_string());
                }
                context_seen = true;
                let v = option_value(&args, &mut i, name, inline)?;
                let parsed = v
                    .parse::<i64>()
                    .map_err(|_| format!("--context expects an integer, got {:?}", v))?;
                if !(0..=4).contains(&parsed) {
                    return Err("--context must be in 0..=4".to_string());
                }
                context = parsed;
            }
            "--window" => {
                if window_seen {
                    return Err("--window specified more than once".to_string());
                }
                window_seen = true;
                let v = option_value(&args, &mut i, name, inline)?;
                let parsed = v
                    .parse::<i64>()
                    .map_err(|_| format!("--window expects an integer, got {:?}", v))?;
                if !(0..=8).contains(&parsed) {
                    return Err("--window must be in 0..=8".to_string());
                }
                window = if parsed == 0 { 1 } else { parsed };
            }
            "--elide" => {
                if elide_seen {
                    return Err("--elide specified more than once".to_string());
                }
                elide_seen = true;
                let v = option_value(&args, &mut i, name, inline)?;
                elide = parse_bool01(&v, "--elide")?;
            }
            "--risk-gate" => {
                if risk_gate_seen {
                    return Err("--risk-gate specified more than once".to_string());
                }
                risk_gate_seen = true;
                let v = option_value(&args, &mut i, name, inline)?;
                risk_gate = parse_bool01(&v, "--risk-gate")?;
            }
            "--force" => {
                let v = option_value(&args, &mut i, name, inline)?;
                if v.split('|').all(|p| p.trim().is_empty()) {
                    return Err("--force requires a non-empty pattern".to_string());
                }
                forces.push(v);
            }
            "--json" => {
                if inline.is_some() {
                    return Err("--json does not take a value".to_string());
                }
                json = true;
            }
            "--stat" => {
                if inline.is_some() {
                    return Err("--stat does not take a value".to_string());
                }
                stat = true;
            }
            "--no-stats" => {
                if inline.is_some() {
                    return Err("--no-stats does not take a value".to_string());
                }
                no_stats = true;
            }
            _ if raw.starts_with("--") || (raw.starts_with('-') && raw.len() > 1) => {
                return Err(format!("unknown option: {}", raw));
            }
            _ => files.push(raw.clone()),
        }
        i += 1;
    }

    if stat && no_stats {
        return Err("--stat and --no-stats cannot be used together".to_string());
    }
    let query = query.unwrap_or_default();
    if query.is_empty() && forces.is_empty() {
        return Err("missing --query (or provide at least one --force pattern)".to_string());
    }
    Ok(Opts {
        query,
        ratio,
        context,
        window,
        elide,
        risk_gate,
        json,
        stat,
        no_stats,
        forces,
        files,
    })
}

fn parse_args() -> Opts {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut after_terminator = false;
    for a in &args {
        if a == "--" {
            after_terminator = true;
            continue;
        }
        if !after_terminator && (a == "--help" || a == "-h") {
            println!("{}", USAGE);
            std::process::exit(0);
        }
        if !after_terminator && (a == "--version" || a == "-V") {
            println!("scx {}", VERSION);
            std::process::exit(0);
        }
    }
    match parse_args_from(args) {
        Ok(opts) => opts,
        Err(e) => {
            eprintln!("scx: {}", e);
            eprintln!("{}", USAGE);
            std::process::exit(2);
        }
    }
}

fn decode_bytes(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    }
}

fn read_limited<R: Read>(reader: R, limit: u64) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    reader.take(limit.saturating_add(1)).read_to_end(&mut buf)?;
    Ok(buf)
}

fn exceeds_limit(len: usize, limit: u64) -> bool {
    len as u64 > limit
}

fn limit_error(limit: u64) -> String {
    format!(
        "input exceeds the {} MiB limit; split the input first",
        limit / (1024 * 1024)
    )
}

fn read_files(files: &[String], limit: u64) -> Result<String, String> {
    let mut parts = Vec::new();
    let mut total_bytes = 0usize;
    for f in files {
        let file =
            std::fs::File::open(f).map_err(|e| format!("cannot read input: {}: {}", f, e))?;
        let remaining = limit.saturating_sub(total_bytes as u64);
        let buf = read_limited(file, remaining)
            .map_err(|e| format!("cannot read input: {}: {}", f, e))?;
        total_bytes = total_bytes.saturating_add(buf.len());
        if exceeds_limit(total_bytes, limit) {
            return Err(limit_error(limit));
        }
        parts.push(decode_bytes(buf));
    }
    Ok(parts.join("\n"))
}

fn read_stdin(limit: u64) -> Result<String, String> {
    let stdin = io::stdin();
    let buf = read_limited(stdin.lock(), limit).map_err(|e| format!("cannot read input: {}", e))?;
    if exceeds_limit(buf.len(), limit) {
        return Err(limit_error(limit));
    }
    Ok(decode_bytes(buf))
}

fn read_input(files: &[String]) -> Result<String, String> {
    if files.is_empty() {
        read_stdin(MAX_INPUT_BYTES)
    } else {
        read_files(files, MAX_INPUT_BYTES)
    }
}

fn stats_line(r: &Outcome) -> String {
    format!(
        "compressed: kept={}/{} lines chars={}->{} ({:.1}% cut) dup={} seeds={} evidence={} risk={} guarded={} regions={} window={} terms={} forced={} mode={}",
        r.kept, r.total, r.chars_in, r.chars_out, r.cut_pct, r.dup, r.seeds, r.evidence, r.risk, r.guarded, r.regions, r.window, r.terms, r.forced, r.mode
    )
}

fn json_line(r: &Outcome) {
    eprintln!(
        "scxjson: {{\"kept\":{},\"total\":{},\"chars_in\":{},\"chars_out\":{},\"cut_pct\":{:.1},\"dup\":{},\"seeds\":{},\"evidence\":{},\"risk\":\"{}\",\"guarded\":{},\"regions\":{},\"window\":{},\"terms\":{},\"forced\":{},\"mode\":\"{}\"}}",
        r.kept, r.total, r.chars_in, r.chars_out, r.cut_pct, r.dup, r.seeds, r.evidence, r.risk, r.guarded, r.regions, r.window, r.terms, r.forced, r.mode
    );
}

fn main() {
    let opts = parse_args();
    let text = match read_input(&opts.files) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("scx: {}", e);
            std::process::exit(2);
        }
    };
    let cfg = Config {
        ratio: opts.ratio,
        context: opts.context,
        window: opts.window,
        elide: opts.elide,
        risk_gate: opts.risk_gate,
        forces: opts.forces,
    };
    let o = compress(&text, &opts.query, &cfg);
    let stdout = io::stdout();
    let mut out = stdout.lock();
    if !opts.stat {
        if let Err(e) = out.write_all(o.text.as_bytes()) {
            report_write_error(e);
            return;
        }
        if !o.text.is_empty() && !o.text.ends_with('\n') {
            if let Err(e) = out.write_all(b"\n") {
                report_write_error(e);
                return;
            }
        }
    }
    if !opts.no_stats {
        if let Err(e) = writeln!(out, "{}", stats_line(&o)) {
            report_write_error(e);
            return;
        }
    }
    if opts.json {
        json_line(&o);
    }
}

fn report_write_error(e: io::Error) {
    if e.kind() != io::ErrorKind::BrokenPipe {
        eprintln!("scx: cannot write output: {}", e);
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn rejects_unknown_options_and_bad_values() {
        assert!(parse_args_from(args(&["--bogus", "x", "--query", "a"])).is_err());
        assert!(parse_args_from(args(&["--query", "a", "--ratio", "abc"])).is_err());
        assert!(parse_args_from(args(&["--query", "a", "--ratio", "-1"])).is_err());
        assert!(parse_args_from(args(&["--query", "a", "--context", "5"])).is_err());
        assert!(parse_args_from(args(&["--query", "a", "--window", "9"])).is_err());
        assert!(parse_args_from(args(&["--query", "a", "--elide", "2"])).is_err());
    }

    #[test]
    fn rejects_missing_values_and_duplicate_scalars() {
        assert!(parse_args_from(args(&["--query"])).is_err());
        assert!(parse_args_from(args(&["--force", "--json"])).is_err());
        assert!(parse_args_from(args(&["--query", "a", "--query", "b"])).is_err());
        assert!(parse_args_from(args(&["--query", "a", "--stat", "--no-stats"])).is_err());
    }

    #[test]
    fn supports_equals_and_terminator() {
        let o = parse_args_from(args(&[
            "--query=alpha",
            "--ratio=0.5",
            "--context=0",
            "--",
            "--weird-file",
        ]))
        .unwrap();
        assert_eq!(o.query, "alpha");
        assert_eq!(o.ratio, 0.5);
        assert_eq!(o.context, 0);
        assert_eq!(o.files, vec!["--weird-file".to_string()]);
    }

    #[test]
    fn query_can_be_empty_only_with_force() {
        assert!(parse_args_from(args(&[])).is_err());
        assert!(parse_args_from(args(&["--force="])).is_err());
        assert!(parse_args_from(args(&["--force", "|"])).is_err());
        let o = parse_args_from(args(&["--force", "SPECIAL"])).unwrap();
        assert_eq!(o.query, "");
        assert_eq!(o.forces, vec!["SPECIAL".to_string()]);
    }

    #[test]
    fn windows_and_booleans_are_normalized() {
        let o = parse_args_from(args(&[
            "--query",
            "a",
            "--window",
            "0",
            "--elide",
            "0",
            "--risk-gate",
            "0",
        ]))
        .unwrap();
        assert_eq!(o.window, 1);
        assert!(!o.elide);
        assert!(!o.risk_gate);
        let o2 = parse_args_from(args(&["--force=--json"])).unwrap();
        assert_eq!(o2.forces, vec!["--json".to_string()]);
    }

    #[test]
    fn capped_reader_detects_one_byte_over_limit() {
        let exact = read_limited(std::io::Cursor::new(b"12345678"), 8).unwrap();
        assert_eq!(exact, b"12345678");
        assert!(!exceeds_limit(exact.len(), 8));

        let over = read_limited(std::io::Cursor::new(b"123456789"), 8).unwrap();
        assert_eq!(over, b"123456789");
        assert!(exceeds_limit(over.len(), 8));
    }

    #[test]
    fn file_limit_is_aggregate_across_inputs() {
        let dir = std::env::temp_dir();
        let pid = std::process::id();
        let a = dir.join(format!("scx-cap-a-{}.txt", pid));
        let b = dir.join(format!("scx-cap-b-{}.txt", pid));
        std::fs::write(&a, b"1234").unwrap();
        std::fs::write(&b, b"5678").unwrap();
        let files = vec![
            a.to_string_lossy().into_owned(),
            b.to_string_lossy().into_owned(),
        ];

        assert_eq!(read_files(&files[..1], 4).unwrap(), "1234");
        assert!(read_files(&files, 7).is_err());
        assert_eq!(read_files(&files, 8).unwrap(), "1234\n5678");

        let _ = std::fs::remove_file(a);
        let _ = std::fs::remove_file(b);
    }
}
