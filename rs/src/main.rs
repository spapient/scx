// scx — CLI：参数解析、输入读取、输出与统计格式化。筛选逻辑全在库 crate（scx::compress），
// 本文件不含引擎代码；任何程序都可直接复用库。
use std::io::Read;
use scx::{compress, Config, Outcome, VERSION};

struct Opts {
    query: String,
    ratio: f64,
    context: i64,
    window: i64,
    elide: bool,
    risk_gate: bool,
    json: bool,
    stat: bool,
    forces: Vec<String>,
    files: Vec<String>,
}

fn take(args: &mut Vec<String>, name: &str) -> Option<String> {
    let flag = format!("--{}", name);
    let i = args.iter().position(|a| a == &flag)?;
    let has_v = i + 1 < args.len();
    let v = if has_v { args[i + 1].clone() } else { String::new() };
    args.drain(i..=(if has_v { i + 1 } else { i }));
    Some(v)
}
fn take_arg(args: &mut Vec<String>, name: &str, dflt: &str) -> String {
    take(args, name).unwrap_or_else(|| dflt.to_string())
}
fn take_switch(args: &mut Vec<String>, name: &str) -> bool {
    let flag = format!("--{}", name);
    if args.iter().any(|a| a == &flag) {
        args.retain(|a| a != &flag);
        true
    } else {
        false
    }
}

fn parse_args() -> Opts {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("usage: scx --query \"...\" [--ratio 0.35] [--window 2] [--context 1] [--elide 1] [--risk-gate 1] [--force SUBSTR]... [--json] [--stat] [file...]");
        std::process::exit(0);
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("scx {}", VERSION);
        std::process::exit(0);
    }
    let query = take_arg(&mut args, "query", "");
    let ratio = take_arg(&mut args, "ratio", "0.35").parse::<f64>().unwrap_or(0.35).clamp(0.02, 1.0);
    let context = take_arg(&mut args, "context", "1").parse::<i64>().unwrap_or(1).clamp(0, 4);
    let wraw = take_arg(&mut args, "window", "2").parse::<i64>().unwrap_or(2).clamp(0, 8);
    let window = if wraw == 0 { 1 } else { wraw };
    let elide = take_arg(&mut args, "elide", "1") != "0";
    let risk_gate = take_arg(&mut args, "risk-gate", "1") != "0";
    let json = take_switch(&mut args, "json");
    let stat = take_switch(&mut args, "stat");
    let mut forces = Vec::new();
    while let Some(v) = take(&mut args, "force") {
        forces.push(v);
    }
    let files = args.into_iter().filter(|a| !a.starts_with("--")).collect();
    Opts { query, ratio, context, window, elide, risk_gate, json, stat, forces, files }
}

fn read_input(files: &[String]) -> String {
    if !files.is_empty() {
        let mut parts = Vec::new();
        for f in files {
            match std::fs::read(f).map(|b| String::from_utf8_lossy(&b).into_owned()) {
                Ok(s) => parts.push(s),
                Err(e) => {
                    eprintln!("scx: cannot read input: {}: {}", f, e);
                    std::process::exit(2);
                }
            }
        }
        parts.join("\n")
    } else {
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = std::io::stdin().read_to_end(&mut buf) {
            eprintln!("scx: cannot read input: {}", e);
            std::process::exit(2);
        }
        String::from_utf8_lossy(&buf).into_owned()
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
    let text = read_input(&opts.files);
    let cfg = Config {
        ratio: opts.ratio,
        context: opts.context,
        window: opts.window,
        elide: opts.elide,
        risk_gate: opts.risk_gate,
        forces: opts.forces,
    };
    let o = compress(&text, &opts.query, &cfg);
    if !opts.stat {
        print!("{}", o.text);
        if !o.text.ends_with('\n') {
            println!();
        }
    }
    println!("{}", stats_line(&o));
    if opts.json {
        json_line(&o);
    }
}
