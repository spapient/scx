//! 计量与审计层（P3 治理层）：Token 分桶统计、压缩效果指标、预算管控、快照追溯。
//!
//! 数据流：分类化载荷 `[(category, text)]` →（复用 [`crate::compress`] 内核逐桶压缩）
//! → 分桶 token → 聚合全局/占比/效果指标/预算 → [`Snapshot`]（可序列化 JSON，入
//! [`Ledger`] 追溯）。本层是薄壳，不复制裁剪逻辑；仅复用 [`crate::terms::estimate`]
//! （全链路唯一计量口径）、`compress`、[`crate::bucket::ContentType`]（核心桶标签
//! 与价值梯度默认预算）与 [`crate::json`]。
//!
//! # 指标口径（全部确定性、可复现，除零保护固定）
//! - 压缩比 `ratio = tokens_out / tokens_in`（in=0 → 1.0）
//! - 节省率 `saved = (tokens_in - tokens_out) / tokens_in`（in=0 → 0）
//! - 高价值留存率 `retention = Σ高价值桶 out / Σ高价值桶 in`（in=0 → 1.0）
//! - 密度提升 `density = (高价值out/总out) / (高价值in/总in)`（任一分母 0 → 1.0）
//! - 桶占比 `share = 桶 tokens / 全局 tokens × 100`
//! - 预算使用率 `used_pct = used / cap × 100`；`level` 按 `used/cap` 越过 warn/over 阈值。
//!
//! 高价值桶 = [`ContentType::StaticContract`] + [`ContentType::SystemDefinition`]
//! （价值梯度最顶）。快照不含 wall-clock：`id`/`round` 由调用方传入，保证同输入
//! 同配置输出逐字节一致。

#![forbid(unsafe_code)]

use crate::bucket::ContentType;
use crate::json::Json;
use crate::{compress, Config};

/// 计量维度标签：核心桶取 [`ContentType::as_str`]，自定义桶用任意字符串。
pub type Category = String;

/// 对任意文本片段执行 token 计数（`terms::estimate` 向上取整；空串为 0）。
/// 全链路唯一计量口径，与压缩主链路的 `tokens_in/out` 同一把尺。
pub fn count(s: &str) -> usize {
    crate::terms::count(s)
}

/// 高价值桶判定：价值梯度最顶（静态契约 + 系统定义），参与留存率/密度。
fn high_value(cat: &str) -> bool {
    cat == ContentType::StaticContract.as_str() || cat == ContentType::SystemDefinition.as_str()
}

/// 核心桶的默认预算比例；自定义桶回退到调用方 `cfg.ratio`。
fn core_ratio(cat: &str) -> Option<f64> {
    cat.parse::<ContentType>()
        .ok()
        .map(|ct| ct.default_ratio())
}

/// 削减百分比（token 口径）：`(in - out) / in × 100`，in=0 → 0。
fn cut_pct(tin: f64, tout: f64) -> f64 {
    if tin > 0.0 {
        (tin - tout) * 100.0 / tin
    } else {
        0.0
    }
}

/// 单桶计量（绝对值 + 占比 + 可选配额）。
#[derive(Debug, Clone)]
pub struct BucketStat {
    pub cat: Category,
    pub tokens_in: f64,
    pub tokens_out: f64,
    /// 占全局输入 token 的百分比。
    pub share_in_pct: f64,
    /// 占全局输出 token 的百分比。
    pub share_out_pct: f64,
    pub cut_pct: f64,
    /// 该桶独立配额（`None` = 未分配）。
    pub quota: Option<usize>,
    /// 配额使用率（%）：`tokens_out / quota × 100`；无配额为 0。
    pub quota_used_pct: f64,
    /// 配额剩余（`quota - tokens_out`，可为负 = 超支；无配额为 +∞）。
    pub quota_left: f64,
}

/// 压缩效果指标（口径见模块 doc）。
#[derive(Debug, Clone, Copy)]
pub struct Metrics {
    /// 压缩比 = tokens_out / tokens_in。
    pub ratio: f64,
    /// 节省率 = (tokens_in - tokens_out) / tokens_in。
    pub saved: f64,
    /// 高价值留存率 = 核心桶 out / 核心桶 in。
    pub retention: f64,
    /// 信息密度提升 = 压缩后高价值占比 / 原始高价值占比。
    pub density: f64,
}

/// 预算状态等级（多级使用率阈值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Normal,
    Warn,
    Over,
}

impl Level {
    pub const fn as_str(self) -> &'static str {
        match self {
            Level::Normal => "normal",
            Level::Warn => "warn",
            Level::Over => "over",
        }
    }
}

/// 全局预算 + 多级阈值 + 按桶配额（meter 层独立配置，不污染引擎 `Config`）。
#[derive(Debug, Clone)]
pub struct BudgetSpec {
    /// 全局 token 预算上限（`None` = 不限）。
    pub cap: Option<usize>,
    /// 预警阈值（使用率比例，默认 0.7）。
    pub warn: f64,
    /// 超限阈值（使用率比例，默认 1.0）。
    pub over: f64,
    /// 按桶配额（category → 上限 token 数）。
    pub quotas: Vec<(Category, usize)>,
}

impl Default for BudgetSpec {
    fn default() -> Self {
        BudgetSpec {
            cap: None,
            warn: 0.7,
            over: 1.0,
            quotas: Vec::new(),
        }
    }
}

/// 预算使用情况（绝对值 + 占比 + 状态）。
#[derive(Debug, Clone)]
pub struct Budget {
    pub cap: Option<usize>,
    /// 当前用量 = 压缩后 token 总量。
    pub used: f64,
    pub used_pct: f64,
    /// 剩余额度（`cap - used`，可为负 = 超支；无上限为 +∞）。
    pub left: f64,
    pub left_pct: f64,
    pub level: Level,
}

/// 计量快照：唯一 ID + 轮次 + 分桶明细 + 效果指标 + 预算。
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub id: String,
    pub round: u64,
    pub buckets: Vec<BucketStat>,
    pub tokens_in: f64,
    pub tokens_out: f64,
    pub metrics: Metrics,
    pub budget: Budget,
}

impl Snapshot {
    /// 按输入占比降序返回分桶统计（需求：分类项按 Token 占比排序）。
    pub fn by_share(&self) -> Vec<&BucketStat> {
        let mut v: Vec<&BucketStat> = self.buckets.iter().collect();
        v.sort_by(|a, b| {
            b.share_in_pct
                .partial_cmp(&a.share_in_pct)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        v
    }

    /// 单查指定分类的用量（需求：单独查询指定分类）。
    pub fn find(&self, cat: &str) -> Option<&BucketStat> {
        self.buckets.iter().find(|b| b.cat == cat)
    }
}

/// 相邻/任意快照对比：token 与效果指标的变化量。
#[derive(Debug, Clone, Copy)]
pub struct Diff {
    pub delta_in: f64,
    pub delta_out: f64,
    pub delta_saved: f64,
    pub delta_ratio: f64,
}

/// 快照账本：内存批量查询 + JSONL 持久化 + 对比（确定性，无 wall-clock）。
#[derive(Debug, Clone, Default)]
pub struct Ledger {
    pub snaps: Vec<Snapshot>,
}

impl Ledger {
    pub fn push(&mut self, s: Snapshot) {
        self.snaps.push(s);
    }

    /// 按 ID 精确追溯。
    pub fn by_id(&self, id: &str) -> Option<&Snapshot> {
        self.snaps.iter().find(|s| s.id == id)
    }

    /// 按轮次精确追溯。
    pub fn by_round(&self, round: u64) -> Option<&Snapshot> {
        self.snaps.iter().find(|s| s.round == round)
    }

    /// 对比两个快照（按 ID），返回 token 与指标变化量。
    pub fn diff(&self, a: &str, b: &str) -> Option<Diff> {
        let x = self.by_id(a)?;
        let y = self.by_id(b)?;
        Some(Diff {
            delta_in: y.tokens_in - x.tokens_in,
            delta_out: y.tokens_out - x.tokens_out,
            delta_saved: y.metrics.saved - x.metrics.saved,
            delta_ratio: y.metrics.ratio - x.metrics.ratio,
        })
    }

    /// 序列化为 JSONL（每行一个快照；纯函数，无 I/O）。
    pub fn to_jsonl(&self) -> String {
        let mut out = String::new();
        for s in &self.snaps {
            out.push_str(&render(s));
            out.push('\n');
        }
        out
    }

    /// 从 JSONL 反序列化（跳过无法解析的行；纯函数，无 I/O）。
    pub fn from_jsonl(s: &str) -> Ledger {
        let snaps = s
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| crate::json::parse(l).ok())
            .filter_map(|v| from_json(&v))
            .collect();
        Ledger { snaps }
    }

    /// 导出账本为 JSONL 文件（审计日志导出）。
    pub fn save_jsonl(&self, path: &str) -> std::io::Result<()> {
        std::fs::write(path, self.to_jsonl())
    }

    /// 从 JSONL 文件恢复账本。
    pub fn load_jsonl(path: &str) -> std::io::Result<Ledger> {
        std::fs::read_to_string(path).map(|s| Ledger::from_jsonl(&s))
    }
}

/// 最简入口：分类化载荷分桶压缩并计量。
///
/// `parts` = `[(category, text)]`；核心桶 category 取 [`ContentType::as_str`]，
/// 自定义桶用任意字符串。`budget` 为 `None` 时预算字段为空态（cap=None、level=Normal）。
pub fn measure(
    id: &str,
    round: u64,
    parts: &[(&str, &str)],
    query: &str,
    cfg: &Config,
    budget: Option<&BudgetSpec>,
) -> Snapshot {
    let mut buckets: Vec<BucketStat> = parts
        .iter()
        .map(|(cat, text)| compress_bucket(cat, text, query, cfg))
        .collect();
    let tin = buckets.iter().map(|b| b.tokens_in).sum::<f64>();
    let tout = buckets.iter().map(|b| b.tokens_out).sum::<f64>();
    fill_shares(&mut buckets, tin, tout);
    if let Some(spec) = budget {
        apply_quotas(&mut buckets, spec);
    }
    let hi_in = buckets
        .iter()
        .filter(|b| high_value(&b.cat))
        .map(|b| b.tokens_in)
        .sum::<f64>();
    let hi_out = buckets
        .iter()
        .filter(|b| high_value(&b.cat))
        .map(|b| b.tokens_out)
        .sum::<f64>();
    Snapshot {
        id: id.to_string(),
        round,
        buckets,
        tokens_in: tin,
        tokens_out: tout,
        metrics: compute_metrics(tin, tout, hi_in, hi_out),
        budget: compute_budget(budget, tout),
    }
}

/// 压缩单个分类并返回其计量（占比/配额由后续阶段填充）。
fn compress_bucket(cat: &str, text: &str, query: &str, cfg: &Config) -> BucketStat {
    let mut c = cfg.clone();
    if let Some(r) = core_ratio(cat) {
        c.ratio = r;
    }
    let o = compress(text, query, &c);
    // passthrough/fail-open 时 `o.tokens` 不保证，重新估算输出（与 bucket 同口径）。
    let out = crate::terms::estimate(&o.text);
    BucketStat {
        cat: cat.to_string(),
        tokens_in: o.tokens_in,
        tokens_out: out,
        share_in_pct: 0.0,
        share_out_pct: 0.0,
        cut_pct: cut_pct(o.tokens_in, out),
        quota: None,
        quota_used_pct: 0.0,
        quota_left: f64::INFINITY,
    }
}

fn fill_shares(buckets: &mut [BucketStat], tin: f64, tout: f64) {
    for b in buckets.iter_mut() {
        b.share_in_pct = share(b.tokens_in, tin);
        b.share_out_pct = share(b.tokens_out, tout);
    }
}

fn apply_quotas(buckets: &mut [BucketStat], spec: &BudgetSpec) {
    for (cat, quota) in &spec.quotas {
        if let Some(b) = buckets.iter_mut().find(|b| &b.cat == cat) {
            b.quota = Some(*quota);
            b.quota_used_pct = if *quota > 0 {
                b.tokens_out * 100.0 / *quota as f64
            } else {
                0.0
            };
            b.quota_left = *quota as f64 - b.tokens_out;
        }
    }
}

/// 占比：`part / whole × 100`，whole=0 → 0。
fn share(part: f64, whole: f64) -> f64 {
    if whole > 0.0 {
        part * 100.0 / whole
    } else {
        0.0
    }
}

fn compute_metrics(tin: f64, tout: f64, hi_in: f64, hi_out: f64) -> Metrics {
    let hi_share_in = div_or(hi_in, tin, 0.0);
    let hi_share_out = div_or(hi_out, tout, 0.0);
    Metrics {
        ratio: div_or(tout, tin, 1.0),
        saved: div_or(tin - tout, tin, 0.0),
        retention: div_or(hi_out, hi_in, 1.0),
        density: div_or(hi_share_out, hi_share_in, 1.0),
    }
}

/// 除零保护除法：分母 ≤0 时返回 `fallback`。
fn div_or(a: f64, b: f64, fallback: f64) -> f64 {
    if b > 0.0 {
        a / b
    } else {
        fallback
    }
}

fn compute_budget(spec: Option<&BudgetSpec>, used: f64) -> Budget {
    let cap = spec.and_then(|s| s.cap).map(|c| c as f64);
    let Some(cap) = cap else {
        return Budget {
            cap: None,
            used,
            used_pct: 0.0,
            left: f64::INFINITY,
            left_pct: 0.0,
            level: Level::Normal,
        };
    };
    let (warn, over) = spec
        .map(|s| (s.warn, s.over))
        .unwrap_or((0.7, 1.0));
    let used_pct = if cap > 0.0 { used * 100.0 / cap } else { 0.0 };
    let left = cap - used;
    let left_pct = if cap > 0.0 { left * 100.0 / cap } else { 0.0 };
    let level = if cap > 0.0 {
        let ratio = used / cap;
        if ratio >= over {
            Level::Over
        } else if ratio >= warn {
            Level::Warn
        } else {
            Level::Normal
        }
    } else {
        Level::Normal
    };
    Budget {
        cap: Some(cap as usize),
        used,
        used_pct,
        left,
        left_pct,
        level,
    }
}

/// 标准化 JSON 输出（P3 可视化/链路监控/审计导出消费）。
pub fn render(s: &Snapshot) -> String {
    let mut out = format!(
        "{{\"id\":{},\"round\":{},\"tokens_in\":{:.2},\"tokens_out\":{:.2},",
        crate::json::string(&s.id),
        s.round,
        s.tokens_in,
        s.tokens_out
    );
    out.push_str(&render_metrics(&s.metrics));
    out.push(',');
    out.push_str(&render_budget(&s.budget));
    out.push_str(",\"buckets\":[");
    for (i, b) in s.buckets.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&render_bucket(b));
    }
    out.push_str("]}");
    out
}

fn render_metrics(m: &Metrics) -> String {
    format!(
        "\"metrics\":{{\"ratio\":{:.4},\"saved\":{:.4},\"retention\":{:.4},\"density\":{:.4}}}",
        m.ratio, m.saved, m.retention, m.density
    )
}

fn render_budget(b: &Budget) -> String {
    format!(
        "\"budget\":{{\"cap\":{},\"used\":{:.2},\"used_pct\":{:.1},\"left\":{},\"left_pct\":{:.1},\"level\":\"{}\"}}",
        opt_num(b.cap.map(|c| c as f64)),
        b.used,
        b.used_pct,
        opt_f2(finite(b.left)),
        b.left_pct,
        b.level.as_str()
    )
}

fn render_bucket(b: &BucketStat) -> String {
    format!(
        "{{\"cat\":{},\"tokens_in\":{:.2},\"tokens_out\":{:.2},\"share_in_pct\":{:.1},\"share_out_pct\":{:.1},\"cut_pct\":{:.1},\"quota\":{},\"quota_used_pct\":{:.1},\"quota_left\":{}}}",
        crate::json::string(&b.cat),
        b.tokens_in,
        b.tokens_out,
        b.share_in_pct,
        b.share_out_pct,
        b.cut_pct,
        opt_num(b.quota.map(|q| q as f64)),
        b.quota_used_pct,
        opt_f2(finite(b.quota_left))
    )
}

/// `+∞` 序列化为 `null`（JSON 无 Infinity 字面量）。
fn finite(x: f64) -> Option<f64> {
    if x.is_finite() {
        Some(x)
    } else {
        None
    }
}

/// 整数数值或 `null`（cap/quota）。
fn opt_num(x: Option<f64>) -> String {
    x.map_or("null".to_string(), |n| format!("{n:.0}"))
}

/// f64 数值或 `null`（token/剩余额度；两位小数，保证 round-trip 无损）。
fn opt_f2(x: Option<f64>) -> String {
    x.map_or("null".to_string(), |n| format!("{n:.2}"))
}

/// 从 `render` 输出的 JSON 反序列化（供 `Ledger::from_jsonl` 使用；`None` = 字段缺失）。
fn from_json(v: &Json) -> Option<Snapshot> {
    Some(Snapshot {
        id: str_of(v, "id")?.to_string(),
        round: num_of(v, "round")? as u64,
        buckets: from_buckets(v.get("buckets")?),
        tokens_in: num_of(v, "tokens_in")?,
        tokens_out: num_of(v, "tokens_out")?,
        metrics: from_metrics(v.get("metrics")?)?,
        budget: from_budget(v.get("budget")?)?,
    })
}

fn from_buckets(b: &Json) -> Vec<BucketStat> {
    match b {
        Json::Arr(arr) => arr.iter().filter_map(from_bucket).collect(),
        _ => Vec::new(),
    }
}

/// 取字段并转数值；`None` = 字段缺失或非数值。
fn num_of(v: &Json, k: &str) -> Option<f64> {
    v.get(k).and_then(Json::num)
}

/// 取字段并转字符串引用；`None` = 字段缺失或非字符串。
fn str_of<'a>(v: &'a Json, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Json::str)
}

fn from_metrics(m: &Json) -> Option<Metrics> {
    Some(Metrics {
        ratio: m.get("ratio")?.num()?,
        saved: m.get("saved")?.num()?,
        retention: m.get("retention")?.num()?,
        density: m.get("density")?.num()?,
    })
}

fn from_budget(b: &Json) -> Option<Budget> {
    Some(Budget {
        cap: b.get("cap").and_then(Json::num).map(|n| n as usize),
        used: b.get("used")?.num()?,
        used_pct: b.get("used_pct")?.num()?,
        left: b.get("left").and_then(Json::num).unwrap_or(f64::INFINITY),
        left_pct: b.get("left_pct")?.num()?,
        level: match b.get("level").and_then(Json::str).unwrap_or("normal") {
            "over" => Level::Over,
            "warn" => Level::Warn,
            _ => Level::Normal,
        },
    })
}

fn from_bucket(b: &Json) -> Option<BucketStat> {
    Some(BucketStat {
        cat: str_of(b, "cat")?.to_string(),
        tokens_in: num_of(b, "tokens_in")?,
        tokens_out: num_of(b, "tokens_out")?,
        share_in_pct: num_of(b, "share_in_pct")?,
        share_out_pct: num_of(b, "share_out_pct")?,
        cut_pct: num_of(b, "cut_pct")?,
        quota: num_of(b, "quota").map(|n| n as usize),
        quota_used_pct: num_of(b, "quota_used_pct")?,
        quota_left: num_of(b, "quota_left").unwrap_or(f64::INFINITY),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text() -> String {
        let mut lines: Vec<String> = (0..200)
            .map(|i| format!("filler alpha beta line {i}"))
            .collect();
        lines[100] = "ERROR db timeout conn_id=9f2a".to_string();
        lines.join("\n")
    }

    fn parts(t: &str) -> Vec<(&str, &str)> {
        vec![
            (ContentType::StaticContract.as_str(), t),
            (ContentType::SystemDefinition.as_str(), t),
            (ContentType::SessionMessage.as_str(), t),
            (ContentType::ToolOutput.as_str(), t),
            ("skill-config", "skill: db pool tuning"),
        ]
    }

    #[test]
    fn count_matches_estimate_ceil_and_empty_is_zero() {
        assert_eq!(count(""), 0);
        assert_eq!(count("hello"), crate::terms::estimate("hello").ceil() as usize);
        assert_eq!(count("数据库连接超时"), crate::terms::estimate("数据库连接超时").ceil() as usize);
    }

    #[test]
    fn measure_aggregates_shares_sum_to_100() {
        let t = text();
        let p = parts(&t);
        let s = measure("s1", 1, &p, "db timeout", &Config::default(), None);
        let in_sum: f64 = s.buckets.iter().map(|b| b.share_in_pct).sum();
        let out_sum: f64 = s.buckets.iter().map(|b| b.share_out_pct).sum();
        assert!((in_sum - 100.0).abs() < 0.5, "input shares sum: {in_sum}");
        assert!((out_sum - 100.0).abs() < 0.5, "output shares sum: {out_sum}");
        assert_eq!(s.buckets.len(), p.len());
    }

    #[test]
    fn metrics_match_hand_computation() {
        let t = text();
        let s = measure("s1", 1, &parts(&t), "db timeout", &Config::default(), None);
        let hi_in: f64 = s
            .buckets
            .iter()
            .filter(|b| high_value(&b.cat))
            .map(|b| b.tokens_in)
            .sum();
        let hi_out: f64 = s
            .buckets
            .iter()
            .filter(|b| high_value(&b.cat))
            .map(|b| b.tokens_out)
            .sum();
        let tin = s.tokens_in;
        let tout = s.tokens_out;
        assert!((s.metrics.ratio - tout / tin).abs() < 1e-9);
        assert!((s.metrics.saved - (tin - tout) / tin).abs() < 1e-9);
        assert!((s.metrics.retention - hi_out / hi_in).abs() < 1e-9);
        let expect_density = (hi_out / tout) / (hi_in / tin);
        assert!((s.metrics.density - expect_density).abs() < 1e-9);
    }

    #[test]
    fn budget_levels_normal_warn_over() {
        let t = text();
        let p = parts(&t);
        let base = measure("s", 1, &p, "db timeout", &Config::default(), None);
        let used = base.tokens_out;
        let cap = (used * 1.5) as usize; // 使用率 ~66% → normal
        let spec = BudgetSpec {
            cap: Some(cap),
            warn: 0.7,
            over: 1.0,
            quotas: Vec::new(),
        };
        let s = measure("s", 1, &p, "db timeout", &Config::default(), Some(&spec));
        assert_eq!(s.budget.level, Level::Normal);
        // 使用率 ~85% → warn
        let spec2 = BudgetSpec {
            cap: Some((used * 1.17) as usize),
            warn: 0.7,
            over: 1.0,
            quotas: Vec::new(),
        };
        let s2 = measure("s", 1, &p, "db timeout", &Config::default(), Some(&spec2));
        assert_eq!(s2.budget.level, Level::Warn);
        // 使用率 > 100% → over
        let spec3 = BudgetSpec {
            cap: Some((used * 0.5) as usize),
            warn: 0.7,
            over: 1.0,
            quotas: Vec::new(),
        };
        let s3 = measure("s", 1, &p, "db timeout", &Config::default(), Some(&spec3));
        assert_eq!(s3.budget.level, Level::Over);
        assert!(s3.budget.left < 0.0, "超限时剩余为负");
    }

    #[test]
    fn quota_usage_and_left() {
        let t = text();
        let p = parts(&t);
        let spec = BudgetSpec {
            cap: None,
            warn: 0.7,
            over: 1.0,
            quotas: vec![(ContentType::ToolOutput.as_str().to_string(), 100)],
        };
        let s = measure("s", 1, &p, "db timeout", &Config::default(), Some(&spec));
        let b = s.find(ContentType::ToolOutput.as_str()).unwrap();
        assert_eq!(b.quota, Some(100));
        assert!((b.quota_used_pct - b.tokens_out * 100.0 / 100.0).abs() < 1e-9);
        assert!((b.quota_left - (100.0 - b.tokens_out)).abs() < 1e-9);
    }

    #[test]
    fn render_is_deterministic_and_valid_json_shape() {
        let t = text();
        let p = parts(&t);
        let spec = BudgetSpec {
            cap: Some(5000),
            warn: 0.7,
            over: 1.0,
            quotas: Vec::new(),
        };
        let a = render(&measure("s1", 7, &p, "db timeout", &Config::default(), Some(&spec)));
        let b = render(&measure("s1", 7, &p, "db timeout", &Config::default(), Some(&spec)));
        assert_eq!(a, b, "同输入同配置渲染必须逐字节一致");
        assert!(a.starts_with("{\"id\":\"s1\",\"round\":7"));
        assert!(a.contains("\"metrics\":{\"ratio\":"));
        assert!(a.contains("\"budget\":{\"cap\":5000"));
        assert!(a.contains("\"buckets\":[{\"cat\":\"static-contract\""));
        assert!(a.ends_with("]}"));
    }

    #[test]
    fn ledger_lookup_and_diff() {
        let t = text();
        let p = parts(&t);
        let mut ledger = Ledger::default();
        ledger.push(measure("a", 1, &p, "db timeout", &Config::default(), None));
        ledger.push(measure("b", 2, &p, "db timeout pool", &Config::default(), None));
        assert!(ledger.by_id("a").is_some());
        assert!(ledger.by_round(2).is_some());
        assert!(ledger.by_id("zz").is_none());
        let d = ledger.diff("a", "b").unwrap();
        assert_eq!(d.delta_in, ledger.by_id("b").unwrap().tokens_in - ledger.by_id("a").unwrap().tokens_in);
    }

    #[test]
    fn ledger_jsonl_roundtrip() {
        let t = text();
        let p = parts(&t);
        let mut ledger = Ledger::default();
        ledger.push(measure("a", 1, &p, "db timeout", &Config::default(), None));
        ledger.push(measure("b", 2, &p, "db timeout", &Config::default(), None));
        let jsonl = ledger.to_jsonl();
        let back = Ledger::from_jsonl(&jsonl);
        assert_eq!(back.snaps.len(), 2);
        assert_eq!(back.by_id("a").map(|s| s.round), Some(1));
        assert_eq!(back.by_round(2).map(|s| s.id.as_str()), Some("b"));
        assert_eq!(back.snaps[0].tokens_in, ledger.snaps[0].tokens_in);
    }
}
