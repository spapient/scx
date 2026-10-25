//! 反馈双环：`--rate good|bad <case-id>` 落地 case 状态机、质量分、
//! 词级调权与 L1 自适应置信基线。
//!
//! - case 环：quality = good/(good+bad)，状态机 open→solved→regressed，
//!   quality 进入排序先验（recall::score_pass 乘 (1+quality*quality_prior)）。
//! - 词环：good 词 +Δ / bad 词 −Δ（meta.toml [word_w]，默认上下限 ±0.5）。
//! - L1 自适应：recall 侧把每次 top1.sim 记入 [l1] 历史（push_l1_hist），
//!   rate 时重算 l1_base = P50(历史) 写回 [params]，CLI 过滤用动态基线 + margin。

use crate::memory::meta::{self, Meta};
use crate::memory::store;
use crate::memory::store::{Case, Feedback};
use crate::memory::util::utc_now;

/// 反馈后重算 L1 自适应基线并写回 meta [params]（失败静默，旁路）。
fn adapt_l1_base(root: &str) {
    let Ok(meta) = meta::load_meta(root) else {
        return;
    };
    let min_history = meta.param("l1_adapt_min_history").max(1.0) as usize;
    if meta.l1_hist.len() < min_history {
        return; // 历史太少不滑动，保持默认 0.55。
    }
    let mut sorted = meta.l1_hist.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = sorted.len() / 2;
    let p50 = if sorted.len() % 2 == 0 {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    };
    let base = p50.clamp(meta.param("l1_adapt_min"), meta.param("l1_adapt_max"));
    let _ = meta::set_param(root, "l1_base", base);
}

/// 反馈标注：good=true 记 good，false 记 bad。
/// 返回更新后的 case（含 quality/status），供 CLI 输出核对。
pub(crate) fn rate(root: &str, id: &str, good: bool) -> std::io::Result<Case> {
    let _case_lock = store::lock_case(root, id)?;
    let mut c = store::read_case(root, id)?;
    let fb = next_feedback(&c, good);
    c.feedback = fb.clone();
    c.quality = if fb.good + fb.bad == 0 {
        0.0
    } else {
        fb.good as f64 / (fb.good + fb.bad) as f64
    };
    c.status = next_status(&c.status, good, c.quality);
    c.updated_at = utc_now();
    // 词环：该 case 词表 ±Δ（仅质量≥0.5 的 good / bad 参与，防误标污染）。
    let terms = merged_terms(&c);
    if good || c.status == "regressed" {
        meta::bump_word_w(root, &terms, good);
    }
    // 写回 case（原子写；write_case 内部 bump gen，qmap/缓存随之失效）。
    store::write_case(root, &c)?;
    // L1 自适应：基于 [l1] 历史重算 l1_base。
    adapt_l1_base(root);
    Ok(c)
}

fn next_feedback(c: &Case, good: bool) -> Feedback {
    if good {
        Feedback {
            good: c.feedback.good + 1,
            bad: c.feedback.bad,
        }
    } else {
        Feedback {
            good: c.feedback.good,
            bad: c.feedback.bad + 1,
        }
    }
}

/// 状态机（§4.1）：open → solved（good 且 quality≥0.5）→ regressed（bad 且曾 solved）。
fn next_status(prev: &str, good: bool, quality: f64) -> String {
    match (prev, good, quality) {
        (_, true, q) if q >= 0.5 => "solved".to_string(),
        ("solved", false, _) => "regressed".to_string(),
        (s, false, _) if s != "solved" => s.to_string(), // 直接 bad 标 open：无效经验，不改态
        (s, _, _) => s.to_string(),
    }
}

/// case 词表并入 query 切词结果（保序去重）。
fn merged_terms(c: &Case) -> Vec<String> {
    let mut terms = c.terms.clone();
    for t in crate::terms::build_mem_terms(&c.query) {
        if !terms.contains(&t) {
            terms.push(t);
        }
    }
    terms
}

/// L1 自适应置信判据（§5.1）：sim_top1 ≥ l1_base 且 (top1−top2) ≥ margin。
/// `ext_hit`：PRF 扩展词命中率，设计预留；当前 recall 未返回扩展词统计，
/// 传 1.0 视为满足。
pub(crate) fn l1_qualified(
    top1_sim: f64,
    top2_sim: f64,
    l1_base: f64,
    margin: f64,
    ext_hit: f64,
) -> bool {
    top1_sim >= l1_base && (top1_sim - top2_sim) >= margin && ext_hit >= 0.30
}

/// 从 meta 读 l1_base / l1_margin_min；缺省值统一由 `meta::DEFAULT_PARAMS` 提供。
pub(crate) fn l1_params(meta: &Meta) -> (f64, f64) {
    (meta.param("l1_base"), meta.param("l1_margin_min"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::ingest::ingest;
    use crate::memory::store::{self, CaseStats};
    use crate::memory::util::tmp_root;
    use std::fs;

    fn stats() -> CaseStats {
        crate::memory::store::sample_stats()
    }

    #[test]
    fn rate_good_moves_open_to_solved() {
        let root = tmp_root("good");
        ingest(
            &root,
            "连接超时",
            vec!["timeout".into()],
            "ERROR conn timeout alpha",
            vec!["/a.log".into()],
            &stats(),
            vec![],
        )
        .unwrap();
        let id = store::list_ids(&root).unwrap()[0].clone();
        let c = rate(&root, &id, true).unwrap();
        assert_eq!(c.status, "solved");
        assert_eq!(c.feedback.good, 1);
        assert_eq!(c.quality, 1.0);
        // 词环已写：timeout / conn / error 应有词权重。
        let m = store::load_meta(&root).unwrap();
        assert!(
            m.word_w.iter().any(|(k, v)| k == "timeout" && *v > 0.0),
            "good 词应有正权重: {:?}",
            m.word_w
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rate_bad_regresses_solved_case() {
        let root = tmp_root("bad");
        ingest(
            &root,
            "连接超时",
            vec!["timeout".into()],
            "ERROR conn timeout beta",
            vec!["/a.log".into()],
            &stats(),
            vec![],
        )
        .unwrap();
        let id = store::list_ids(&root).unwrap()[0].clone();
        rate(&root, &id, true).unwrap();
        let c = rate(&root, &id, false).unwrap();
        assert_eq!(c.status, "regressed");
        assert_eq!(c.quality, 0.5);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn l1_qualified_requires_base_and_margin() {
        assert!(l1_qualified(0.8, 0.4, 0.6, 0.15, 1.0), "达标");
        assert!(!l1_qualified(0.8, 0.7, 0.6, 0.15, 1.0), "margin 不足");
        assert!(!l1_qualified(0.5, 0.1, 0.6, 0.15, 1.0), "低于基线");
        assert!(!l1_qualified(0.8, 0.4, 0.6, 0.15, 0.2), "扩展命中不足");
    }

    #[test]
    fn l1_base_adapts_after_enough_history() {
        let root = tmp_root("adapt");
        ingest(
            &root,
            "连接超时",
            vec!["timeout".into()],
            "ERROR conn timeout gamma",
            vec!["/a.log".into()],
            &stats(),
            vec![],
        )
        .unwrap();
        // 写入 4 条历史：0.7/0.8/0.9/1.0 → P50=0.85。
        for s in [0.7, 0.8, 0.9, 1.0] {
            meta::push_l1_hist(&root, s);
        }
        let id = store::list_ids(&root).unwrap()[0].clone();
        rate(&root, &id, true).unwrap();
        let m = store::load_meta(&root).unwrap();
        let (base, _) = l1_params(&m);
        assert!(
            (base - 0.85).abs() < 0.01,
            "l1_base 应滑动到 P50≈0.85, got {base}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn l1_adapt_clamp_comes_from_config() {
        let root = tmp_root("adaptcfg");
        ingest(
            &root,
            "连接超时",
            vec!["timeout".into()],
            "ERROR conn timeout delta",
            vec!["/a.log".into()],
            &stats(),
            vec![],
        )
        .unwrap();
        meta::set_param(&root, "l1_adapt_max", 0.80).unwrap();
        for s in [0.7, 0.8, 0.9, 1.0] {
            meta::push_l1_hist(&root, s);
        }
        let id = store::list_ids(&root).unwrap()[0].clone();
        rate(&root, &id, true).unwrap();
        let m = store::load_meta(&root).unwrap();
        let (base, _) = l1_params(&m);
        assert!((base - 0.80).abs() < 1e-9, "上限应来自配置: {base}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn concurrent_rate_same_case_does_not_lose_feedback() {
        let root = tmp_root("conc");
        ingest(
            &root,
            "连接超时",
            vec!["timeout".into()],
            "ERROR conn timeout concurrent",
            vec!["/a.log".into()],
            &stats(),
            vec![],
        )
        .unwrap();
        let id = store::list_ids(&root).unwrap()[0].clone();
        let root = std::sync::Arc::new(root);
        let id = std::sync::Arc::new(id);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let root = root.clone();
            let id = id.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                for _ in 0..5 {
                    rate(&root, &id, true).unwrap();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let c = store::read_case(&root, &id).unwrap();
        assert_eq!(c.feedback.good, 40, "并发 rate 的读改写不得丢更新");
        assert_eq!(c.quality, 1.0);
        let _ = fs::remove_dir_all(root.as_str());
    }
}
