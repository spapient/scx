//! meta.toml 读写与库根初始化（零依赖行式子集）。
//!
//! 布局见 store.rs 头注释。meta 是"库元信息"（schema/引擎版本/case 数/DF 快照/参数），
//! 与 case 资产分离：损坏时整库仍可用，rebuild 可重建。

// 阶段化开发过渡：df 快照为 P2（领域 IDF）预留，非 test 构建暂放宽 dead_code。
#![cfg_attr(not(test), allow(dead_code))]

use crate::memory::model::{ENG_VER, SCHEMA};
use crate::memory::util::utc_now;
use std::fs;
use std::path::Path;

/// 建库根目录（含 cases/、index/）与初始 meta.toml。
pub(crate) fn ensure_root(root: &str) -> std::io::Result<()> {
    for sub in ["", "cases", "index"] {
        let p = Path::new(root).join(sub);
        if !p.exists() {
            fs::create_dir_all(&p)?;
        }
    }
    let meta_p = Path::new(root).join("meta.toml");
    if !meta_p.exists() {
        fs::write(&meta_p, meta_toml(0).as_bytes())?;
    }
    Ok(())
}

pub(crate) struct Meta {
    pub case_count: usize,
    pub df: Vec<(String, usize)>,
    pub params: Vec<(String, f64)>,
    /// O5 词级调权表：词 → 增量权重（Rocchio 简化版，§3.9），无词条=0。
    pub word_w: Vec<(String, f64)>,
    /// O5 L1 自适应：最近 top1_sim 滑动历史（上限 L1_HIST_CAP，逗号分隔存 [l1] 节）。
    pub l1_hist: Vec<f64>,
}

pub(crate) const L1_HIST_CAP: usize = 64;

pub(crate) fn load_meta(root: &str) -> std::io::Result<Meta> {
    let p = Path::new(root).join("meta.toml");
    let txt = fs::read_to_string(p)?;
    let mut case_count = 0usize;
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
                if k == "case_count" {
                    case_count = v.parse().unwrap_or(0);
                }
            }
            "df" => {
                if let Ok(n) = v.parse::<usize>() {
                    df.push((k.to_string(), n));
                }
            }
            "params" => {
                if let Ok(f) = v.parse::<f64>() {
                    params.push((k.to_string(), f));
                }
            }
            "word_w" => {
                if let Ok(f) = v.parse::<f64>() {
                    word_w.push((k.to_string(), f));
                }
            }
            "l1" if k == "hist" => {
                let body = v.trim().trim_matches('"');
                for tok in body.split(',') {
                    if let Ok(f) = tok.trim().parse::<f64>() {
                        l1_hist.push(f);
                    }
                }
                l1_hist.truncate(L1_HIST_CAP);
            }
            _ => {}
        }
    }
    Ok(Meta {
        case_count,
        df,
        params,
        word_w,
        l1_hist,
    })
}

/// 追加一条 top1_sim 到 L1 历史并重写 meta.toml（O5 自适应基线数据源）。
/// 失败静默（旁路：历史只是置信校准的输入，不影响召回正确性）。
pub(crate) fn push_l1_hist(root: &str, sim: f64) {
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
    let _ = fs::write(&p, body.as_bytes());
}

/// 词级调权：good 词 +step / bad 词 −step（上下限 ±0.5，§3.9），重写 [word_w] 节。
pub(crate) fn bump_word_w(root: &str, terms: &[String], good: bool) {
    let step = if good { 0.1 } else { -0.15 };
    let Ok(mut meta) = load_meta(root) else {
        return;
    };
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
        let next = (cur + step).clamp(-0.5, 0.5);
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
    let _ = fs::write(&p, body.as_bytes());
}

/// 写/更新 [params] 单个参数：节内有 key 原位替换，无则节内末尾插入，无节则文件尾建节。
/// 供测试与调参共用（P4.3 edge_enable/edge_weight 等）。
pub(crate) fn set_param(root: &str, key: &str, val: f64) -> std::io::Result<()> {
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
    fs::write(&p, body.as_bytes())
}

fn meta_toml(case_count: usize) -> String {
    format!(
        "schema = {SCHEMA}\nembed_engine = \"{ENG_VER}\"\ncreated_at = \"{}\"\ncase_count = {case_count}\n\n[params]\nbm25_k1 = 1.2\nbm25_b = 0.75\nfield_weight_error_code = 3.0\nfield_weight_file_loc = 2.5\nfield_weight_symbol = 2.0\nfield_weight_num_fp = 2.0\ndecay_tau_days = 90\ndecay_tau_solved = 180\ndecay_tau_regressed = 30\nmmr_lambda = 0.5\nfeedback_step = 0.1\ncache_sim_threshold = 0.85\nl1_base = 0.55\nl1_margin_min = 0.15\nquality_prior = 0.5\ngc_ttl_regressed = 180\ngc_ttl_stale = 365\nedge_enable = 1\nedge_weight = 1.0\n",
        utc_now()
    )
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
        assert!(m.df.is_empty(), "新库 df 快照应为空（摄取后才增量写入）");
        assert!(
            m.params
                .iter()
                .any(|(k, v)| k == "bm25_k1" && (*v - 1.2).abs() < 1e-9),
            "默认参数应含 bm25_k1=1.2"
        );
        let _ = fs::remove_dir_all(&root);
    }
}
