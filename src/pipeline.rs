//! 压缩管线配置（`--pipeline <path>`）：JSON 声明式装配 plan/trim/anchor 阶段。
//!
//! 三层优先级：CLI 显式参数 > pipeline.json > policy 常量（与 `--policy-file`
//! 同语义：文件只填 CLI 未显式指定的字段）。算子/策略只以配置声明存在，
//! 未知键与越界值 fail-loud 报错，绝不静默忽略。
//!
//! 设计约束：本模块零 I/O 之外不依赖 args 层——`apply` 通过 `set` 回调询问
//! 某键是否已被 CLI 显式设置，装配决策由调用方（args）持有。

#![forbid(unsafe_code)]

use crate::json::Json;
use crate::{AnchorKind, Config};

/// 规划阶段（行级选择）：保留比例、邻域窗口、上下文行数与 token 上限。
#[derive(Clone, Debug, Default)]
pub struct Plan {
    pub ratio: Option<f64>,
    pub window: Option<i64>,
    pub context: Option<i64>,
    pub limit: Option<usize>,
}

/// 行内 token 级裁剪阶段：开关、保留比例与代码/数字/符号行保护。
#[derive(Clone, Debug, Default)]
pub struct Trim {
    pub enable: Option<bool>,
    pub ratio: Option<f64>,
    pub op_keep: Option<bool>,
    pub code_keep: Option<bool>,
    pub num_line_keep: Option<bool>,
    pub sym_line_keep: Option<bool>,
}

/// 骨架保护阶段：结构感知邻域、结构锚点种类与填充行最低分门槛。
#[derive(Clone, Debug, Default)]
pub struct Anchor {
    pub structure: Option<bool>,
    pub anchors: Option<Vec<AnchorKind>>,
    pub fill_floor: Option<f64>,
}

/// 管线装配：三阶段配置，全字段 `Option`（缺省 = 保持内核默认）。
#[derive(Clone, Debug, Default)]
pub struct Pipeline {
    pub plan: Plan,
    pub trim: Trim,
    pub anchor: Anchor,
}

/// 顶层允许键（含说明性 comment）。
const TOP_KEYS: &[&str] = &["comment", "plan", "trim", "anchor"];
const PLAN_KEYS: &[&str] = &["ratio", "window", "context", "limit"];
const TRIM_KEYS: &[&str] = &[
    "enable",
    "ratio",
    "op_keep",
    "code_keep",
    "num_line_keep",
    "sym_line_keep",
];
const ANCHOR_KEYS: &[&str] = &["structure", "anchors", "fill_floor"];

/// 锚点字符串映射（与 [`AnchorKind::as_str`] 同源）。
fn anchor_of(s: &str) -> Result<AnchorKind, String> {
    match s {
        "json" => Ok(AnchorKind::Json),
        "xml" => Ok(AnchorKind::Xml),
        "idoc" => Ok(AnchorKind::Idoc),
        "yaml" => Ok(AnchorKind::Yaml),
        "markdown" | "md" => Ok(AnchorKind::Markdown),
        other => Err(format!(
            "pipeline: unknown anchor kind `{other}` (json/xml/idoc/yaml/markdown)"
        )),
    }
}

fn reject_unknown(keys: &[&str], obj: &Json, path: &str, where_: &str) -> Result<(), String> {
    if let Json::Obj(kv) = obj {
        for (k, _) in kv {
            if !keys.contains(&k.as_str()) {
                return Err(format!(
                    "pipeline {path}: {where_} unknown key `{k}` (allowed: {})",
                    keys.join("/")
                ));
            }
        }
    }
    Ok(())
}

fn from_json(root: &Json, path: &str) -> Result<Pipeline, String> {
    reject_unknown(TOP_KEYS, root, path, "top-level")?;
    let mut p = Pipeline::default();
    let plan = root.get("plan");
    let trim = root.get("trim");
    let anchor = root.get("anchor");
    if let Some(obj) = plan {
        reject_unknown(PLAN_KEYS, obj, path, "plan")?;
        p.plan.ratio = f64_of(obj, "ratio", path, "plan")?;
        p.plan.window = i64_of(obj, "window", path, "plan")?;
        p.plan.context = i64_of(obj, "context", path, "plan")?;
        p.plan.limit = usize_of(obj, "limit", path, "plan")?;
    }
    if let Some(obj) = trim {
        reject_unknown(TRIM_KEYS, obj, path, "trim")?;
        p.trim.enable = bool_of(obj, "enable", path, "trim")?;
        p.trim.ratio = f64_of(obj, "ratio", path, "trim")?;
        p.trim.op_keep = bool_of(obj, "op_keep", path, "trim")?;
        p.trim.code_keep = bool_of(obj, "code_keep", path, "trim")?;
        p.trim.num_line_keep = bool_of(obj, "num_line_keep", path, "trim")?;
        p.trim.sym_line_keep = bool_of(obj, "sym_line_keep", path, "trim")?;
    }
    if let Some(obj) = anchor {
        reject_unknown(ANCHOR_KEYS, obj, path, "anchor")?;
        p.anchor.structure = bool_of(obj, "structure", path, "anchor")?;
        p.anchor.fill_floor = f64_of(obj, "fill_floor", path, "anchor")?;
        if let Some(arr) = obj.get("anchors") {
            let mut v = Vec::new();
            if let Json::Arr(items) = arr {
                for item in items {
                    let s = item.str().ok_or_else(|| {
                        format!("pipeline {path}: anchor: anchors entries must be strings")
                    })?;
                    v.push(anchor_of(s)?);
                }
            } else {
                return Err(format!("pipeline {path}: anchor: anchors must be an array"));
            }
            p.anchor.anchors = Some(v);
        }
    }
    validate_self(&p, path)?;
    Ok(p)
}

/// 范围校验（范围常量以 policy 为准，不在此重写）。
fn validate_self(p: &Pipeline, path: &str) -> Result<(), String> {
    if let Some(r) = p.plan.ratio {
        if !(crate::policy::MIN_RATIO..=1.0).contains(&r) {
            return Err(format!(
                "pipeline {path}: plan.ratio must be in {}..=1.0",
                crate::policy::MIN_RATIO
            ));
        }
    }
    if let Some(w) = p.plan.window {
        if !(Config::MIN_WINDOW..=Config::MAX_WINDOW).contains(&w) {
            return Err(format!(
                "pipeline {path}: plan.window must be in {}..={}",
                Config::MIN_WINDOW,
                Config::MAX_WINDOW
            ));
        }
    }
    if let Some(c) = p.plan.context {
        if !(0..=Config::MAX_CONTEXT).contains(&c) {
            return Err(format!(
                "pipeline {path}: plan.context must be 0..={}",
                Config::MAX_CONTEXT
            ));
        }
    }
    if let Some(r) = p.trim.ratio {
        if !(crate::policy::MIN_TRIM_RATIO..=crate::policy::MAX_TRIM_RATIO).contains(&r) {
            return Err(format!(
                "pipeline {path}: trim.ratio must be in {}..={}",
                crate::policy::MIN_TRIM_RATIO,
                crate::policy::MAX_TRIM_RATIO
            ));
        }
    }
    if let Some(f) = p.anchor.fill_floor {
        if !(0.0..=crate::policy::MAX_FILL_FLOOR).contains(&f) {
            return Err(format!(
                "pipeline {path}: anchor.fill_floor must be 0.0..={}",
                crate::policy::MAX_FILL_FLOOR
            ));
        }
    }
    Ok(())
}

impl Pipeline {
    /// 读取并解析管线配置；任何越界/未知键/类型错误都 fail-loud 返回。
    pub fn load(path: &str) -> Result<Pipeline, String> {
        let body = std::fs::read_to_string(path)
            .map_err(|e| format!("pipeline: cannot read {path}: {e}"))?;
        let root = crate::json::parse(&body).map_err(|e| format!("pipeline {path}: {e}"))?;
        from_json(&root, path)
    }
    /// 应用到 `cfg`。`set(flag)` 返回 true 表示该键已被 CLI 显式设置（跳过，
    /// 保持 CLI 显式 > pipeline > DEFAULT 的三层优先级）。
    /// 返回实际写入的 flag 名列表（供调用方记录来源，防止后续默认覆盖）。
    pub fn apply(&self, cfg: &mut Config, set: &dyn Fn(&str) -> bool) -> Vec<&'static str> {
        let mut applied = Vec::new();
        applied.extend(apply_plan(&self.plan, cfg, set));
        applied.extend(apply_trim(&self.trim, cfg, set));
        applied.extend(apply_anchor(&self.anchor, cfg, set));
        applied
    }
}

/// 单个可选字段落位：CLI 未显式设置（`set` 为 false）才写入并记录来源。
fn set1(
    applied: &mut Vec<&'static str>,
    cfg: &mut Config,
    set: &dyn Fn(&str) -> bool,
    flag: &'static str,
    field: impl Fn(&mut Config),
) {
    if !set(flag) {
        field(cfg);
        applied.push(flag);
    }
}

fn apply_plan(p: &Plan, cfg: &mut Config, set: &dyn Fn(&str) -> bool) -> Vec<&'static str> {
    let mut a = Vec::new();
    if p.ratio.is_some() {
        set1(&mut a, cfg, set, "--ratio", |c| c.ratio = p.ratio.unwrap());
    }
    if p.window.is_some() {
        set1(&mut a, cfg, set, "--window", |c| {
            c.window = p.window.unwrap()
        });
    }
    if p.context.is_some() {
        set1(&mut a, cfg, set, "--context", |c| {
            c.context = p.context.unwrap()
        });
    }
    if p.limit.is_some() {
        set1(&mut a, cfg, set, "--max-tokens", |c| c.limit = p.limit);
    }
    a
}

fn apply_trim(t: &Trim, cfg: &mut Config, set: &dyn Fn(&str) -> bool) -> Vec<&'static str> {
    let mut a = Vec::new();
    a.extend(apply_trim_switch(t, cfg, set));
    a.extend(apply_trim_keep(t, cfg, set));
    a
}

fn apply_trim_switch(t: &Trim, cfg: &mut Config, set: &dyn Fn(&str) -> bool) -> Vec<&'static str> {
    let mut a = Vec::new();
    if t.enable.is_some() {
        set1(&mut a, cfg, set, "--trim", |c| c.trim = t.enable.unwrap());
    }
    if t.ratio.is_some() {
        set1(&mut a, cfg, set, "--trim-ratio", |c| {
            c.trim_ratio = t.ratio.unwrap()
        });
    }
    if t.op_keep.is_some() {
        set1(&mut a, cfg, set, "--trim-op-keep", |c| {
            c.trim_op_keep = t.op_keep.unwrap()
        });
    }
    a
}

fn apply_trim_keep(t: &Trim, cfg: &mut Config, set: &dyn Fn(&str) -> bool) -> Vec<&'static str> {
    let mut a = Vec::new();
    if t.code_keep.is_some() {
        set1(&mut a, cfg, set, "--code-keep", |c| {
            c.code_keep = t.code_keep.unwrap()
        });
    }
    if t.num_line_keep.is_some() {
        set1(&mut a, cfg, set, "--num-line-keep", |c| {
            c.num_line_keep = t.num_line_keep.unwrap()
        });
    }
    if t.sym_line_keep.is_some() {
        set1(&mut a, cfg, set, "--sym-line-keep", |c| {
            c.sym_line_keep = t.sym_line_keep.unwrap()
        });
    }
    a
}

fn apply_anchor(a: &Anchor, cfg: &mut Config, set: &dyn Fn(&str) -> bool) -> Vec<&'static str> {
    let mut out = Vec::new();
    if a.structure.is_some() {
        set1(&mut out, cfg, set, "--structure", |c| {
            c.structure = a.structure.unwrap()
        });
    }
    if a.fill_floor.is_some() {
        set1(&mut out, cfg, set, "--fill-floor", |c| {
            c.fill_floor = a.fill_floor.unwrap()
        });
    }
    if let Some(anchors) = &a.anchors {
        if !set("--structure-anchors") {
            cfg.structure_anchors = anchors.clone();
            out.push("--structure-anchors");
        }
    }
    out
}

fn f64_of(obj: &Json, key: &str, path: &str, where_: &str) -> Result<Option<f64>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(Json::Num(n)) => Ok(Some(*n)),
        Some(_) => Err(format!("pipeline {path}: {where_}.{key} must be a number")),
    }
}

fn i64_of(obj: &Json, key: &str, path: &str, where_: &str) -> Result<Option<i64>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(Json::Num(n)) => Ok(Some(*n as i64)),
        Some(_) => Err(format!("pipeline {path}: {where_}.{key} must be a number")),
    }
}

fn usize_of(obj: &Json, key: &str, path: &str, where_: &str) -> Result<Option<usize>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(Json::Num(n)) if *n >= 0.0 => Ok(Some(*n as usize)),
        Some(_) => Err(format!(
            "pipeline {path}: {where_}.{key} must be a non-negative number"
        )),
    }
}

fn bool_of(obj: &Json, key: &str, path: &str, where_: &str) -> Result<Option<bool>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(Json::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(format!("pipeline {path}: {where_}.{key} must be a boolean")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(body: &str) -> Result<Pipeline, String> {
        let root = crate::json::parse(body)?;
        from_json(&root, "<test>")
    }

    #[test]
    fn full_pipeline_applies_all_fields() {
        let p = parse(
            r#"{"plan":{"ratio":0.3,"window":2,"context":1},
            "trim":{"enable":true,"ratio":0.3,"op_keep":true,"code_keep":true,
            "num_line_keep":true,"sym_line_keep":true},
            "anchor":{"structure":true,"anchors":["md"],"fill_floor":0.0}}"#,
        )
        .unwrap();
        let mut cfg = Config::default();
        let applied = p.apply(&mut cfg, &|_| false);
        assert!(applied.contains(&"--ratio"));
        assert!(applied.contains(&"--structure-anchors"));
        assert_eq!(cfg.ratio, 0.3);
        assert!(cfg.trim);
        assert_eq!(cfg.trim_ratio, 0.3);
        assert!(cfg.trim_op_keep && cfg.code_keep && cfg.num_line_keep && cfg.sym_line_keep);
        assert!(cfg.structure);
        assert_eq!(cfg.structure_anchors, vec![AnchorKind::Markdown]);
        assert_eq!(cfg.fill_floor, 0.0);
    }

    #[test]
    fn cli_explicit_wins_over_pipeline() {
        let p = parse(r#"{"plan":{"ratio":0.3}}"#).unwrap();
        let mut cfg = Config::default();
        // CLI 已显式 --ratio → pipeline 不覆盖
        let applied = p.apply(&mut cfg, &|f| f == "--ratio");
        assert!(applied.is_empty());
        assert_eq!(cfg.ratio, Config::DEFAULT_RATIO);
    }

    #[test]
    fn partial_pipeline_keeps_defaults() {
        let p = parse(r#"{"trim":{"enable":true}}"#).unwrap();
        let mut cfg = Config::default();
        let applied = p.apply(&mut cfg, &|_| false);
        assert!(applied.contains(&"--trim"));
        assert_eq!(cfg.trim_ratio, crate::policy::DEFAULT_TRIM_RATIO);
        assert!(cfg.trim);
        assert_eq!(cfg.trim_ratio, crate::policy::DEFAULT_TRIM_RATIO);
        assert_eq!(cfg.ratio, Config::DEFAULT_RATIO);
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(parse(r#"{"bogus":1}"#).is_err());
        assert!(parse(r#"{"plan":{"bogus":1}}"#).is_err());
        assert!(parse(r#"{"trim":{"bogus":1}}"#).is_err());
    }

    #[test]
    fn rejects_out_of_range() {
        assert!(parse(r#"{"plan":{"ratio":5.0}}"#).is_err());
        assert!(parse(r#"{"plan":{"window":99}}"#).is_err());
        assert!(parse(r#"{"trim":{"ratio":0.05}}"#).is_err());
        assert!(parse(r#"{"anchor":{"fill_floor":99.0}}"#).is_err());
    }

    #[test]
    fn rejects_wrong_types_and_bad_anchors() {
        assert!(parse(r#"{"trim":{"enable":1}}"#).is_err());
        assert!(parse(r#"{"anchor":{"anchors":"md"}}"#).is_err());
        assert!(parse(r#"{"anchor":{"anchors":["bogus"]}}"#).is_err());
    }
}
