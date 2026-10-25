//! meta.toml 读写与库根初始化（零依赖行式子集）。
//!
//! 布局见 store.rs 头注释。meta 是"库元信息"（schema/引擎版本/case 数/DF 快照/参数），
//! 与 case 资产分离：损坏时整库仍可用，rebuild 可重建。

use crate::memory::lock;
/// case.json 的当前 schema 版本。旧版本读取按缺字段默认值兼容。
pub(crate) const SCHEMA: u32 = 2;
pub(crate) const ENG_VER: &str = "stat-v1";
use crate::memory::util::utc_now;
use std::fs;
use std::path::Path;

/// 建库根目录（含 cases/、index/）与初始 meta.toml。
pub(crate) fn ensure_root(root: &str) -> std::io::Result<()> {
    for sub in ["", "cases", "index"] {
        let p = Path::new(root).join(sub);
        if !p.exists() {
            lock::create_dir_all(&p)?;
        }
    }
    let meta_p = Path::new(root).join("meta.toml");
    if !meta_p.exists() {
        // 多进程同时首次打开：锁内二次检查，避免两边写坏同一 meta。
        let _g = lock::acquire(root, "meta.lock")?;
        if !meta_p.exists() {
            lock::atomic_write(&meta_p, meta_toml(0).as_bytes())?;
        }
    }
    Ok(())
}

pub(crate) struct Meta {
    /// 建库时间（UTC）。评估/自检用它冻结衰减时钟，保证同一库快照可重放；
    /// 线上召回仍使用当前时间。
    pub created_at: String,
    pub case_count: usize,
    /// 库代际（P4.4 评审处置）：任何 case 写入 / 删除都 +1；
    /// qmap 快照与会话缓存条目记录写入时的 gen，`gen < 当前` 视为过期
    /// （新 case / 反馈 / 删除必须对同 query 召回可见）。
    pub gen: u64,
    pub df: Vec<(String, usize)>,
    pub params: Vec<(String, f64)>,
    /// O5 词级调权表：词 → 增量权重（Rocchio 简化版，§3.9），无词条=0。
    pub word_w: Vec<(String, f64)>,
    /// O5 L1 自适应：最近 top1_sim 滑动历史（上限 L1_HIST_CAP，逗号分隔存 [l1] 节）。
    pub l1_hist: Vec<f64>,
}

impl Meta {
    /// 参数读取的唯一入口：库内值优先，缺省值来自同一张默认表。
    pub(crate) fn param(&self, key: &str) -> f64 {
        if let Some((_, value)) = self.params.iter().find(|(k, _)| k == key) {
            return *value;
        }
        debug_assert!(
            DEFAULT_PARAMS.iter().any(|(k, _)| *k == key),
            "unknown config key: {key}"
        );
        default_param(key)
    }
}

/// 默认参数查表；未知键返回 0.0（老库兼容），开发期由 `Meta::param` 的断言兜住。
pub(crate) fn default_param(key: &str) -> f64 {
    DEFAULT_PARAMS
        .iter()
        .find(|(k, _)| *k == key)
        .map_or(0.0, |(_, value)| *value)
}

/// 新库参数模板，也是所有缺省读取的唯一来源。
pub(crate) const DEFAULT_PARAMS: &[(&str, f64)] = &[
    ("bm25_k1", 1.2),
    ("bm25_b", 0.75),
    ("field_weight_error_code", 3.0),
    ("field_weight_file_loc", 2.5),
    ("field_weight_symbol", 2.0),
    ("field_weight_num_fp", 2.0),
    ("decay_tau_days", 90.0),
    ("decay_tau_solved", 180.0),
    ("decay_tau_regressed", 30.0),
    ("mmr_lambda", 0.5),
    ("mmr_enable", 0.0),
    ("bucket_enable", 1.0),
    ("bucket_allow_other", 1.0),
    ("bucket_bypass_sim", 0.30),
    ("prf_enable", 1.0),
    ("confidence_high_sim", 0.99),
    ("index_force", 0.0),
    ("feedback_step", 0.1),
    ("feedback_step_bad", 0.15),
    ("word_w_limit", 0.5),
    ("session_cache_minutes", 30.0),
    ("l1_base", 0.55),
    ("l1_margin_min", 0.15),
    ("l1_adapt_min", 0.30),
    ("l1_adapt_max", 0.90),
    ("l1_adapt_min_history", 4.0),
    ("quality_prior", 0.5),
    ("gc_ttl_regressed", 180.0),
    ("gc_ttl_stale", 365.0),
    ("edge_enable", 1.0),
    ("edge_weight", 1.0),
    ("query_field_weight", 0.80),
    ("query_field_min_sim", 0.07),
    ("query_field_trigger_sim", 0.30),
    ("sibling_coherence", 1.0),
    ("sibling_min_sim", 0.30),
    ("synonyms_enable", 1.0),
    ("synonym_weight", 0.5),
];

pub(crate) const L1_HIST_CAP: usize = 64;

pub(crate) fn load_meta(root: &str) -> std::io::Result<Meta> {
    let p = Path::new(root).join("meta.toml");
    let txt = fs::read_to_string(p)?;
    let mut created_at = String::new();
    let mut case_count = 0usize;
    let mut gen = 0u64;
    let mut df = Vec::new();
    let mut params = Vec::new();
    let mut word_w = Vec::new();
    let mut l1_hist = Vec::new();
    let mut section = "";
    for line in txt.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = &line[1..line.len() - 1];
            continue;
        }
        let Some(eq) = line.find('=') else { continue };
        let k = line[..eq].trim();
        let v = line[eq + 1..].trim();
        match section {
            "" => {
                if k == "created_at" {
                    created_at = v.trim_matches('"').to_string();
                } else if k == "case_count" {
                    case_count = v.parse().unwrap_or(0);
                } else if k == "gen" {
                    gen = v.parse().unwrap_or(0);
                }
            }
            "df" => {
                if let Ok(n) = v.parse::<usize>() {
                    df.push((k.to_string(), n));
                }
            }
            "params" => {
                if let Some(f) = v.parse::<f64>().ok().filter(|f| f.is_finite()) {
                    params.push((k.to_string(), f));
                }
            }
            "word_w" => {
                if let Some(f) = v.parse::<f64>().ok().filter(|f| f.is_finite()) {
                    word_w.push((k.to_string(), f));
                }
            }
            "l1" if k == "hist" => {
                let body = v.trim().trim_matches('"');
                for tok in body.split(',') {
                    if let Some(f) = tok.trim().parse::<f64>().ok().filter(|f| f.is_finite()) {
                        l1_hist.push(f);
                    }
                }
                l1_hist.truncate(L1_HIST_CAP);
            }
            _ => {}
        }
    }
    Ok(Meta {
        created_at,
        case_count,
        gen,
        df,
        params,
        word_w,
        l1_hist,
    })
}

/// 库代际 +1 并原子写回（case 写入/删除的兜底失效信号；失败返回 Err，
/// 由调用方决定是否致命——case 已写盘，gen 落后只会多一次重算，不丢数据）。
pub(crate) fn bump_gen(root: &str) -> std::io::Result<()> {
    let _g = lock::acquire(root, "meta.lock")?;
    let p = Path::new(root).join("meta.toml");
    let txt = fs::read_to_string(&p)?;
    let mut next: Option<u64> = None;
    let mut out: Vec<String> = Vec::new();
    for line in txt.lines() {
        let t = line.trim();
        if !t.starts_with('[') && !t.starts_with('#') {
            if let Some(eq) = t.find('=') {
                if t[..eq].trim() == "gen" {
                    if let Ok(v) = t[eq + 1..].trim().parse::<u64>() {
                        next = Some(v.saturating_add(1));
                        out.push(format!("gen = {}", next.unwrap()));
                        continue;
                    }
                }
            }
        }
        out.push(line.to_string());
    }
    let gen = next.unwrap_or(1);
    if next.is_none() {
        // 无 gen 行（老库）：在 case_count 之后补一行。
        if let Some(ci) = out.iter().position(|l| l.trim().starts_with("case_count")) {
            out.insert(ci + 1, format!("gen = {gen}"));
        } else {
            out.push(format!("gen = {gen}"));
        }
    }
    let mut body = out.join("\n");
    body.push('\n');
    lock::atomic_write(&p, body.as_bytes())
}

/// 追加一条 top1_sim 到 L1 历史并重写 meta.toml（O5 自适应基线数据源）。
/// 失败静默（旁路：历史只是置信校准的输入，不影响召回正确性）。
pub(crate) fn push_l1_hist(root: &str, sim: f64) {
    let Ok(_g) = lock::acquire(root, "meta.lock") else {
        return;
    };
    let Ok(mut meta) = load_meta(root) else {
        return;
    };
    meta.l1_hist.push(sim);
    if meta.l1_hist.len() > L1_HIST_CAP {
        let drop = meta.l1_hist.len() - L1_HIST_CAP;
        meta.l1_hist.drain(..drop);
    }
    let hist_txt = meta
        .l1_hist
        .iter()
        .map(|f| format!("{f:.4}"))
        .collect::<Vec<_>>()
        .join(",");
    // 重写 meta.toml：保留 df/params/word_w 节，仅更新 [l1] hist。
    let p = Path::new(root).join("meta.toml");
    let Ok(txt) = fs::read_to_string(&p) else {
        return;
    };
    let mut out: Vec<String> = Vec::new();
    let mut in_l1 = false;
    let mut hist_written = false;
    for line in txt.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            in_l1 = t == "[l1]";
        } else if in_l1 && t.starts_with("hist") {
            out.push(format!("hist = \"{hist_txt}\""));
            hist_written = true;
            continue;
        }
        out.push(line.to_string());
    }
    if !hist_written {
        // 无 [l1] 节：在文件末尾追加。
        out.push("[l1]".to_string());
        out.push(format!("hist = \"{hist_txt}\""));
    }
    let mut body = out.join("\n");
    body.push('\n');
    let _ = lock::atomic_write(&p, body.as_bytes());
}

/// 词级调权：good 词 +step / bad 词 −step（默认上下限 ±0.5，§3.9），重写 [word_w] 节。
pub(crate) fn bump_word_w(root: &str, terms: &[String], good: bool) {
    let Ok(_g) = lock::acquire(root, "meta.lock") else {
        return;
    };
    let Ok(mut meta) = load_meta(root) else {
        return;
    };
    let step = if good {
        meta.param("feedback_step")
    } else {
        -meta.param("feedback_step_bad")
    };
    let limit = meta.param("word_w_limit").max(0.0);
    for t in terms {
        if t.len() < 2 {
            continue;
        }
        let cur = meta
            .word_w
            .iter()
            .find(|(k, _)| k == t)
            .map(|(_, v)| *v)
            .unwrap_or(0.0);
        let next = (cur + step).clamp(-limit, limit);
        if let Some(e) = meta.word_w.iter_mut().find(|(k, _)| k == t) {
            e.1 = next;
        } else {
            meta.word_w.push((t.clone(), next));
        }
    }
    let p = Path::new(root).join("meta.toml");
    let Ok(txt) = fs::read_to_string(&p) else {
        return;
    };
    let mut out: Vec<String> = Vec::new();
    let mut wrote_w = false;
    for line in txt.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            if t == "[word_w]" {
                // 跳过旧节，统一在末尾重写。
                wrote_w = true;
                continue;
            }
            if !wrote_w && !meta.word_w.is_empty() {
                out.push("[word_w]".to_string());
                for (k, v) in &meta.word_w {
                    out.push(format!("{k} = {v:.3}"));
                }
                out.push("".to_string());
                wrote_w = true;
            }
        }
        out.push(line.to_string());
    }
    if !wrote_w {
        out.push("[word_w]".to_string());
        for (k, v) in &meta.word_w {
            out.push(format!("{k} = {v:.3}"));
        }
    }
    let mut body = out.join("\n");
    body.push('\n');
    let _ = lock::atomic_write(&p, body.as_bytes());
}

/// 写/更新 [params] 单个参数：节内有 key 原位替换，无则节内末尾插入，无节则文件尾建节。
/// 参数写入的唯一入口；建库模板、L1 自适应与测试共用。
pub(crate) fn set_param(root: &str, key: &str, val: f64) -> std::io::Result<()> {
    let _g = lock::acquire(root, "meta.lock")?;
    let p = Path::new(root).join("meta.toml");
    let txt = fs::read_to_string(&p)?;
    let key_line = format!("{key} = {val:.3}");
    let mut lines: Vec<String> = txt.lines().map(|s| s.to_string()).collect();
    if let Some(pi) = lines.iter().position(|l| l.trim() == "[params]") {
        let mut replaced = false;
        for l in lines.iter_mut().skip(pi + 1) {
            let t = l.trim();
            if t.starts_with('[') {
                break;
            }
            if let Some(eq) = t.find('=') {
                if t[..eq].trim() == key {
                    *l = key_line.clone();
                    replaced = true;
                    break;
                }
            }
        }
        if !replaced {
            let end = lines
                .iter()
                .enumerate()
                .skip(pi + 1)
                .find(|(_, l)| l.trim().starts_with('['))
                .map(|(i, _)| i)
                .unwrap_or(lines.len());
            lines.insert(end, key_line.clone());
        }
    } else {
        lines.push("[params]".to_string());
        lines.push(key_line.clone());
    }
    let mut body = lines.join("\n");
    body.push('\n');
    lock::atomic_write(&p, body.as_bytes())
}

fn meta_toml(case_count: usize) -> String {
    let mut body = format!(
        "schema = {SCHEMA}\nembed_engine = \"{ENG_VER}\"\ncreated_at = \"{}\"\ncase_count = {case_count}\ngen = 0\n\n[params]\n",
        utc_now()
    );
    for (key, value) in DEFAULT_PARAMS {
        body.push_str(&format!("{key} = {value}\n"));
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_root(tag: &str) -> String {
        let r = std::env::temp_dir().join(format!(
            "scx-meta-{}-{}",
            tag,
            crate::memory::util::fnv1a("m") & 0xff
        ));
        let _ = fs::remove_dir_all(&r);
        r.to_str().unwrap().to_string()
    }

    #[test]
    fn meta_roundtrip() {
        let root = tmp_root("rt");
        ensure_root(&root).unwrap();
        let m = load_meta(&root).unwrap();
        assert_eq!(m.case_count, 0);
        assert!(
            m.created_at.ends_with('Z') && m.created_at.contains('T'),
            "建库时间必须可读，供评估冻结时钟: {:?}",
            m.created_at
        );
        assert!(m.df.is_empty(), "新库 df 快照应为空（摄取后才增量写入）");
        assert!(
            m.params
                .iter()
                .any(|(k, v)| k == "bm25_k1" && (*v - 1.2).abs() < 1e-9),
            "默认参数应含 bm25_k1=1.2"
        );
        assert!(
            m.params
                .iter()
                .any(|(k, v)| k == "query_field_weight" && (*v - 0.80).abs() < 1e-9),
            "新库默认必须启用降权 query-field 重排"
        );
        assert!(
            m.params
                .iter()
                .any(|(k, v)| k == "sibling_min_sim" && (*v - 0.30).abs() < 1e-9),
            "新库默认弱词面锚点不得启用同族收紧"
        );
        assert!(
            m.params
                .iter()
                .any(|(k, v)| k == "synonym_weight" && (*v - 0.5).abs() < 1e-9),
            "新库默认同义扩展必须降权"
        );
        assert!(
            m.params
                .iter()
                .any(|(k, v)| k == "bucket_bypass_sim" && (*v - 0.30).abs() < 1e-9),
            "新库默认低置信时必须绕过场景桶硬筛"
        );
        let _ = fs::remove_dir_all(&root);
    }
}
