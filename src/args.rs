//! CLI 参数解析：Opts 结构、USAGE、option 扫描、帮助/版本短路（帮助打印在 main）。
//! 只做解析与校验，不含 I/O 与引擎逻辑。

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
    /// P4.4+ 优化：修复结论（--store 配 --fix 写入 case.fix，L1 本地直答）。
    /// 仅在 --store 1 时生效；无 --store 时解析但被忽略。
    /// P4.4+ 优化⑩：--store 不带 --fix 时 stderr warning（L1 无法直答"怎么解决的"）。
    pub fix: String,
    /// P4.4+ 优化⑩：--fix-file <path>——多行修复结论从文件读入（等价 --fix 内容，
    /// 换行保留；适合多行处置步骤）。与 --fix 同时给时报错。
    pub fix_file: Option<String>,
    pub recall: usize,
    pub recall_only: bool,
    /// P4.2 L1 要点摘要：--recall-detail 时 mem 段追加 summary 行。
    pub recall_detail: bool,
    /// 反馈标注：Some((good, case_id))，仅 O4 双环路径使用。
    pub rate: Option<(bool, String)>,
    /// P3.3 库维护：重建索引 / 过期清理 / 删除 case（惰性作废索引）。
    pub rebuild: bool,
    pub gc: bool,
    pub delete: Option<String>,
    /// P4.1 评估系统：冻结 baseline.json 重放（豁免 --query）。
    pub eval: Option<String>,
    /// P4.4+ 优化④：--eval-verbose 逐 case 诊断（A/B top-10 候选 + [hit]/[miss]）。
    pub eval_verbose: bool,
    /// P4.4+ 优化⑭：--gate 机械校验 M4 证据门（须配 --eval）。
    pub eval_gate: bool,
    /// 人工弱相关/负例闭环门：在机械门之上要求全量人工元数据、控制标签与 NDCG。
    pub eval_gate_human: bool,
    /// 逐候选裁决门：允许 AI/非人类裁决人，但仍要求每条候选独立记录。
    pub eval_gate_items: bool,
    pub l1_hist: bool,
    /// P4.4+ 优化：库盘点（--stats，豁免 --query）。
    pub stats: bool,
    /// P4.4+ 优化⑥：智能标注器（--annotate，豁免 --query）——生成 M4 证据门标注集。
    pub annotate: bool,
    /// P4.4+ 优化⑧：盲测模式（--annotate-raw）——不过自检的全变体集（含自检会丢弃的
    /// 变体），每条附目标 case 摘要（note: target/hint/fix），供人工盲测裁决 rel；
    /// 消除"自检用 A 管线验证 = 用 A 自证"的选择偏差。与 --annotate 互斥。
    pub annotate_raw: bool,
    /// --annotate-out <path>：标注集输出路径（缺省 `<mem-dir>/baseline-auto.json`）。
    pub annotate_out: Option<String>,
    /// --seed N：标注生成确定性种子（缺省 0，同 seed 可重放）。
    pub seed: u64,
    /// --annotate-max N：标注覆盖的最大 case 数（缺省 16；0 = 全库）。
    /// 证据门 30–50 组：case 数 × 三层产率约 1.7，24+ case 库可直接落窗。
    pub annotate_max: usize,
    /// P4.4+ 优化⑩：盲测裁决（--judge <baseline.json>）——逐条 stdin 录入 2/1/0/s，
    /// 把 judge:null 替换为 rel+judge 元数据（by/date）；未裁决条目 --eval 拒绝。
    pub judge: Option<String>,
    /// --judge-by <name>：裁决人标识（缺省取 USERNAME）。
    pub judge_by: Option<String>,
    /// P4.4+ 优化⑫（P1-4）：--max-bytes N：记忆库总容量软配额（字节）。
    /// store 前若库总量（cases+meta+index）超过 N 则拒绝写入（提示 --gc / --delete）。
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

fn parse_args_from(args: Vec<String>) -> Result<Opts, String> {
    let mut query: Option<String> = None;
    let mut config = Config::default();
    let mut json = false;
    let mut stat = false;
    let mut no_stats = false;
    let mut store = false;
    let mut fix = String::new();
    let mut fix_file: Option<String> = None;
    let mut recall = 0usize;
    let mut recall_only = false;
    let mut recall_detail = false;
    let mut rate: Option<(bool, String)> = None;
    let mut rebuild = false;
    let mut gc = false;
    let mut delete: Option<String> = None;
    let mut eval: Option<String> = None;
    let mut eval_verbose = false;
    let mut eval_gate = false;
    let mut eval_gate_human = false;
    let mut eval_gate_items = false;
    let mut l1_hist = false;
    let mut stats = false;
    let mut annotate = false;
    let mut annotate_raw = false;
    let mut annotate_out: Option<String> = None;
    let mut seed = 0u64;
    let mut annotate_max = 16usize;
    let mut judge: Option<String> = None;
    let mut judge_by: Option<String> = None;
    let mut max_bytes: Option<u64> = None;
    let mut mem_dir = default_mem_dir();
    let mut files = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();

    let mut i = 0usize;
    while i < args.len() {
        let raw = &args[i];
        if raw == "--" {
            files.extend(args[i + 1..].iter().cloned());
            break;
        }
        let (name, inline) = match raw.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v.to_string())),
            _ => (raw.as_str(), None),
        };
        match name {
            "--query" => {
                once(&mut seen, name)?;
                query = Some(option_value(&args, &mut i, name, inline)?);
            }
            "--ratio" => {
                once(&mut seen, name)?;
                let v = option_value(&args, &mut i, name, inline)?;
                let parsed = v
                    .parse::<f64>()
                    .map_err(|_| format!("--ratio expects a number, got {:?}", v))?;
                if !parsed.is_finite() || !(Config::MIN_RATIO..=1.0).contains(&parsed) {
                    return Err(format!(
                        "--ratio must be a finite number in {}..=1.0",
                        Config::MIN_RATIO
                    ));
                }
                config.ratio = parsed;
            }
            "--context" => {
                once(&mut seen, name)?;
                let v = option_value(&args, &mut i, name, inline)?;
                let parsed = v
                    .parse::<i64>()
                    .map_err(|_| format!("--context expects an integer, got {:?}", v))?;
                if !(0..=Config::MAX_CONTEXT).contains(&parsed) {
                    return Err(format!("--context must be in 0..={}", Config::MAX_CONTEXT));
                }
                config.context = parsed;
            }
            "--window" => {
                once(&mut seen, name)?;
                let v = option_value(&args, &mut i, name, inline)?;
                let parsed = v
                    .parse::<i64>()
                    .map_err(|_| format!("--window expects an integer, got {:?}", v))?;
                if !(0..=Config::MAX_WINDOW).contains(&parsed) {
                    return Err(format!("--window must be in 0..={}", Config::MAX_WINDOW));
                }
                config.window = parsed.max(Config::MIN_WINDOW);
            }
            "--elide" => {
                once(&mut seen, name)?;
                let v = option_value(&args, &mut i, name, inline)?;
                config.elide = parse_bool01(&v, "--elide")?;
            }
            "--risk-gate" => {
                once(&mut seen, name)?;
                let v = option_value(&args, &mut i, name, inline)?;
                config.risk_gate = parse_bool01(&v, "--risk-gate")?;
            }
            "--lineno" => {
                once(&mut seen, name)?;
                let v = option_value(&args, &mut i, name, inline)?;
                config.lineno = parse_bool01(&v, "--lineno")?;
            }
            "--force" => {
                let v = option_value(&args, &mut i, name, inline)?;
                if v.split('|').all(|p| p.trim().is_empty()) {
                    return Err("--force requires a non-empty pattern".to_string());
                }
                config.forces.push(v);
            }
            "--store" => {
                once(&mut seen, name)?;
                let v = option_value(&args, &mut i, name, inline)?;
                store = parse_bool01(&v, "--store")?;
            }
            "--fix" => {
                once(&mut seen, name)?;
                let v = option_value(&args, &mut i, name, inline)?;
                if v.is_empty() {
                    return Err("--fix requires non-empty text".to_string());
                }
                fix = v;
            }
            // P4.4+ 优化⑩：--fix-file <path>——多行修复结论从文件读入。
            "--fix-file" => {
                once(&mut seen, name)?;
                let p = option_value(&args, &mut i, name, inline)?;
                if p.is_empty() {
                    return Err("--fix-file requires a path".to_string());
                }
                fix_file = Some(p);
            }
            "--recall" => {
                once(&mut seen, name)?;
                let v = option_value(&args, &mut i, name, inline)?;
                let parsed = v
                    .parse::<usize>()
                    .map_err(|_| format!("--recall expects a positive integer, got {:?}", v))?;
                if parsed == 0 {
                    return Err("--recall expects a positive integer".to_string());
                }
                recall = parsed;
            }
            "--recall-only" => {
                if inline.is_some() {
                    return Err("--recall-only does not take a value".to_string());
                }
                recall_only = true;
            }
            "--recall-detail" => {
                if inline.is_some() {
                    return Err("--recall-detail does not take a value".to_string());
                }
                recall_detail = true;
            }
            "--eval" => {
                once(&mut seen, name)?;
                let p = option_value(&args, &mut i, name, inline)?;
                if p.is_empty() {
                    return Err("--eval requires a baseline.json path".to_string());
                }
                eval = Some(p);
            }
            "--eval-verbose" => {
                if inline.is_some() {
                    return Err("--eval-verbose does not take a value".to_string());
                }
                if eval.is_none() {
                    return Err("--eval-verbose requires --eval <baseline.json>".to_string());
                }
                eval_verbose = true;
            }
            // P4.4+ 优化⑭：M4 机械门（样本量/分层/裁决元数据/A≥B）。
            "--gate" => {
                if inline.is_some() {
                    return Err("--gate does not take a value".to_string());
                }
                eval_gate = true;
            }
            "--gate-human" => {
                if inline.is_some() {
                    return Err("--gate-human does not take a value".to_string());
                }
                eval_gate_human = true;
            }
            "--gate-items" => {
                if inline.is_some() {
                    return Err("--gate-items does not take a value".to_string());
                }
                eval_gate_items = true;
            }
            "--l1-hist" => {
                if inline.is_some() {
                    return Err("--l1-hist does not take a value".to_string());
                }
                l1_hist = true;
            }
            "--stats" => {
                if inline.is_some() {
                    return Err("--stats does not take a value".to_string());
                }
                stats = true;
            }
            "--annotate" => {
                if inline.is_some() {
                    return Err("--annotate does not take a value".to_string());
                }
                annotate = true;
            }
            // P4.4+ 优化⑧：盲测模式——不过自检的全变体集（人工裁决用，消除选择偏差）。
            "--annotate-raw" => {
                if inline.is_some() {
                    return Err("--annotate-raw does not take a value".to_string());
                }
                annotate_raw = true;
            }
            "--annotate-out" => {
                once(&mut seen, name)?;
                let p = option_value(&args, &mut i, name, inline)?;
                if p.is_empty() {
                    return Err("--annotate-out requires a path".to_string());
                }
                annotate_out = Some(p);
            }
            "--seed" => {
                once(&mut seen, name)?;
                let v = option_value(&args, &mut i, name, inline)?;
                seed = v
                    .parse::<u64>()
                    .map_err(|_| format!("--seed expects a non-negative integer, got {:?}", v))?;
            }
            "--annotate-max" => {
                once(&mut seen, name)?;
                let v = option_value(&args, &mut i, name, inline)?;
                annotate_max = v
                    .parse::<usize>()
                    .map_err(|_| format!("--annotate-max expects an integer, got {:?}", v))?;
            }
            // P4.4+ 优化⑩：盲测裁决（--judge <baseline.json>，交互式 stdin 录入）。
            "--judge" => {
                once(&mut seen, name)?;
                let p = option_value(&args, &mut i, name, inline)?;
                if p.is_empty() {
                    return Err("--judge requires a baseline path".to_string());
                }
                judge = Some(p);
            }
            "--judge-by" => {
                once(&mut seen, name)?;
                judge_by = Some(option_value(&args, &mut i, name, inline)?);
            }
            // P4.4+ 优化⑫（P1-4）：--max-bytes 总容量软配额（store 前检查）。
            "--max-bytes" => {
                once(&mut seen, name)?;
                let v = option_value(&args, &mut i, name, inline)?;
                let parsed = v
                    .parse::<u64>()
                    .map_err(|_| format!("--max-bytes expects a byte count, got {:?}", v))?;
                if parsed == 0 {
                    return Err("--max-bytes must be > 0".to_string());
                }
                max_bytes = Some(parsed);
            }
            "--rebuild" => {
                if inline.is_some() {
                    return Err("--rebuild does not take a value".to_string());
                }
                rebuild = true;
            }
            "--gc" => {
                if inline.is_some() {
                    return Err("--gc does not take a value".to_string());
                }
                gc = true;
            }
            "--delete" => {
                once(&mut seen, name)?;
                let id = option_value(&args, &mut i, name, inline)?;
                if id.is_empty() {
                    return Err("--delete requires a case-id".to_string());
                }
                delete = Some(id);
            }
            "--rate" => {
                once(&mut seen, name)?;
                let v = option_value(&args, &mut i, name, inline)?;
                let good = match v.as_str() {
                    "good" => true,
                    "bad" => false,
                    _ => return Err("--rate expects good or bad".to_string()),
                };
                let id = option_value(&args, &mut i, name, None)?;
                if id.is_empty() {
                    return Err("--rate requires a case-id".to_string());
                }
                rate = Some((good, id));
            }
            "--mem-dir" => {
                once(&mut seen, name)?;
                mem_dir = option_value(&args, &mut i, name, inline)?;
            }
            "--json" => {
                if inline.is_some() {
                    return Err("--json does not take a value".to_string());
                }
                json = true;
            }
            "--stat" => {
                if inline.is_some() {
                    return Err("--stat does not take a value".to_string());
                }
                stat = true;
            }
            "--no-stats" => {
                if inline.is_some() {
                    return Err("--no-stats does not take a value".to_string());
                }
                no_stats = true;
            }
            _ if raw.starts_with("--") || (raw.starts_with('-') && raw.len() > 1) => {
                return Err(format!("unknown option: {}", raw));
            }
            _ => files.push(raw.clone()),
        }
        i += 1;
    }

    if stat && no_stats {
        return Err("--stat and --no-stats cannot be used together".to_string());
    }
    let query = query.unwrap_or_default();
    // O4 反馈标注 / P3.3 库维护 / P4.1 评估 / P2 残余 l1-hist / 库盘点 / 智能标注不要求 --query。
    if rate.is_none()
        && !rebuild
        && !gc
        && delete.is_none()
        && eval.is_none()
        && !l1_hist
        && !stats
        && !annotate
        && !annotate_raw
        && judge.is_none()
        && query.is_empty()
        && config.forces.is_empty()
    {
        return Err("missing --query (or provide at least one --force pattern)".to_string());
    }
    if judge.is_some() && (annotate || annotate_raw || eval.is_some()) {
        return Err("--judge cannot be combined with --annotate/--annotate-raw/--eval".to_string());
    }
    if (annotate || annotate_raw) && (eval.is_some() || l1_hist) {
        return Err(
            "--annotate/--annotate-raw cannot be combined with --eval/--l1-hist".to_string(),
        );
    }
    if annotate && annotate_raw {
        return Err("--annotate and --annotate-raw are mutually exclusive".to_string());
    }
    let actions = [
        rate.is_some(),
        rebuild,
        gc,
        delete.is_some(),
        eval.is_some(),
        l1_hist,
        stats,
        annotate || annotate_raw,
        judge.is_some(),
        recall_only,
    ];
    if actions.iter().filter(|active| **active).count() > 1 {
        return Err("action options are mutually exclusive; choose one".to_string());
    }
    let gate_count = [eval_gate, eval_gate_human, eval_gate_items]
        .iter()
        .filter(|v| **v)
        .count();
    if gate_count > 0 && eval.is_none() {
        return Err("--gate/--gate-human/--gate-items requires --eval <baseline.json>".to_string());
    }
    if gate_count > 1 {
        return Err("--gate, --gate-human and --gate-items are mutually exclusive".to_string());
    }
    if recall_only && query.is_empty() {
        return Err("--recall-only requires --query".to_string());
    }
    if seen.contains("--fix") && !store {
        return Err("--fix requires --store 1".to_string());
    }
    if fix_file.is_some() && !store {
        return Err("--fix-file requires --store 1".to_string());
    }
    if max_bytes.is_some() && !store {
        return Err("--max-bytes requires --store 1".to_string());
    }
    if seen.contains("--fix") && fix_file.is_some() {
        return Err("--fix and --fix-file are mutually exclusive".to_string());
    }
    if recall_only && store {
        return Err("--recall-only cannot be combined with --store 1".to_string());
    }
    if recall_detail && recall == 0 && !recall_only {
        return Err("--recall-detail requires --recall or --recall-only".to_string());
    }
    if (annotate_out.is_some() || seen.contains("--seed") || seen.contains("--annotate-max"))
        && !(annotate || annotate_raw)
    {
        return Err("--annotate-out/--seed/--annotate-max require --annotate".to_string());
    }
    if judge_by.is_some() && judge.is_none() {
        return Err("--judge-by requires --judge".to_string());
    }
    if recall_only && !seen.contains("--recall") {
        recall = 3; // recall-only 未指定 K 时默认 top-3（与主流程 --recall 语义一致）
    }
    Ok(Opts {
        query,
        config,
        json,
        stat,
        no_stats,
        store,
        fix,
        fix_file,
        recall,
        recall_only,
        recall_detail,
        rate,
        rebuild,
        gc,
        delete,
        eval,
        eval_verbose,
        eval_gate,
        eval_gate_human,
        eval_gate_items,
        l1_hist,
        stats,
        annotate,
        annotate_raw,
        annotate_out,
        seed,
        annotate_max,
        judge,
        judge_by,
        max_bytes,
        mem_dir,
        files,
    })
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
        // P4.4+ 优化：--fix 与 --store 绑定。
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
        // P3.3 库维护参数：不要求 --query。
        let o = parse_args_from(args(&["--rebuild", "--mem-dir", "/tmp/m"])).unwrap();
        assert!(o.rebuild);
        let o = parse_args_from(args(&["--gc", "--mem-dir", "/tmp/m"])).unwrap();
        assert!(o.gc);
        let o = parse_args_from(args(&["--delete", "2026-10-22_ab12cd34"])).unwrap();
        assert_eq!(o.delete.as_deref(), Some("2026-10-22_ab12cd34"));
        assert!(parse_args_from(args(&["--delete"])).is_err());
        assert!(parse_args_from(args(&["--rebuild=x"])).is_err());
        // P4.1/P4.2：--eval 豁免 --query；--recall-detail 开关。
        let o = parse_args_from(args(&["--eval", "bl.json", "--mem-dir", "/tmp/m"])).unwrap();
        assert_eq!(o.eval.as_deref(), Some("bl.json"));
        // P4.4+ 优化④：--eval-verbose 需配合 --eval。
        let o = parse_args_from(args(&["--eval", "bl.json", "--eval-verbose"])).unwrap();
        assert!(o.eval_verbose);
        // P4.4+ 优化⑭：--gate 必须配 --eval；不允许带值。
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
        // P4.4+ 优化：--stats 豁免 --query。
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
