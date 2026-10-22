//! case 数据模型（与设计 v0.2 schema=2 对齐）。
//!
//! 纯数据 + 默认构造，无 I/O；序列化/反序列化在 store.rs（case_to_val/val_to_case）。

use crate::memory::util::utc_now;

pub(crate) const SCHEMA: u32 = 2;
pub(crate) const ENG_VER: &str = "stat-v1";

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Fields {
    pub error_code: Vec<String>,
    pub file_loc: Vec<String>,
    pub symbol: Vec<String>,
    pub num_fp: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CaseStats {
    pub kept: usize,
    pub total: usize,
    pub cut_pct: f64,
    pub risk: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Feedback {
    pub good: usize,
    pub bad: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Case {
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    pub query: String,
    pub terms: Vec<String>,
    pub text: String,
    /// 去重指纹（norm(query)+norm(text) 的 FNV-1a，见 ingest::fingerprint）。
    pub fp: String,
    pub source: Vec<String>,
    pub tags: Vec<String>,
    pub bucket: String,
    pub fields: Fields,
    pub status: String,
    pub stats: CaseStats,
    pub forces: Vec<String>,
    pub feedback: Feedback,
    pub quality: f64,
}

impl Case {
    pub fn new(id: String, query: String, terms: Vec<String>, text: String) -> Case {
        let now = utc_now();
        Case {
            id,
            created_at: now.clone(),
            updated_at: now,
            query,
            terms,
            text,
            fp: String::new(),
            source: Vec::new(),
            tags: Vec::new(),
            bucket: String::new(),
            fields: Fields::default(),
            status: "open".to_string(),
            stats: CaseStats {
                kept: 0,
                total: 0,
                cut_pct: 0.0,
                risk: "LOW".to_string(),
            },
            forces: Vec::new(),
            feedback: Feedback { good: 0, bad: 0 },
            quality: 0.0,
        }
    }
}
