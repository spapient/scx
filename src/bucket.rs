//! 类型路由 + 计量桶 + 前后快照（P1 治理层）。
//!
//! 把全量上下文载荷按 `ContentType` 分桶，逐桶复用 `compress` 内核压缩，
//! 聚合成全局与分桶维度的 token 缩减快照；状态可序列化为标准化 JSON（P3 消费）。
//! 本层是薄壳，不复制裁剪逻辑；价值梯度预算分配（P2）在此之上单独叠加。

#![forbid(unsafe_code)]

use crate::{compress, Config, Mode};

/// 内容类型，按价值梯度排列：静态契约 > 系统定义 > 会话消息 > 临时工具输出。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    StaticContract,
    SystemDefinition,
    SessionMessage,
    ToolOutput,
}

impl ContentType {
    pub const fn as_str(self) -> &'static str {
        match self {
            ContentType::StaticContract => "static-contract",
            ContentType::SystemDefinition => "system-definition",
            ContentType::SessionMessage => "session-message",
            ContentType::ToolOutput => "tool-output",
        }
    }

    /// 价值梯度默认预算（ratio，越大保留越多可选内容）：静态契约最保守、工具输出最激进。
    pub const fn default_ratio(self) -> f64 {
        match self {
            ContentType::StaticContract => 1.0,
            ContentType::SystemDefinition => 0.8,
            ContentType::SessionMessage => 0.5,
            ContentType::ToolOutput => 0.2,
        }
    }
}

/// 单桶计量。
#[derive(Debug, Clone, Copy)]
pub struct Bucket {
    pub ty: ContentType,
    pub tokens_in: f64,
    pub tokens_out: f64,
    pub cut_pct: f64,
    pub kept: usize,
    pub total: usize,
    pub mode: Mode,
}

/// 前后双快照：全局 + 分桶。
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub buckets: Vec<Bucket>,
    pub tokens_in: f64,
    pub tokens_out: f64,
    pub cut_pct: f64,
}

fn pct(tin: f64, tout: f64) -> f64 {
    if tin > 0.0 {
        (tin - tout) * 100.0 / tin
    } else {
        0.0
    }
}

/// 分桶压缩：每桶独立 `compress`，聚合全局与分桶计量。
pub fn compress_parts(parts: &[(ContentType, &str)], query: &str, cfg: &Config) -> Snapshot {
    let mut buckets = Vec::with_capacity(parts.len());
    let (mut tin, mut tout) = (0.0, 0.0);
    for (ty, text) in parts {
        let mut c = cfg.clone();
        c.ratio = ty.default_ratio();
        let o = compress(text, query, &c);
        tin += o.tokens_in;
        tout += o.tokens;
        buckets.push(Bucket {
            ty: *ty,
            tokens_in: o.tokens_in,
            tokens_out: o.tokens,
            cut_pct: pct(o.tokens_in, o.tokens),
            kept: o.kept,
            total: o.total,
            mode: o.mode,
        });
    }
    Snapshot {
        buckets,
        tokens_in: tin,
        tokens_out: tout,
        cut_pct: pct(tin, tout),
    }
}

/// 标准化 JSON 输出（P3 可视化/链路监控消费）。
pub fn render(s: &Snapshot) -> String {
    let mut out = format!(
        "{{\"tokens_in\":{:.0},\"tokens_out\":{:.0},\"cut_pct\":{:.1},\"buckets\":[",
        s.tokens_in, s.tokens_out, s.cut_pct
    );
    for (i, b) in s.buckets.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"type\":\"{}\",\"tokens_in\":{:.0},\"tokens_out\":{:.0},\"cut_pct\":{:.1},\"kept\":{},\"total\":{},\"mode\":\"{}\"}}",
            b.ty.as_str(),
            b.tokens_in,
            b.tokens_out,
            b.cut_pct,
            b.kept,
            b.total,
            b.mode
        ));
    }
    out.push_str("]}");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text() -> String {
        let mut lines: Vec<String> = (0..200)
            .map(|i| format!("filler alpha beta line {i}"))
            .collect();
        lines[100] = "ERROR db timeout conn_id=9f2a".to_string();
        lines.join("\n")
    }

    #[test]
    fn compress_parts_aggregates_and_buckets() {
        let t = text();
        let parts = [
            (ContentType::StaticContract, t.as_str()),
            (ContentType::ToolOutput, t.as_str()),
        ];
        let s = compress_parts(&parts, "db timeout", &Config::default());
        assert_eq!(s.buckets.len(), 2);
        assert_eq!(s.buckets[0].ty, ContentType::StaticContract);
        assert_eq!(s.buckets[1].ty, ContentType::ToolOutput);
        assert!(s.tokens_in > 0.0);
        assert!(s.tokens_out < s.tokens_in);
        assert!(s.cut_pct > 0.0);
    }

    #[test]
    fn render_is_valid_json_shape() {
        let t = text();
        let s = compress_parts(
            &[(ContentType::ToolOutput, t.as_str())],
            "db timeout",
            &Config::default(),
        );
        let j = render(&s);
        assert!(j.starts_with("{\"tokens_in\":"));
        assert!(j.contains("\"buckets\":[{\"type\":\"tool-output\""));
        assert!(j.ends_with("]}"));
    }
}
