//! CLI 参数解析（args/parse）：各 flag 的值解析、分派处理器与 policy 文件
//! 应用。纯函数、无 I/O；全部 `pub(crate)` 供参数表（args::FLAGS）引用。

use super::*;

pub fn usage() -> String {
    let mut s = format!(
        "usage: scx --query \"...\" [--ratio {}] [--window {}] [--context {}] [--elide 1] [--risk-gate 1] [--scoped 1] [--strict 1] [--lineno 1] [--max-tokens N] [--structure 0|1] [--structure-anchors LIST] [--redact LIST] [--kind TYPE] [--store 1] [--fix \"修复结论\"] [--fix-file <path>] [--policy-file <path>] [--pipeline <path>] [--embed-cmd CMD] [--sim-weight F] [--sem-weight F] [--minhash-weight F] [--recall K] [--recall-only] [--recall-detail] [--rate good|bad <case-id>] [--rebuild] [--gc] [--delete <case-id>] [--eval <baseline.json>] [--eval-verbose] [--gate|--gate-human|--gate-items] [--l1-hist] [--stats] [--annotate] [--annotate-raw] [--annotate-out <path>] [--seed N] [--annotate-max N] [--judge <baseline.json>] [--judge-by <name>] [--mem-dir DIR] [--force SUBSTR|re:PATTERN]... [--evidence N|substr]... [--fill-floor F] [--trim 0|1] [--trim-ratio F] [--fold 0|1] [--entropy 0|1] [--auto-anchor 0|1] [--cluster 0|1] [--trim-op-keep 0|1] [--code-keep 0|1] [--num-line-keep 0|1] [--sym-line-keep 0|1] [--list-keep 0|1] [--json] [--stat|--no-stats] [file...]",
        Config::DEFAULT_RATIO,
        Config::DEFAULT_WINDOW,
        Config::DEFAULT_CONTEXT
    );
    s.push_str("\noptions:\n");
    s.push_str("  --ratio R            optional-content budget (default ");
    s.push_str(&Config::DEFAULT_RATIO.to_string());
    s.push_str("); evidence lines and minimum neighborhood are exempt\n");
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
        "                       (config/bench/profiles/*.json); precedence CLI > file > DEFAULT\n",
    );
    s.push_str(
        "  --embed-cmd CMD      pluggable similarity backend: sh -c CMD reads Q\\t<query> then\n",
    );
    s.push_str("                       L\\t<line> on stdin, emits one score per line (default off, zero-dep)\n");
    s.push_str(
        "  --sim-weight F       weight of injected similarity in ranking (default 1.0; >=0)\n",
    );
    s.push_str("  --sem-weight F       scale factor of semantic re-score (default 1.0; >=0)\n");
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
        "--query" | "--fix" | "--mem-dir" | "--embed-cmd" => free(s, f, v),
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
    let p = crate::pipeline::Pipeline::load(&path).map_err(|e| format!("{}: {e}", f.name))?;
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
    Ok(())
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
        embed_cmd: s.embed_cmd,
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
