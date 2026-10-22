//! 零依赖性能基准：50k 行（约 2.5MB）全流程中位耗时。
//!
//! 运行（与 README 手动构建一致的 target-dir 策略，不污染技能目录）：
//!   Windows:  cargo run --release --example bench --locked --target-dir "%TEMP%\scx-cargo-target"
//!   macOS/Linux: cargo run --release --example bench --locked --target-dir "$TMPDIR/scx-cargo-target"
//!
//! 输出：5 次采样的中位耗时。用于防止后续版本无声性能退化（回归门阈值由人工对照
//! README 基准记录更新）。

use scx::{compress, Config};
use std::time::Instant;

fn main() {
    // 50k 行，约 2.5MB；每 100 行插入一条真实错误行，模拟构建/运行日志。
    let lines: Vec<String> = (0..50_000)
        .map(|i| {
            if i % 100 == 0 {
                format!("ERROR db timeout conn_id={} pool exhausted", i)
            } else {
                format!("INFO heartbeat seq={} ok pool=3 latency=1ms", i)
            }
        })
        .collect();
    let text = lines.join("\n");
    let cfg = Config::default();

    // 预热（分配路径已热）。
    let _ = compress(&text, "timeout conn_id pool", &cfg);

    let mut samples: Vec<u128> = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        let o = compress(&text, "timeout conn_id pool", &cfg);
        samples.push(t.elapsed().as_millis());
        assert!(o.seeds > 0, "基准语料必须命中种子");
    }
    samples.sort_unstable();
    let med = samples[samples.len() / 2];
    println!(
        "bench: 50k lines ({} chars) median {} ms | samples {:?} ms | kept={}",
        text.chars().count(),
        med,
        samples,
        compress(&text, "timeout conn_id pool", &cfg).kept,
    );
}
