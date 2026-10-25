//! 声明式策略表：压缩内核的全部可调参数与结构加分权重。
//!
//! 算子与策略只以配置声明存在，不落独立硬编码文件；`DEFAULT_*` 逐项等于
//! v0.4.1 硬编码现状，改动默认值必须同时通过 golden 门与 confirm5 门。
//! 本模块是纯数据层（零 I/O、零分配逻辑），被 `compress` 内核消费。

pub const DEFAULT_RATIO: f64 = 0.35;
pub const MIN_RATIO: f64 = 0.02;
pub const MAX_CONTEXT: i64 = 4;
pub const MIN_WINDOW: i64 = 1;
pub const MAX_WINDOW: i64 = 8;

/// 小输入直通阈值：行数 < `SMALL_LINES` 且 token 估算 < `SMALL_TOK`。
pub const SMALL_LINES: usize = 40;
pub const SMALL_TOK: f64 = 1200.0;
/// 超长行直通阈值：单行字节与字符均超过即返回原文。
pub const LONGLINE_BYTES: usize = 100_000;
pub const LONGLINE_CHARS: usize = 100_000;
/// 可选内容预算下限（token 估算）。
pub const BUDGET_MIN: f64 = 200.0;
/// 种子区域合并间隔 = `window * GAP_RATIO`。
pub const GAP_RATIO: usize = 3;

/// 结构加分与排序权重表：评分 if 链的声明式等价物。
/// 全部默认值来自 v0.4.1 硬编码；改写任何一项必须重跑 golden + confirm5。
pub struct Weights {
    /// 基础分 = IDF 加权命中之和 × `sumw_mul`。
    pub sumw_mul: f64,
    /// 命中词数 >= 2 的行加分。
    pub pair_bonus: f64,
    /// 失败级错误行加分。
    pub error_bonus: f64,
    /// 标题行加分。
    pub heading_bonus: f64,
    /// diff 元信息行加分。
    pub diffmeta_bonus: f64,
    /// diff 内容行且命中加分。
    pub diffline_hit_bonus: f64,
    /// 堆栈帧 / 行引用加分。
    pub frame_bonus: f64,
    /// `--force` 命中加分。
    pub force_bonus: f64,
    /// 首 `edge_head` / 尾 `edge_tail` 身份行加分。
    pub edge_bonus: f64,
    pub edge_head: usize,
    pub edge_tail: usize,
    /// 邻域排序权重：邻域最高分 ÷ `boost_div`，上限 `boost_cap`。
    pub boost_div: f64,
    pub boost_cap: f64,
}

pub const DEFAULT_SCORES: Weights = Weights {
    sumw_mul: 3.0,
    pair_bonus: 2.0,
    error_bonus: 4.0,
    heading_bonus: 1.0,
    diffmeta_bonus: 2.0,
    diffline_hit_bonus: 2.0,
    frame_bonus: 2.0,
    force_bonus: 100.0,
    edge_bonus: 2.0,
    edge_head: 2,
    edge_tail: 3,
    boost_div: 4.0,
    boost_cap: 3.0,
};

/// 风险审计阈值表。`hit_drop_w` 保持整数以复刻原始整数表达式（bit 级一致）。
pub struct Limits {
    /// risk_raw >= `high` -> HIGH。
    pub high: f64,
    /// risk_raw >= `med` -> MED。
    pub med: f64,
    /// 丢弃命中行加权（整数，复刻 `err_dropped + hit_dropped * 3` 的整数表达式）。
    pub hit_drop_w: usize,
}

pub const DEFAULT_RISK: Limits = Limits {
    high: 0.35,
    med: 0.1,
    hit_drop_w: 3,
};

/// 结构感知邻域配置（`--structure 1` 开启时的判定参数；默认关闭时全部失效，
/// 行为与 v0.4.1 逐字节一致）。判定规则为数据表，不落独立硬编码文件。
pub struct Region {
    /// 结构区域并入上限：区域超过该行数时只保留区域内种子 ± half（max_span/2）
    /// 的窗口（其余行按 Budget 原因裁剪，不进入可选填充候选）。
    pub max_span: usize,
}

pub const REGION_DEFAULT: Region = Region { max_span: 200 };
