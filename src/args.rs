//! CLI 参数解析：声明式参数表驱动 parse + 校验；帮助/版本短路（帮助打印在 main）。
//! 只做解析与校验，不含 I/O 与引擎逻辑。
//!
//! 每个 flag 在 `FLAGS` 表中声明一次（名称 / 值种类 / 去重 / 错误消息 / 帮助文本），
//! 解析循环查表后按 `Kind` 统一分派，不再为每个 flag 写独立 match 分支；
//! 语义特例（`--window` 归一化、`--eval-verbose` 前置依赖）集中在少数函数内。
//! `usage()` 保持显式 format：其文本含 Config 默认值插值与合并项
//! （`[--gate|--gate-human|--gate-items]`），为保证逐字节一致不改为表生成。

use crate::Config;
use std::collections::HashSet;

/// 帮助文本中的默认值直接取自引擎配置，避免 CLI/文档/引擎三处漂移。
/// 帮助未纳入 golden 基线（无 golden 例覆盖 help 输出），故可自由重构；
/// 用法行保持显式 format，选项说明按功能分组追加。
mod parse;
pub(crate) use parse::preserve_into;
pub use parse::usage;
use parse::*;

pub struct Opts {
    pub query: String,
    /// 压缩引擎配置的唯一来源；CLI 默认值来自 `Config::default()`，不再重复硬编码。
    pub config: Config,
    pub json: bool,
    pub stat: bool,
    pub no_stats: bool,
    /// 逐行决策解释（stderr 输出每行保留/删除原因）。
    pub explain: bool,
    pub store: bool,
    /// 已验证的修复结论，仅在 `--store 1` 时写入 `case.fix`；缺失时输出警告。
    pub fix: String,
    /// 从文件读取多行修复结论，保留换行；与 `--fix` 互斥。
    pub fix_file: Option<String>,
    pub recall: usize,
    pub recall_only: bool,
    /// `--recall-detail` 时在召回段追加摘要与证据行。
    pub recall_detail: bool,
    /// 反馈标注：`Some((good, case_id))`。
    pub rate: Option<(bool, String)>,
    /// 库维护：重建索引、过期清理或删除 case。
    pub rebuild: bool,
    pub gc: bool,
    pub delete: Option<String>,
    /// 版本化（P3）：`--revise <case-id>` 与 --store 同用时，先写替换者再
    /// 标记旧 case revised + superseded_by 新 id。
    pub revise: Option<String>,
    /// 版本化（P3）：`--retire <case-id>` 仅 tombstone（不删除数据）。
    pub retire: Option<String>,
    /// `--retire-reason <text>`：retired case 的审计原因。
    pub retire_reason: String,
    /// 重放冻结的 baseline.json（豁免 `--query`）。
    pub eval: Option<String>,
    /// `--eval-verbose` 输出逐 case 的 A/B top-10 诊断。
    pub eval_verbose: bool,
    /// `--gate` 启用机械证据门，必须与 `--eval` 同用。
    pub eval_gate: bool,
    /// 人工弱相关/负例闭环门：在机械门之上要求全量人工元数据、控制标签与 NDCG。
    pub eval_gate_human: bool,
    /// 逐候选裁决门：允许 AI/非人类裁决人，但仍要求每条候选独立记录。
    pub eval_gate_items: bool,
    pub l1_hist: bool,
    /// 库盘点 `--stats`，豁免 `--query`。
    pub stats: bool,
    /// 智能标注器 `--annotate`，生成证据门标注集。
    pub annotate: bool,
    /// `--annotate-raw` 输出未经自检筛选的全变体集，附目标摘要供人工盲判；
    /// 与 `--annotate` 互斥。
    pub annotate_raw: bool,
    /// --annotate-out `path`：标注集输出路径（缺省 ``mem-dir`/baseline-auto.json`）。
    pub annotate_out: Option<String>,
    /// --seed N：标注生成确定性种子（缺省 0，同 seed 可重放）。
    pub seed: u64,
    /// --annotate-max N：标注覆盖的最大 case 数（缺省 16；0 = 全库）。
    /// 证据门 30–50 组：case 数 × 三层产率约 1.7，24+ case 库可直接落窗。
    pub annotate_max: usize,
    /// `--judge` 逐条读取 2/1/0/s，把 `judge:null` 替换为完整裁决元数据。
    pub judge: Option<String>,
    /// --judge-by `name`：裁决人标识（缺省取 USERNAME）。
    pub judge_by: Option<String>,
    /// 记忆库总容量软配额（字节）；写入前超限则拒绝。
    pub max_bytes: Option<u64>,
    pub mem_dir: String,
    pub files: Vec<String>,
    /// 可插拔相似度后端命令（`--embed-cmd <sh 命令>`）；`None` = 不启用。
    pub embed_cmd: Option<String>,
    /// MinHash-LSH 召回在 RRF 融合中的权重（`--minhash-weight`，默认 0 = 关）。
    pub minhash_weight: f64,
    /// 压缩前脱敏种类（`--redact`，逗号分隔：secret/id-card/phone/email）。
    pub redact: Vec<crate::redact::RedactKind>,
    /// `--kind auto`：正文类型在 ops 层按 infer 推断。
    pub kind_auto: bool,
    /// 是否显式给出 `--ratio`（决定 `--kind` 缺省 ratio 是否被覆盖）。
    pub ratio_explicit: bool,
}
fn home() -> String {
    if let Ok(d) = std::env::var("SCX_MEM_DIR") {
        if !d.trim().is_empty() {
            return d;
        }
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    if home.is_empty() {
        return ".scx-memory".to_string();
    }
    std::path::Path::new(&home)
        .join(".scx-memory")
        .to_string_lossy()
        .into_owned()
}
pub(crate) enum Kind {
    /// 无值开关（`--json`、`--rebuild`…）；带 `=VALUE` 时报错。
    NoValue,
    /// 任意字符串值（`--query`、`--mem-dir`…）；`err_empty` 非空时校验非空。
    Value,
    /// f64 值（`--ratio`，含有限性与范围校验）。
    F64,
    /// i64 值（`--context` / `--window`，含范围与归一化）。
    I64,
    /// u64 值（`--seed` / `--recall` / `--annotate-max` / `--max-bytes`；`min` 下限）。
    U64,
    /// 0/1 布尔值（`--elide` / `--store`…）。
    Bool01,
    /// 可重复子串（`--force`；非空且可 `A|B`）。
    Force,
    /// 双值（`--rate good|bad <case-id>`）。
    RatePair,
    /// 语义权重文件（`--semantic <path>`，feature=semantic 下可用）。
    #[cfg(feature = "semantic")]
    Semantic,
    /// 策略文件（`--policy-file <path>`：文本覆盖 DEFAULT，CLI 显式参数优先）。
    PolicyFile,
    /// 管线配置（`--pipeline <path>`：JSON 声明式装配 plan/trim/anchor 阶段，
    /// 优先级 CLI > pipeline.json > DEFAULT，语义同 `--policy-file`）。
    Pipeline,
    /// 显式证据声明（`--evidence`，可重复；正整数=行号，其他=子串）。
    Evidence,
    /// 结构锚点种类（`--structure-anchors`，逗号分隔：json/xml/idoc/yaml/md）。
    Anchors,
    /// 脱敏种类（`--redact`，逗号分隔：secret/id-card/phone/email）。
    Redact,
    /// 内容类型标注（`--kind <type>|auto`）。
    ContentType,
}
pub(crate) struct Flag {
    name: &'static str,
    /// 是否允许出现多次（`--force` 除外全部去重）。
    once: bool,
    kind: Kind,
    /// 值解析失败消息片段：`{name} expects {what}, got {:?}`。
    what: &'static str,
    /// Value 空值错误片段：`{name} {err_empty}`（空串 = 无检查）。
    err_empty: &'static str,
    /// U64 低于 `min` 的错误片段：`{name} {err_zero}`。
    err_zero: &'static str,
    /// U64 下限（0 或 1）。
    min: u64,
}
pub(crate) static FLAGS: &[Flag] = &[
    Flag {
        name: "--query",
        once: true,
        kind: Kind::Value,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--ratio",
        once: true,
        kind: Kind::F64,
        what: "a number",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--budget",
        once: true,
        kind: Kind::F64,
        what: "a number in (0, 1]",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--context",
        once: true,
        kind: Kind::I64,
        what: "an integer",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--window",
        once: true,
        kind: Kind::I64,
        what: "an integer",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--elide",
        once: true,
        kind: Kind::Bool01,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--risk-gate",
        once: true,
        kind: Kind::Bool01,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--scoped",
        once: true,
        kind: Kind::Bool01,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--strict",
        once: true,
        kind: Kind::Bool01,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--lineno",
        once: true,
        kind: Kind::Bool01,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--structure",
        once: true,
        kind: Kind::Bool01,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    #[cfg(feature = "semantic")]
    Flag {
        name: "--semantic",
        once: true,
        kind: Kind::Semantic,
        what: "weights.json",
        err_empty: "requires a weights file path",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--policy-file",
        once: true,
        kind: Kind::PolicyFile,
        what: "policy.toml",
        err_empty: "requires a policy file path",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--pipeline",
        once: true,
        kind: Kind::Pipeline,
        what: "pipeline.json",
        err_empty: "requires a pipeline json path",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--embed-cmd",
        once: true,
        kind: Kind::Value,
        what: "",
        err_empty: "requires a command",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--sim-weight",
        once: true,
        kind: Kind::F64,
        what: "a number",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--sem-weight",
        once: true,
        kind: Kind::F64,
        what: "a number",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--sem-gate",
        once: true,
        kind: Kind::F64,
        what: "a number (>=0; 0 disables)",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--minhash-weight",
        once: true,
        kind: Kind::F64,
        what: "a number",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--force",
        once: false,
        kind: Kind::Force,
        what: "",
        err_empty: "requires a non-empty pattern",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--preserve",
        once: false,
        kind: Kind::Force,
        what: "",
        err_empty: "requires a type (url/ip/email/errno)",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--fill-floor",
        once: true,
        kind: Kind::F64,
        what: "a number",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--trim",
        once: true,
        kind: Kind::Bool01,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--fold",
        once: true,
        kind: Kind::Bool01,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--entropy",
        once: true,
        kind: Kind::Bool01,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--auto-anchor",
        once: true,
        kind: Kind::Bool01,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--cluster",
        once: true,
        kind: Kind::Bool01,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--trim-ratio",
        once: true,
        kind: Kind::F64,
        what: "a number",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--trim-op-keep",
        once: true,
        kind: Kind::Bool01,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--code-keep",
        once: true,
        kind: Kind::Bool01,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--zh-keep",
        once: true,
        kind: Kind::Bool01,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--list-keep",
        once: true,
        kind: Kind::Bool01,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--num-line-keep",
        once: true,
        kind: Kind::Bool01,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--sym-line-keep",
        once: true,
        kind: Kind::Bool01,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--evidence",
        once: false,
        kind: Kind::Evidence,
        what: "",
        err_empty: "requires a line number or a substring",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--structure-anchors",
        once: true,
        kind: Kind::Anchors,
        what: "",
        err_empty: "requires a comma-separated list (json/xml/idoc/yaml/md)",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--redact",
        once: true,
        kind: Kind::Redact,
        what: "",
        err_empty: "requires a comma-separated list (secret/id-card/phone/email)",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--kind",
        once: true,
        kind: Kind::ContentType,
        what: "",
        err_empty: "requires static-contract/system-definition/session-message/tool-output/auto",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--store",
        once: true,
        kind: Kind::Bool01,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--fix",
        once: true,
        kind: Kind::Value,
        what: "",
        err_empty: "requires non-empty text",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--fix-file",
        once: true,
        kind: Kind::Value,
        what: "",
        err_empty: "requires a path",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--recall",
        once: true,
        kind: Kind::U64,
        what: "a positive integer",
        err_empty: "",
        err_zero: "expects a positive integer",
        min: 1,
    },
    Flag {
        name: "--recall-only",
        once: false,
        kind: Kind::NoValue,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--recall-detail",
        once: false,
        kind: Kind::NoValue,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--rate",
        once: true,
        kind: Kind::RatePair,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--rebuild",
        once: false,
        kind: Kind::NoValue,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--gc",
        once: false,
        kind: Kind::NoValue,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--delete",
        once: true,
        kind: Kind::Value,
        what: "",
        err_empty: "requires a case-id",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--revise",
        once: true,
        kind: Kind::Value,
        what: "",
        err_empty: "requires a case-id",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--retire",
        once: true,
        kind: Kind::Value,
        what: "",
        err_empty: "requires a case-id",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--retire-reason",
        once: true,
        kind: Kind::Value,
        what: "",
        err_empty: "requires non-empty text",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--eval",
        once: true,
        kind: Kind::Value,
        what: "",
        err_empty: "requires a baseline.json path",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--eval-verbose",
        once: false,
        kind: Kind::NoValue,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--gate",
        once: false,
        kind: Kind::NoValue,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--gate-human",
        once: false,
        kind: Kind::NoValue,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--gate-items",
        once: false,
        kind: Kind::NoValue,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--l1-hist",
        once: false,
        kind: Kind::NoValue,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--stats",
        once: false,
        kind: Kind::NoValue,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--annotate",
        once: false,
        kind: Kind::NoValue,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--annotate-raw",
        once: false,
        kind: Kind::NoValue,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--annotate-out",
        once: true,
        kind: Kind::Value,
        what: "",
        err_empty: "requires a path",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--seed",
        once: true,
        kind: Kind::U64,
        what: "a non-negative integer",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--annotate-max",
        once: true,
        kind: Kind::U64,
        what: "an integer",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--judge",
        once: true,
        kind: Kind::Value,
        what: "",
        err_empty: "requires a baseline path",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--judge-by",
        once: false,
        kind: Kind::Value,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--max-bytes",
        once: true,
        kind: Kind::U64,
        what: "a byte count",
        err_empty: "",
        err_zero: "must be > 0",
        min: 1,
    },
    Flag {
        name: "--max-tokens",
        once: true,
        kind: Kind::U64,
        what: "a token count",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--mem-dir",
        once: true,
        kind: Kind::Value,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--json",
        once: false,
        kind: Kind::NoValue,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--stat",
        once: false,
        kind: Kind::NoValue,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--no-stats",
        once: false,
        kind: Kind::NoValue,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--explain",
        once: false,
        kind: Kind::NoValue,
        what: "",
        err_empty: "",
        err_zero: "",
        min: 0,
    },
];

pub(crate) fn find(name: &str) -> Option<&'static Flag> {
    FLAGS.iter().find(|f| f.name == name)
}
pub(crate) struct ParseState {
    pub(crate) query: Option<String>,
    pub(crate) config: Config,
    pub(crate) json: bool,
    pub(crate) stat: bool,
    pub(crate) no_stats: bool,
    pub(crate) explain: bool,
    pub(crate) store: bool,
    pub(crate) fix: String,
    pub(crate) fix_file: Option<String>,
    pub(crate) recall: usize,
    pub(crate) recall_only: bool,
    pub(crate) recall_detail: bool,
    pub(crate) rate: Option<(bool, String)>,
    pub(crate) rebuild: bool,
    pub(crate) gc: bool,
    pub(crate) delete: Option<String>,
    pub(crate) revise: Option<String>,
    pub(crate) retire: Option<String>,
    pub(crate) retire_reason: String,
    pub(crate) eval: Option<String>,
    pub(crate) eval_verbose: bool,
    pub(crate) eval_gate: bool,
    pub(crate) eval_gate_human: bool,
    pub(crate) eval_gate_items: bool,
    pub(crate) l1_hist: bool,
    pub(crate) stats: bool,
    pub(crate) annotate: bool,
    pub(crate) annotate_raw: bool,
    pub(crate) annotate_out: Option<String>,
    pub(crate) seed: u64,
    pub(crate) annotate_max: usize,
    pub(crate) judge: Option<String>,
    pub(crate) judge_by: Option<String>,
    pub(crate) max_bytes: Option<u64>,
    pub(crate) limit: Option<u64>,
    pub(crate) mem_dir: String,
    pub(crate) files: Vec<String>,
    pub(crate) embed_cmd: Option<String>,
    pub(crate) minhash_weight: f64,
    pub(crate) redact: Vec<crate::redact::RedactKind>,
    /// `--kind auto` 标记：正文类型在 ops 层按 infer 推断。
    pub(crate) kind_auto: bool,
    /// CLI 显式给出的参数名（`once` 与优先级判定用）。
    pub(crate) seen: HashSet<&'static str>,
    /// 由 `--policy-file` 提供过的键对应 flag：优先级高于类型默认值，
    /// 但低于 CLI 显式参数。与 `seen` 分开，避免文件键被当成重复的 CLI 参数。
    pub(crate) sourced: HashSet<&'static str>,
}
impl ParseState {
    fn new() -> Self {
        ParseState {
            query: None,
            config: Config::default(),
            json: false,
            stat: false,
            no_stats: false,
            explain: false,
            store: false,
            fix: String::new(),
            fix_file: None,
            recall: 0,
            recall_only: false,
            recall_detail: false,
            rate: None,
            rebuild: false,
            gc: false,
            delete: None,
            revise: None,
            retire: None,
            retire_reason: String::new(),
            eval: None,
            eval_verbose: false,
            eval_gate: false,
            eval_gate_human: false,
            eval_gate_items: false,
            l1_hist: false,
            stats: false,
            annotate: false,
            annotate_raw: false,
            annotate_out: None,
            seed: 0,
            annotate_max: 16,
            judge: None,
            judge_by: None,
            max_bytes: None,
            limit: None,
            mem_dir: home(),
            files: Vec::new(),
            embed_cmd: None,
            minhash_weight: 0.0,
            redact: Vec::new(),
            kind_auto: false,
            seen: HashSet::new(),
            sourced: HashSet::new(),
        }
    }
}
pub fn parse(args: &[String]) -> Result<Opts, String> {
    // help/version 短路由由 main 在调用前处理；此处只解析与校验。
    all(args.to_vec())
}
mod tests {
    #[allow(unused_imports)] // clippy 误报：super::* 传递 parse 名称与 Config
    use super::*;

    #[allow(dead_code)] // clippy 对 test 辅助函数误报（调用真实存在）
    fn argv(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn rejects_unknown_options_and_bad_values() {
        assert!(all(argv(&["--bogus", "x", "--query", "a"])).is_err());
        assert!(all(argv(&["--query", "a", "--ratio", "abc"])).is_err());
        assert!(all(argv(&["--query", "a", "--ratio", "-1"])).is_err());
        assert!(all(argv(&["--query", "a", "--context", "5"])).is_err());
        assert!(all(argv(&["--query", "a", "--window", "9"])).is_err());
        assert!(all(argv(&["--query", "a", "--elide", "2"])).is_err());
    }

    #[test]
    fn rejects_missing_values_and_duplicate_scalars() {
        assert!(all(argv(&["--query"])).is_err());
        assert!(all(argv(&["--force", "--json"])).is_err());
        assert!(all(argv(&["--query", "a", "--query", "b"])).is_err());
        assert!(all(argv(&["--query", "a", "--seed", "1", "--seed", "2"])).is_err());
        assert!(all(argv(&[
            "--query",
            "a",
            "--annotate-max",
            "1",
            "--annotate-max",
            "2"
        ]))
        .is_err());
        assert!(all(argv(&["--query", "a", "--stat", "--no-stats"])).is_err());
    }

    /// README 的策略键白名单必须与 POLICY_TABLE 完全一致：新增策略键却忘了
    /// 写文档（或反之）会让 `--policy-file` 的用户文档失真。
    #[test]
    fn readme_policy_keys_match_whitelist() {
        let readme = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md"))
            .expect("README.md 必须存在");
        let marker = "键白名单 `";
        let start = readme.find(marker).expect("README 必须列出策略键白名单") + marker.len();
        // README 以「`key`/`key`/…；」列出：截到中文分号，再按 / 拆分、去反引号。
        let rest = &readme[start..];
        let end = rest.find('；').expect("白名单必须以中文分号收尾");
        let mut doc: Vec<&str> = rest[..end]
            .split('/')
            .map(|s| s.trim().trim_matches('`'))
            .collect();
        let mut code: Vec<&str> = POLICY_TABLE.iter().map(|(k, _)| *k).collect();
        doc.sort_unstable();
        code.sort_unstable();
        assert_eq!(doc, code, "README 策略键白名单与 POLICY_TABLE 不一致");
    }

    #[test]
    fn rejects_duplicate_kind() {
        // FLAGS 声明 --kind once:true，必须与 --ratio 等标量同口径拒绝重复。
        assert!(all(argv(&["--query", "q", "--kind", "tool-output"])).is_ok());
        assert!(all(argv(&[
            "--query",
            "q",
            "--kind",
            "tool-output",
            "--kind",
            "session-message"
        ]))
        .is_err());
    }

    /// 回归：策略文件的 ratio 不得被 `--kind` 的类型默认值覆盖，且结果与参数
    /// 顺序无关（CLI > 文件 > DEFAULT）。此前文件在前时会被静默覆盖。
    #[test]
    fn policy_ratio_survives_kind_in_any_order() {
        let p = std::env::temp_dir().join(format!("scx-policy-kind-{}.toml", std::process::id()));
        std::fs::write(&p, "ratio = 0.02\n").unwrap();
        let path = p.to_str().unwrap();
        let file = all(argv(&[
            "--query",
            "q",
            "--policy-file",
            path,
            "--kind",
            "static-contract",
        ]))
        .unwrap();
        assert!(
            (file.config.ratio - 0.02).abs() < 1e-9,
            "文件 ratio 必须胜过 kind 默认: {}",
            file.config.ratio
        );
        let kind = all(argv(&[
            "--query",
            "q",
            "--kind",
            "static-contract",
            "--policy-file",
            path,
        ]))
        .unwrap();
        assert!(
            (kind.config.ratio - 0.02).abs() < 1e-9,
            "顺序不得影响结果: {}",
            kind.config.ratio
        );
        // CLI 显式 --ratio 仍然最高优先。
        let cli = all(argv(&[
            "--query",
            "q",
            "--policy-file",
            path,
            "--kind",
            "static-contract",
            "--ratio",
            "0.5",
        ]))
        .unwrap();
        assert!((cli.config.ratio - 0.5).abs() < 1e-9);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn force_re_prefix_is_case_insensitive() {
        // 引擎先整体小写再识别 `re:`；CLI 校验必须同口径，否则 `RE:[` 会绕过
        // 前置校验，在引擎侧触发 debug 断言或静默丢弃该模式。
        assert!(all(argv(&["--query", "q", "--force", "RE:["])).is_err());
        assert!(all(argv(&["--query", "q", "--force", "RE:error|fatal"])).is_ok());
    }

    #[test]
    fn supports_equals_and_terminator() {
        let o = all(argv(&[
            "--query=alpha",
            "--ratio=0.5",
            "--context=0",
            "--",
            "--weird-file",
        ]))
        .unwrap();
        assert_eq!(o.query, "alpha");
        assert_eq!(o.config.ratio, 0.5);
        assert_eq!(o.config.context, 0);
        assert_eq!(o.files, vec!["--weird-file".to_string()]);
    }

    #[test]
    fn defaults_come_from_engine_config() {
        let o = all(argv(&["--query", "q"])).unwrap();
        let d = Config::default();
        assert_eq!(o.config.ratio, d.ratio);
        assert_eq!(o.config.context, d.context);
        assert_eq!(o.config.window, d.window);
        assert_eq!(o.config.elide, d.elide);
        assert_eq!(o.config.gate, d.gate);
        assert_eq!(o.config.lineno, d.lineno);
    }

    #[test]
    fn embed_cmd_and_sim_weight_parse() {
        let o = all(argv(&[
            "--query",
            "q",
            "--embed-cmd",
            "python embed.py",
            "--sim-weight",
            "2.5",
            "--sem-weight",
            "3.5",
        ]))
        .unwrap();
        assert_eq!(o.embed_cmd.as_deref(), Some("python embed.py"));
        assert_eq!(o.config.sim_weight, 2.5);
        assert_eq!(o.config.sem_weight, 3.5);
        assert_eq!(o.config.sims, None, "sims 由 ops 层填充，arg 解析不设置");
    }

    #[test]
    fn sem_gate_parses_and_validates() {
        let o = all(argv(&["--query", "q", "--sem-gate", "2.5"])).unwrap();
        assert_eq!(o.config.sem_gate, 2.5);
        let o = all(argv(&["--query", "q"])).unwrap();
        assert_eq!(o.config.sem_gate, 0.0, "默认关");
        assert!(all(argv(&["--query", "q", "--sem-gate", "-0.1"])).is_err());
        assert!(all(argv(&["--query", "q", "--sem-gate", "abc"])).is_err());
    }

    #[test]
    fn sem_weight_rejects_nonfinite_and_negative() {
        assert!(all(argv(&["--query", "q", "--sem-weight", "-1"])).is_err());
        assert!(all(argv(&["--query", "q", "--sem-weight", "abc"])).is_err());
        let o = all(argv(&["--query", "q", "--sem-weight", "0"])).unwrap();
        assert_eq!(o.config.sem_weight, 0.0);
    }

    #[test]
    fn sim_weight_rejects_nonfinite_and_negative() {
        assert!(all(argv(&["--query", "q", "--sim-weight", "-1"])).is_err());
        assert!(all(argv(&["--query", "q", "--sim-weight", "abc"])).is_err());
        let o = all(argv(&["--query", "q", "--sim-weight", "0"])).unwrap();
        assert_eq!(o.config.sim_weight, 0.0);
    }

    #[test]
    fn query_can_be_empty_only_with_force() {
        assert!(all(argv(&[])).is_err());
        assert!(all(argv(&["--force="])).is_err());
        assert!(all(argv(&["--force", "|"])).is_err());
        let o = all(argv(&["--force", "SPECIAL"])).unwrap();
        assert_eq!(o.query, "");
        assert_eq!(o.config.forces, vec!["SPECIAL".to_string()]);
    }

    #[test]
    fn preserve_expands_to_builtin_patterns() {
        // --preserve 展开为内置 force 正则，命中行视为证据；无需 --query。
        let o = all(argv(&[
            "--preserve",
            "url",
            "--preserve",
            "errno",
            "--force",
            "SPECIAL",
        ]))
        .unwrap();
        assert_eq!(o.query, "");
        assert_eq!(
            o.config.forces,
            vec![
                "re://".to_string(),
                "re:errno[= ]*[0-9]+".to_string(),
                "SPECIAL".to_string()
            ]
        );
        // 非法类型报错并列出白名单。
        let err = match all(argv(&["--preserve", "bogus"])) {
            Ok(_) => panic!("--preserve bogus 必须报错"),
            Err(e) => e,
        };
        assert!(err.contains("unknown type"), "err: {err}");
        assert!(err.contains("url/ip/email/errno"), "err: {err}");
        // 空类型报错。
        assert!(all(argv(&["--preserve="])).is_err());
    }

    #[test]
    fn kind_flag_tags_and_defaults_ratio() {
        // 显式类型：kind 落位，缺省 ratio 用该类型价值梯度默认。
        let o = all(argv(&["--query", "q", "--kind", "static-contract"])).unwrap();
        assert_eq!(o.config.kind, crate::ContentType::StaticContract);
        assert_eq!(
            o.config.ratio,
            crate::ContentType::StaticContract.default_ratio()
        );
        assert!(!o.kind_auto);
        assert!(!o.ratio_explicit);

        // 显式 --ratio 优先于类型默认。
        let o = all(argv(&[
            "--query",
            "q",
            "--kind",
            "tool-output",
            "--ratio",
            "0.5",
        ]))
        .unwrap();
        assert_eq!(o.config.kind, crate::ContentType::ToolOutput);
        assert_eq!(o.config.ratio, 0.5);
        assert!(o.ratio_explicit);

        // auto：类型留待 ops 层推断，ratio 保持默认。
        let o = all(argv(&["--query", "q", "--kind", "auto"])).unwrap();
        assert!(o.kind_auto);
        assert_eq!(o.config.kind, crate::ContentType::ToolOutput);
        assert!(!o.ratio_explicit);

        // 非法类型报错并列出白名单。
        let err = match all(argv(&["--query", "q", "--kind", "bogus"])) {
            Ok(_) => panic!("--kind bogus 必须报错"),
            Err(e) => e,
        };
        assert!(err.contains("static-contract"), "err: {err}");
        assert!(all(argv(&["--query", "q", "--kind"])).is_err());
    }

    #[test]
    fn windows_and_booleans_are_normalized() {
        let o = all(argv(&[
            "--query",
            "a",
            "--window",
            "0",
            "--elide",
            "0",
            "--risk-gate",
            "0",
        ]))
        .unwrap();
        assert_eq!(o.config.window, 1);
        assert!(!o.config.elide);
        assert!(!o.config.gate);
        let o2 = all(argv(&["--force=--json"])).unwrap();
        assert_eq!(o2.config.forces, vec!["--json".to_string()]);
        let o2b = all(argv(&["--query", "a", "--scoped", "1"])).unwrap();
        assert!(o2b.config.scoped);
        let o3 = all(argv(&["--query", "a", "--lineno", "1"])).unwrap();
        assert!(o3.config.lineno);
        let o4 = all(argv(&["--query", "a", "--lineno=0"])).unwrap();
        assert!(!o4.config.lineno);
        assert!(all(argv(&["--query", "a", "--lineno", "2"])).is_err());
        assert!(all(argv(&["--query", "a", "--lineno"])).is_err());
    }

    #[test]
    fn memory_flags_are_parsed() {
        let o = all(argv(&[
            "--query",
            "q",
            "--store",
            "1",
            "--recall",
            "3",
            "--mem-dir",
            "/tmp/m",
        ]))
        .unwrap();
        assert!(o.store);
        assert_eq!(o.recall, 3);
        assert_eq!(o.mem_dir, "/tmp/m");
        // --fix 与 --store 绑定。
        let o = all(argv(&[
            "--query",
            "q",
            "--store",
            "1",
            "--fix",
            "enlarge pool",
        ]))
        .unwrap();
        assert_eq!(o.fix, "enlarge pool");
        assert!(all(argv(&["--query", "q", "--fix", "x"])).is_err());
        assert!(all(argv(&["--query", "q", "--store", "1", "--fix="])).is_err());
        assert!(all(argv(&["--query", "q", "--store", "2"])).is_err());
        assert!(all(argv(&["--query", "q", "--recall", "0"])).is_err());
        assert!(all(argv(&["--recall-only"])).is_err());
        let o = all(argv(&["--query", "q", "--recall-only"])).unwrap();
        assert_eq!(o.recall, 3, "recall-only 未指定 K 时应默认 top-3");
        // 库维护参数不要求 --query。
        let o = all(argv(&["--rebuild", "--mem-dir", "/tmp/m"])).unwrap();
        assert!(o.rebuild);
        let o = all(argv(&["--gc", "--mem-dir", "/tmp/m"])).unwrap();
        assert!(o.gc);
        let o = all(argv(&["--delete", "2026-10-22_ab12cd34"])).unwrap();
        assert_eq!(o.delete.as_deref(), Some("2026-10-22_ab12cd34"));
        assert!(all(argv(&["--delete"])).is_err());
        assert!(all(argv(&["--rebuild=x"])).is_err());
        // --eval 豁免 --query；--recall-detail 是独立开关。
        let o = all(argv(&["--eval", "bl.json", "--mem-dir", "/tmp/m"])).unwrap();
        assert_eq!(o.eval.as_deref(), Some("bl.json"));
        // --eval-verbose 需配合 --eval。
        let o = all(argv(&["--eval", "bl.json", "--eval-verbose"])).unwrap();
        assert!(o.eval_verbose);
        // --gate 必须配 --eval，且不允许带值。
        let o = all(argv(&["--eval", "bl.json", "--gate"])).unwrap();
        assert!(o.eval_gate);
        assert!(all(argv(&["--gate"])).is_err());
        assert!(all(argv(&["--eval", "bl.json", "--gate=x"])).is_err());
        let o = all(argv(&["--eval", "bl.json", "--gate-human"])).unwrap();
        assert!(o.eval_gate_human);
        assert!(all(argv(&["--gate-human"])).is_err());
        assert!(all(argv(&["--eval", "bl.json", "--gate", "--gate-human"])).is_err());
        let o = all(argv(&["--eval", "bl.json", "--gate-items"])).unwrap();
        assert!(o.eval_gate_items);
        assert!(all(argv(&["--eval", "bl.json", "--gate-human", "--gate-items"])).is_err());
        assert!(
            all(argv(&["--eval-verbose"])).is_err(),
            "无 --eval 时 --eval-verbose 应报错"
        );
        let o = all(argv(&["--query", "q", "--recall", "3", "--recall-detail"])).unwrap();
        assert!(o.recall_detail);
        assert!(all(argv(&["--recall-detail=x"])).is_err());
        assert!(all(argv(&["--eval"])).is_err());
        // --stats 豁免 --query。
        let o = all(argv(&["--stats", "--mem-dir", "/tmp/m"])).unwrap();
        assert!(o.stats);
        assert!(all(argv(&["--stats=x"])).is_err());
    }

    #[test]
    fn rejects_conflicting_or_orphaned_actions() {
        assert!(all(argv(&["--rebuild", "--gc"])).is_err());
        assert!(all(argv(&["--stats", "--eval", "b.json"])).is_err());
        assert!(all(argv(&["--query", "q", "--max-bytes", "10"])).is_err());
        assert!(all(argv(&["--query", "q", "--recall-detail"])).is_err());
        assert!(all(argv(&["--query", "q", "--seed", "1"])).is_err());
        assert!(all(argv(&["--judge-by", "hao"])).is_err());
        assert!(all(argv(&["--query", "q", "--store", "1", "--recall-only"])).is_err());
    }
}
