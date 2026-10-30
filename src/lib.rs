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
mod fold;
/// 阶段计时器（feature = "perf" 可选诊断；默认零代码）。
mod perf;
/// 压缩管线配置（`--pipeline <path>`）：JSON 声明式装配 plan/trim/anchor，
/// 三层优先级 CLI > pipeline.json > policy 常量；未知键/越界 fail-loud。
pub mod pipeline;
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

/// 记忆知识库（旁路，`memory` feature 默认启用）：I/O 侧模块，与纯函数
/// `compress` 完全隔离；`--no-default-features` 时整模块 cfg 排除。
#[cfg(feature = "memory")]
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
    /// Markdown 标题行（ATX `#`…`######`），文档结构骨架。
    Markdown,
}

impl AnchorKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            AnchorKind::Json => "json",
            AnchorKind::Xml => "xml",
            AnchorKind::Idoc => "idoc",
            AnchorKind::Yaml => "yaml",
            AnchorKind::Markdown => "md",
        }
    }

    /// 名称 → 枚举（`--structure-anchors` 与 policy 表共用）。
    pub fn parse(s: &str) -> Option<AnchorKind> {
        match s {
            "json" => Some(AnchorKind::Json),
            "xml" => Some(AnchorKind::Xml),
            "idoc" => Some(AnchorKind::Idoc),
            "yaml" => Some(AnchorKind::Yaml),
            "md" => Some(AnchorKind::Markdown),
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
    /// 硬 kept-token 上限（0 = 关，现状行为）：最终保留 token 占比 ≤ budget，
    /// 超预算行按 reason 优先级（证据/数字/结构锚 → 可选填充）反向裁剪。
    pub budget: f64,
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
    /// semantic 加分缩放（`--sem-weight`，默认 1.0；feature=semantic 且
    /// `--semantic` 启用时生效）。放大可增强语义信号对保留决策的影响。
    pub sem_weight: f64,
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
    /// 跨行模板折叠（`--fold 1`，默认 0 = 现状逐字节一致）。连续块内
    /// 骨架相同的相似行（≥3）折叠为首行 + 标记行，压缩日志风暴类输入。
    pub fold: bool,
    /// 熵近似评分（`--entropy 1`，默认 0 = 现状逐字节一致）。无模型
    /// 统计信息密度信号，补偿无 query 命中的信息密集行召回。
    pub entropy: bool,
    /// 自动锚（`--auto-anchor 1`，默认 0 = 现状逐字节一致）。query 词形
    /// 变体自动加入命中锚，降低 --evidence 配置负担。
    pub auto_anchor: bool,
    /// 证据簇密度加权（`--cluster 1`，默认 0 = 现状逐字节一致）。boost
    /// 窗口内证据行聚集处整簇加权，保留完整上下文块。
    pub cluster: bool,
    pub trim_ratio: f64,
    /// 算术符号（`+ = * -`）作为结构锚保留（`--trim-op-keep`，默认 0）。
    /// 代码/算式场景需要；GSM8K 数学场景默认关保持 P6d 无损最优。
    pub trim_op_keep: bool,
    /// 代码围栏行保护（`--code-keep`，默认 0）。``` 围栏内代码行整行
    /// 强制保留并跳过行内 trim（防短变量/运算符被摘、防 gap elide），
    /// 压缩率转移到说明行；GSM8K 文本场景默认关零影响。
    pub code_keep: bool,
    /// 数字行保留（`--num-line-keep`，默认 0）。含 ASCII 数字的非折叠行
    /// 整行保留（行内仍走 trim，数字 token 本身永不摘）——长代码问答
    /// 场景：中文问题无法命中英文代码行（P6j），数字常量/赋值行是答案
    /// 高频载体，保行不删、行内仍压。
    pub num_line_keep: bool,
    /// 符号/签名行保留（`--sym-line-keep`，默认 0）。含函数/类型声明特征的
    /// 行整行保留（行内仍走 trim）——生成型任务 prompt 中函数签名是硬约束，
    /// 其 `fn`/`->`/`&`/`(` 符号在 token 级低重要会被摘行（P6l 实测保持率
    /// 0.500 根因：签名行整行消失，模型不知签名生成错误代码）。
    pub sym_line_keep: bool,
    /// 中文问句行保护（`--zh-keep`，默认 0）。含疑问词/问号的中文行
    /// 整行保留并跳过行内 trim（防 trim 逐字摘碎中文题干）——中文代码
    /// 问答场景 P8 实测 r=0.3 档保持率 0.2 根因：题干被裁成"线 线程 ？"。
    pub zh_keep: bool,
    /// 列表行保护（`--list-keep`，默认 0）。列表项行（`- `/`* `/`1. `/`[ ] `
    /// 等行首标记）整行保留并跳过行内 trim——枚举型文档的列表项是"全量
    /// 枚举"任务的硬信息，trim 摘词导致模型恢复清单漏项（P11-B2 实测
    /// instruct 保持率 0.000 根因；缓解候选，代价是该场景压缩率下降）。
    pub list_keep: bool,
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
            budget: 0.0,
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
            sem_weight: 1.0,
            sims: None,
            evidence: Vec::new(),
            structure_anchors: Vec::new(),
            kind: ContentType::ToolOutput,
            fill_floor: crate::policy::DEFAULT_FILL_FLOOR,
            trim: crate::policy::DEFAULT_TRIM,
            fold: crate::policy::DEFAULT_FOLD,
            entropy: crate::policy::DEFAULT_ENTROPY,
            auto_anchor: crate::policy::DEFAULT_AUTO_ANCHOR,
            cluster: crate::policy::DEFAULT_CLUSTER,
            trim_ratio: crate::policy::DEFAULT_TRIM_RATIO,
            trim_op_keep: crate::policy::DEFAULT_TRIM_OP_KEEP,
            code_keep: crate::policy::DEFAULT_CODE_KEEP,
            num_line_keep: crate::policy::DEFAULT_NUM_LINE_KEEP,
            sym_line_keep: crate::policy::DEFAULT_SYM_LINE_KEEP,
            zh_keep: crate::policy::DEFAULT_ZH_KEEP,
            list_keep: crate::policy::DEFAULT_LIST_KEEP,
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
    /// 声明证据总条数（`--evidence` 参数个数；bucketed 模式为 0）。配合
    /// `ev_retention` 供报告卡给出真实"保留/丢失"口径，替代 v0.6.4 误用
    /// `seeds` 当证据数的假"全部保留"（P12.6 修复）。
    pub ev_total: usize,
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
    /// 数字行兜底（`--num-line-keep`：含数字行整行保留，行内仍 trim）。
    NumLine,
    /// 符号/签名行（`--sym-line-keep`：函数/类型声明行整行保留，行内仍 trim）。
    SymLine,
    /// 中文问句行（`--zh-keep`：含疑问词/问号的中文行整行保留并跳过行内 trim）。
    ZhLine,
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
            Explain::NumLine => "numline",
            Explain::SymLine => "symline",
            Explain::ZhLine => "zhline",
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
    out.push_str(std::str::from_utf8(&buf[i..]).expect("自生成 ASCII 数字恒合法 UTF-8"));
    out.push_str(": ");
}

#[cfg(test)]
#[cfg(test)]
mod lib_tests;
