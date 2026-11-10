//! 统一并行度：全仓线程数的唯一口径。
//!
//! 取 `available_parallelism()` 并受固定上限约束；查询失败（如受限容器）
//! 回退单线程。并行只切分独立行区间，输出由分块保序保持逐字节确定，
//! 因此线程数变化不改变任何结果。

/// 线程上限：避免在大核数机器上过度派生与调度开销。
pub(crate) const CAP: usize = 32;

/// 当前可用并行度（1..=CAP）。
pub(crate) fn count() -> usize {
    std::thread::available_parallelism()
        .map(|v| v.get())
        .unwrap_or(1)
        .min(CAP)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_is_in_range() {
        let n = count();
        assert!((1..=CAP).contains(&n));
    }
}
