//! 记忆分层与晋升门控（P1）。
//!
//! 分层语义：L0 原始（仅摄取，证据不全）→ L1 候选（证据完整：evidence+fix
//! 非空）→ L2 可信（反馈累积证明：good 达线且 quality 达标）。
//!
//! 门控原则（零依赖、确定性、只升不降）：
//! - L0→L1 由写入侧触发：同指纹去重已有更高 tier 时不回退（不覆盖已有结论）；
//! - L1→L2 由反馈侧触发：good 累计 ≥ `tier_l2_good` 且 quality ≥ `tier_l1_q`，
//!   缺省 (3, 0.4)。quality 未达线的 case 即使 good 数达标也留在候选层；
//! - 降级不在此模块（status 的 regressed 状态与质量先验已在 feedback 处理）。
//!
//! 召回侧先验在 recall::score：L2/L1 各加 `tier_prior_l2/l1`，L0 不加分
//! （兜底展示不直答）。

use crate::memory::meta::Meta;
use crate::memory::store::{stamp, Case};

/// 证据完整性门槛：L0→L1 要求 fix 与证据同时非空（可作为候选答案展示）。
pub(crate) const MIN_EVIDENCE: usize = 1;

/// 对 case 执行一次晋升检查（只升不降）。`good` 为最新累计 good 数
/// （feedback 路径传更新后的 fb.good，ingest 路径传 case 当前值）。
/// 返回是否发生晋升；晋升时写入 `tier_at`（UTC Unix 秒）。
pub(crate) fn promote(meta: &Meta, c: &mut Case, good: usize) -> bool {
    if c.tier >= 2 {
        return false;
    }
    let target = if c.tier == 0 {
        // L0→L1：证据完整即候选；不要求反馈（新 case 尚无 good）。
        if c.evidence.len() >= MIN_EVIDENCE && !c.fix.is_empty() {
            1
        } else {
            return false;
        }
    } else {
        // L1→L2：反馈累积 + 质量门槛双达线才可信。
        let l2_good = meta.param("tier_l2_good");
        let l1_q = meta.param("tier_l1_q");
        if good as f64 >= l2_good && c.quality >= l1_q {
            2
        } else {
            return false;
        }
    };
    c.tier = target;
    c.tier_at = stamp();
    true
}

#[cfg(test)]
mod tests {
    use crate::memory::meta::{self, Meta};
    use crate::memory::store::{Case, Feedback, Fields, Stats};
    use crate::memory::tier::promote;
    use crate::memory::util::fnv1a;
    use std::path::PathBuf;

    fn root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("scx-mem-tier-{}-{}", tag, fnv1a(tag) & 0xffff))
    }
    fn meta(root: &std::path::Path) -> Meta {
        meta::load(root.to_str().unwrap()).unwrap()
    }
    fn case() -> Case {
        let mut c = Case::new(
            "t1".into(),
            "conn timeout".into(),
            vec!["conn".into(), "timeout".into()],
            "net conn timeout".into(),
        );
        c.fields = Fields::default();
        c.feedback = Feedback { good: 0, bad: 0 };
        c.stats = Stats {
            kept: 5,
            total: 100,
            cut: 95.0,
            risk: "HIGH".into(),
        };
        c
    }

    #[test]
    fn l0_stays_original_without_evidence() {
        let r = root("noev");
        meta::ensure(r.to_str().unwrap()).unwrap();
        let m = meta(&r);
        let mut c = case();
        assert!(!promote(&m, &mut c, 0));
        assert_eq!(c.tier, 0);
        assert!(c.tier_at.is_empty());
    }

    #[test]
    fn evidence_and_fix_promote_to_l1() {
        let r = root("l1");
        meta::ensure(r.to_str().unwrap()).unwrap();
        let m = meta(&r);
        let mut c = case();
        c.evidence = vec!["line 12: io error".into()];
        c.fix = "retry".into();
        assert!(promote(&m, &mut c, 0));
        assert_eq!(c.tier, 1);
        assert!(!c.tier_at.is_empty());
    }

    #[test]
    fn good_count_and_quality_gate_l2() {
        let r = root("l2");
        meta::ensure(r.to_str().unwrap()).unwrap();
        let m = meta(&r);
        // L1 候选，good 未达线 → 不升。
        let mut c = case();
        c.evidence = vec!["line 12: io error".into()];
        c.fix = "retry".into();
        assert!(promote(&m, &mut c, 0));
        assert!(!promote(&m, &mut c, 1));
        assert_eq!(c.tier, 1);
        // good 达线但 quality 不足 → 不升。
        c.feedback.good = 3;
        c.quality = 0.1;
        assert!(!promote(&m, &mut c, 3));
        assert_eq!(c.tier, 1);
        // 双达线 → L2。
        c.quality = 0.5;
        assert!(promote(&m, &mut c, 3));
        assert_eq!(c.tier, 2);
        // 只升不降。
        c.quality = 0.0;
        c.feedback.good = 0;
        assert!(!promote(&m, &mut c, 0));
        assert_eq!(c.tier, 2);
    }
}
