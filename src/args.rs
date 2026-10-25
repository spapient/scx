//! CLI 参数解析：声明式参数表驱动 parse + 校验；帮助/版本短路（帮助打印在 main）。
//! 只做解析与校验，不含 I/O 与引擎逻辑。
//!
//! 每个 flag 在 `FLAGS` 表中声明一次（名称 / 值种类 / 去重 / 错误消息 / 帮助文本），
//! 解析循环查表后按 `Kind` 统一分派，不再为每个 flag 写独立 match 分支；
//! 语义特例（`--window` 归一化、`--eval-verbose` 前置依赖）集中在少数函数内。
//! `usage()` 保持显式 format：其文本含 Config 默认值插值与合并项
//! （`[--gate|--gate-human|--gate-items]`），为保证逐字节一致不改为表生成。

use scx::Config;
use std::collections::HashSet;

/// 帮助文本中的默认值直接取自引擎配置，避免 CLI/文档/引擎三处漂移。
pub(crate) fn usage() -> String {
    format!(
        "usage: scx --query \"...\" [--ratio {}] [--window {}] [--context {}] [--elide 1] [--risk-gate 1] [--lineno 1] [--store 1] [--fix \"修复结论\"] [--fix-file <path>] [--recall K] [--recall-only] [--recall-detail] [--rate good|bad <case-id>] [--rebuild] [--gc] [--delete <case-id>] [--eval <baseline.json>] [--eval-verbose] [--gate|--gate-human|--gate-items] [--l1-hist] [--stats] [--annotate] [--annotate-raw] [--annotate-out <path>] [--seed N] [--annotate-max N] [--judge <baseline.json>] [--judge-by <name>] [--mem-dir DIR] [--force SUBSTR]... [--json] [--stat|--no-stats] [file...]",
        Config::DEFAULT_RATIO,
        Config::DEFAULT_WINDOW,
        Config::DEFAULT_CONTEXT
    )
}

pub(crate) struct Opts {
    pub query: String,
    /// 压缩引擎配置的唯一来源；CLI 默认值来自 `Config::default()`，不再重复硬编码。
    pub config: Config,
    pub json: bool,
    pub stat: bool,
    pub no_stats: bool,
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
    /// --annotate-out <path>：标注集输出路径（缺省 `<mem-dir>/baseline-auto.json`）。
    pub annotate_out: Option<String>,
    /// --seed N：标注生成确定性种子（缺省 0，同 seed 可重放）。
    pub seed: u64,
    /// --annotate-max N：标注覆盖的最大 case 数（缺省 16；0 = 全库）。
    /// 证据门 30–50 组：case 数 × 三层产率约 1.7，24+ case 库可直接落窗。
    pub annotate_max: usize,
    /// `--judge` 逐条读取 2/1/0/s，把 `judge:null` 替换为完整裁决元数据。
    pub judge: Option<String>,
    /// --judge-by <name>：裁决人标识（缺省取 USERNAME）。
    pub judge_by: Option<String>,
    /// 记忆库总容量软配额（字节）；写入前超限则拒绝。
    pub max_bytes: Option<u64>,
    pub mem_dir: String,
    pub files: Vec<String>,
}

/// 默认记忆库目录：$SCX_MEM_DIR 优先，否则用户主目录下的 .scx-memory。
fn default_mem_dir() -> String {
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

fn option_value(
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

fn parse_bool01(value: &str, name: &str) -> Result<bool, String> {
    match value {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(format!("{} expects 0 or 1", name)),
    }
}

fn once<'a>(seen: &mut HashSet<&'a str>, name: &'a str) -> Result<(), String> {
    if seen.insert(name) {
        Ok(())
    } else {
        Err(format!("{name} specified more than once"))
    }
}

/// 参数值种类：决定解析循环按哪种方式读取并校验值。
#[derive(Clone, Copy, PartialEq)]
enum Kind {
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
}

/// 单 flag 声明：解析循环的唯一样板；新增 flag 只改表，不新增分支。
struct Flag {
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
    /// 帮助文本片段（供未来详细 `--help` 列表；`usage()` 仍为显式 format）。
    help: &'static str,
}

/// 参数声明表：parse 与校验的唯一事实源。
static FLAGS: &[Flag] = &[
    Flag { name: "--query", once: true, kind: Kind::Value, what: "", err_empty: "", err_zero: "", min: 0, help: "[--query \"...\"]" },
    Flag { name: "--ratio", once: true, kind: Kind::F64, what: "a number", err_empty: "", err_zero: "", min: 0, help: "[--ratio {ratio}]" },
    Flag { name: "--context", once: true, kind: Kind::I64, what: "an integer", err_empty: "", err_zero: "", min: 0, help: "[--context {context}]" },
    Flag { name: "--window", once: true, kind: Kind::I64, what: "an integer", err_empty: "", err_zero: "", min: 0, help: "[--window {window}]" },
    Flag { name: "--elide", once: true, kind: Kind::Bool01, what: "", err_empty: "", err_zero: "", min: 0, help: "[--elide 1]" },
    Flag { name: "--risk-gate", once: true, kind: Kind::Bool01, what: "", err_empty: "", err_zero: "", min: 0, help: "[--risk-gate 1]" },
    Flag { name: "--lineno", once: true, kind: Kind::Bool01, what: "", err_empty: "", err_zero: "", min: 0, help: "[--lineno 1]" },
    Flag { name: "--force", once: false, kind: Kind::Force, what: "", err_empty: "requires a non-empty pattern", err_zero: "", min: 0, help: "[--force SUBSTR]..." },
    Flag { name: "--store", once: true, kind: Kind::Bool01, what: "", err_empty: "", err_zero: "", min: 0, help: "[--store 1]" },
    Flag { name: "--fix", once: true, kind: Kind::Value, what: "", err_empty: "requires non-empty text", err_zero: "", min: 0, help: "[--fix \"修复结论\"]" },
    Flag { name: "--fix-file", once: true, kind: Kind::Value, what: "", err_empty: "requires a path", err_zero: "", min: 0, help: "[--fix-file <path>]" },
    Flag { name: "--recall", once: true, kind: Kind::U64, what: "a positive integer", err_empty: "", err_zero: "expects a positive integer", min: 1, help: "[--recall K]" },
    Flag { name: "--recall-only", once: false, kind: Kind::NoValue, what: "", err_empty: "", err_zero: "", min: 0, help: "[--recall-only]" },
    Flag { name: "--recall-detail", once: false, kind: Kind::NoValue, what: "", err_empty: "", err_zero: "", min: 0, help: "[--recall-detail]" },
    Flag { name: "--rate", once: true, kind: Kind::RatePair, what: "", err_empty: "", err_zero: "", min: 0, help: "[--rate good|bad <case-id>]" },
    Flag { name: "--rebuild", once: false, kind: Kind::NoValue, what: "", err_empty: "", err_zero: "", min: 0, help: "[--rebuild]" },
    Flag { name: "--gc", once: false, kind: Kind::NoValue, what: "", err_empty: "", err_zero: "", min: 0, help: "[--gc]" },
    Flag { name: "--delete", once: true, kind: Kind::Value, what: "", err_empty: "requires a case-id", err_zero: "", min: 0, help: "[--delete <case-id>]" },
    Flag { name: "--eval", once: true, kind: Kind::Value, what: "", err_empty: "requires a baseline.json path", err_zero: "", min: 0, help: "[--eval <baseline.json>]" },
    Flag { name: "--eval-verbose", once: false, kind: Kind::NoValue, what: "", err_empty: "", err_zero: "", min: 0, help: "[--eval-verbose]" },
    Flag { name: "--gate", once: false, kind: Kind::NoValue, what: "", err_empty: "", err_zero: "", min: 0, help: "[--gate]" },
    Flag { name: "--gate-human", once: false, kind: Kind::NoValue, what: "", err_empty: "", err_zero: "", min: 0, help: "[--gate-human]" },
    Flag { name: "--gate-items", once: false, kind: Kind::NoValue, what: "", err_empty: "", err_zero: "", min: 0, help: "[--gate-items]" },
    Flag { name: "--l1-hist", once: false, kind: Kind::NoValue, what: "", err_empty: "", err_zero: "", min: 0, help: "[--l1-hist]" },
    Flag { name: "--stats", once: false, kind: Kind::NoValue, what: "", err_empty: "", err_zero: "", min: 0, help: "[--stats]" },
    Flag { name: "--annotate", once: false, kind: Kind::NoValue, what: "", err_empty: "", err_zero: "", min: 0, help: "[--annotate]" },
    Flag { name: "--annotate-raw", once: false, kind: Kind::NoValue, what: "", err_empty: "", err_zero: "", min: 0, help: "[--annotate-raw]" },
    Flag { name: "--annotate-out", once: true, kind: Kind::Value, what: "", err_empty: "requires a path", err_zero: "", min: 0, help: "[--annotate-out <path>]" },
    Flag { name: "--seed", once: true, kind: Kind::U64, what: "a non-negative integer", err_empty: "", err_zero: "", min: 0, help: "[--seed N]" },
    Flag { name: "--annotate-max", once: true, kind: Kind::U64, what: "an integer", err_empty: "", err_zero: "", min: 0, help: "[--annotate-max N]" },
    Flag { name: "--judge", once: true, kind: Kind::Value, what: "", err_empty: "requires a baseline path", err_zero: "", min: 0, help: "[--judge <baseline.json>]" },
    Flag { name: "--judge-by", once: false, kind: Kind::Value, what: "", err_empty: "", err_zero: "", min: 0, help: "[--judge-by <name>]" },
    Flag { name: "--max-bytes", once: true, kind: Kind::U64, what: "a byte count", err_empty: "", err_zero: "must be > 0", min: 1, help: "[--max-bytes BYTES]" },
    Flag { name: "--mem-dir", once: true, kind: Kind::Value, what: "", err_empty: "", err_zero: "", min: 0, help: "[--mem-dir DIR]" },
    Flag { name: "--json", once: false, kind: Kind::NoValue, what: "", err_empty: "", err_zero: "", min: 0, help: "[--json]" },
    Flag { name: "--stat", once: false, kind: Kind::NoValue, what: "", err_empty: "", err_zero: "", min: 0, help: "[--stat]" },
    Flag { name: "--no-stats", once: false, kind: Kind::NoValue, what: "", err_empty: "", err_zero: "", min: 0, help: "[--no-stats]" },
];

fn flag_by_name(name: &str) -> Option<&'static Flag> {
    FLAGS.iter().find(|f| f.name == name)
}

/// 解析期中间状态：字段与 `Opts` 一一对应（除 `seen` 与 `query` 的 Option 形式）。
struct ParseState {
    query: Option<String>,
    config: Config,
    json: bool,
    stat: bool,
    no_stats: bool,
    store: bool,
    fix: String,
    fix_file: Option<String>,
    recall: usize,
    recall_only: bool,
    recall_detail: bool,
    rate: Option<(bool, String)>,
    rebuild: bool,
    gc: bool,
    delete: Option<String>,
    eval: Option<String>,
    eval_verbose: bool,
    eval_gate: bool,
    eval_gate_human: bool,
    eval_gate_items: bool,
    l1_hist: bool,
    stats: bool,
    annotate: bool,
    annotate_raw: bool,
    annotate_out: Option<String>,
    seed: u64,
    annotate_max: usize,
    judge: Option<String>,
    judge_by: Option<String>,
    max_bytes: Option<u64>,
    mem_dir: String,
    files: Vec<String>,
    seen: HashSet<&'static str>,
}

impl ParseState {
    fn new() -> Self {
        ParseState {
            query: None,
            config: Config::default(),
            json: false,
            stat: false,
            no_stats: false,
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
            mem_dir: default_mem_dir(),
            files: Vec::new(),
            seen: HashSet::new(),
        }
    }
}

/// 自由字符串字段赋值（query / fix / mem-dir）。
fn set_free(s: &mut ParseState, f: &Flag, v: String) {
    match f.name {
        "--query" => s.query = Some(v),
        "--fix" => s.fix = v,
        _ => s.mem_dir = v,
    }
}

/// 路径/标识字段赋值（fix-file / eval / annotate-out / judge / judge-by / delete）。
fn set_path(s: &mut ParseState, f: &Flag, v: String) {
    match f.name {
        "--fix-file" => s.fix_file = Some(v),
        "--eval" => s.eval = Some(v),
        "--annotate-out" => s.annotate_out = Some(v),
        "--judge" => s.judge = Some(v),
        "--judge-by" => s.judge_by = Some(v),
        _ => s.delete = Some(v),
    }
}

/// 输出形态开关（json / stat / no-stats）。
fn set_output(s: &mut ParseState, name: &str) {
    match name {
        "--json" => s.json = true,
        "--stat" => s.stat = true,
        _ => s.no_stats = true,
    }
}

/// 库维护开关（rebuild / gc）。
fn set_house(s: &mut ParseState, name: &str) {
    match name {
        "--rebuild" => s.rebuild = true,
        _ => s.gc = true,
    }
}

/// 标注与证据门开关（annotate / annotate-raw / gate / gate-human / gate-items）。
fn set_eval_switch(s: &mut ParseState, name: &str) {
    match name {
        "--annotate" => s.annotate = true,
        "--annotate-raw" => s.annotate_raw = true,
        "--gate" => s.eval_gate = true,
        "--gate-human" => s.eval_gate_human = true,
        _ => s.eval_gate_items = true,
    }
}

/// 召回与杂项开关（recall-only / recall-detail / l1-hist / stats / eval-verbose）。
/// `--eval-verbose` 依赖已出现的 `--eval`（顺序敏感，与历史行为一致）。
fn set_recall_switch(s: &mut ParseState, name: &str) -> Result<(), String> {
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

/// 按 `Kind` 统一分派单个 flag 的取值与写入。
fn parse_flag(
    s: &mut ParseState,
    f: &Flag,
    inline: Option<String>,
    args: &[String],
    i: &mut usize,
) -> Result<(), String> {
    match f.kind {
        Kind::NoValue => {
            if inline.is_some() {
                return Err(format!("{} does not take a value", f.name));
            }
            match f.name {
                "--json" | "--stat" | "--no-stats" => set_output(s, f.name),
                "--rebuild" | "--gc" => set_house(s, f.name),
                "--annotate" | "--annotate-raw" | "--gate" | "--gate-human" | "--gate-items" => {
                    set_eval_switch(s, f.name)
                }
                _ => set_recall_switch(s, f.name)?,
            }
            Ok(())
        }
        Kind::Value => {
            if f.once {
                once(&mut s.seen, f.name)?;
            }
            let v = option_value(args, i, f.name, inline)?;
            if !f.err_empty.is_empty() && v.is_empty() {
                return Err(format!("{} {}", f.name, f.err_empty));
            }
            match f.name {
                "--query" | "--fix" | "--mem-dir" => set_free(s, f, v),
                _ => set_path(s, f, v),
            }
            Ok(())
        }
        Kind::F64 => {
            once(&mut s.seen, f.name)?;
            let v = option_value(args, i, f.name, inline)?;
            let parsed: f64 = v
                .parse()
                .map_err(|_| format!("{} expects {}, got {:?}", f.name, f.what, v))?;
            if !parsed.is_finite() || !(Config::MIN_RATIO..=1.0).contains(&parsed) {
                return Err(format!(
                    "{} must be a finite number in {}..=1.0",
                    f.name,
                    Config::MIN_RATIO
                ));
            }
            s.config.ratio = parsed;
            Ok(())
        }
        Kind::I64 => {
            once(&mut s.seen, f.name)?;
            let v = option_value(args, i, f.name, inline)?;
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
        Kind::U64 => {
            once(&mut s.seen, f.name)?;
            let v = option_value(args, i, f.name, inline)?;
            let parsed: u64 = v
                .parse()
                .map_err(|_| format!("{} expects {}, got {:?}", f.name, f.what, v))?;
            if parsed < f.min {
                return Err(format!("{} {}", f.name, f.err_zero));
            }
            match f.name {
                "--seed" => s.seed = parsed,
                "--annotate-max" => s.annotate_max = parsed as usize,
                "--recall" => s.recall = parsed as usize,
                _ => s.max_bytes = Some(parsed),
            }
            Ok(())
        }
        Kind::Bool01 => {
            once(&mut s.seen, f.name)?;
            let v = option_value(args, i, f.name, inline)?;
            let b = parse_bool01(&v, f.name)?;
            match f.name {
                "--elide" => s.config.elide = b,
                "--risk-gate" => s.config.risk_gate = b,
                "--lineno" => s.config.lineno = b,
                _ => s.store = b,
            }
            Ok(())
        }
        Kind::Force => {
            let v = option_value(args, i, f.name, inline)?;
            if v.split('|').all(|p| p.trim().is_empty()) {
                return Err(format!("{} {}", f.name, f.err_empty));
            }
            s.config.forces.push(v);
            Ok(())
        }
        Kind::RatePair => {
            once(&mut s.seen, f.name)?;
            let v = option_value(args, i, f.name, inline)?;
            let good = match v.as_str() {
                "good" => true,
                "bad" => false,
                _ => return Err("--rate expects good or bad".to_string()),
            };
            let id = option_value(args, i, f.name, None)?;
            if id.is_empty() {
                return Err("--rate requires a case-id".to_string());
            }
            s.rate = Some((good, id));
            Ok(())
        }
    }
}

/// 无维护动作（rebuild/gc/delete/eval/l1-hist/stats/judge）时是否豁免 `--query`。
fn action_free(s: &ParseState) -> bool {
    s.rate.is_none()
        && !s.rebuild
        && !s.gc
        && s.delete.is_none()
        && s.eval.is_none()
        && !s.l1_hist
        && !s.stats
        && s.judge.is_none()
}

/// 标注动作（annotate/annotate-raw）豁免 `--query`。
fn annotate_free(s: &ParseState) -> bool {
    !s.annotate && !s.annotate_raw
}

fn validate_query(s: &ParseState) -> Result<(), String> {
    let query_empty = s.query.as_deref().unwrap_or("").is_empty();
    if action_free(s) && annotate_free(s) && query_empty && s.config.forces.is_empty() {
        return Err("missing --query (or provide at least one --force pattern)".to_string());
    }
    if s.recall_only && query_empty {
        return Err("--recall-only requires --query".to_string());
    }
    Ok(())
}

fn validate_judge(s: &ParseState) -> Result<(), String> {
    if s.judge.is_some() && (s.annotate || s.annotate_raw || s.eval.is_some()) {
        return Err("--judge cannot be combined with --annotate/--annotate-raw/--eval".to_string());
    }
    if s.judge_by.is_some() && s.judge.is_none() {
        return Err("--judge-by requires --judge".to_string());
    }
    Ok(())
}

fn validate_annotate(s: &ParseState) -> Result<(), String> {
    if (s.annotate || s.annotate_raw) && (s.eval.is_some() || s.l1_hist) {
        return Err(
            "--annotate/--annotate-raw cannot be combined with --eval/--l1-hist".to_string(),
        );
    }
    if s.annotate && s.annotate_raw {
        return Err("--annotate and --annotate-raw are mutually exclusive".to_string());
    }
    if (s.annotate_out.is_some()
        || s.seen.contains("--seed")
        || s.seen.contains("--annotate-max"))
        && !(s.annotate || s.annotate_raw)
    {
        return Err("--annotate-out/--seed/--annotate-max require --annotate".to_string());
    }
    Ok(())
}

fn validate_actions(s: &ParseState) -> Result<(), String> {
    let actions = [
        s.rate.is_some(),
        s.rebuild,
        s.gc,
        s.delete.is_some(),
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

fn validate_gates(s: &ParseState) -> Result<(), String> {
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

/// `--store` 相关的绑定约束（fix/fix-file/max-bytes/recall-only）。
fn validate_store_binds(s: &ParseState) -> Result<(), String> {
    if s.seen.contains("--fix") && !s.store {
        return Err("--fix requires --store 1".to_string());
    }
    if s.fix_file.is_some() && !s.store {
        return Err("--fix-file requires --store 1".to_string());
    }
    if s.max_bytes.is_some() && !s.store {
        return Err("--max-bytes requires --store 1".to_string());
    }
    if s.seen.contains("--fix") && s.fix_file.is_some() {
        return Err("--fix and --fix-file are mutually exclusive".to_string());
    }
    Ok(())
}

/// `--recall` 相关的绑定约束（recall-only/recall-detail 与 store 互斥）。
fn validate_recall_binds(s: &ParseState) -> Result<(), String> {
    if s.recall_only && s.store {
        return Err("--recall-only cannot be combined with --store 1".to_string());
    }
    if s.recall_detail && s.recall == 0 && !s.recall_only {
        return Err("--recall-detail requires --recall or --recall-only".to_string());
    }
    Ok(())
}

fn validate(s: &ParseState) -> Result<(), String> {
    if s.stat && s.no_stats {
        return Err("--stat and --no-stats cannot be used together".to_string());
    }
    validate_query(s)?;
    validate_judge(s)?;
    validate_annotate(s)?;
    validate_actions(s)?;
    validate_gates(s)?;
    validate_store_binds(s)?;
    validate_recall_binds(s)?;
    Ok(())
}

fn into_opts(s: ParseState) -> Opts {
    let recall = if s.recall_only && !s.seen.contains("--recall") {
        3 // recall-only 未指定 K 时默认 top-3（与主流程 --recall 语义一致）
    } else {
        s.recall
    };
    Opts {
        query: s.query.unwrap_or_default(),
        config: s.config,
        json: s.json,
        stat: s.stat,
        no_stats: s.no_stats,
        store: s.store,
        fix: s.fix,
        fix_file: s.fix_file,
        recall,
        recall_only: s.recall_only,
        recall_detail: s.recall_detail,
        rate: s.rate,
        rebuild: s.rebuild,
        gc: s.gc,
        delete: s.delete,
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
    }
}

fn parse_args_from(args: Vec<String>) -> Result<Opts, String> {
    let mut s = ParseState::new();
    let mut i = 0usize;
    while i < args.len() {
        let raw = &args[i];
        if raw == "--" {
            s.files.extend(args[i + 1..].iter().cloned());
            break;
        }
        let (name, inline) = match raw.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v.to_string())),
            _ => (raw.as_str(), None),
        };
        match flag_by_name(name) {
            Some(f) => parse_flag(&mut s, f, inline, &args, &mut i)?,
            None if raw.starts_with("--") || (raw.starts_with('-') && raw.len() > 1) => {
                return Err(format!("unknown option: {}", raw));
            }
            None => s.files.push(raw.clone()),
        }
        i += 1;
    }
    validate(&s)?;
    Ok(into_opts(s))
}

pub(crate) fn parse_args(args: &[String]) -> Result<Opts, String> {
    // help/version 短路由由 main 在调用前处理；此处只解析与校验。
    parse_args_from(args.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn rejects_unknown_options_and_bad_values() {
        assert!(parse_args_from(args(&["--bogus", "x", "--query", "a"])).is_err());
        assert!(parse_args_from(args(&["--query", "a", "--ratio", "abc"])).is_err());
        assert!(parse_args_from(args(&["--query", "a", "--ratio", "-1"])).is_err());
        assert!(parse_args_from(args(&["--query", "a", "--context", "5"])).is_err());
        assert!(parse_args_from(args(&["--query", "a", "--window", "9"])).is_err());
        assert!(parse_args_from(args(&["--query", "a", "--elide", "2"])).is_err());
    }

    #[test]
    fn rejects_missing_values_and_duplicate_scalars() {
        assert!(parse_args_from(args(&["--query"])).is_err());
        assert!(parse_args_from(args(&["--force", "--json"])).is_err());
        assert!(parse_args_from(args(&["--query", "a", "--query", "b"])).is_err());
        assert!(parse_args_from(args(&["--query", "a", "--seed", "1", "--seed", "2"])).is_err());
        assert!(parse_args_from(args(&[
            "--query",
            "a",
            "--annotate-max",
            "1",
            "--annotate-max",
            "2"
        ]))
        .is_err());
        assert!(parse_args_from(args(&["--query", "a", "--stat", "--no-stats"])).is_err());
    }

    #[test]
    fn supports_equals_and_terminator() {
        let o = parse_args_from(args(&[
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
        let o = parse_args_from(args(&["--query", "q"])).unwrap();
        let d = Config::default();
        assert_eq!(o.config.ratio, d.ratio);
        assert_eq!(o.config.context, d.context);
        assert_eq!(o.config.window, d.window);
        assert_eq!(o.config.elide, d.elide);
        assert_eq!(o.config.risk_gate, d.risk_gate);
        assert_eq!(o.config.lineno, d.lineno);
    }

    #[test]
    fn query_can_be_empty_only_with_force() {
        assert!(parse_args_from(args(&[])).is_err());
        assert!(parse_args_from(args(&["--force="])).is_err());
        assert!(parse_args_from(args(&["--force", "|"])).is_err());
        let o = parse_args_from(args(&["--force", "SPECIAL"])).unwrap();
        assert_eq!(o.query, "");
        assert_eq!(o.config.forces, vec!["SPECIAL".to_string()]);
    }

    #[test]
    fn windows_and_booleans_are_normalized() {
        let o = parse_args_from(args(&[
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
        assert!(!o.config.risk_gate);
        let o2 = parse_args_from(args(&["--force=--json"])).unwrap();
        assert_eq!(o2.config.forces, vec!["--json".to_string()]);
        let o3 = parse_args_from(args(&["--query", "a", "--lineno", "1"])).unwrap();
        assert!(o3.config.lineno);
        let o4 = parse_args_from(args(&["--query", "a", "--lineno=0"])).unwrap();
        assert!(!o4.config.lineno);
        assert!(parse_args_from(args(&["--query", "a", "--lineno", "2"])).is_err());
        assert!(parse_args_from(args(&["--query", "a", "--lineno"])).is_err());
    }

    #[test]
    fn memory_flags_are_parsed() {
        let o = parse_args_from(args(&[
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
        let o = parse_args_from(args(&[
            "--query",
            "q",
            "--store",
            "1",
            "--fix",
            "enlarge pool",
        ]))
        .unwrap();
        assert_eq!(o.fix, "enlarge pool");
        assert!(parse_args_from(args(&["--query", "q", "--fix", "x"])).is_err());
        assert!(parse_args_from(args(&["--query", "q", "--store", "1", "--fix="])).is_err());
        assert!(parse_args_from(args(&["--query", "q", "--store", "2"])).is_err());
        assert!(parse_args_from(args(&["--query", "q", "--recall", "0"])).is_err());
        assert!(parse_args_from(args(&["--recall-only"])).is_err());
        let o = parse_args_from(args(&["--query", "q", "--recall-only"])).unwrap();
        assert_eq!(o.recall, 3, "recall-only 未指定 K 时应默认 top-3");
        // 库维护参数不要求 --query。
        let o = parse_args_from(args(&["--rebuild", "--mem-dir", "/tmp/m"])).unwrap();
        assert!(o.rebuild);
        let o = parse_args_from(args(&["--gc", "--mem-dir", "/tmp/m"])).unwrap();
        assert!(o.gc);
        let o = parse_args_from(args(&["--delete", "2026-10-22_ab12cd34"])).unwrap();
        assert_eq!(o.delete.as_deref(), Some("2026-10-22_ab12cd34"));
        assert!(parse_args_from(args(&["--delete"])).is_err());
        assert!(parse_args_from(args(&["--rebuild=x"])).is_err());
        // --eval 豁免 --query；--recall-detail 是独立开关。
        let o = parse_args_from(args(&["--eval", "bl.json", "--mem-dir", "/tmp/m"])).unwrap();
        assert_eq!(o.eval.as_deref(), Some("bl.json"));
        // --eval-verbose 需配合 --eval。
        let o = parse_args_from(args(&["--eval", "bl.json", "--eval-verbose"])).unwrap();
        assert!(o.eval_verbose);
        // --gate 必须配 --eval，且不允许带值。
        let o = parse_args_from(args(&["--eval", "bl.json", "--gate"])).unwrap();
        assert!(o.eval_gate);
        assert!(parse_args_from(args(&["--gate"])).is_err());
        assert!(parse_args_from(args(&["--eval", "bl.json", "--gate=x"])).is_err());
        let o = parse_args_from(args(&["--eval", "bl.json", "--gate-human"])).unwrap();
        assert!(o.eval_gate_human);
        assert!(parse_args_from(args(&["--gate-human"])).is_err());
        assert!(parse_args_from(args(&["--eval", "bl.json", "--gate", "--gate-human"])).is_err());
        let o = parse_args_from(args(&["--eval", "bl.json", "--gate-items"])).unwrap();
        assert!(o.eval_gate_items);
        assert!(
            parse_args_from(args(&["--eval", "bl.json", "--gate-human", "--gate-items"])).is_err()
        );
        assert!(
            parse_args_from(args(&["--eval-verbose"])).is_err(),
            "无 --eval 时 --eval-verbose 应报错"
        );
        let o =
            parse_args_from(args(&["--query", "q", "--recall", "3", "--recall-detail"])).unwrap();
        assert!(o.recall_detail);
        assert!(parse_args_from(args(&["--recall-detail=x"])).is_err());
        assert!(parse_args_from(args(&["--eval"])).is_err());
        // --stats 豁免 --query。
        let o = parse_args_from(args(&["--stats", "--mem-dir", "/tmp/m"])).unwrap();
        assert!(o.stats);
        assert!(parse_args_from(args(&["--stats=x"])).is_err());
    }

    #[test]
    fn rejects_conflicting_or_orphaned_actions() {
        assert!(parse_args_from(args(&["--rebuild", "--gc"])).is_err());
        assert!(parse_args_from(args(&["--stats", "--eval", "b.json"])).is_err());
        assert!(parse_args_from(args(&["--query", "q", "--max-bytes", "10"])).is_err());
        assert!(parse_args_from(args(&["--query", "q", "--recall-detail"])).is_err());
        assert!(parse_args_from(args(&["--query", "q", "--seed", "1"])).is_err());
        assert!(parse_args_from(args(&["--judge-by", "hao"])).is_err());
        assert!(parse_args_from(args(&["--query", "q", "--store", "1", "--recall-only"])).is_err());
    }
}
