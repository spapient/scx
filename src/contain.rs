//! 内核评测协议：evidence containment（证据保留率）判定。
//!
//! 纯函数：给定压缩输出与语料证据行，判定证据行原文是否逐字节存活于输出。
//! 语料编排与统计由 `bin/scx-eval contain`（Python）承担；本模块是判定/计分
//! 的唯一事实源，两者不得复刻彼此的语义。
//! 口径与 `eval/CONTAINMENT.md` 一致；结果属作者侧自测，非第三方独立评测。

use crate::compress;
use crate::{Config, Mode, Outcome};

/// 语料切片（只读判定输入）：`evidence` 为证据行原文（与冻结语料逐字一致）。
pub struct Slice<'a> {
    pub id: &'a str,
    pub query: &'a str,
    pub full_text: &'a str,
    pub evidence: &'a [&'a str],
}

/// 单切片判定报告。
pub struct Report {
    pub id: String,
    pub mode: Mode,
    pub kept: usize,
    pub total: usize,
    pub chars_in: usize,
    pub chars_out: usize,
    pub cut_pct: f64,
    /// 证据保留计数（等值判定：输出行集包含证据行原文）。
    pub evidence_kept: usize,
    pub evidence_total: usize,
    /// 未保留的证据行原文（空 = 全保留）。
    pub miss: Vec<String>,
}

/// 汇总统计：证据保留率、平均字符压减、逐切片未保留证据明细。
pub struct Summary {
    pub slices: usize,
    pub evidence_kept: usize,
    pub evidence_total: usize,
    pub mean_cut_pct: f64,
    pub miss: Vec<(String, String)>,
}

/// 运行单切片：`cfg` 由调用方提供（工具侧默认 ratio=0.35 / context=1）。
pub fn run(case: &Slice, cfg: &Config) -> Report {
    let o: Outcome = compress::compress(case.full_text, case.query, cfg);
    let out_lines: Vec<&str> = o.text.split('\n').collect();
    let mut kept = 0usize;
    let mut miss: Vec<String> = Vec::new();
    for ev in case.evidence {
        if out_lines.iter().any(|l| *l == *ev) {
            kept += 1;
        } else {
            miss.push(ev.to_string());
        }
    }
    Report {
        id: case.id.to_string(),
        mode: o.mode,
        kept: o.kept,
        total: o.total,
        chars_in: o.chars_in,
        chars_out: o.chars_out,
        cut_pct: o.cut_pct,
        evidence_kept: kept,
        evidence_total: case.evidence.len(),
        miss,
    }
}

/// 汇总一组报告（空输入返回零统计）。
pub fn summarize(reports: &[Report]) -> Summary {
    let mut ev_kept = 0usize;
    let mut ev_total = 0usize;
    let mut cut_sum = 0.0f64;
    let mut miss: Vec<(String, String)> = Vec::new();
    for r in reports {
        ev_kept += r.evidence_kept;
        ev_total += r.evidence_total;
        cut_sum += r.cut_pct;
        for m in &r.miss {
            miss.push((r.id.clone(), m.clone()));
        }
    }
    let mean_cut = if reports.is_empty() {
        0.0
    } else {
        cut_sum / reports.len() as f64
    };
    Summary {
        slices: reports.len(),
        evidence_kept: ev_kept,
        evidence_total: ev_total,
        mean_cut_pct: mean_cut,
        miss,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config {
            ratio: 0.35,
            context: 1,
            ..Config::default()
        }
    }

    /// 与语料同构的最小切片（synthetic，非真实事故）；填充段重复至 >40 行
    /// 以确保走 sieve 路径（与冻结语料 build-01 结构一致）。
    fn build_case() -> Slice<'static> {
        let filler: String = (0..5)
            .map(|_| {
                concat!(
                    "   Compiling scx-core v0.4.1 (/workspace/project/crates/core)\n",
                    "   Compiling scx-util v0.4.1 (/workspace/project/crates/util)\n",
                    "warning: unreachable pattern\n",
                    "  --> crates/util/src/lib.rs:80:9\n",
                    "   |\n",
                    "81 |         _ => unreachable!(\"arm removed in refactor\"),\n",
                    "   |         ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n",
                    "   = note: `#[warn(unreachable_patterns)]` on by default\n",
                    "   Compiling scx-cli v0.4.1 (/workspace/project/crates/cli)\n",
                    "    Finished `dev` profile [unoptimized + debugin] target(s) in 3m 12s\n",
                )
            })
            .collect();
        let mut full = String::new();
        full.push_str(&filler);
        full.push_str("error[E0382]: borrow of moved value: `order`\n");
        full.push_str("  --> crates/service/src/order.rs:142:9\n");
        full.push_str("142 |     let total = order.compute_total();\n");
        full.push_str("    |                 ^^^^^ value borrowed here after move\n");
        full.push_str(&filler);
        Slice {
            id: "build-01",
            query: "build error E0382 borrow moved value",
            full_text: Box::leak(full.into_boxed_str()),
            evidence: &[
                "error[E0382]: borrow of moved value: `order`",
                "  --> crates/service/src/order.rs:142:9",
                "142 |     let total = order.compute_total();",
                "    |                 ^^^^^ value borrowed here after move",
            ],
        }
    }

    #[test]
    fn containment_full_on_evidence_case() {
        let r = run(&build_case(), &cfg());
        assert_eq!(r.evidence_kept, r.evidence_total, "证据必须全保留: {:?}", r.miss);
        assert!(r.miss.is_empty());
        assert!(r.cut_pct > 0.0, "压缩必须产生真实压减");
        assert_eq!(r.mode, Mode::Sieve);
    }

    #[test]
    fn summarize_aggregates_counts() {
        let r = run(&build_case(), &cfg());
        let s = summarize(&[r]);
        assert_eq!(s.evidence_kept, 4);
        assert_eq!(s.evidence_total, 4);
        assert_eq!(s.slices, 1);
        assert!(s.miss.is_empty());
    }
}
