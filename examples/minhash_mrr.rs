//! 测量工具：对 stdin 每行一个 query，输出 `search_fused` 融合召回的 top-10 排名。
//!
//! 每行一个 case 的 tail16 hash（与 Gauge 标签 `expected` 的匹配口径一致），
//! 相邻 query 间以空行分隔。配合 `tools/scx.py seed` 的 `--csv` 输出即可机械核算
//! 各层的 recall@k / MRR（MinHash 权重由 `--minhash-weight` 控制，0 = 关）。
//!
//! 用法：
//! ```text
//! cargo build --release --examples --locked
//! target/release/examples/minhash_mrr --mem-dir DIR --minhash-weight 1.0 < queries.txt
//! ```

use std::io::{BufRead, Write};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut mem_dir = String::new();
    let mut weight = 0.0f64;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--mem-dir" => {
                i += 1;
                mem_dir = args[i].clone();
            }
            "--minhash-weight" => {
                i += 1;
                weight = args[i].parse().expect("minhash weight must be a number");
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    if mem_dir.is_empty() {
        eprintln!("usage: minhash_mrr --mem-dir DIR [--minhash-weight F]");
        std::process::exit(2);
    }

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let query = line.expect("read query line");
        if query.trim().is_empty() {
            continue;
        }
        let hits = scx::memory::search_fused(&mem_dir, &query, 10, true, None, 0.0, weight)
            .unwrap_or_else(|e| {
                eprintln!("recall error for {query:?}: {e}");
                Vec::new()
            });
        for h in &hits {
            let tail = h.id.rsplit('_').next().unwrap_or(&h.id);
            writeln!(out, "{tail}").expect("write hit");
        }
        writeln!(out).expect("write separator");
    }
}
