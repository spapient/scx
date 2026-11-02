//! 评测资产读取：普通文件路径与内置资产选择器共用同一入口。
//!
//! 选择器格式为 `<kind>:<name>`：
//! - `corpus:gauge` / `corpus:v2` / `corpus:real` / `corpus:fidelity`
//! - `labels:gauge-policy` / `labels:gauge-ai` / `labels:gauge-human`
//! - `packet:gauge`
//!
//! 资产包固定在 `config/evaluation.json`；普通路径仍按文件读取，
//! 便于用户提供临时或外部 baseline。

use crate::json::{parse, Json};
use std::path::PathBuf;

const BUNDLE: &str = "config/evaluation.json";

/// 读取文本资产。选择器命中时从内置资产包提取，否则读取文件。
pub fn read_text(path: &str) -> Result<String, String> {
    let Some((kind, name)) = path.split_once(':') else {
        return std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"));
    };
    if !matches!(kind, "corpus" | "labels" | "packet") {
        return std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"));
    }
    let bundle_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(BUNDLE);
    let text = std::fs::read_to_string(&bundle_path)
        .map_err(|e| format!("{}: {e}", bundle_path.display()))?;
    let bundle = parse(&text).map_err(|e| format!("{}: {e}", bundle_path.display()))?;
    let section = match kind {
        "corpus" => "corpora",
        "labels" => "labels",
        _ => "packets",
    };
    bundle
        .get(section)
        .and_then(|value| value.get(name))
        .and_then(Json::str)
        .map(str::to_string)
        .ok_or_else(|| format!("asset: unknown selector `{path}`"))
}

/// 读取并解析 JSON 资产。
pub fn read_json(path: &str) -> Result<Json, String> {
    let text = read_text(path)?;
    parse(&text).map_err(|e| format!("{path}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_selectors_resolve() {
        assert!(read_json("corpus:gauge").is_ok());
        assert!(read_json("labels:gauge-human").is_ok());
        assert!(read_text("packet:gauge").unwrap().contains('\t'));
    }

    #[test]
    fn unknown_selector_is_explicit() {
        let error = read_text("corpus:missing").unwrap_err();
        assert!(error.contains("unknown selector"));
    }
}
