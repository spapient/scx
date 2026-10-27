#![forbid(unsafe_code)]
#![warn(clippy::cognitive_complexity)]
// 测试代码是数据驱动的断言清单，按行拆分反而降低可读性；认知复杂度门禁只约束生产代码。
#![cfg_attr(test, allow(clippy::cognitive_complexity))]

//! scx 引擎：本地查询感知证据筛选压缩（纯函数、无 I/O、零依赖、确定性）。
//!
//! 语义（v0.4.1）：种子不变量（证据行与 ±context（默认 1，可设 0）最小邻域无条件保留，`ratio` 只删可选）；证据仅
//! 来自 IDF 加权查询命中与 `forces`；种子区域门控（可选预算只填种子邻域区域：±window
//! 扩展，间隔 <= window*3 合并；种子为零回退全图）；邻域权重只排序；context=0 完全跳过可选填充（针尖模式）；无信号 fail-open；
//! `gate` 在风险 HIGH 时保护失败级错误行(warn/警告 级除外)与事故标记行(root cause / fix hint / mitigation / recommendation / action / assertionerror)。只删不改、保留行逐字原文；省略以单行 "..." 标记；`lineno` 可为保留行附加原文行号前缀。
//!
//! 模块划分：`terms` 负责词面与估算，`analysis` 负责归一化与结构分类，
//! `compress` 是算法主体。三者均为纯函数层；本文件只保留公共类型、模块声明和
//! 跨模块行为测试。
//!
//! 压缩模式与风险等级以 `Mode` / `Risk` 枚举表达（`Display` 输出保持 v0.4.1
//! 字符串格式：`passthrough:small` / `failopen:zero-signal` / `LOW` / `HIGH` 等）。

use std::fmt;
mod analysis;
mod compress;
pub mod contain;
mod policy;
mod terms;
/// TOML 子集解析（`--policy-file` 与记忆库 `meta.toml` 共用；零第三方 crate）。
pub mod toml;
mod trim;

/// CLI 与集成层模块（库内可用）：`args` 声明式参数解析、`input` 输入读取、
/// `ops` 操作编排（Op 枚举 + run + 纯渲染）。`ops::render` 供 MCP 等
/// 集成层复用，不依赖 CLI 渲染。
pub mod args;
pub mod input;
pub mod ops;

/// 记忆知识库（旁路，默认不启用）：I/O 侧模块，与纯函数 `compress` 完全隔离。
pub mod memory;

/// SHA-256 与十六进制编码（常驻，供 semantic 权重校验与 meter 快照指纹共用）。
pub mod hash;
pub mod json;
pub mod mcp;
pub mod setup;
/// `scx wrap`：包装命令自动压缩其大输出（壳层，复用 compress 内核）。
pub mod wrap;

#[cfg(feature = "semantic")]
pub mod semantic;

/// 类型路由 + 计量桶 + 前后快照（P1 治理层，复用 `compress` 内核）。
pub mod bucket;
/// PostToolUse 自动拦截（P0）：压缩 Bash 大输出替换 tool_response。
pub mod hook;
/// Token 计量与压缩效果审计（P3 治理层：分桶统计/效果指标/预算管控/快照追溯）。
pub mod meter;
pub mod redact;
pub mod regex;
/// 可插拔相似度后端（`--embed-cmd`；默认无后端，见 `sim::compute`）。
pub mod sim;

pub use compress::{compress, compress_bucketed};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 显式证据声明：压缩后校验其是否被保留，只计量留存率、不改变裁剪行为。
/// `--evidence N`（正整数 = 原始行号）与 `--evidence <substr>`（其他 = 输出必须
/// 出现的子串）统一落入本枚举；MCP `evidence` 数组元素同理。
#[derive(Clone, Debug, PartialEq)]
pub enum Evidence {
    /// 必须保留的原始行号（1-based，LF 归一化后的输入行）。
    Line(usize),
    /// 输出中必须逐字出现的子串。
    Substr(String),
}

/// 结构锚点种类（P6 骨架保护）：压缩时把对应结构边界行标记为保留锚点。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorKind {
    /// JSON 花括号/方括号行。
    Json,
    /// XML 起止标签行。
    Xml,
    /// SAP IDoc 段标签行（复用 XML 标签启发式）。
    Idoc,
    /// YAML 缩进层级键行（映射键/列表项）。
    Yaml,
}

impl AnchorKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            AnchorKind::Json => "json",
            AnchorKind::Xml => "xml",
            AnchorKind::Idoc => "idoc",
            AnchorKind::Yaml => "yaml",
        }
    }

    /// 名称 → 枚举（`--structure-anchors` 与 policy 表共用）。
    pub fn parse(s: &str) -> Option<AnchorKind> {
        match s {
            "json" => Some(AnchorKind::Json),
            "xml" => Some(AnchorKind::Xml),
            "idoc" => Some(AnchorKind::Idoc),
            "yaml" => Some(AnchorKind::Yaml),
            _ => None,
        }
    }
}

/// 内容类型，按价值梯度排列：静态契约 > 系统定义 > 会话消息 > 临时工具输出。
/// 用于把上下文载荷路由到不同裁剪力度的桶；显式标注优先，缺失时隐式推断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    StaticContract,
    SystemDefinition,
    SessionMessage,
    ToolOutput,
}

impl ContentType {
    pub const fn as_str(self) -> &'static str {
        match self {
            ContentType::StaticContract => "static-contract",
            ContentType::SystemDefinition => "system-definition",
            ContentType::SessionMessage => "session-message",
            ContentType::ToolOutput => "tool-output",
        }
    }

    /// 价值梯度默认预算（ratio，越大保留越多可选内容）：静态契约最保守、工具输出最激进。
    pub const fn default_ratio(self) -> f64 {
        match self {
            ContentType::StaticContract => 1.0,
            ContentType::SystemDefinition => 0.8,
            ContentType::SessionMessage => 0.5,
            ContentType::ToolOutput => 0.2,
        }
    }

    /// 显式标注优先，缺失时隐式推断（`infer`）。
    pub fn resolve(tag: Option<&str>, text: &str) -> ContentType {
        match tag.and_then(|t| t.parse().ok()) {
            Some(kind) => kind,
            None => ContentType::infer(text),
        }
    }

    /// 隐式推断：抽样前若干行做特征判定，按序命中即停；不确定 fail-open 到 ToolOutput。
    pub fn infer(text: &str) -> ContentType {
        let sample = sample(text);
        if contract_like(&sample) {
            return ContentType::StaticContract;
        }
        if system_like(&sample) {
            return ContentType::SystemDefinition;
        }
        if session_like(&sample) {
            return ContentType::SessionMessage;
        }
        ContentType::ToolOutput
    }
}

impl std::str::FromStr for ContentType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "static-contract" => Ok(ContentType::StaticContract),
            "system-definition" => Ok(ContentType::SystemDefinition),
            "session-message" => Ok(ContentType::SessionMessage),
            "tool-output" => Ok(ContentType::ToolOutput),
            _ => Err(format!("unknown content type: {s}")),
        }
    }
}

/// 推断抽样：前 20 行 / 4096 字节，避免整文扫描。
fn sample(text: &str) -> String {
    let mut out = String::new();
    for (n, line) in text.lines().enumerate() {
        if n >= 20 || out.len() > 4096 {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// 契约特征：JSON schema / 工具定义。
/// 追加 JSON Schema 组合与 `$` 关键字（`allOf`/`oneOf`/`anyOf`/`$ref`/`$defs`/
/// `$schema`/`type:array`）：这些关键字几乎不会出现在普通工具输出里，却覆盖了
/// 大量「无 `type:object`/`properties` 声明」的引用式/组合式 schema，避免契约被
/// 误判为工具输出而过度裁剪（fail-open 反方向）。
fn contract_like(s: &str) -> bool {
    s.contains("\"type\": \"object\"")
        || s.contains("\"type\":\"object\"")
        || s.contains("\"type\": \"array\"")
        || s.contains("\"type\":\"array\"")
        || s.contains("\"properties\"")
        || s.contains("\"required\"")
        || s.contains("\"parameters\"")
        || s.contains("\"$schema\"")
        || s.contains("\"$ref\"")
        || s.contains("\"definitions\"")
        || s.contains("\"$defs\"")
        || s.contains("\"allOf\"")
        || s.contains("\"oneOf\"")
        || s.contains("\"anyOf\"")
        || s.contains("<tools>")
        || s.contains("<function")
}

/// 系统定义特征：系统注入元数据 / 提示。
fn system_like(s: &str) -> bool {
    s.contains("<system-reminder>")
        || s.contains("<system")
        || s.contains("[sys]")
        || s.contains("system prompt")
        || s.contains("system_prompt")
}

/// 会话特征：user/assistant 前缀。
fn session_like(s: &str) -> bool {
    s.contains("<|im_start|>")
        || s.lines().any(|l| {
            let t = l.trim_start();
            t.starts_with("user:")
                || t.starts_with("assistant:")
                || t.starts_with("system:")
                || t.starts_with("Human:")
                || t.starts_with("Assistant:")
        })
}

/// 引擎配置。`ratio` 为可选内容预算占输入 token 估算的比例；入口会做边界规范化。
#[derive(Clone)]
pub struct Config {
    pub ratio: f64,
    pub context: i64,
    pub window: i64,
    pub elide: bool,
    pub gate: bool,
    /// 区域门控风险守卫（`--scoped`）：HIGH 风险时只保护证据邻域（±window
    /// 区域）内的失败级错误行与事故标记行，而非全量错误行。默认 false = 现状行为。
    pub scoped: bool,
    /// 精度优先风险守卫（`--strict`）：HIGH 风险时用结构化错误判定
    /// （行首错误标签/强结构标记）替代宽召回 `flags().1`，只保护「真正的错误行」
    /// 而非「提到 error/失败 的行」。默认 false = 现状宽召回行为。
    pub strict: bool,
    /// 保留行输出时附加原文行号前缀（grep -n 风格 `N: `）；passthrough/fail-open 返回原文时不附加。
    pub lineno: bool,
    /// `--force` 值按 `|` 拆分后的原始子串（大小写不敏感字面匹配）。
    pub forces: Vec<String>,
    /// 绝对 token 硬上限（估算口径，非真实 tokenizer；`None` = 现状行为不变）。
    /// 证据地板最高优先：证据行与最小邻域无条件保留，超限时输出
    /// `mode=overflow:evidence` / `overflow:fill` 并如实报告，绝不静默丢弃证据。
    pub limit: Option<usize>,
    /// 结构感知邻域（`--structure 1`）：fence 配对 / import·use·#include 块 /
    /// 堆栈 at 链 + `Caused by:` 延续整段并入种子区域（超长区域按
    /// `REGION_DEFAULT.max_span` 裁剪为种子邻域窗口）。默认 `false` =
    /// 行为与 v0.4.1 逐字节一致。
    pub structure: bool,
    /// 语义增强权重（`--semantic <weights.json>`，feature=semantic 下可用）；
    /// `None` = 不启用，默认路径逐字节不变。
    #[cfg(feature = "semantic")]
    pub semantic: Option<crate::semantic::Weights>,
    /// 相似度注入权重（`--sim-weight`，默认 1.0）；仅在 `sims` 存在时参与打分。
    pub sim_weight: f64,
    /// 预计算的「query 与每行」相似度（`--embed-cmd` 时由 `ops` 层填充；
    /// 长度须 == 输入行数）。`None` = 不注入，默认路径逐字节不变。
    pub sims: Option<Vec<f64>>,
    /// 显式证据声明（压缩后校验留存，见 [`Evidence`]）；空 = 走默认行保留率代理。
    /// 注意与 [`Outcome::evidence`]（证据行计数 = seeds）语义不同：本字段是
    /// 调用方声明"必须保留"的行号/子串，仅用于计量 [`Outcome::ev_retention`]。
    pub evidence: Vec<Evidence>,
    /// 结构锚点种类（P6 骨架保护，默认空 = 现状行为不变）：结构边界行标记为
    /// 保留锚点，优先级低于证据行、高于普通噪声行。
    pub structure_anchors: Vec<AnchorKind>,
    /// 填充/邻域行最低分数门槛（`--fill-floor`，默认 0.0 = 现状逐字节一致）。
    /// 大于 0 时低分行既不无条件保留为上下文、也不进入可选填充。
    pub fill_floor: f64,
    /// 行内 token 级裁剪（`--trim 1`，默认 0 = 现状逐字节一致）。
    /// 1 时保留行内部按重要性摘除低信息量 token；`trim_ratio` 控制保留比例。
    pub trim: bool,
    pub trim_ratio: f64,
    /// 主压缩路径的单桶内容类型（`--kind` 显式标注，默认 ToolOutput = 现状行为不变）。
    /// 仅给单桶打标签并决定缺省 `--ratio`（args 层解析为 `default_ratio()`）；
    /// `compress_bucketed` 按 part 覆盖，不受此字段影响。
    pub kind: ContentType,
}

impl Config {
    pub const DEFAULT_RATIO: f64 = 0.35;
    pub const MIN_RATIO: f64 = 0.02;
    pub const DEFAULT_CONTEXT: i64 = 1;
    pub const MAX_CONTEXT: i64 = 4;
    pub const DEFAULT_WINDOW: i64 = 2;
    pub const MIN_WINDOW: i64 = 1;
    pub const MAX_WINDOW: i64 = 8;
}

impl Default for Config {
    fn default() -> Self {
        Config {
            ratio: Self::DEFAULT_RATIO,
            context: Self::DEFAULT_CONTEXT,
            window: Self::DEFAULT_WINDOW,
            elide: true,
            gate: true,
            scoped: false,
            strict: false,
            lineno: false,
            forces: Vec::new(),
            limit: None,
            structure: false,
            #[cfg(feature = "semantic")]
            semantic: None,
            sim_weight: 1.0,
            sims: None,
            evidence: Vec::new(),
            structure_anchors: Vec::new(),
            kind: ContentType::ToolOutput,
            fill_floor: crate::policy::DEFAULT_FILL_FLOOR,
            trim: crate::policy::DEFAULT_TRIM,
            trim_ratio: crate::policy::DEFAULT_TRIM_RATIO,
        }
    }
}

/// 压缩模式。`as_str` / `Display` 输出与 v0.4.1 统计行字符串逐字一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 小输入直通（< `policy::SMALL_LINES` 行且 token 估算 < `policy::SMALL_TOK`）。
    PassSmall,
    /// 超长行直通（单行字节与字符均超过 `policy::LONGLINE_*`）。
    PassLongline,
    /// 行数直通（超过 `policy::MAX_LINES`），返回输入原文避免按行数放大内存。
    PassTooManyLines,
    /// 无查询信号 fail-open（零命中且无 `--force` 命中），返回输入原文。
    FailOpenZeroSignal,
    /// 预算过紧未选出任何行，回退原文。
    PassNoKept,
    /// 正常筛选（sieve）。
    Sieve,
    /// 证据地板（证据行 + 最小邻域）已超 `--max-tokens` 上限；证据仍全保留，
    /// 如实报告（绝不静默丢弃证据）。
    OverflowEvidence,
    /// 削减可选填充后输出仍超 `--max-tokens` 上限（如风险守卫追加行）；
    /// 如实报告，不丢弃已保留的证据性内容。
    OverflowFill,
}

impl Mode {
    /// v0.4.1 统计行字符串（向后兼容；新增变体须保持冒号分段格式）。
    pub const fn as_str(self) -> &'static str {
        match self {
            Mode::PassSmall => "passthrough:small",
            Mode::PassLongline => "passthrough:longline",
            Mode::PassTooManyLines => "passthrough:too-many-lines",
            Mode::FailOpenZeroSignal => "failopen:zero-signal",
            Mode::PassNoKept => "passthrough:no-kept",
            Mode::Sieve => "sieve",
            Mode::OverflowEvidence => "overflow:evidence",
            Mode::OverflowFill => "overflow:fill",
        }
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 压缩风险等级（风险审计输出）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    Low,
    Medium,
    High,
}

impl Risk {
    /// v0.4.1 统计行字符串（`LOW` / `MED` / `HIGH`）。
    pub const fn as_str(self) -> &'static str {
        match self {
            Risk::Low => "LOW",
            Risk::Medium => "MED",
            Risk::High => "HIGH",
        }
    }
}

impl fmt::Display for Risk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 单桶计量（统一分桶统计，挂在 [`Outcome::buckets`]）。
/// 只存事实字段；`cut_pct`/`share_*` 为派生值，用方法计算（不存冗余）。
#[derive(Debug, Clone, Copy)]
pub struct BucketStat {
    pub kind: ContentType,
    pub tokens_in: f64,
    pub tokens_out: f64,
    pub kept: usize,
    pub total: usize,
    pub mode: Mode,
}

impl BucketStat {
    /// 削减百分比（token 口径）：`(in - out) / in × 100`，in=0 → 0。
    pub fn cut_pct(&self) -> f64 {
        if self.tokens_in > 0.0 {
            (self.tokens_in - self.tokens_out) * 100.0 / self.tokens_in
        } else {
            0.0
        }
    }

    /// 占全局输入 token 的百分比（global=0 → 0）。
    pub fn share_in_pct(&self, global_in: f64) -> f64 {
        if global_in > 0.0 {
            self.tokens_in * 100.0 / global_in
        } else {
            0.0
        }
    }

    /// 占全局输出 token 的百分比（global=0 → 0）。
    pub fn share_out_pct(&self, global_out: f64) -> f64 {
        if global_out > 0.0 {
            self.tokens_out * 100.0 / global_out
        } else {
            0.0
        }
    }
}

/// 压缩结果与统计。`seeds == evidence` 恒成立（结构性不变量，可断言）。
pub struct Outcome {
    /// 压缩后的文本；passthrough / fail-open 时为输入原文。
    pub text: String,
    /// 保留行的原始输入行号（1-based，按原顺序；仅筛选模式有值，
    /// passthrough/fail-open 为空）。记忆库 `--store` 据此从输入提取原始证据行，
    /// L1 直答可回放原文。
    pub numbers: Vec<usize>,
    pub kept: usize,
    pub total: usize,
    pub input: usize,
    pub output: usize,
    pub cut: f64,
    pub dup: usize,
    pub seeds: usize,
    pub evidence: usize,
    pub risk: Risk,
    pub guarded: u8,
    pub regions: usize,
    pub window: i64,
    pub terms: usize,
    pub forced: usize,
    pub mode: Mode,
    /// 生效的 `--max-tokens` 上限（`None` = 未启用）。
    pub limit: Option<usize>,
    /// 输出文本的 token 估算（`terms::estimate` 口径）；passthrough/fail-open 不保证。
    pub tokens: f64,
    /// 输入全文的 token 估算（`terms::estimate` 口径）；全模式恒定填充，
    /// 供 v0.6.4 效果报告与 JSON 展示"原始 token → 压缩后 token → 缩减率"。
    pub tokens_in: f64,
    /// 被删除的连续区间（1-based 原始行号闭区间）与原因；仅在筛选模式下有值，
    /// passthrough/fail-open/no-kept 为空。`--json` 或新渲染模式消费；默认
    /// stdout 统计行不输出（与 v0.4.1 逐字节一致）。
    pub dropped: Vec<Dropped>,
    /// 可选预算利用率（%）：实际填充的可选内容 token 估算 ÷ 预算上限。仅筛选
    /// 模式有值；passthrough/fail-open/no-kept 为 0。`--json` 消费，默认统计行不输出。
    pub budget_used_pct: f64,
    /// 信息留存率（0..=1，确定性）：显式证据时 = 保留证据 / 总证据；无证据时 =
    /// 行保留率 kept/total（轻量代理，非信息留存语义，见 docs/context-governance-plan.md P4）。
    /// passthrough/fail-open/no-kept 恒 1.0（未删任何行）。`--json` 消费，默认统计行不输出。
    pub ev_retention: f64,
    /// 分桶统计（默认单桶 = 整输入视作 ToolOutput）；`compress_bucketed` 时逐桶填充。
    /// `--json` 消费，默认统计行不输出。
    pub buckets: Vec<BucketStat>,
    /// 逐行决策解释（`--explain`/`--json` 消费）；长度 == total，仅筛选模式有值，
    /// passthrough/fail-open/no-kept 为空。默认统计行不输出（与 v0.4.1 逐字节一致）。
    pub explain: Vec<Explain>,
}

/// 删除原因（`dropped` 每区间一条）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// 可选内容预算不足（含 `--structure 1` 超长结构区域裁剪）。
    Budget,
    /// `--max-tokens` 硬上限导致可选内容容量不足。
    MaxTokens,
    /// 归一化后第 3 次起的重复行折叠（非查询命中、非错误、非 force）。
    Dedup,
}

impl Reason {
    /// JSON 字段值（小写 kebab）。
    pub const fn as_str(self) -> &'static str {
        match self {
            Reason::Budget => "budget",
            Reason::MaxTokens => "max-tokens",
            Reason::Dedup => "dedup",
        }
    }
}

/// 单个删除区间：原始行号闭区间 `[start, end]`（1-based）与删除原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dropped {
    pub start: usize,
    pub end: usize,
    pub reason: Reason,
}

/// 逐行决策解释：每行的保留/删除原因（`Outcome::explain`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Explain {
    /// 证据行（query 词命中或 `--force`/`--preserve` 命中）。
    Seed,
    /// 证据的最小邻域（±context）。
    Context,
    /// 可选预算填充。
    Fill,
    /// 风险守卫（失败级错误行/事故标记行）。
    Guard,
    /// 结构锚点（P6 骨架保护：结构边界行，优先级低于证据）。
    Anchor,
    /// 去重折叠（第 3 次起的重复行）。
    Dedup,
    /// 删除（预算不足/结构裁剪/硬上限，细粒度见 `dropped`）。
    Dropped,
}

impl Explain {
    /// JSON / 文本字段值（小写）。
    pub const fn as_str(self) -> &'static str {
        match self {
            Explain::Seed => "seed",
            Explain::Context => "context",
            Explain::Fill => "fill",
            Explain::Guard => "guard",
            Explain::Anchor => "anchor",
            Explain::Dedup => "dedup",
            Explain::Dropped => "dropped",
        }
    }
}

// 零分配 u64→十进制行号写入（`N: ` 前缀，grep -n 风格；供 --lineno 使用）。
pub(crate) fn prefix(out: &mut String, n: usize) {
    let mut buf = [0u8; 20];
    let mut m = n;
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (m % 10) as u8;
        m /= 10;
        if m == 0 {
            break;
        }
    }
    out.push_str(std::str::from_utf8(&buf[i..]).unwrap());
    out.push_str(": ");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{diff, frame, lineref, marker};
    use crate::analysis::{lowercase, normalize};
    use crate::terms::terms;

    fn cfg() -> Config {
        Config {
            ratio: 0.35,
            ..Config::default()
        }
    }
    fn filler() -> String {
        "// filler line with enough tokens to pass the small guard".to_string()
    }

    #[test]
    fn invariant_seeds_kept() {
        let mut lines: Vec<String> = (0..200).map(|_| filler()).collect();
        lines[100] = "fn target() { return 1; }".to_string();
        let text = lines.join("\n");
        let o = compress(&text, "target", &cfg());
        assert_eq!(o.seeds, o.evidence);
        assert!(o.seeds >= 1);
        assert!(o.text.contains("fn target"));
    }

    #[test]
    fn sieve_kept_lno_aligns_with_output_lines() {
        // 保留行原始行号（1-based）与输出行一一对应、严格递增。
        let mut lines: Vec<String> = (0..200)
            .map(|i| format!("pad alpha beta line {}", i))
            .collect();
        lines[50] = "ERROR db timeout conn_id=9f2a".to_string();
        lines[51] = "Root cause: pool exhausted".to_string();
        lines[150] = "at Pool.java:214".to_string();
        let text = lines.join("\n");
        let o = compress(&text, "db timeout conn_id pool", &cfg());
        assert_eq!(o.mode, Mode::Sieve);
        assert_eq!(o.numbers.len(), o.kept, "numbers 数量必须等于 kept");
        assert!(!o.numbers.is_empty(), "筛选模式必须给出证据行号");
        for w in o.numbers.windows(2) {
            assert!(w[0] < w[1], "证据行号必须严格递增（原顺序）");
        }
        assert!(o.numbers.contains(&51), "保留行含 Root cause 行");
        // 输出的保留行内容与输入行一致（按行号回查）。
        let out_lines: Vec<&str> = o
            .text
            .split('\n')
            .filter(|l| !l.is_empty() && *l != "...")
            .collect();
        assert_eq!(out_lines.len(), o.kept, "输出非省略行数必须等于 kept");
    }

    #[test]
    fn passthrough_kept_lno_empty() {
        // passthrough/fail-open 不产生证据行号（记忆库只对筛选结果提取原始证据）。
        let text: String = (0..50)
            .map(|i| format!("line {} with padding text alpha beta gamma", i))
            .collect::<Vec<_>>()
            .join("\n");
        let o = compress(&text, "绝对不存在的词zz", &cfg());
        assert!(o.numbers.is_empty(), "failopen 无证据行号");
        let o2 = compress("small", "q", &cfg());
        assert!(o2.numbers.is_empty(), "passthrough:small 无证据行号");
    }

    #[test]
    fn zero_signal_failopen() {
        let text: String = (0..200)
            .map(|i| format!("line {} with padding text alpha beta gamma", i))
            .collect::<Vec<_>>()
            .join("\n");
        let o = compress(&text, "绝对不存在的词zz", &cfg());
        assert_eq!(o.mode, Mode::FailOpenZeroSignal);
        assert_eq!(o.text, text);
    }

    #[test]
    fn warn_mass_not_guarded_in_needle() {
        let mut lines: Vec<String> = (0..300)
            .map(|i| format!("WARN queue depth={} consumer_lag=2s pool=3", i))
            .collect();
        lines[150] = "ERROR db timeout at 12:01 pool exhausted (conn_id=9f2a)".to_string();
        lines[151] = "Root cause: connection pool size 4 < peak concurrency 11".to_string();
        let text = lines.join(&char::from(10u8).to_string());
        let needle = Config {
            ratio: 0.35,
            context: 0,
            ..Config::default()
        };
        let o = compress(&text, "what failed db timeout pool conn_id", &needle);
        assert!(o.text.contains("conn_id=9f2a"));
        assert!(o.text.contains("Root cause:"));
        assert!(
            !o.text.contains("WARN queue depth"),
            "warn 级行不应被风险守卫成批保留"
        );
        assert!(o.kept <= 4);
    }

    #[test]
    fn marker_guard_keeps_incident_marker_in_needle() {
        let mut lines: Vec<String> = (0..300)
            .map(|i| format!("INFO heartbeat seq={} ok pool=3 latency=1ms", i))
            .collect();
        lines[150] = "ERROR db timeout at 12:01 pool exhausted (conn_id=9f2a)".to_string();
        lines[151] = "at com.ingest.Pool.acquire(Pool.java:214)".to_string();
        lines[152] = "Root cause: connection pool size 4 < peak concurrency 11".to_string();
        let text = lines.join(&char::from(10u8).to_string());
        let q = "what failed db timeout pool conn_id";
        let needle = Config {
            ratio: 0.35,
            context: 0,
            ..Config::default()
        };
        let o = compress(&text, q, &needle);
        assert_eq!(o.risk, Risk::High);
        assert_eq!(o.guarded, 1);
        assert!(
            o.text.contains("Root cause:"),
            "needle 档必须在 HIGH 风险下保留事故标记行"
        );
        assert!(o.text.contains("conn_id=9f2a"));
        let gated_off = Config {
            ratio: 0.35,
            context: 0,
            gate: false,
            ..Config::default()
        };
        let o2 = compress(&text, q, &gated_off);
        assert!(
            !o2.text.contains("Root cause:"),
            "gate=0 时标记行不被强制保留"
        );
        assert_eq!(o2.guarded, 0);
    }

    #[test]
    fn lineref_semantics() {
        assert!(lineref("foo.rs:10"));
        assert!(lineref("(bar.c:3)"));
        assert!(lineref(":123"));
        assert!(lineref("y:3)"));
        assert!(!lineref("abc:"));
        assert!(!lineref("12"));
        assert!(!lineref(":"));
        assert!(!lineref("x)y"));
    }

    #[test]
    fn unified_diff_content_lines_are_recognized() {
        assert!(diff("+let value = 1;"));
        assert!(diff("-old_value"));
        assert!(diff("+"));
        assert!(diff("-"));
        assert!(!diff("+++ b/src/lib.rs"));
        assert!(!diff("--- a/src/lib.rs"));
        assert!(!diff("diff --git a/x b/x"));
    }

    #[test]
    fn analysis_strips_ansi_and_bare_carriage_returns() {
        let mut low = String::new();
        lowercase("\u{1b}[31mRoot cause:\u{1b}[0m 10%\rnext", &mut low);
        assert_eq!(low, "root cause: 10% next");
        assert!(marker(&low));
        low.clear();
        lowercase("\u{1b}]8;;https://example.com\u{7}link", &mut low);
        assert_eq!(low, "link");
    }

    #[test]
    fn norm_into_semantics() {
        let n = |t: &str| {
            let mut o = String::new();
            normalize(t, &mut o);
            o
        };
        assert_eq!(n("  a   b  "), "a b");
        assert_eq!(n("xyz123def"), "xyz#def");
        assert_eq!(n("abcdefab"), "abcdefab"); // 纯字母 hex 串保留词面
        assert_eq!(n("0xDEADBEEF1234"), "#x#"); // 含数字的 hex 串整体折叠
        assert_eq!(n("a1b2c3"), "a#b#c#");
        assert_eq!(n("seq=42 ok"), "seq=# ok");
        assert_eq!(n(""), "");
        assert_eq!(n("汉字 42"), "汉字 #");
    }

    #[test]
    fn crlf_equivalence() {
        let lf = char::from(10u8).to_string();
        let crlf_sep = String::from(char::from(13u8)) + lf.as_str();
        let base: String = (0..200)
            .map(|i| format!("line {} alpha kvwrite beta gamma delta", i))
            .collect::<Vec<_>>()
            .join(lf.as_str());
        let crlf = base.replace(lf.as_str(), crlf_sep.as_str());
        let a = compress(&base, "kvwrite", &cfg());
        let b = compress(&crlf, "kvwrite", &cfg());
        assert_eq!(a.text, b.text);
        assert_eq!(a.kept, b.kept);
        assert_eq!(a.output, b.output);
        assert!(b.input > a.input, "CRLF 输入的原始字符数应包含 CR");
    }

    #[test]
    fn passthrough_modes() {
        let lf = char::from(10u8).to_string();
        let small: String = (0..10)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join(lf.as_str());
        let o = compress(&small, "line", &cfg());
        assert_eq!(o.mode, Mode::PassSmall);
        assert_eq!(o.text, small);
        let mut lines = vec!["x".repeat(150000)];
        lines.extend((0..60).map(|i| format!("filler error kvwrite line {}", i)));
        let text = lines.join(lf.as_str());
        let o2 = compress(&text, "kvwrite", &cfg());
        assert_eq!(o2.mode, Mode::PassLongline);
        assert_eq!(o2.text, text);
    }

    #[test]
    fn cjk_error_words_guard_and_class() {
        let mut lines: Vec<String> = (0..300)
            .map(|i| format!("INFO 心跳 seq={} 状态=正常 latency=1ms", i))
            .collect();
        lines[150] = "错误：数据库连接超时 (conn_id=9f2a)".to_string();
        lines[151] = "错误：磁盘已满 errno=28".to_string();
        lines[152] = "警告：队列深度=40".to_string();
        let text = lines.join(&char::from(10u8).to_string());
        let needle = Config {
            ratio: 0.35,
            context: 0,
            ..Config::default()
        };
        let o = compress(&text, "数据库 超时 conn_id", &needle);
        assert!(o.text.contains("conn_id=9f2a"), "命中行作为种子保留");
        assert!(
            o.text.contains("磁盘已满"),
            "失败级 CJK 错误行在 HIGH 风险下受守卫"
        );
        assert!(!o.text.contains("队列深度"), "警告级 CJK 行不成批保留");
        assert_eq!(o.guarded, 1);
    }

    #[test]
    fn deterministic_output() {
        let text: String = (0..300)
            .map(|i| format!("l{} error alpha beta kvwrite", i % 7))
            .collect::<Vec<_>>()
            .join("\n");
        let a = compress(&text, "kvwrite", &cfg());
        let b = compress(&text, "kvwrite", &cfg());
        assert_eq!(a.text, b.text);
        assert_eq!(a.cut.to_bits(), b.cut.to_bits());
    }

    #[test]
    fn force_duplicates_are_all_kept() {
        let mut lines: Vec<String> = (0..200)
            .map(|i| format!("INFO filler {} alpha beta", i))
            .collect();
        for idx in [150usize, 151, 152, 153] {
            lines[idx] = "SPECIAL_FORCE_LINE".to_string();
        }
        let text = lines.join("\n");
        let needle = Config {
            ratio: 0.02,
            context: 0,
            forces: vec!["SPECIAL_FORCE_LINE".to_string()],
            ..Config::default()
        };
        let o = compress(&text, "zzzznomatch", &needle);
        assert_eq!(o.text.matches("SPECIAL_FORCE_LINE").count(), 4);
        assert_eq!(o.seeds, 4);
    }

    #[test]
    fn no_kept_falls_back_to_passthrough() {
        let line = format!("alpha {}", "x".repeat(5000));
        let text: String = (0..40).map(|_| line.clone()).collect::<Vec<_>>().join("\n");
        let needle = Config {
            ratio: 0.02,
            context: 1,
            ..Config::default()
        };
        let o = compress(&text, "alpha", &needle);
        assert_eq!(o.mode, Mode::PassNoKept);
        assert_eq!(o.text, text);
        assert_eq!(o.kept, o.total);
        assert_eq!(o.output, o.input);
        assert_eq!(o.cut, 0.0);
    }

    #[test]
    fn config_values_are_normalized() {
        let mut lines: Vec<String> = (0..200).map(|_| filler()).collect();
        lines[100] = "fn target() { return 1; }".to_string();
        let text = lines.join("\n");
        let weird = Config {
            ratio: f64::NAN,
            context: -9,
            window: -1,
            ..Config::default()
        };
        let o = compress(&text, "target", &weird);
        assert_eq!(o.window, 1);
        assert!(o.text.contains("fn target"));

        let huge = Config {
            window: i64::MAX,
            context: i64::MAX,
            ..Config::default()
        };
        let o2 = compress(&text, "target", &huge);
        assert_eq!(o2.window, 8);
        assert!(o2.text.contains("fn target"));
    }

    #[test]
    fn small_guard_requires_few_tokens() {
        let line = format!("alpha {}", "x".repeat(5000));
        let text: String = (0..39).map(|_| line.clone()).collect::<Vec<_>>().join("\n");
        let o = compress(&text, "alpha", &cfg());
        assert_ne!(o.mode, Mode::PassSmall);
        assert!(o.seeds == 0);
    }

    #[test]
    fn small_guard_cuts_at_500_not_1200() {
        // 39 行 × 65 字符 ≈ 660 token：位于新阈值 500 之上、旧阈值 1200 之下 → 必须压缩。
        let line = "alpha filler line with padding text for token estimation purposes".to_string();
        let text: String = (0..39).map(|_| line.clone()).collect::<Vec<_>>().join("\n");
        let o = compress(&text, "alpha", &cfg());
        assert_ne!(
            o.mode,
            Mode::PassSmall,
            "token≈660 必须进入压缩（阈值已降至 500）"
        );
        assert!(o.kept > 0 && o.kept < 39, "压缩应保留部分证据行");

        // 39 行 × 8 字符 ≈ 80 token < 500 → 仍透传。
        let tiny: String = (0..39)
            .map(|i| format!("alpha {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let o2 = compress(&tiny, "alpha", &cfg());
        assert_eq!(o2.mode, Mode::PassSmall, "token≈80 仍属小输入透传");
    }

    #[test]
    fn budget_is_optional_only() {
        let mut lines: Vec<String> = (0..99)
            .map(|i| format!("alpha filler {} {}", i, "x".repeat(100)))
            .collect();
        lines.insert(50, format!("SPECIAL_FORCE {}", "x".repeat(10000)));
        let text = lines.join("\n");
        let needle = Config {
            ratio: 0.02,
            context: 1,
            forces: vec!["SPECIAL_FORCE".to_string()],
            ..Config::default()
        };
        let o = compress(&text, "alpha", &needle);
        assert_eq!(o.seeds, 1);
        assert!(o.kept > 3, "种子与上下文不应挤占可选内容预算");
    }

    #[test]
    fn cjk_stop_and_tail_terms() {
        assert!(terms("这个").is_empty());
        assert!(terms("数据库连接超时").contains(&"超时".to_string()));
        assert!(terms("エラー").contains(&"エラー".to_string()));
    }

    #[test]
    fn runaway_token_is_dropped() {
        let long = "x".repeat(crate::terms::MAX_TERM_BYTES + 1);
        assert!(
            terms(&long).is_empty(),
            "超长 token 不得进入索引词表（u8 长度字段会截断）"
        );
        let max = "x".repeat(crate::terms::MAX_TERM_BYTES);
        assert_eq!(terms(&max), vec![max], "上限内 token 保持可用");
    }

    #[test]
    fn crlf_failopen_preserves_original() {
        let lines: Vec<String> = (0..200)
            .map(|i| format!("line {} alpha beta gamma delta", i))
            .collect();
        let text = lines.join("\r\n");
        let o = compress(&text, "zzzznomatch", &cfg());
        assert_eq!(o.mode, Mode::FailOpenZeroSignal);
        assert_eq!(o.text, text);
        assert_eq!(o.input, text.chars().count());
    }

    #[test]
    fn frame_extension_scan_is_bounded() {
        assert!(frame("  foo.rs:10"));
        assert!(!frame(&format!("  foo.{}:10", "a".repeat(50))));
    }

    #[test]
    fn lineno_prefixes_kept_lines() {
        let mut lines: Vec<String> = (0..200).map(|_| filler()).collect();
        lines[99] = "fn target() { return 1; }".to_string();
        let text = lines.join("\n");
        let cfg = Config {
            ratio: 0.35,
            context: 0,
            lineno: true,
            ..Config::default()
        };
        let o = compress(&text, "target", &cfg);
        assert!(
            o.text.contains("100: fn target() { return 1; }"),
            "种子行应带原文行号"
        );
        // passthrough / fail-open 返回原文，不加行号
        let small = Config {
            lineno: true,
            ..Config::default()
        };
        let s: String = (0..10)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let o2 = compress(&s, "line", &small);
        assert_eq!(o2.mode, Mode::PassSmall);
        assert_eq!(o2.text, s);
        assert!(!o2.text.contains("1: line 0"), "passthrough 不附加行号");
    }

    #[test]
    fn max_tokens_overflow_evidence_keeps_evidence() {
        // 证据地板（证据 + 最小邻域）已超 --max-tokens：证据仍全保留，如实报告。
        let mut lines: Vec<String> = (0..200).map(|_| filler()).collect();
        lines[100] = "conn 9f2a broken db crash".to_string();
        let text = lines.join("\n");
        let cfg = Config {
            limit: Some(3),
            ..cfg()
        };
        let o = compress(&text, "alpha conn broken", &cfg);
        assert_eq!(o.mode, Mode::OverflowEvidence);
        assert_eq!(o.limit, Some(3));
        assert!(
            o.text.contains("conn 9f2a"),
            "证据行必须保留（绝不静默丢证据）"
        );
        assert!(o.tokens > 3.0, "tokens 应如实报告超限后的估算");
    }

    #[test]
    fn max_tokens_overflow_fill_after_guard() {
        // 证据地板未超限，但风险守卫追加证据性错误行后总估算超限：OverflowFill。
        let mut lines: Vec<String> = (0..200)
            .map(|i| format!("pad alpha line {} beta gamma", i))
            .collect();
        lines[5] = "conn 9f2a broken".to_string();
        lines[6] = "error alpha pipe burst 7a1c now".to_string();
        let text = lines.join("\n");
        let cfg = Config {
            ratio: 0.35,
            context: 0,
            limit: Some(5),
            ..cfg()
        };
        let o = compress(&text, "alpha conn broken", &cfg);
        assert_eq!(o.mode, Mode::OverflowFill);
        assert_eq!(o.guarded, 1, "HIGH 风险守卫必须仍生效");
        assert!(o.text.contains("conn 9f2a"), "证据行保留");
        assert!(o.text.contains("error alpha pipe burst"), "守卫行保留");
        assert!(o.tokens > 5.0, "tokens 如实报告超限");
    }

    #[test]
    fn max_tokens_normal_sieve_respects_cap() {
        // 上限足够时：可选填充受 min(budget, max - floor) 约束，最终估算不超过上限。
        let mut lines: Vec<String> = (0..200).map(|_| filler()).collect();
        lines[100] = "conn 9f2a broken db crash".to_string();
        let text = lines.join("\n");
        let cfg = Config {
            limit: Some(1000),
            ..cfg()
        };
        let o = compress(&text, "alpha conn broken", &cfg);
        assert_eq!(o.mode, Mode::Sieve);
        assert_eq!(o.limit, Some(1000));
        assert!(o.tokens <= 1000.0 + 1e-9, "正常模式下输出估算不得超上限");
        assert!(o.text.contains("conn 9f2a"));
    }

    #[test]
    fn max_tokens_conservative_bound_caps_fill() {
        // --max-tokens 保守上界：可选填充按 limit/BUDGET_SAFETY_FACTOR 封顶（证据地板
        // 仍按诚实 limit 诊断）。构造 seed ±window(8) 区域内大量可填行，使 remaining
        // 成为约束：若无上界，填充会逼近 limit（≈300），有上界则封顶于 limit/factor。
        let mut lines: Vec<String> = (0..200)
            .map(|_| format!("alpha pad {}", "x".repeat(60)))
            .collect();
        lines[100] = "alpha beta target conn broken".to_string();
        let text = lines.join("\n");
        let cfg = Config {
            ratio: 0.5,
            window: 8,
            limit: Some(300),
            ..cfg()
        };
        let o = compress(&text, "alpha beta", &cfg);
        assert_eq!(o.mode, Mode::Sieve);
        assert!(o.text.contains("target conn"));
        assert!(
            o.tokens <= 300.0 / crate::terms::BUDGET_SAFETY_FACTOR + 1e-6,
            "可选填充应被保守上界封顶于 limit/factor，got tokens={:.1}",
            o.tokens
        );
    }

    #[test]
    fn max_tokens_default_none_keeps_legacy_shape() {
        // 默认 None = 现状行为不变：Outcome 不携带上限字段语义。
        let mut lines: Vec<String> = (0..200).map(|_| filler()).collect();
        lines[100] = "conn 9f2a broken db crash".to_string();
        let text = lines.join("\n");
        let o = compress(&text, "alpha conn broken", &cfg());
        assert_eq!(o.mode, Mode::Sieve);
        assert_eq!(o.limit, None);
    }

    #[test]
    fn explain_reports_per_line_reason() {
        // 唯一字母 filler（不 dedup、无 query 命中、scores=0 → Dropped），
        // 种子 + 邻域（Seed/Context），首两行 edge 身份（Fill）。
        let mut lines: Vec<String> = (0..60).map(|i| format!("INFO {}", word(i))).collect();
        lines[30] = "ERROR db timeout conn_id=9f2a".to_string();
        let text = lines.join("\n");
        let o = compress(&text, "db timeout", &cfg());
        assert_eq!(o.explain.len(), o.total, "explain 长度必须 == total");
        assert_eq!(o.explain[30], Explain::Seed, "query 命中行是 seed");
        assert_eq!(o.explain[29], Explain::Context, "种子邻域是 context");
        assert_eq!(o.explain[31], Explain::Context, "种子下方邻域是 context");
        assert_eq!(o.explain[0], Explain::Fill, "首行 edge 身份行是 fill");
        assert!(
            o.explain.contains(&Explain::Dropped),
            "非 dup、无得分的未保留行应标记 dropped"
        );
        // as_str 映射。
        assert_eq!(Explain::Seed.as_str(), "seed");
        assert_eq!(Explain::Dropped.as_str(), "dropped");
    }

    #[test]
    fn ev_retention_default_is_line_retention_ratio() {
        let mut lines: Vec<String> = (0..200)
            .map(|i| format!("pad alpha beta line {i}"))
            .collect();
        lines[100] = "ERROR db timeout conn_id=9f2a".to_string();
        let text = lines.join("\n");
        let o = compress(&text, "db timeout", &cfg());
        assert_eq!(o.mode, Mode::Sieve);
        assert!(
            (o.ev_retention - o.kept as f64 / o.total as f64).abs() < 1e-9,
            "默认代理 = kept/total，got {} vs {}",
            o.ev_retention,
            o.kept as f64 / o.total as f64
        );
        assert!(o.ev_retention > 0.0 && o.ev_retention < 1.0);
    }

    #[test]
    fn ev_retention_passthrough_and_failopen_are_one() {
        // 小输入 passthrough：未删任何行。
        let o = compress("a\nb\nc\n", "a", &cfg());
        assert_eq!(o.mode, Mode::PassSmall);
        assert_eq!(o.ev_retention, 1.0);
        // 零信号 fail-open：原样保留。
        let text = (0..200)
            .map(|i| format!("line {i} alpha beta gamma"))
            .collect::<Vec<_>>()
            .join("\n");
        let o2 = compress(&text, "zzz_no_such", &cfg());
        assert_eq!(o2.mode, Mode::FailOpenZeroSignal);
        assert_eq!(o2.ev_retention, 1.0);
    }

    #[test]
    fn ev_retention_explicit_evidence_all_kept() {
        let mut lines: Vec<String> = (0..200)
            .map(|i| format!("pad alpha beta line {i}"))
            .collect();
        lines[100] = "ERROR db timeout conn_id=9f2a".to_string();
        let text = lines.join("\n");
        let mut c = cfg();
        c.evidence = vec![
            crate::Evidence::Line(101), // 命中行（1-based）
            crate::Evidence::Substr("conn_id=9f2a".to_string()),
        ];
        let o = compress(&text, "db timeout", &c);
        assert_eq!(o.mode, Mode::Sieve);
        assert_eq!(o.ev_retention, 1.0, "声明的证据全部保留");
    }

    #[test]
    fn ev_retention_explicit_evidence_reports_dropped() {
        // 声明一个不会被命中的行号作为证据 → 留存率如实 < 1.0（只校验、不强制保留）。
        let mut lines: Vec<String> = (0..200)
            .map(|i| format!("pad alpha beta line {i}"))
            .collect();
        lines[100] = "ERROR db timeout conn_id=9f2a".to_string();
        let text = lines.join("\n");
        let mut c = cfg();
        c.context = 0; // 针尖模式：只留 seed + 守卫，纯 filler 必删。
        c.evidence = vec![
            crate::Evidence::Line(101), // seed 行保留
            crate::Evidence::Line(51),  // 纯 filler，远离 seed，必删
        ];
        let o = compress(&text, "db timeout", &c);
        assert_eq!(o.mode, Mode::Sieve);
        assert_eq!(o.ev_retention, 0.5, "1/2 证据保留");
    }

    #[test]
    fn structure_anchors_preserve_json_boundaries() {
        let mut lines: Vec<String> = vec!["{".into(), "  \"meta\": {".into()];
        for i in 0..200 {
            lines.push(format!("    \"filler{i}\": \"alpha beta line {i}\","));
        }
        lines.push("    \"target\": \"db timeout conn_id=9f2a\",".into());
        lines.push("  }".into());
        lines.push("}".into());
        let text = lines.join("\n");

        let mut c = cfg();
        c.structure_anchors = vec![crate::AnchorKind::Json];
        let o = compress(&text, "db timeout conn_id", &c);
        assert_eq!(o.mode, Mode::Sieve);
        assert!(o.text.contains("db timeout conn_id=9f2a"), "证据行保留");
        // 结构边界行全部保留。
        for b in ["{", "  \"meta\": {", "  }", "}"] {
            assert!(o.text.lines().any(|l| l == b), "结构锚点保留: {b}");
        }
        // 花括号平衡（结构骨架完整）。
        let open = o.text.chars().filter(|&ch| ch == '{').count();
        let close = o.text.chars().filter(|&ch| ch == '}').count();
        assert_eq!(open, close, "花括号平衡: open={open} close={close}");
        assert!(open >= 2, "至少保留两个开括号，got {open}");
    }

    #[test]
    fn structure_anchors_preserve_xml_tags() {
        let mut lines: Vec<String> = vec!["<root>".into()];
        for i in 0..200 {
            lines.push(format!("  <item{i}>alpha beta line {i}</item{i}>"));
        }
        lines.push("  <target>db timeout conn_id=9f2a</target>".into());
        lines.push("</root>".into());
        let text = lines.join("\n");

        let mut c = cfg();
        c.structure_anchors = vec![crate::AnchorKind::Xml];
        let o = compress(&text, "db timeout conn_id", &c);
        assert!(o.text.contains("db timeout conn_id=9f2a"), "证据行保留");
        assert!(o.text.lines().any(|l| l == "<root>"), "<root> 保留");
        assert!(o.text.lines().any(|l| l == "</root>"), "</root> 保留");
    }

    #[test]
    fn structure_anchors_default_off_no_anchor_reason() {
        let mut lines: Vec<String> = vec!["{".into()];
        for i in 0..200 {
            lines.push(format!("  \"filler{i}\": \"alpha beta line {i}\","));
        }
        lines.push("  \"target\": \"db timeout conn_id=9f2a\",".into());
        lines.push("}".into());
        let text = lines.join("\n");
        let o = compress(&text, "db timeout conn_id", &cfg());
        assert_eq!(o.mode, Mode::Sieve);
        assert!(
            !o.explain.contains(&crate::Explain::Anchor),
            "默认 off 不产生 anchor 原因"
        );
    }

    #[test]
    fn content_type_infer_classifies_by_feature() {
        assert_eq!(
            ContentType::infer("{\n  \"type\": \"object\",\n  \"properties\": {\"a\": {}}\n}"),
            ContentType::StaticContract
        );
        assert_eq!(
            ContentType::infer("<tools>\n<function name=\"get\">\n</function>\n</tools>"),
            ContentType::StaticContract
        );
        assert_eq!(
            ContentType::infer("<system-reminder>\nYou are an assistant.\n</system-reminder>"),
            ContentType::SystemDefinition
        );
        assert_eq!(
            ContentType::infer("[sys] system prompt"),
            ContentType::SystemDefinition
        );
        assert_eq!(
            ContentType::infer("user: hello\nassistant: hi"),
            ContentType::SessionMessage
        );
        assert_eq!(
            ContentType::infer("<|im_start|>user\nhi\n<|im_end|>"),
            ContentType::SessionMessage
        );
        assert_eq!(
            ContentType::infer("INFO heartbeat ok pool=3\nERROR db timeout"),
            ContentType::ToolOutput
        );
    }

    #[test]
    fn content_type_resolve_prefers_explicit_tag() {
        // 显式标注优先，忽略文本特征。
        assert_eq!(
            ContentType::resolve(Some("tool-output"), "{\"type\":\"object\"}"),
            ContentType::ToolOutput
        );
        // 缺失时回退推断。
        assert_eq!(
            ContentType::resolve(None, "user: hi"),
            ContentType::SessionMessage
        );
        // 非法 tag 也回退推断（fail-open）。
        assert_eq!(
            ContentType::resolve(Some("bogus"), "user: hi"),
            ContentType::SessionMessage
        );
    }

    #[test]
    fn config_kind_labels_single_bucket() {
        // 缺省单桶 = ToolOutput；显式 cfg.kind 只改单桶标签，不改变筛选行为本身。
        let mut lines: Vec<String> = (0..200).map(|_| filler()).collect();
        lines[100] = "fn target() { return 1; }".to_string();
        let text = lines.join("\n");

        let default = compress(&text, "target", &cfg());
        assert_eq!(default.buckets.len(), 1);
        assert_eq!(default.buckets[0].kind, ContentType::ToolOutput);

        let mut tagged = cfg();
        tagged.kind = ContentType::StaticContract;
        let o = compress(&text, "target", &tagged);
        assert_eq!(o.buckets.len(), 1);
        assert_eq!(o.buckets[0].kind, ContentType::StaticContract);
        // 标签不影响保留的正文（相同 ratio 下筛选结果一致）。
        assert_eq!(o.text, default.text);
    }

    /// 带标签的小语料（人为采样，覆盖清晰正例与已知混淆负例）。这是回归护栏，
    /// 度量 `ContentType::infer` 的启发式在此采样上的精度，不是真实分布上的
    /// 准确率声明。标签为内容类型的客观类别，非人工相关性判断。
    fn content_type_corpus() -> Vec<(&'static str, ContentType)> {
        use ContentType::*;
        vec![
            // —— StaticContract：JSON Schema / 工具定义 ——
            ("{\n  \"type\": \"object\",\n  \"properties\": {\"a\": {\"type\": \"string\"}}\n}", StaticContract),
            ("{\"type\":\"object\",\"properties\":{}}", StaticContract),
            ("{\"name\":\"get_weather\",\"parameters\":{\"type\":\"object\"}}", StaticContract),
            ("<tools>\n<function name=\"get\">\n</function>\n</tools>", StaticContract),
            ("<function name=\"search\">", StaticContract),
            ("{\"required\":[\"a\"],\"properties\":{\"a\":{}}}", StaticContract),
            ("{\n  \"$schema\": \"http://json-schema.org/draft-07/schema#\",\n  \"definitions\": {\"Foo\": {\"type\": \"string\"}}\n}", StaticContract),
            ("{\"allOf\":[{\"$ref\":\"#/definitions/Foo\"}]}", StaticContract),
            ("{\"oneOf\":[{\"type\":\"string\"},{\"type\":\"integer\"}]}", StaticContract),
            ("{\"$defs\":{\"Bar\":{\"type\":\"object\"}}}", StaticContract),
            ("{\"type\": \"array\", \"items\": {\"type\": \"string\"}}", StaticContract),
            ("openapi: 3.0.0\npaths:\n  /x:\n    get:\n      parameters: []", StaticContract),
            // —— SystemDefinition：系统注入元数据 / 提示 ——
            ("<system-reminder>\nYou are an assistant.\n</system-reminder>", SystemDefinition),
            ("[sys] system prompt", SystemDefinition),
            ("<system>\nbe concise\n</system>", SystemDefinition),
            ("system prompt: you are a helpful coding assistant", SystemDefinition),
            ("system_prompt = \"answer briefly\"", SystemDefinition),
            ("<system_reminder>inline</system_reminder>", SystemDefinition),
            ("You are Claude, a coding assistant. Follow these rules.", SystemDefinition),
            ("## System Prompt\nYou are an agent.", SystemDefinition),
            ("System: you are a shell assistant", SystemDefinition),
            // —— SessionMessage：user/assistant 前缀 ——
            ("user: hello\nassistant: hi", SessionMessage),
            ("<|im_start|>user\nhi\n<|im_end|>", SessionMessage),
            ("Human: question\nAssistant: answer", SessionMessage),
            ("system: reminder text", SessionMessage),
            ("  user: indented\n  assistant: reply", SessionMessage),
            ("User: hello\nAssistant: hi", SessionMessage),
            ("**User:** hi\n**Assistant:** hello", SessionMessage),
            ("USER: turn", SessionMessage),
            ("[user] hello", SessionMessage),
            // —— ToolOutput：日志 / 命令输出 / 数据 ——
            ("INFO heartbeat ok pool=3\nERROR db timeout conn_id=9f2a", ToolOutput),
            ("[12:00] INFO cache warmup complete\n[12:01] WARN retrying", ToolOutput),
            ("at com.ingest.Pool.acquire(Pool.java:214)\nCaused by: SQLException", ToolOutput),
            ("{\"name\":\"db\",\"status\":\"ok\"}", ToolOutput),
            ("{\"type\":\"error\",\"message\":\"timeout\"}", ToolOutput),
            ("total 123\ndrwxr-xr-x 2 user staff 68 dir", ToolOutput),
            ("diff --git a/x b/x\n--- a/x\n+++ b/x", ToolOutput),
            ("[{\"a\":1},{\"b\":2}]", ToolOutput),
            ("KEY=value\nSECRET=hidden", ToolOutput),
            ("user: connection reset by peer", ToolOutput),
            ("{\"properties\": {\"host\": \"a\", \"port\": 8080}}", ToolOutput),
        ]
    }

    #[test]
    fn content_type_infer_quantified() {
        let corpus = content_type_corpus();
        let mut correct = 0usize;
        let mut contract_total = 0usize;
        let mut contract_correct = 0usize;
        for (text, gold) in &corpus {
            let pred = ContentType::infer(text);
            if pred == *gold {
                correct += 1;
            }
            if *gold == ContentType::StaticContract {
                contract_total += 1;
                if pred == ContentType::StaticContract {
                    contract_correct += 1;
                }
            }
        }
        let accuracy = correct as f64 / corpus.len() as f64;
        let contract_recall = contract_correct as f64 / contract_total as f64;
        // 冻结护栏：整集精度 + 契约召回（契约→工具输出是危险方向，会过度裁剪）。
        // 实测（本语料 41 例，v0.6.7）：acc=0.780，契约 f1=0.917、系统 f1=0.800、
        // 会话 f1=0.750、工具 f1=0.667。残余误判集中在固有歧义：无标记纯系统提示、
        // 会话前缀大小写/格式变体（User:/**User:**/USER:）、带 user:/properties 的
        // 工具输出与 YAML 契约（OpenAPI）；这些靠启发式无法消解，显式 `--kind`
        // 才是首选，隐式仅作缺省。
        assert!(accuracy >= 0.75, "infer 整集精度退化: {accuracy:.3}");
        assert!(
            contract_recall >= 0.9,
            "契约召回（危险方向）退化: {contract_recall:.3}"
        );
    }

    /// 唯一字母词（base-26），长度不限，避免归一化后相同触发去重。
    fn word(i: usize) -> String {
        let mut n = i;
        let mut s = Vec::new();
        loop {
            s.push((b'a' + (n % 26) as u8) as char);
            n /= 26;
            if n == 0 {
                break;
            }
        }
        s.into_iter().rev().collect()
    }
}
