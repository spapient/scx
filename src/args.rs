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
/// 用法行保持显式 format，选项说明按功能分组追加。
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
    /// `--recall-kind`：按记忆类型（situation/experience/procedure/semantic）
    /// 过滤召回结果。
    pub recall_kind: Option<String>,
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
    /// 向量后端命令（`--embed-vec <sh 命令>`，输出原始向量、scx 内算余弦）；`None` = 不启用。
    pub embed_vec: Option<String>,
    /// 向量持久化缓存目录（`--embed-cache <dir>`，仿 usearch mmap：候选向量编码一次落盘、仅 query 在线编码）。
    pub embed_cache: Option<String>,
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
    /// 管线配置（`--pipeline <path|profile>`：JSON 声明式装配 plan/trim/anchor 阶段，
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
    #[cfg(feature = "semantic")]
    Flag {
        name: "--sem-backend",
        once: true,
        kind: Kind::Value,
        what: "cpu|gpu",
        err_empty: "requires cpu|gpu",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--max-mem",
        once: true,
        kind: Kind::U64,
        what: "MB",
        err_empty: "",
        err_zero: "must be >= 1",
        min: 1,
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
        what: "pipeline.json|profile",
        err_empty: "requires a pipeline path or profile name",
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
        name: "--embed-vec",
        once: true,
        kind: Kind::Value,
        what: "",
        err_empty: "requires a command",
        err_zero: "",
        min: 0,
    },
    Flag {
        name: "--embed-cache",
        once: true,
        kind: Kind::Value,
        what: "",
        err_empty: "requires a directory",
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
        name: "--recall-kind",
        once: true,
        kind: Kind::Value,
        what: "situation|experience|procedure|semantic",
        err_empty: "requires a kind",
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
    pub(crate) recall_kind: Option<String>,
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
    pub(crate) embed_vec: Option<String>,
    pub(crate) embed_cache: Option<String>,
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
            recall_kind: None,
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
            embed_vec: None,
            embed_cache: None,
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

    /// P4 回归：--recall-kind 必须落入 recall_kind（word() free 白名单），
    /// 不得被 path() 误判为 --delete。
    #[test]
    fn p4_recall_kind_parses_into_recall_kind() {
        let opts = parse(&[
            "--query".to_string(),
            "q".to_string(),
            "--recall".to_string(),
            "4".to_string(),
            "--recall-kind".to_string(),
            "experience".to_string(),
            "--mem-dir".to_string(),
            "/tmp/x".to_string(),
        ])
        .expect("parse");
        assert_eq!(opts.recall_kind.as_deref(), Some("experience"));
        assert!(opts.delete.is_none(), "delete 不得被污染");
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

pub fn usage() -> String {
    let mut s = format!(
        "usage: scx --query \"...\" [--ratio {}] [--budget F] [--window {}] [--context {}] [--elide 1] [--risk-gate 1] [--scoped 1] [--strict 1] [--lineno 1] [--max-tokens N] [--structure 0|1] [--structure-anchors LIST] [--redact LIST] [--kind TYPE] [--store 1] [--fix \"修复结论\"] [--fix-file <path>] [--policy-file <path>] [--pipeline <path>] [--embed-cmd CMD] [--sim-weight F] [--sem-weight F] [--sem-gate F] [--minhash-weight F] [--recall K] [--recall-only] [--recall-detail] [--recall-kind <kind>] [--rate good|bad <case-id>] [--rebuild] [--gc] [--delete <case-id>] [--revise <case-id>] [--retire <case-id> --retire-reason <text>] [--eval <baseline.json>] [--eval-verbose] [--gate|--gate-human|--gate-items] [--l1-hist] [--stats] [--annotate] [--annotate-raw] [--annotate-out <path>] [--seed N] [--annotate-max N] [--judge <baseline.json>] [--judge-by <name>] [--mem-dir DIR] [--force SUBSTR|re:PATTERN]... [--evidence N|substr]... [--fill-floor F] [--trim 0|1] [--trim-ratio F] [--fold 0|1] [--entropy 0|1] [--auto-anchor 0|1] [--cluster 0|1] [--trim-op-keep 0|1] [--code-keep 0|1] [--num-line-keep 0|1] [--sym-line-keep 0|1] [--list-keep 0|1] [--json] [--stat|--no-stats] [file...]",
        Config::DEFAULT_RATIO,
        Config::DEFAULT_WINDOW,
        Config::DEFAULT_CONTEXT
    );
    s.push_str("\noptions:\n");
    s.push_str("  --ratio R            optional-content budget (default ");
    s.push_str(&Config::DEFAULT_RATIO.to_string());
    s.push_str("); evidence lines and minimum neighborhood are exempt\n");
    s.push_str("  --budget F           hard kept-token ceiling (0 < F <= 1, default 0 = off):\n");
    s.push_str("                       final kept tokens capped at F of input; over-budget rows\n");
    s.push_str(
        "                       dropped by reason priority (evidence/number/structure first)\n",
    );
    s.push_str("  --context N          context 0 = needle mode: skip optional fill, keep evidence lines only (0..=4)\n");
    s.push_str(
        "  --max-tokens N       hard token ceiling; evidence floor wins, overflow is reported as\n",
    );
    s.push_str(
        "                       mode=overflow:evidence/fill, never silently drops evidence\n",
    );
    s.push_str("  --structure 1        structural-aware neighborhood: fenced blocks, import/use/#include\n");
    s.push_str("                       blocks, stack-trace at-chains (default off, byte-identical to v0.4.1)\n");
    s.push_str("  --strict 1    HIGH-risk guard detects structural errors only (leading\n");
    s.push_str(
        "                       error label / strong markers), not lines that merely mention\n",
    );
    s.push_str("                       'error' (default off = wide recall)\n");
    s.push_str("  --scoped 1     HIGH-risk guard only keeps error/marker lines inside the\n");
    s.push_str(
        "                       evidence neighborhood (default off = keep all, byte-identical)\n",
    );
    s.push_str("  --policy-file PATH   policy text override: ratio/context/window/elide/risk_gate/lineno/\n");
    s.push_str("                       max_tokens/structure; precedence CLI > file > DEFAULT\n");
    s.push_str("  --pipeline PATH      JSON pipeline config: plan/trim/anchor stage assembly\n");
    s.push_str(
        "                       (config/pipeline.json, or named profile: aggressive/balanced/\n",
    );
    s.push_str("                       conservative/deep); precedence CLI > file > DEFAULT\n");
    s.push_str(
        "  --embed-cmd CMD      pluggable similarity backend: sh -c CMD reads Q\\t<query> then\n",
    );
    s.push_str("                       L\\t<line> on stdin, emits one score per line (default off, zero-dep)\n");
    s.push_str(
        "  --sim-weight F       weight of injected similarity in ranking (default 1.0; >=0)\n",
    );
    s.push_str("  --sem-weight F       scale factor of semantic re-score (default 1.0; >=0)\n");
    s.push_str(
        "  --sem-gate F         semantic keep threshold (default 0 = off; >=0): lines with\n",
    );
    s.push_str(
        "                       centered semantic score >= F are kept whole, independent of\n",
    );
    s.push_str("                       optional-budget trimming (requires --semantic)\n");
    s.push_str("  --sem-backend B      semantic scorer backend (default cpu; feature=semantic):\n");
    s.push_str(
        "                       cpu = deterministic parallel (bitwise-stable), gpu = optional\n",
    );
    s.push_str(
        "                       self-built kernel via SCX_GPU_BIN (falls back to cpu if unavailable)\n",
    );
    s.push_str(
        "  --max-mem MB         semantic batch scoring memory budget in MB (default 256; >=1)\n",
    );
    s.push_str(
        "  --minhash-weight F   weight of MinHash-LSH Jaccard recall in RRF fusion (default 0; >=0)\n",
    );
    s.push_str("  --force P            keep lines matching substring (case-insensitive); prefix re: enters\n");
    s.push_str("                       a finite regex subset: | alternation, [..] classes, * + ? quantifiers\n");
    s.push_str(
        "  --fill-floor F       min score for context/fill lines (default 0 = current behavior);\n",
    );
    s.push_str(
        "                       F>0 drops low-value neighborhood/fill lines for deeper compression\n",
    );
    s.push_str(
        "  --trim 1             token-level trim of kept lines (default 0 = current behavior);\n",
    );
    s.push_str(
        "                       keeps numerics/structure anchors/query hits, drops stopwords\n",
    );
    s.push_str(
        "  --trim-ratio F       per-line token keep ratio for --trim (default 0.5; 0.1..=1.0)\n",
    );
    s.push_str(
        "  --fold 0|1           fold ≥3 same-skeleton consecutive rows into 1 row + marker\n",
    );
    s.push_str(
        "                       (default 0; collapses log-storm lines, keeps first row verbatim)\n",
    );
    s.push_str(
        "  --entropy 0|1        add model-free info-density signal to scoring (default 0)\n",
    );
    s.push_str(
        "  --auto-anchor 0|1    auto-add query word-form variants as hit anchors (default 0)\n",
    );
    s.push_str(
        "  --cluster 0|1        cluster-weight dense evidence rows in boost window (default 0)\n",
    );
    s.push_str("  --trim-op-keep 1     keep arithmetic ops (+ = * -) as anchors in --trim\n");
    s.push_str("                       (default 0; GSM8K lossless optimum, enable 1 for code)\n");
    s.push_str("  --code-keep 1        keep ``` fenced code lines whole in --trim\n");
    s.push_str("                       (default 0; fence-aware line keep, prose-only trim)\n");
    s.push_str("  --num-line-keep 1    keep lines containing ASCII digits in --trim\n");
    s.push_str("  --sym-line-keep 1   keep fn/type signature lines in --trim\n");
    s.push_str("                       (default 0; long-code QA: digit lines carry answers)\n");
    s.push_str("  --list-keep 1        keep markdown list lines whole in --trim\n");
    s.push_str("                       (default 0; list-keep: enumeration docs keep item lines)\n");
    s.push_str("  --preserve T         keep lines with builtin entity T (url/ip/email/errno); repeatable\n");
    s.push_str("  --evidence N|substr  declare must-keep evidence: positive int = line no, else substring;\n");
    s.push_str(
        "                       repeatable; only meters ev_retention, never changes trimming\n",
    );
    s.push_str("  --structure-anchors L comma-separated: json/xml/idoc/yaml/md; keep structural\n");
    s.push_str(
        "                       boundary lines (braces/tags/keys/md headings) as anchors below evidence\n",
    );
    s.push_str(
        "  --redact L           comma-separated: secret/id-card/phone/email; redact before\n",
    );
    s.push_str(
        "                       compression (mask secrets/PII as <redacted>), audit to stderr\n",
    );
    s.push_str("  --kind T             content-type tag: static-contract/system-definition/\n");
    s.push_str(
        "                       session-message/tool-output/auto (auto = infer from text);\n",
    );
    s.push_str(
        "                       per-kind default ratio unless --ratio given (default off =\n",
    );
    s.push_str("                       single tool-output bucket, byte-identical to v0.4.1)\n");
    #[cfg(feature = "semantic")]
    {
        s.push_str("  --semantic <weights>    optional neural re-score (cargo feature=semantic); weights file\n");
        s.push_str("                       self-describing JSON {schema,dim,weights,sha256}, SHA256 verified\n");
    }
    s.push_str(
        "  --store 1 --fix S    persist evidence block + fix conclusion into local memory\n",
    );
    s.push_str("  --recall K           side-channel recall from memory (mem: section)\n");
    s.push_str("  --json               structured JSON stats (outcome + dropped ledger)\n");
    s.push_str(
        "  --eval/--gate        replay frozen baseline / mechanical evidence gate (see config/)\n",
    );
    s.push_str("  file...              input file(s); stdin when none\n");
    s
}

pub(crate) fn value(
    args: &[String],
    i: &mut usize,
    name: &str,
    inline: Option<String>,
) -> Result<String, String> {
    if let Some(v) = inline {
        return Ok(v);
    }
    *i += 1;
    if *i >= args.len() {
        return Err(format!("{} requires a value", name));
    }
    let v = &args[*i];
    if v == "--" || v.starts_with("--") {
        return Err(format!(
            "{} requires a value (use {}=VALUE for values starting with --)",
            name, name
        ));
    }
    Ok(v.clone())
}

pub(crate) fn boolean(value: &str, name: &str) -> Result<bool, String> {
    match value {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(format!("{} expects 0 or 1", name)),
    }
}

pub(crate) fn once<'a>(seen: &mut HashSet<&'a str>, name: &'a str) -> Result<(), String> {
    if seen.insert(name) {
        Ok(())
    } else {
        Err(format!("{name} specified more than once"))
    }
}

pub(crate) fn free(s: &mut ParseState, f: &Flag, v: String) {
    match f.name {
        "--query" => s.query = Some(v),
        "--fix" => s.fix = v,
        "--embed-cmd" => s.embed_cmd = Some(v),
        "--embed-vec" => s.embed_vec = Some(v),
        "--embed-cache" => s.embed_cache = Some(v),
        "--revise" => s.revise = Some(v),
        "--retire" => s.retire = Some(v),
        "--retire-reason" => s.retire_reason = v,
        "--recall-kind" => s.recall_kind = Some(v),
        _ => s.mem_dir = v,
    }
}

pub(crate) fn path(s: &mut ParseState, f: &Flag, v: String) {
    match f.name {
        "--fix-file" => s.fix_file = Some(v),
        "--eval" => s.eval = Some(v),
        "--annotate-out" => s.annotate_out = Some(v),
        "--judge" => s.judge = Some(v),
        "--judge-by" => s.judge_by = Some(v),
        _ => s.delete = Some(v),
    }
}

pub(crate) fn output(s: &mut ParseState, name: &str) {
    match name {
        "--json" => s.json = true,
        "--stat" => s.stat = true,
        "--explain" => s.explain = true,
        _ => s.no_stats = true,
    }
}

pub(crate) fn house(s: &mut ParseState, name: &str) {
    match name {
        "--rebuild" => s.rebuild = true,
        _ => s.gc = true,
    }
}

pub(crate) fn evaluate(s: &mut ParseState, name: &str) {
    match name {
        "--annotate" => s.annotate = true,
        "--annotate-raw" => s.annotate_raw = true,
        "--gate" => s.eval_gate = true,
        "--gate-human" => s.eval_gate_human = true,
        _ => s.eval_gate_items = true,
    }
}

pub(crate) fn repeat(s: &mut ParseState, name: &str) -> Result<(), String> {
    match name {
        "--recall-only" => s.recall_only = true,
        "--recall-detail" => s.recall_detail = true,
        "--l1-hist" => s.l1_hist = true,
        "--stats" => s.stats = true,
        _ => {
            if s.eval.is_none() {
                return Err("--eval-verbose requires --eval <baseline.json>".to_string());
            }
            s.eval_verbose = true;
        }
    }
    Ok(())
}

pub(crate) fn flag(
    s: &mut ParseState,
    f: &Flag,
    inline: Option<String>,
    args: &[String],
    i: &mut usize,
) -> Result<(), String> {
    match f.kind {
        Kind::NoValue
        | Kind::Value
        | Kind::Force
        | Kind::RatePair
        | Kind::Evidence
        | Kind::Anchors
        | Kind::Redact
        | Kind::ContentType => raw(s, f, inline, args, i),
        Kind::F64 | Kind::I64 | Kind::U64 | Kind::Bool01 => numeric(s, f, inline, args, i),
        #[cfg(feature = "semantic")]
        Kind::Semantic => weights(s, f, inline, args, i),
        Kind::PolicyFile => policy(s, f, inline, args, i),
        Kind::Pipeline => pipeline(s, f, inline, args, i),
    }
}

pub(crate) fn raw(
    s: &mut ParseState,
    f: &Flag,
    inline: Option<String>,
    args: &[String],
    i: &mut usize,
) -> Result<(), String> {
    match f.kind {
        Kind::NoValue => none(s, f, inline),
        Kind::Value => word(s, f, inline, args, i),
        Kind::Force => force(s, f, inline, args, i),
        Kind::RatePair => pair(s, f, inline, args, i),
        Kind::Evidence => evidence(s, f, inline, args, i),
        Kind::Anchors => anchors(s, f, inline, args, i),
        Kind::Redact => redact(s, f, inline, args, i),
        Kind::ContentType => content_kind(s, f, inline, args, i),
        _ => Err(format!("{} 缺少值处理器", f.name)),
    }
}

pub(crate) fn numeric(
    s: &mut ParseState,
    f: &Flag,
    inline: Option<String>,
    args: &[String],
    i: &mut usize,
) -> Result<(), String> {
    match f.kind {
        Kind::F64 => float(s, f, inline, args, i),
        Kind::I64 => signed(s, f, inline, args, i),
        Kind::U64 => count(s, f, inline, args, i),
        Kind::Bool01 => either(s, f, inline, args, i),
        _ => Err(format!("{} 缺少数值处理器", f.name)),
    }
}

pub(crate) fn none(s: &mut ParseState, f: &Flag, inline: Option<String>) -> Result<(), String> {
    if inline.is_some() {
        return Err(format!("{} does not take a value", f.name));
    }
    match f.name {
        "--json" | "--stat" | "--no-stats" | "--explain" => output(s, f.name),
        "--rebuild" | "--gc" => house(s, f.name),
        "--annotate" | "--annotate-raw" | "--gate" | "--gate-human" | "--gate-items" => {
            evaluate(s, f.name)
        }
        _ => repeat(s, f.name)?,
    }
    Ok(())
}

pub(crate) fn word(
    s: &mut ParseState,
    f: &Flag,
    inline: Option<String>,
    args: &[String],
    i: &mut usize,
) -> Result<(), String> {
    if f.once {
        once(&mut s.seen, f.name)?;
    }
    let v = value(args, i, f.name, inline)?;
    if !f.err_empty.is_empty() && v.is_empty() {
        return Err(format!("{} {}", f.name, f.err_empty));
    }
    match f.name {
        "--query" | "--fix" | "--mem-dir" | "--embed-cmd" | "--embed-vec" | "--embed-cache"
        | "--revise" | "--retire" | "--retire-reason" | "--recall-kind" => free(s, f, v),
        #[cfg(feature = "semantic")]
        "--sem-backend" => {
            s.config.sem_backend = crate::sem::Backend::parse(&v)?;
        }
        _ => path(s, f, v),
    }
    Ok(())
}

pub(crate) fn float(
    s: &mut ParseState,
    f: &Flag,
    inline: Option<String>,
    args: &[String],
    i: &mut usize,
) -> Result<(), String> {
    once(&mut s.seen, f.name)?;
    let v = value(args, i, f.name, inline)?;
    let parsed: f64 = v
        .parse()
        .map_err(|_| format!("{} expects {}, got {:?}", f.name, f.what, v))?;
    if !parsed.is_finite() {
        return Err(format!("{} expects {}, got {:?}", f.name, f.what, v));
    }
    match f.name {
        "--ratio" => {
            if !(Config::MIN_RATIO..=1.0).contains(&parsed) {
                return Err(format!(
                    "{} must be a finite number in {}..=1.0",
                    f.name,
                    Config::MIN_RATIO
                ));
            }
            s.config.ratio = parsed;
        }
        "--budget" => {
            if parsed <= 0.0 || parsed > 1.0 {
                return Err(format!("{} must be a finite number in (0, 1]", f.name));
            }
            s.config.budget = parsed;
        }
        "--sim-weight" => {
            if parsed < 0.0 {
                return Err(format!("{} must be a finite number >= 0", f.name));
            }
            s.config.sim_weight = parsed;
        }
        "--sem-weight" => {
            if parsed < 0.0 {
                return Err(format!("{} must be a finite number >= 0", f.name));
            }
            s.config.sem_weight = parsed;
        }
        "--sem-gate" => {
            if parsed < 0.0 {
                return Err(format!("{} must be a finite number >= 0", f.name));
            }
            s.config.sem_gate = parsed;
        }
        "--minhash-weight" => {
            if parsed < 0.0 {
                return Err(format!("{} must be a finite number >= 0", f.name));
            }
            s.minhash_weight = parsed;
        }
        "--fill-floor" => {
            if !(0.0..=crate::policy::MAX_FILL_FLOOR).contains(&parsed) {
                return Err(format!(
                    "{} must be a finite number in 0..={}",
                    f.name,
                    crate::policy::MAX_FILL_FLOOR
                ));
            }
            s.config.fill_floor = parsed;
        }
        "--trim-ratio" => {
            if !(crate::policy::MIN_TRIM_RATIO..=crate::policy::MAX_TRIM_RATIO).contains(&parsed) {
                return Err(format!(
                    "{} must be a finite number in {}..={}",
                    f.name,
                    crate::policy::MIN_TRIM_RATIO,
                    crate::policy::MAX_TRIM_RATIO
                ));
            }
            s.config.trim_ratio = parsed;
        }
        _ => return Err(format!("{} 缺少 F64 分支", f.name)),
    }
    Ok(())
}

pub(crate) fn signed(
    s: &mut ParseState,
    f: &Flag,
    inline: Option<String>,
    args: &[String],
    i: &mut usize,
) -> Result<(), String> {
    once(&mut s.seen, f.name)?;
    let v = value(args, i, f.name, inline)?;
    let parsed: i64 = v
        .parse()
        .map_err(|_| format!("{} expects {}, got {:?}", f.name, f.what, v))?;
    let max = if f.name == "--window" {
        Config::MAX_WINDOW
    } else {
        Config::MAX_CONTEXT
    };
    if !(0..=max).contains(&parsed) {
        return Err(format!("{} must be in 0..={}", f.name, max));
    }
    if f.name == "--window" {
        s.config.window = parsed.max(Config::MIN_WINDOW);
    } else {
        s.config.context = parsed;
    }
    Ok(())
}

pub(crate) fn count(
    s: &mut ParseState,
    f: &Flag,
    inline: Option<String>,
    args: &[String],
    i: &mut usize,
) -> Result<(), String> {
    once(&mut s.seen, f.name)?;
    let v = value(args, i, f.name, inline)?;
    let parsed: u64 = v
        .parse()
        .map_err(|_| format!("{} expects {}, got {:?}", f.name, f.what, v))?;
    if parsed < f.min {
        return Err(format!("{} {}", f.name, f.err_zero));
    }
    unsigned(s, f, parsed)
}

pub(crate) fn unsigned(s: &mut ParseState, f: &Flag, parsed: u64) -> Result<(), String> {
    match f.name {
        "--seed" => s.seed = parsed,
        "--annotate-max" => s.annotate_max = parsed as usize,
        "--recall" => s.recall = parsed as usize,
        "--max-bytes" => s.max_bytes = Some(parsed),
        "--max-tokens" => s.limit = Some(parsed),
        "--max-mem" => s.config.max_mem = parsed as usize,
        _ => return Err(format!("{} 缺少 U64 分支", f.name)),
    }
    Ok(())
}

#[cfg(feature = "semantic")]
pub(crate) fn weights(
    s: &mut ParseState,
    f: &Flag,
    inline: Option<String>,
    args: &[String],
    i: &mut usize,
) -> Result<(), String> {
    once(&mut s.seen, f.name)?;
    let v = value(args, i, f.name, inline)?;
    let w = crate::semantic::Weights::load(&v).map_err(|e| format!("{}: {e}", f.name))?;
    s.config.semantic = Some(w);
    Ok(())
}

pub(crate) fn policy(
    s: &mut ParseState,
    f: &Flag,
    inline: Option<String>,
    args: &[String],
    i: &mut usize,
) -> Result<(), String> {
    once(&mut s.seen, f.name)?;
    let path = value(args, i, f.name, inline)?;
    let body = std::fs::read_to_string(&path)
        .map_err(|e| format!("{}: cannot read {}: {}", f.name, path, e))?;
    let entries = crate::toml::numbered(body.lines().enumerate().map(|(n, l)| (n + 1, l)));
    for le in entries {
        if !le.entry.section.is_empty() {
            return Err(format!(
                "{}: {}:{}: section [{}] not supported",
                f.name, path, le.line, le.entry.section
            ));
        }
        let key = le.entry.key;
        let v = le.entry.value;
        let Some(flag) = lookup(key) else {
            let allowed = POLICY_TABLE
                .iter()
                .map(|(k, _)| *k)
                .collect::<Vec<_>>()
                .join("/");
            return Err(format!(
                "{}: {}:{}: unknown policy key `{}` (allowed: {})",
                f.name, path, le.line, key, allowed
            ));
        };
        // CLI 显式参数优先：文件只填 DEFAULT 层（CLI 未指定的字段）。
        if s.seen.contains(flag) {
            continue;
        }
        apply(s, key, v, &path, le.line)?;
        // 记录来源：后续参数（如 --kind 的类型默认 ratio）不得覆盖文件值。
        s.sourced.insert(flag);
    }
    Ok(())
}

pub(crate) fn pipeline(
    s: &mut ParseState,
    f: &Flag,
    inline: Option<String>,
    args: &[String],
    i: &mut usize,
) -> Result<(), String> {
    once(&mut s.seen, f.name)?;
    let path = value(args, i, f.name, inline)?;
    let p = if std::path::Path::new(&path).is_file() {
        crate::pipeline::Pipeline::load(&path)
    } else {
        crate::pipeline::Pipeline::load_profile(&path)
    }
    .map_err(|e| format!("{}: {e}", f.name))?;
    // CLI 显式参数优先：文件只填 DEFAULT 层（CLI 未指定的字段）。
    let applied = p.apply(&mut s.config, &|flag| {
        s.seen.contains(flag) || s.sourced.contains(flag)
    });
    for flag in applied {
        s.sourced.insert(flag);
    }
    Ok(())
}

pub(crate) const POLICY_TABLE: &[(&str, &str)] = &[
    ("ratio", "--ratio"),
    ("budget", "--budget"),
    ("context", "--context"),
    ("window", "--window"),
    ("elide", "--elide"),
    ("risk_gate", "--risk-gate"),
    ("scoped", "--scoped"),
    ("strict", "--strict"),
    ("lineno", "--lineno"),
    ("max_tokens", "--max-tokens"),
    ("structure", "--structure"),
    ("fill_floor", "--fill-floor"),
    ("trim", "--trim"),
    ("trim_ratio", "--trim-ratio"),
    ("trim_op_keep", "--trim-op-keep"),
    ("code_keep", "--code-keep"),
    ("zh_keep", "--zh-keep"),
    ("num_line_keep", "--num-line-keep"),
    ("sym_line_keep", "--sym-line-keep"),
    ("list_keep", "--list-keep"),
];

pub(crate) fn lookup(key: &str) -> Option<&'static str> {
    POLICY_TABLE
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, flag)| *flag)
}

pub(crate) fn apply(
    s: &mut ParseState,
    key: &str,
    v: &str,
    path: &str,
    line: usize,
) -> Result<(), String> {
    match key {
        "ratio" | "context" | "window" => number(s, key, v, path, line)?,
        "elide" | "risk_gate" | "scoped" | "strict" | "lineno" | "structure" | "trim"
        | "trim_op_keep" | "code_keep" | "zh_keep" | "num_line_keep" | "sym_line_keep"
        | "list_keep" => toggle(s, key, v, path, line)?,
        "max_tokens" => tokens(s, v, path, line)?,
        "fill_floor" => fill_floor(s, v, path, line)?,
        "trim_ratio" => trim_ratio_key(s, v, path, line)?,
        _ => return Err(error(path, line, format!("unknown policy key {key:?}"))),
    }
    Ok(())
}

pub(crate) fn number(
    s: &mut ParseState,
    key: &str,
    v: &str,
    path: &str,
    line: usize,
) -> Result<(), String> {
    match key {
        "ratio" => ratio(s, v, path, line),
        "context" => context(s, v, path, line),
        "window" => window(s, v, path, line),
        _ => Err(error(path, line, format!("{key:?} 不是数值型策略键"))),
    }
}

pub(crate) fn error(path: &str, line: usize, msg: String) -> String {
    format!("--policy-file: {path}:{line}: {msg}")
}

pub(crate) fn ratio(s: &mut ParseState, v: &str, path: &str, line: usize) -> Result<(), String> {
    let r: f64 = crate::toml::float(v)
        .ok_or_else(|| error(path, line, format!("ratio must be a finite f64, got {v:?}")))?;
    if !(crate::policy::MIN_RATIO..=1.0).contains(&r) {
        return Err(error(
            path,
            line,
            format!("ratio must be in {}..=1.0", crate::policy::MIN_RATIO),
        ));
    }
    s.config.ratio = r;
    Ok(())
}

pub(crate) fn context(s: &mut ParseState, v: &str, path: &str, line: usize) -> Result<(), String> {
    let c = range(v, 0, crate::policy::MAX_CONTEXT).ok_or_else(|| {
        error(
            path,
            line,
            format!("context must be 0..={}", crate::policy::MAX_CONTEXT),
        )
    })?;
    s.config.context = c;
    Ok(())
}

pub(crate) fn window(s: &mut ParseState, v: &str, path: &str, line: usize) -> Result<(), String> {
    // 与 CLI 同口径：文档契约为 0..=8，0 归一化为 1（此前策略文件拒绝 0，
    // 与 `--window 0` 行为不一致）。
    let w = range(v, 0, crate::policy::MAX_WINDOW).ok_or_else(|| {
        error(
            path,
            line,
            format!("window must be 0..={}", crate::policy::MAX_WINDOW),
        )
    })?;
    s.config.window = w.max(crate::policy::MIN_WINDOW);
    Ok(())
}

pub(crate) fn toggle(
    s: &mut ParseState,
    key: &str,
    v: &str,
    path: &str,
    line: usize,
) -> Result<(), String> {
    let b = zero(v).ok_or_else(|| error(path, line, format!("{key} must be 0/1")))?;
    match key {
        "elide" => s.config.elide = b,
        "risk_gate" => s.config.gate = b,
        "scoped" => s.config.scoped = b,
        "strict" => s.config.strict = b,
        "lineno" => s.config.lineno = b,
        "structure" => s.config.structure = b,
        "trim" => s.config.trim = b,
        "trim_op_keep" => s.config.trim_op_keep = b,
        "code_keep" => s.config.code_keep = b,
        "zh_keep" => s.config.zh_keep = b,
        "num_line_keep" => s.config.num_line_keep = b,
        "sym_line_keep" => s.config.sym_line_keep = b,
        "list_keep" => s.config.list_keep = b,
        _ => return Err(error(path, line, format!("{key:?} 不是布尔型策略键"))),
    }
    Ok(())
}

pub(crate) fn tokens(s: &mut ParseState, v: &str, path: &str, line: usize) -> Result<(), String> {
    let m: usize = crate::toml::size(v).ok_or_else(|| {
        error(
            path,
            line,
            format!("max_tokens must be a non-negative integer, got {v:?}"),
        )
    })?;
    s.config.limit = Some(m);
    Ok(())
}

pub(crate) fn fill_floor(
    s: &mut ParseState,
    v: &str,
    path: &str,
    line: usize,
) -> Result<(), String> {
    let f: f64 = crate::toml::float(v).ok_or_else(|| {
        error(
            path,
            line,
            format!("fill_floor must be a finite f64, got {v:?}"),
        )
    })?;
    if !(0.0..=crate::policy::MAX_FILL_FLOOR).contains(&f) {
        return Err(error(
            path,
            line,
            format!(
                "fill_floor must be in 0..={}",
                crate::policy::MAX_FILL_FLOOR
            ),
        ));
    }
    s.config.fill_floor = f;
    Ok(())
}

pub(crate) fn trim_ratio_key(
    s: &mut ParseState,
    v: &str,
    path: &str,
    line: usize,
) -> Result<(), String> {
    let f: f64 = crate::toml::float(v).ok_or_else(|| {
        error(
            path,
            line,
            format!("trim_ratio must be a finite f64, got {v:?}"),
        )
    })?;
    if !(crate::policy::MIN_TRIM_RATIO..=crate::policy::MAX_TRIM_RATIO).contains(&f) {
        return Err(error(
            path,
            line,
            format!(
                "trim_ratio must be in {}..={}",
                crate::policy::MIN_TRIM_RATIO,
                crate::policy::MAX_TRIM_RATIO
            ),
        ));
    }
    s.config.trim_ratio = f;
    Ok(())
}

pub(crate) fn zero(v: &str) -> Option<bool> {
    match v {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}

pub(crate) fn range(v: &str, lo: i64, hi: i64) -> Option<i64> {
    let n: i64 = v.parse().ok()?;
    (lo..=hi).contains(&n).then_some(n)
}

pub(crate) fn either(
    s: &mut ParseState,
    f: &Flag,
    inline: Option<String>,
    args: &[String],
    i: &mut usize,
) -> Result<(), String> {
    once(&mut s.seen, f.name)?;
    let v = value(args, i, f.name, inline)?;
    let b = boolean(&v, f.name)?;
    match f.name {
        "--elide" => s.config.elide = b,
        "--risk-gate" => s.config.gate = b,
        "--scoped" => s.config.scoped = b,
        "--strict" => s.config.strict = b,
        "--lineno" => s.config.lineno = b,
        "--structure" => s.config.structure = b,
        "--trim" => s.config.trim = b,
        "--fold" => s.config.fold = b,
        "--entropy" => s.config.entropy = b,
        "--auto-anchor" => s.config.auto_anchor = b,
        "--cluster" => s.config.cluster = b,
        "--trim-op-keep" => s.config.trim_op_keep = b,
        "--code-keep" => s.config.code_keep = b,
        "--zh-keep" => s.config.zh_keep = b,
        "--num-line-keep" => s.config.num_line_keep = b,
        "--sym-line-keep" => s.config.sym_line_keep = b,
        "--list-keep" => s.config.list_keep = b,
        _ => s.store = b,
    }
    Ok(())
}

pub(crate) const PRESERVE_PATTERNS: &[(&str, &str)] = &[
    ("url", "re://"), // 任意协议 ://（http/https/postgres/mysql/redis/...）
    ("ip", "re:[0-9]+.[0-9]+.[0-9]+.[0-9]+"), // IPv4 点分十进制
    ("email", "re:[a-z0-9._%+-]+@"), // email local part + @
    ("errno", "re:errno[= ]*[0-9]+"), // errno=28 / errno 28
];

pub(crate) fn preserve_into(config: &mut Config, ty: &str) -> Result<(), String> {
    let ty = ty.trim();
    if ty.is_empty() {
        return Err("preserve requires a type (url/ip/email/errno)".to_string());
    }
    let pat = PRESERVE_PATTERNS
        .iter()
        .find(|(name, _)| *name == ty)
        .map(|(_, p)| *p)
        .ok_or_else(|| {
            let allowed: Vec<&str> = PRESERVE_PATTERNS.iter().map(|(n, _)| *n).collect();
            format!(
                "preserve unknown type {ty:?} (allowed: {})",
                allowed.join("/")
            )
        })?;
    config.forces.push(pat.to_string());
    Ok(())
}

pub(crate) fn force(
    s: &mut ParseState,
    f: &Flag,
    inline: Option<String>,
    args: &[String],
    i: &mut usize,
) -> Result<(), String> {
    let v = value(args, i, f.name, inline)?;
    if f.name == "--preserve" {
        preserve_into(&mut s.config, &v).map_err(|e| format!("{} {}", f.name, e))?;
        return Ok(());
    }
    let trimmed = v.trim();
    // 引擎（compress::forces）先整体小写再识别 `re:` 前缀，此处必须同口径校验，
    // 否则 `RE:[` 之类会绕过前置校验，在引擎侧触发断言/静默丢弃。
    if let Some(rest) = trimmed.to_lowercase().strip_prefix("re:") {
        // `re:` 前缀：整个值是单个正则，`|` 是正则交替不拆分。CLI 层前置
        // 校验手写正则子集，非法即报错拒绝运行。
        if rest.is_empty() {
            return Err(format!("{} {}", f.name, f.err_empty));
        }
        crate::regex::parse(rest).map_err(|e| format!("{}: {e}", f.name))?;
    } else if v.split('|').all(|p| p.trim().is_empty()) {
        return Err(format!("{} {}", f.name, f.err_empty));
    }
    s.config.forces.push(v);
    Ok(())
}

pub(crate) fn evidence(
    s: &mut ParseState,
    f: &Flag,
    inline: Option<String>,
    args: &[String],
    i: &mut usize,
) -> Result<(), String> {
    let v = value(args, i, f.name, inline)?;
    let trimmed = v.trim();
    if trimmed.is_empty() {
        return Err(format!("{} {}", f.name, f.err_empty));
    }
    let ev = match trimmed.parse::<usize>() {
        Ok(n) if n >= 1 => crate::Evidence::Line(n),
        _ => crate::Evidence::Substr(trimmed.to_string()),
    };
    s.config.evidence.push(ev);
    Ok(())
}

pub(crate) fn anchors(
    s: &mut ParseState,
    f: &Flag,
    inline: Option<String>,
    args: &[String],
    i: &mut usize,
) -> Result<(), String> {
    let v = value(args, i, f.name, inline)?;
    for part in v.split(',') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        match crate::AnchorKind::parse(p) {
            Some(k) if !s.config.structure_anchors.contains(&k) => {
                s.config.structure_anchors.push(k);
            }
            Some(_) => {}
            None => {
                return Err(format!(
                    "{} unknown anchor type {p:?} (allowed: json/xml/idoc/yaml/md)",
                    f.name
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn redact(
    s: &mut ParseState,
    f: &Flag,
    inline: Option<String>,
    args: &[String],
    i: &mut usize,
) -> Result<(), String> {
    let v = value(args, i, f.name, inline)?;
    for part in v.split(',') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        match crate::redact::RedactKind::parse(p) {
            Some(k) if !s.redact.contains(&k) => {
                s.redact.push(k);
            }
            Some(_) => {}
            None => {
                return Err(format!(
                    "{} unknown redact kind {p:?} (allowed: secret/id-card/phone/email)",
                    f.name
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn content_kind(
    s: &mut ParseState,
    f: &Flag,
    inline: Option<String>,
    args: &[String],
    i: &mut usize,
) -> Result<(), String> {
    once(&mut s.seen, f.name)?;
    let v = value(args, i, f.name, inline)?;
    let v = v.trim();
    if v == "auto" {
        s.kind_auto = true;
        return Ok(());
    }
    let kind: crate::ContentType = v.parse().map_err(|_| {
        format!(
            "{} expects static-contract/system-definition/session-message/tool-output/auto, got {v:?}",
            f.name
        )
    })?;
    s.config.kind = kind;
    // 优先级 CLI > 策略文件 > 类型默认：`--ratio` 无论来自 CLI 还是文件，
    // 都不得被类型默认值覆盖（此前文件值在 --kind 之后出现时会被静默覆盖）。
    if !s.seen.contains("--ratio") && !s.sourced.contains("--ratio") {
        s.config.ratio = kind.default_ratio();
    }
    Ok(())
}

pub(crate) fn pair(
    s: &mut ParseState,
    f: &Flag,
    inline: Option<String>,
    args: &[String],
    i: &mut usize,
) -> Result<(), String> {
    once(&mut s.seen, f.name)?;
    let v = value(args, i, f.name, inline)?;
    let good = match v.as_str() {
        "good" => true,
        "bad" => false,
        _ => return Err("--rate expects good or bad".to_string()),
    };
    let id = value(args, i, f.name, None)?;
    if id.is_empty() {
        return Err("--rate requires a case-id".to_string());
    }
    s.rate = Some((good, id));
    Ok(())
}

pub(crate) fn idle(s: &ParseState) -> bool {
    s.rate.is_none()
        && !s.rebuild
        && !s.gc
        && s.delete.is_none()
        && s.retire.is_none()
        && s.eval.is_none()
        && !s.l1_hist
        && !s.stats
        && s.judge.is_none()
}

pub(crate) fn exempt(s: &ParseState) -> bool {
    !s.annotate && !s.annotate_raw
}

pub(crate) fn query(s: &ParseState) -> Result<(), String> {
    let query_empty = s.query.as_deref().unwrap_or("").is_empty();
    if idle(s) && exempt(s) && query_empty && s.config.forces.is_empty() {
        return Err("missing --query (or provide at least one --force pattern)".to_string());
    }
    if s.recall_only && query_empty {
        return Err("--recall-only requires --query".to_string());
    }
    Ok(())
}

pub(crate) fn judge(s: &ParseState) -> Result<(), String> {
    if s.judge.is_some() && (s.annotate || s.annotate_raw || s.eval.is_some()) {
        return Err("--judge cannot be combined with --annotate/--annotate-raw/--eval".to_string());
    }
    if s.judge_by.is_some() && s.judge.is_none() {
        return Err("--judge-by requires --judge".to_string());
    }
    Ok(())
}

pub(crate) fn annotation(s: &ParseState) -> Result<(), String> {
    if conflict(s) {
        return Err(
            "--annotate/--annotate-raw cannot be combined with --eval/--l1-hist".to_string(),
        );
    }
    if s.annotate && s.annotate_raw {
        return Err("--annotate and --annotate-raw are mutually exclusive".to_string());
    }
    if missing(s) {
        return Err("--annotate-out/--seed/--annotate-max require --annotate".to_string());
    }
    Ok(())
}

pub(crate) fn conflict(s: &ParseState) -> bool {
    (s.annotate || s.annotate_raw) && (s.eval.is_some() || s.l1_hist)
}

pub(crate) fn missing(s: &ParseState) -> bool {
    (s.annotate_out.is_some() || s.seen.contains("--seed") || s.seen.contains("--annotate-max"))
        && !(s.annotate || s.annotate_raw)
}

pub(crate) fn actions(s: &ParseState) -> Result<(), String> {
    let actions = [
        s.rate.is_some(),
        s.rebuild,
        s.gc,
        s.delete.is_some(),
        s.retire.is_some(),
        s.eval.is_some(),
        s.l1_hist,
        s.stats,
        s.annotate || s.annotate_raw,
        s.judge.is_some(),
        s.recall_only,
    ];
    if actions.iter().filter(|active| **active).count() > 1 {
        return Err("action options are mutually exclusive; choose one".to_string());
    }
    Ok(())
}

pub(crate) fn gates(s: &ParseState) -> Result<(), String> {
    let gate_count = [s.eval_gate, s.eval_gate_human, s.eval_gate_items]
        .iter()
        .filter(|v| **v)
        .count();
    if gate_count > 0 && s.eval.is_none() {
        return Err("--gate/--gate-human/--gate-items requires --eval <baseline.json>".to_string());
    }
    if gate_count > 1 {
        return Err("--gate, --gate-human and --gate-items are mutually exclusive".to_string());
    }
    Ok(())
}

pub(crate) fn store(s: &ParseState) -> Result<(), String> {
    if fix(s) {
        return Err("--fix requires --store 1".to_string());
    }
    if file(s) {
        return Err("--fix-file requires --store 1".to_string());
    }
    if bytes(s) {
        return Err("--max-bytes requires --store 1".to_string());
    }
    if clash(s) {
        return Err("--fix and --fix-file are mutually exclusive".to_string());
    }
    if revise(s) {
        return Err("--revise requires --store 1".to_string());
    }
    Ok(())
}

pub(crate) fn revise(s: &ParseState) -> bool {
    s.revise.is_some() && !s.store
}

pub(crate) fn fix(s: &ParseState) -> bool {
    s.seen.contains("--fix") && !s.store
}

pub(crate) fn file(s: &ParseState) -> bool {
    s.fix_file.is_some() && !s.store
}

pub(crate) fn bytes(s: &ParseState) -> bool {
    s.max_bytes.is_some() && !s.store
}

pub(crate) fn clash(s: &ParseState) -> bool {
    s.seen.contains("--fix") && s.fix_file.is_some()
}

pub(crate) fn recall(s: &ParseState) -> Result<(), String> {
    if s.recall_only && s.store {
        return Err("--recall-only cannot be combined with --store 1".to_string());
    }
    if s.recall_detail && s.recall == 0 && !s.recall_only {
        return Err("--recall-detail requires --recall or --recall-only".to_string());
    }
    Ok(())
}

pub(crate) fn check(s: &ParseState) -> Result<(), String> {
    if stat(s) {
        return Err("--stat and --no-stats cannot be used together".to_string());
    }
    type Check = fn(&ParseState) -> Result<(), String>;
    pub(crate) const CHECKS: [Check; 7] = [query, judge, annotation, actions, gates, store, recall];
    for check in CHECKS {
        check(s)?;
    }
    Ok(())
}

pub(crate) fn stat(s: &ParseState) -> bool {
    s.stat && s.no_stats
}

pub(crate) fn finish(s: ParseState) -> Opts {
    let recall = if s.recall_only && !s.seen.contains("--recall") {
        3 // recall-only 未指定 K 时默认 top-3（与主流程 --recall 语义一致）
    } else {
        s.recall
    };
    let mut config = s.config;
    // 仅 CLI 显式 `--max-tokens` 覆盖（优先级 CLI > 文件 > DEFAULT）；
    // 未显式时保留 `--policy-file` 写入的值，避免 None 覆盖文件设置。
    if s.seen.contains("--max-tokens") {
        config.limit = s.limit.map(|v| v as usize);
    }
    Opts {
        query: s.query.unwrap_or_default(),
        config,
        json: s.json,
        stat: s.stat,
        no_stats: s.no_stats,
        explain: s.explain,
        store: s.store,
        fix: s.fix,
        fix_file: s.fix_file,
        recall,
        recall_only: s.recall_only,
        recall_detail: s.recall_detail,
        recall_kind: s.recall_kind.clone(),
        rate: s.rate,
        rebuild: s.rebuild,
        gc: s.gc,
        delete: s.delete,
        revise: s.revise,
        retire: s.retire,
        retire_reason: s.retire_reason,
        eval: s.eval,
        eval_verbose: s.eval_verbose,
        eval_gate: s.eval_gate,
        eval_gate_human: s.eval_gate_human,
        eval_gate_items: s.eval_gate_items,
        l1_hist: s.l1_hist,
        stats: s.stats,
        annotate: s.annotate,
        annotate_raw: s.annotate_raw,
        annotate_out: s.annotate_out,
        seed: s.seed,
        annotate_max: s.annotate_max,
        judge: s.judge,
        judge_by: s.judge_by,
        max_bytes: s.max_bytes,
        mem_dir: s.mem_dir,
        files: s.files,
        embed_cmd: s.embed_cmd,
        embed_vec: s.embed_vec,
        embed_cache: s.embed_cache,
        minhash_weight: s.minhash_weight,
        redact: s.redact,
        kind_auto: s.kind_auto,
        ratio_explicit: s.seen.contains("--ratio"),
    }
}

pub(crate) fn all(args: Vec<String>) -> Result<Opts, String> {
    let mut s = ParseState::new();
    let mut i = 0usize;
    while i < args.len() {
        if args[i] == "--" {
            s.files.extend(args[i + 1..].iter().cloned());
            break;
        }
        one(&mut s, &args, &mut i)?;
        i += 1;
    }
    check(&s)?;
    Ok(finish(s))
}

pub(crate) fn one(s: &mut ParseState, args: &[String], i: &mut usize) -> Result<(), String> {
    let raw = &args[*i];
    let (name, inline) = split(raw);
    match find(name) {
        Some(f) => flag(s, f, inline, args, i),
        None if option(raw) => Err(format!("unknown option: {}", raw)),
        None => {
            s.files.push(raw.clone());
            Ok(())
        }
    }
}

pub(crate) fn split(raw: &str) -> (&str, Option<String>) {
    match raw.split_once('=') {
        Some((n, v)) if n.starts_with("--") => (n, Some(v.to_string())),
        _ => (raw, None),
    }
}

pub(crate) fn option(raw: &str) -> bool {
    raw.starts_with("--") || (raw.starts_with('-') && raw.len() > 1)
}
