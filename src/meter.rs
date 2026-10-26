//! 计量与审计层（P3 治理层）：Token 分桶统计、压缩效果指标、预算管控、快照追溯。
//!
//! 数据流：分类化载荷 `[(category, text)]` →（复用 [`crate::compress_bucketed`] 逐桶压缩）
//! → 分桶 token → 聚合全局/占比/效果指标/预算 → [`crate::meter::Snapshot`]（可序列化 JSON，入
//! [`crate::meter::Ledger`] 追溯）。本层是薄壳，不复制裁剪逻辑；仅复用
//! [`crate::terms::estimate`]（主链路计量口径）、`compress_bucketed`、
//! [`crate::ContentType`]（核心桶标签与价值梯度默认预算）与 [`crate::json`]。
//!
//! # 指标口径（全部确定性、可复现，除零保护固定）
//! - 压缩比 `ratio = tokens_out / tokens_in`（in=0 → 1.0）
//! - 节省率 `saved = (tokens_in - tokens_out) / tokens_in`（in=0 → 0）
//! - 高价值留存率 `retention = Σ高价值桶 out / Σ高价值桶 in`（in=0 → 1.0）
//! - 密度提升 `density = (高价值out/总out) / (高价值in/总in)`（任一分母 0 → 1.0）
//! - 桶占比 `share = 桶 tokens / 全局 tokens × 100`
//! - 预算使用率 `used_pct = used / cap × 100`；`level` 按 `used/cap` 越过 warn/over 阈值。
//!
//! 高价值桶 = [`crate::ContentType::StaticContract`] + [`crate::ContentType::SystemDefinition`]
//! （价值梯度最顶）。快照不含 wall-clock：`id`/`round` 由调用方传入，保证同输入
//! 同配置输出逐字节一致。

#![forbid(unsafe_code)]

use crate::json::Json;
use crate::{BucketStat, Config, ContentType};

/// 对任意文本片段执行 token 计数（`terms::estimate` 向上取整；空串为 0）。
/// 全链路唯一计量口径，与压缩主链路的 `tokens_in/out` 同一把尺。
pub fn count(s: &str) -> usize {
    crate::terms::count(s)
}

/// 高价值桶判定：价值梯度最顶（静态契约 + 系统定义），参与留存率/密度。
fn high_value(kind: ContentType) -> bool {
    matches!(
        kind,
        ContentType::StaticContract | ContentType::SystemDefinition
    )
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
    /// 按桶配额（kind → 上限 token 数）。
    pub quotas: Vec<(ContentType, usize)>,
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

/// 计量快照：内容寻址 ID + 轮次 + 分桶明细 + 效果指标 + 预算。
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// 内容派生 ID = sha256(payload_sha256 + config_fingerprint) 前 16 位十六进制。
    pub id: String,
    pub round: u64,
    /// 输入载荷（parts 文本）的 SHA-256（hex）。
    pub payload_sha256: String,
    /// 配置指纹 = SHA-256(VERSION + query + 压缩参数)（hex）。
    pub config_fingerprint: String,
    pub buckets: Vec<BucketStat>,
    pub tokens_in: f64,
    pub tokens_out: f64,
    /// 按桶配额（kind → 上限）；渲染时由 `tokens_out` 派生使用率/剩余。
    pub quotas: Vec<(ContentType, usize)>,
    pub metrics: Metrics,
    pub budget: Budget,
}

impl Snapshot {
    /// 按输入占比降序返回分桶统计（需求：分类项按 Token 占比排序）。
    pub fn by_share(&self) -> Vec<&BucketStat> {
        let mut v: Vec<&BucketStat> = self.buckets.iter().collect();
        v.sort_by(|a, b| {
            b.tokens_in
                .partial_cmp(&a.tokens_in)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        v
    }

    /// 单查指定分类的用量（需求：单独查询指定分类）。
    pub fn find(&self, kind: ContentType) -> Option<&BucketStat> {
        self.buckets.iter().find(|b| b.kind == kind)
    }

    /// 单查指定分类的配额（`None` = 未分配）。
    fn quota_of(&self, kind: ContentType) -> Option<usize> {
        self.quotas
            .iter()
            .find(|(k, _)| *k == kind)
            .map(|(_, q)| *q)
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

/// 输入载荷哈希：parts 的 (category + text) 拼接后 SHA-256（hex）。
fn payload_hash(parts: &[(ContentType, &str)]) -> String {
    let mut buf = String::new();
    for (kind, text) in parts {
        buf.push_str(kind.as_str());
        buf.push('\n');
        buf.push_str(text);
        buf.push('\n');
    }
    crate::hash::hex(&crate::hash::sha256(buf.as_bytes()))
}

/// 配置指纹：SHA-256(VERSION + query + 压缩参数)（hex）。确定性，捕获决定
/// 压缩结果的全部配置维度（不含运行时嵌入 sims）。
fn fingerprint(query: &str, cfg: &Config) -> String {
    let mut buf = String::new();
    buf.push_str(crate::VERSION);
    buf.push('\n');
    buf.push_str(query);
    buf.push('\n');
    buf.push_str(&format!("ratio={:.17}\n", cfg.ratio));
    buf.push_str(&format!("context={}\n", cfg.context));
    buf.push_str(&format!("window={}\n", cfg.window));
    buf.push_str(&format!("elide={}\n", cfg.elide));
    buf.push_str(&format!("gate={}\n", cfg.gate));
    buf.push_str(&format!("lineno={}\n", cfg.lineno));
    buf.push_str(&format!("structure={}\n", cfg.structure));
    buf.push_str(&format!("sim_weight={:.17}\n", cfg.sim_weight));
    buf.push_str(&format!("limit={:?}\n", cfg.limit));
    for f in &cfg.forces {
        buf.push_str("force=");
        buf.push_str(f);
        buf.push('\n');
    }
    for k in &cfg.structure_anchors {
        buf.push_str("anchor=");
        buf.push_str(k.as_str());
        buf.push('\n');
    }
    crate::hash::hex(&crate::hash::sha256(buf.as_bytes()))
}

/// 最简入口：分类化载荷分桶压缩并计量。
///
/// `parts` = `[(ContentType, text)]`。`budget` 为 `None` 时预算字段为空态。
/// `id` 内容派生（确定性），`round` 由调用方传入。
pub fn measure(
    round: u64,
    parts: &[(ContentType, &str)],
    query: &str,
    cfg: &Config,
    budget: Option<&BudgetSpec>,
) -> Snapshot {
    // 分桶压缩只此一份实现（`compress_bucketed`）；本函数仅从其 `Outcome` 组装快照，
    // 不复制逐桶裁剪逻辑。`outcome.tokens_in/out` 已按各桶求和，与逐桶明细一致。
    let outcome = crate::compress_bucketed(parts, query, cfg);
    let buckets = outcome.buckets;
    let tin = outcome.tokens_in;
    let tout = outcome.tokens;
    let quotas = budget.map(|spec| spec.quotas.clone()).unwrap_or_default();
    let hi_in = buckets
        .iter()
        .filter(|b| high_value(b.kind))
        .map(|b| b.tokens_in)
        .sum::<f64>();
    let hi_out = buckets
        .iter()
        .filter(|b| high_value(b.kind))
        .map(|b| b.tokens_out)
        .sum::<f64>();
    let payload_sha256 = payload_hash(parts);
    let config_fingerprint = fingerprint(query, cfg);
    let id = crate::hash::hex(&crate::hash::sha256(
        format!("{payload_sha256}{config_fingerprint}").as_bytes(),
    ))[..16]
        .to_string();
    Snapshot {
        id,
        round,
        payload_sha256,
        config_fingerprint,
        buckets,
        tokens_in: tin,
        tokens_out: tout,
        quotas,
        metrics: compute_metrics(tin, tout, hi_in, hi_out),
        budget: compute_budget(budget, tout),
    }
}

/// 纯计量（不压缩）：把 `text` 计为一个 kind 桶（token in == out），构造前置快照。
/// `config_fingerprint` 仅含 VERSION（无 query/压缩参数，因为未压缩）。
pub fn snapshot(round: u64, kind: ContentType, text: &str) -> Snapshot {
    let tokens = count(text) as f64;
    let lines = text.lines().count();
    let payload_sha256 = crate::hash::hex(&crate::hash::sha256(text.as_bytes()));
    let config_fingerprint = crate::hash::hex(&crate::hash::sha256(crate::VERSION.as_bytes()));
    let id = crate::hash::hex(&crate::hash::sha256(
        format!("{payload_sha256}{config_fingerprint}").as_bytes(),
    ))[..16]
        .to_string();
    Snapshot {
        id,
        round,
        payload_sha256,
        config_fingerprint,
        buckets: vec![BucketStat {
            kind,
            tokens_in: tokens,
            tokens_out: tokens,
            kept: lines,
            total: lines,
            mode: crate::Mode::Sieve,
        }],
        tokens_in: tokens,
        tokens_out: tokens,
        quotas: Vec::new(),
        metrics: Metrics {
            ratio: 1.0,
            saved: 0.0,
            retention: 1.0,
            density: 1.0,
        },
        budget: Budget {
            cap: None,
            used: tokens,
            used_pct: 0.0,
            left: f64::INFINITY,
            left_pct: 0.0,
            level: Level::Normal,
        },
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
    let (warn, over) = spec.map(|s| (s.warn, s.over)).unwrap_or((0.7, 1.0));
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
        "{{\"id\":{},\"payload_sha256\":{},\"config_fingerprint\":{},\"round\":{},\"tokens_in\":{:.2},\"tokens_out\":{:.2},",
        crate::json::string(&s.id),
        crate::json::string(&s.payload_sha256),
        crate::json::string(&s.config_fingerprint),
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
        out.push_str(&render_bucket(
            b,
            s.tokens_in,
            s.tokens_out,
            s.quota_of(b.kind),
        ));
    }
    out.push_str("]}");
    out
}

/// 账本聚合：按 kind（字典序）汇总 token 与调用次数，输出确定性 JSON。
/// `est_cost_saved_usd` 默认 null（价格表未配置时绝不编造成本）。
pub fn summarize(ledger: &Ledger) -> String {
    let by_kind = aggregate(ledger);
    let tin: f64 = ledger.snaps.iter().map(|s| s.tokens_in).sum();
    let tout: f64 = ledger.snaps.iter().map(|s| s.tokens_out).sum();
    let mut out = String::from("{\"by_kind\":[");
    for (i, (kind, (ki, ko, calls))) in by_kind.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&render_kind(kind, *ki, *ko, *calls));
    }
    out.push_str("],\"total\":{");
    out.push_str(&render_total(tin, tout, ledger.snaps.len()));
    out.push('}');
    out
}

/// 按 kind 累加 token 与调用次数（kind 字典序）。
fn aggregate(ledger: &Ledger) -> std::collections::BTreeMap<String, (f64, f64, usize)> {
    let mut by_kind = std::collections::BTreeMap::new();
    for s in &ledger.snaps {
        for b in &s.buckets {
            let e = by_kind
                .entry(b.kind.as_str().to_string())
                .or_insert((0.0, 0.0, 0));
            e.0 += b.tokens_in;
            e.1 += b.tokens_out;
            e.2 += 1;
        }
    }
    by_kind
}

/// 单 kind 聚合项 JSON。
fn render_kind(kind: &str, ki: f64, ko: f64, calls: usize) -> String {
    let saved = (ki - ko).max(0.0);
    let ratio = if ki > 0.0 { ko / ki } else { 1.0 };
    format!(
        "{{\"kind\":{},\"tokens_in\":{:.2},\"tokens_out\":{:.2},\"tokens_saved\":{:.2},\"ratio\":{:.4},\"calls\":{}}}",
        crate::json::string(kind),
        ki,
        ko,
        saved,
        ratio,
        calls
    )
}

/// 全局汇总项 JSON。
fn render_total(tin: f64, tout: f64, calls: usize) -> String {
    let saved = (tin - tout).max(0.0);
    let ratio = if tin > 0.0 { tout / tin } else { 1.0 };
    format!(
        "\"tokens_in\":{:.2},\"tokens_out\":{:.2},\"tokens_saved\":{:.2},\"ratio\":{:.4},\"calls\":{},\"est_cost_saved_usd\":null",
        tin,
        tout,
        saved,
        ratio,
        calls
    )
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

/// 单桶 JSON：占比/削减用派生方法算，配额使用率/剩余由配额与 `tokens_out` 派生。
fn render_bucket(b: &BucketStat, tin: f64, tout: f64, quota: Option<usize>) -> String {
    let (quota_used_pct, quota_left) = match quota {
        Some(q) if q > 0 => (b.tokens_out * 100.0 / q as f64, q as f64 - b.tokens_out),
        _ => (0.0, f64::INFINITY),
    };
    format!(
        "{{\"kind\":\"{}\",\"tokens_in\":{:.2},\"tokens_out\":{:.2},\"share_in_pct\":{:.1},\"share_out_pct\":{:.1},\"cut_pct\":{:.1},\"kept\":{},\"total\":{},\"mode\":\"{}\",\"quota\":{},\"quota_used_pct\":{:.1},\"quota_left\":{}}}",
        b.kind.as_str(),
        b.tokens_in,
        b.tokens_out,
        share(b.tokens_in, tin),
        share(b.tokens_out, tout),
        cut_pct(b.tokens_in, b.tokens_out),
        b.kept,
        b.total,
        b.mode,
        opt_num(quota.map(|q| q as f64)),
        quota_used_pct,
        opt_f2(finite(quota_left))
    )
}

fn cut_pct(tin: f64, tout: f64) -> f64 {
    if tin > 0.0 {
        (tin - tout) * 100.0 / tin
    } else {
        0.0
    }
}

fn share(part: f64, whole: f64) -> f64 {
    if whole > 0.0 {
        part * 100.0 / whole
    } else {
        0.0
    }
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
    let (id, round, payload_sha256, config_fingerprint) = head_of(v)?;
    Some(Snapshot {
        id,
        round,
        payload_sha256,
        config_fingerprint,
        buckets: from_buckets(v.get("buckets")?),
        tokens_in: num_of(v, "tokens_in")?,
        tokens_out: num_of(v, "tokens_out")?,
        quotas: from_quotas(v.get("quotas")),
        metrics: from_metrics(v.get("metrics")?)?,
        budget: from_budget(v.get("budget")?)?,
    })
}

/// 解析快照头部标量字段（id/round/载荷哈希/配置指纹）。
fn head_of(v: &Json) -> Option<(String, u64, String, String)> {
    Some((
        str_of(v, "id")?.to_string(),
        num_of(v, "round")? as u64,
        str_of(v, "payload_sha256")?.to_string(),
        str_of(v, "config_fingerprint")?.to_string(),
    ))
}

fn from_buckets(b: &Json) -> Vec<BucketStat> {
    match b {
        Json::Arr(arr) => arr.iter().filter_map(from_bucket).collect(),
        _ => Vec::new(),
    }
}

/// 配额字段在 render 中未单独序列化（折叠在 bucket 里），反序列化时回退为空。
fn from_quotas(_v: Option<&Json>) -> Vec<(ContentType, usize)> {
    Vec::new()
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
    let kind = str_of(b, "kind")?.parse().ok()?;
    let mode = str_of(b, "mode")
        .and_then(mode_of)
        .unwrap_or(crate::Mode::Sieve);
    Some(BucketStat {
        kind,
        tokens_in: num_of(b, "tokens_in")?,
        tokens_out: num_of(b, "tokens_out")?,
        kept: num_of(b, "kept")? as usize,
        total: num_of(b, "total")? as usize,
        mode,
    })
}

/// mode 字符串 → 枚举（反序列化用；未知回退 Sieve）。
fn mode_of(s: &str) -> Option<crate::Mode> {
    use crate::Mode::*;
    Some(match s {
        "passthrough:small" => PassSmall,
        "passthrough:longline" => PassLongline,
        "passthrough:too-many-lines" => PassTooManyLines,
        "failopen:zero-signal" => FailOpenZeroSignal,
        "passthrough:no-kept" => PassNoKept,
        "sieve" => Sieve,
        "overflow:evidence" => OverflowEvidence,
        "overflow:fill" => OverflowFill,
        _ => return None,
    })
}

/// `scx ledger` 入口：读账本 JSONL（文件或 stdin），输出聚合 JSON。
/// 用法：`scx ledger --summarize [--json] [ledger.jsonl]`。
pub fn run_ledger(args: &[String]) -> i32 {
    let mut do_summarize = false;
    let mut path: Option<&str> = None;
    for a in args {
        match a.as_str() {
            "--summarize" => do_summarize = true,
            "--json" => {}
            other if !other.starts_with("--") => path = Some(other),
            other => {
                eprintln!("scx ledger: unknown argument {other}");
                return 2;
            }
        }
    }
    if !do_summarize {
        eprintln!("scx ledger: usage: scx ledger --summarize [--json] [ledger.jsonl]");
        return 2;
    }
    let ledger = match path {
        Some(p) => match Ledger::load_jsonl(p) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("scx ledger: {e}");
                return 1;
            }
        },
        None => {
            let mut s = String::new();
            if std::io::Read::read_to_string(&mut std::io::stdin(), &mut s).is_err() {
                eprintln!("scx ledger: read stdin failed");
                return 2;
            }
            Ledger::from_jsonl(&s)
        }
    };
    println!("{}", summarize(&ledger));
    0
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

    fn parts(t: &str) -> Vec<(ContentType, &str)> {
        vec![
            (ContentType::StaticContract, t),
            (ContentType::SystemDefinition, t),
            (ContentType::SessionMessage, t),
            (ContentType::ToolOutput, t),
        ]
    }

    #[test]
    fn count_matches_estimate_ceil_and_empty_is_zero() {
        assert_eq!(count(""), 0);
        assert_eq!(
            count("hello"),
            crate::terms::estimate("hello").ceil() as usize
        );
        assert_eq!(
            count("数据库连接超时"),
            crate::terms::estimate("数据库连接超时").ceil() as usize
        );
    }

    #[test]
    fn measure_aggregates_shares_sum_to_100() {
        let t = text();
        let p = parts(&t);
        let s = measure(1, &p, "db timeout", &Config::default(), None);
        let in_sum: f64 = s
            .buckets
            .iter()
            .map(|b| share(b.tokens_in, s.tokens_in))
            .sum();
        let out_sum: f64 = s
            .buckets
            .iter()
            .map(|b| share(b.tokens_out, s.tokens_out))
            .sum();
        assert!((in_sum - 100.0).abs() < 0.5, "input shares sum: {in_sum}");
        assert!(
            (out_sum - 100.0).abs() < 0.5,
            "output shares sum: {out_sum}"
        );
        assert_eq!(s.buckets.len(), p.len());
    }

    #[test]
    fn metrics_match_hand_computation() {
        let t = text();
        let s = measure(1, &parts(&t), "db timeout", &Config::default(), None);
        let hi_in: f64 = s
            .buckets
            .iter()
            .filter(|b| high_value(b.kind))
            .map(|b| b.tokens_in)
            .sum();
        let hi_out: f64 = s
            .buckets
            .iter()
            .filter(|b| high_value(b.kind))
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
        let base = measure(1, &p, "db timeout", &Config::default(), None);
        let used = base.tokens_out;
        let cap = (used * 1.5) as usize;
        let spec = BudgetSpec {
            cap: Some(cap),
            warn: 0.7,
            over: 1.0,
            quotas: Vec::new(),
        };
        let s = measure(1, &p, "db timeout", &Config::default(), Some(&spec));
        assert_eq!(s.budget.level, Level::Normal);
        let spec2 = BudgetSpec {
            cap: Some((used * 1.17) as usize),
            warn: 0.7,
            over: 1.0,
            quotas: Vec::new(),
        };
        let s2 = measure(1, &p, "db timeout", &Config::default(), Some(&spec2));
        assert_eq!(s2.budget.level, Level::Warn);
        let spec3 = BudgetSpec {
            cap: Some((used * 0.5) as usize),
            warn: 0.7,
            over: 1.0,
            quotas: Vec::new(),
        };
        let s3 = measure(1, &p, "db timeout", &Config::default(), Some(&spec3));
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
            quotas: vec![(ContentType::ToolOutput, 100)],
        };
        let s = measure(1, &p, "db timeout", &Config::default(), Some(&spec));
        let b = s.find(ContentType::ToolOutput).unwrap();
        assert_eq!(s.quota_of(ContentType::ToolOutput), Some(100));
        assert!(b.tokens_out < 100.0, "配额内输出");
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
        let a = render(&measure(
            1,
            &p,
            "db timeout",
            &Config::default(),
            Some(&spec),
        ));
        let b = render(&measure(
            1,
            &p,
            "db timeout",
            &Config::default(),
            Some(&spec),
        ));
        assert_eq!(a, b, "同输入同配置渲染必须逐字节一致");
        assert!(a.starts_with("{\"id\":\""), "id 为内容派生");
        assert!(a.contains("\"payload_sha256\":"));
        assert!(a.contains("\"config_fingerprint\":"));
        assert!(a.contains("\"round\":1"));
        assert!(a.contains("\"metrics\":{\"ratio\":"));
        assert!(a.contains("\"budget\":{\"cap\":5000"));
        assert!(a.contains("\"buckets\":[{\"kind\":\"static-contract\""));
        assert!(a.ends_with("]}"));
    }

    #[test]
    fn ledger_lookup_and_diff() {
        let t = text();
        let p = parts(&t);
        let a = measure(1, &p, "db timeout", &Config::default(), None);
        let b = measure(2, &p, "db timeout pool", &Config::default(), None);
        let a_id = a.id.clone();
        let b_id = b.id.clone();
        let mut ledger = Ledger::default();
        ledger.push(a);
        ledger.push(b);
        assert!(ledger.by_id(&a_id).is_some());
        assert!(ledger.by_round(2).is_some());
        assert!(ledger.by_id("zz").is_none());
        let d = ledger.diff(&a_id, &b_id).unwrap();
        assert_eq!(
            d.delta_in,
            ledger.by_id(&b_id).unwrap().tokens_in - ledger.by_id(&a_id).unwrap().tokens_in
        );
    }

    #[test]
    fn ledger_jsonl_roundtrip() {
        let t = text();
        let p = parts(&t);
        let mut ledger = Ledger::default();
        ledger.push(measure(1, &p, "db timeout", &Config::default(), None));
        ledger.push(measure(2, &p, "db timeout pool", &Config::default(), None));
        let jsonl = ledger.to_jsonl();
        let back = Ledger::from_jsonl(&jsonl);
        assert_eq!(back.snaps.len(), 2);
        assert_eq!(back.snaps[0].id, ledger.snaps[0].id);
        assert_eq!(back.snaps[0].round, 1);
        assert_eq!(back.snaps[1].round, 2);
        assert_eq!(back.snaps[0].tokens_in, ledger.snaps[0].tokens_in);
    }

    #[test]
    fn fingerprint_deterministic_and_content_addressed() {
        let t = text();
        let p = parts(&t);
        let a = measure(1, &p, "db timeout", &Config::default(), None);
        let b = measure(2, &p, "db timeout", &Config::default(), None);
        assert_eq!(a.id, b.id);
        assert_eq!(a.payload_sha256, b.payload_sha256);
        assert_eq!(a.config_fingerprint, b.config_fingerprint);
        assert_eq!(a.id.len(), 16, "id 为 16 位十六进制");
        let c = measure(1, &p, "db timeout pool", &Config::default(), None);
        assert_ne!(a.config_fingerprint, c.config_fingerprint);
        let d = measure(
            1,
            &[(ContentType::ToolOutput, "hello")],
            "db timeout",
            &Config::default(),
            None,
        );
        assert_ne!(a.payload_sha256, d.payload_sha256);
    }

    #[test]
    fn summarize_aggregates_by_kind() {
        let t = text();
        let p = parts(&t);
        let mut ledger = Ledger::default();
        ledger.push(measure(1, &p, "db timeout", &Config::default(), None));
        let s = summarize(&ledger);
        assert!(s.starts_with("{\"by_kind\":["));
        assert!(s.contains("\"kind\":\"static-contract\""));
        assert!(s.contains("\"calls\":"));
        assert!(s.contains("\"total\":{\"tokens_in\":"));
        assert!(s.contains("\"est_cost_saved_usd\":null"));
        assert!(s.ends_with("}"));
    }
}
