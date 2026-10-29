//! 阶段计时器（feature = "perf" 时启用，默认编译零代码零开销）。
//!
//! 诊断用途：在压缩主链各阶段插桩 `mark`，结束时打印分阶段耗时（微秒）。
//! 行为中性：`perf` feature 关闭时本模块 cfg 排除，压缩输出与默认构建逐字节一致；
//! 开启时也只向 stderr 打印，不进入 Outcome/文本输出。

#![forbid(unsafe_code)]

#[cfg(feature = "perf")]
mod inner {
    use super::*;
    use std::time::Instant;

    /// 跨调用累计表（高频路径逐行分段累计，循环结束一次输出）。
    thread_local! {
        static AGG: std::cell::RefCell<Vec<(&'static str, f64)>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    #[derive(Default)]
    pub struct T {
        marks: Vec<(&'static str, f64)>,
        t0: Option<Instant>,
        last: Option<Instant>,
    }

    impl T {
        /// 创建计时器（不打印，直到 finish）。
        pub fn start() -> T {
            let now = Instant::now();
            T {
                marks: Vec::new(),
                t0: Some(now),
                last: Some(now),
            }
        }

        /// 记录一个阶段结束时刻（相对上次 mark 的耗时，微秒）。
        pub fn mark(&mut self, name: &'static str) {
            let now = Instant::now();
            let since = now.duration_since(self.last.unwrap_or(now)).as_micros() as f64;
            self.marks.push((name, since));
            self.last = Some(now);
        }

        /// 输出分阶段耗时表（stderr），随后清空。
        pub fn finish(&mut self, label: &str) {
            let now = Instant::now();
            let total = now.duration_since(self.t0.unwrap_or(now)).as_micros() as f64;
            eprintln!("[perf] {label} total={total:.0}us");
            for (name, us) in &self.marks {
                eprintln!(
                    "[perf]   {name:<24} {us:>10.0}us {:.1}%",
                    us / total * 100.0
                );
            }
            self.marks.clear();
            self.t0 = Some(now);
            self.last = Some(now);
        }

        /// 重置累计段的起点（每次循环迭代开头调用）。
        pub fn start_agg(&mut self) {
            self.last = Some(Instant::now());
        }

        /// 累计一段耗时（相对上次 `start_agg` / `mark_agg`），并入全局表。
        pub fn mark_agg(&mut self, name: &'static str) {
            let now = Instant::now();
            let since = now.duration_since(self.last.unwrap_or(now)).as_micros() as f64;
            AGG.with(|a| a.borrow_mut().push((name, since)));
            self.last = Some(now);
        }

        /// 输出累计表（循环结束后调用一次）。
        pub fn dump_agg(&self, label: &str) {
            AGG.with(|a| {
                let a = a.borrow();
                let mut total = 0.0;
                let mut map: Vec<(&'static str, f64)> = Vec::new();
                for (name, us) in a.iter() {
                    total += us;
                    match map.iter_mut().find(|(n, _)| *n == *name) {
                        Some((_, t)) => *t += us,
                        None => map.push((*name, *us)),
                    }
                }
                eprintln!("[perf] {label} agg-total={total:.0}us");
                for (name, us) in &map {
                    eprintln!(
                        "[perf]   {name:<24} {us:>10.0}us {:.1}%",
                        us / total * 100.0
                    );
                }
            });
        }
    }
}

#[cfg(not(feature = "perf"))]
mod inner {
    /// 关闭时为空实现：调用全部内联消除，零运行时开销。
    #[derive(Default)]
    pub struct T;
    impl T {
        pub fn start() -> T {
            T
        }
        pub fn mark(&mut self, _name: &'static str) {}
        pub fn finish(&mut self, _label: &str) {}
        #[allow(dead_code)]
        pub fn start_agg(&mut self) {}
        #[allow(dead_code)]
        pub fn mark_agg(&mut self, _name: &'static str) {}
        #[allow(dead_code)]
        pub fn dump_agg(&self, _label: &str) {}
    }
}

/// 计时器类型（feature 状态由 inner 决定）。
pub use inner::T;
