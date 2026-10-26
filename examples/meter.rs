//! 计量与审计模块最小调用示例。
//!
//! ```text
//! cargo run --release --locked --example meter
//! ```
//!
//! 演示：四核心桶 + 一个自定义桶的分桶压缩 → 占比/效果指标/预算 → 快照 JSON；
//! 两个轮次入账本，输出相邻快照对比。

use scx::bucket::ContentType;
use scx::meter::{self, BudgetSpec, Ledger};
use scx::Config;

fn text() -> String {
    let mut lines: Vec<String> = (0..200)
        .map(|i| format!("filler alpha beta line {i}"))
        .collect();
    lines[100] = "ERROR db timeout conn_id=9f2a".to_string();
    lines[150] = "Root cause: connection pool exhausted".to_string();
    lines.join("\n")
}

fn main() {
    let t = text();
    let parts: Vec<(&str, &str)> = vec![
        (ContentType::StaticContract.as_str(), &t),
        (ContentType::SystemDefinition.as_str(), &t),
        (ContentType::SessionMessage.as_str(), &t),
        (ContentType::ToolOutput.as_str(), &t),
        (
            "skill-config",
            "skill: db pool tuning\nretry: 3\nbackoff: 1s",
        ),
    ];
    let cfg = Config::default();
    let budget = BudgetSpec {
        cap: Some(5000),
        warn: 0.7,
        over: 1.0,
        quotas: vec![(ContentType::ToolOutput.as_str().to_string(), 400)],
    };

    let mut ledger = Ledger::default();
    ledger.push(meter::measure(
        "round-1",
        1,
        &parts,
        "db timeout",
        &cfg,
        Some(&budget),
    ));
    ledger.push(meter::measure(
        "round-2",
        2,
        &parts,
        "db timeout pool",
        &cfg,
        Some(&budget),
    ));

    for s in &ledger.snaps {
        println!("{}", meter::render(s));
        println!("\n按占比排序:");
        for b in s.by_share() {
            println!(
                "  {:<20} in={:.0} out={:.0} share_in={:.1}% share_out={:.1}% cut={:.1}%",
                b.cat, b.tokens_in, b.tokens_out, b.share_in_pct, b.share_out_pct, b.cut_pct
            );
        }
        println!();
    }

    if let Some(d) = ledger.diff("round-1", "round-2") {
        println!(
            "相邻快照对比: delta_in={:+.0} delta_out={:+.0} delta_saved={:+.4} delta_ratio={:+.4}",
            d.delta_in, d.delta_out, d.delta_saved, d.delta_ratio
        );
    }

    println!(
        "\n单文本计数示例: count(\"数据库连接超时\") = {}",
        meter::count("数据库连接超时")
    );
}
