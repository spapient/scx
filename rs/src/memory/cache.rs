//! 会话缓存（O3）：进程内存 LRU，键 = query 指纹 + 30 分钟窗口，仅 recall 路径。
//!
//! 同一会话连续追问同一问题直接命中，零磁盘检索。窗口从首次命中时间起算
//! （30 分钟），窗口外 miss 重新召回。进程退出即释放——无持久化、无文件 I/O。
//! 线程安全：`OnceLock<Mutex<Cache>>` 全局单例，懒初始化。

use crate::memory::util::fnv1a;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// 缓存窗口（秒）：30 分钟。
const WINDOW_SECS: u64 = 30 * 60;
/// LRU 容量上限。
const CAP: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CacheHit {
    /// 快照：id + score + sim（P2.6 后完整保留，L1 自适应过滤可直接复用）。
    pub hits: Vec<(String, f64, f64)>,
    /// 首次命中时间（Unix 秒），用于窗口判定。
    pub born: u64,
    /// 写入时的库代际（P4.4 评审处置）：与当前 meta gen 不符即 miss，
    /// 防止 store/rate/delete 后会话缓存仍返回旧快照。
    pub gen: u64,
}

struct Cache {
    map: HashMap<u64, CacheHit>,
    /// 插入顺序队列（近似 LRU：命中时移到队尾）。
    q: Vec<u64>,
}

impl Cache {
    fn new() -> Cache {
        Cache {
            map: HashMap::new(),
            q: Vec::new(),
        }
    }

    fn touch(&mut self, key: u64) {
        if let Some(pos) = self.q.iter().position(|k| *k == key) {
            self.q.remove(pos);
        }
        self.q.push(key);
        while self.q.len() > CAP {
            let old = self.q.remove(0);
            self.map.remove(&old);
        }
    }

    fn get(&mut self, key: u64, now: u64, gen: u64) -> Option<CacheHit> {
        match self.map.get(&key) {
            Some(h) => {
                if h.gen != gen {
                    // 库代际变化（store/rate/delete）：快照过期，清键走正常召回。
                    self.map.remove(&key);
                    if let Some(pos) = self.q.iter().position(|k| *k == key) {
                        self.q.remove(pos);
                    }
                    None
                } else if now.saturating_sub(h.born) > WINDOW_SECS {
                    // 窗口外：清除该键（会话级过期，非 LRU 淘汰）。
                    self.map.remove(&key);
                    if let Some(pos) = self.q.iter().position(|k| *k == key) {
                        self.q.remove(pos);
                    }
                    None
                } else {
                    let h = h.clone();
                    self.touch(key);
                    Some(h)
                }
            }
            None => None,
        }
    }

    fn put(&mut self, key: u64, hit: CacheHit) {
        self.map.insert(key, hit);
        self.touch(key);
    }
}

/// 全局单例（进程内，仅 recall 路径）。
fn cache() -> &'static Mutex<Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Cache::new()))
}

/// query 指纹（确定性；保留数字的身份归一化后 FNV-1a 低 64 位）。
/// 不能用 `norm_into`：8080/9090 折叠成同一键会把 A 查询的快照返回给 B。
fn qkey(query: &str) -> u64 {
    let mut out = String::new();
    crate::norm::norm_identity_into(query, &mut out);
    fnv1a(&out)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 窗口内且代际一致命中 → Some(快照)；否则 None（调用方走正常召回）。
pub(crate) fn get(query: &str, gen: u64) -> Option<Vec<(String, f64, f64)>> {
    let key = qkey(query);
    let now = now_secs();
    cache()
        .lock()
        .ok()
        .and_then(|mut c| c.get(key, now, gen))
        .map(|h| h.hits)
}

/// 写入缓存（召回成功后调用；快照含 score/sim，供 L1 过滤复用）。
/// 记录写入时的库代际，代际变化后自动失效。
pub(crate) fn put(query: &str, hits: Vec<(String, f64, f64)>, gen: u64) {
    let key = qkey(query);
    let born = now_secs();
    if let Ok(mut c) = cache().lock() {
        c.put(key, CacheHit { hits, born, gen });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset() {
        // 测试间清空全局缓存：重建单例内部状态不可行（OnceLock 不可重置），
        // 用不同 query 规避跨测试污染；每条测试独立 query。
    }

    #[test]
    fn session_cache_hit_within_window() {
        reset();
        assert_eq!(get("缓存命中测试 query alpha", 1), None, "首次必 miss");
        put(
            "缓存命中测试 query alpha",
            vec![("c1".to_string(), 1.5, 0.9), ("c2".to_string(), 1.2, 0.8)],
            1,
        );
        let hit = get("缓存命中测试 query alpha", 1).unwrap();
        assert_eq!(
            hit,
            vec![("c1".to_string(), 1.5, 0.9), ("c2".to_string(), 1.2, 0.8)],
            "窗口内二次命中（快照逐字段一致）"
        );
        // 变体 query（词面不同，指纹不同）不命中。
        assert_eq!(get("缓存命中测试 query beta", 1), None);
        // 代际变化：库 gen 前进后同 query 必须 miss（走正常召回）。
        assert_eq!(
            get("缓存命中测试 query alpha", 2),
            None,
            "gen 不符必须 miss"
        );
    }

    #[test]
    fn session_cache_expires_after_window() {
        reset();
        // 直接操纵内部状态：写入一条 born=很久以前 的缓存，验证窗口外 miss。
        let key = qkey("过期测试 query");
        let old = CacheHit {
            hits: vec![("cx".to_string(), 1.0, 1.0)],
            born: now_secs() - WINDOW_SECS - 1,
            gen: 0,
        };
        cache().lock().unwrap().put(key, old);
        assert_eq!(get("过期测试 query", 0), None, "窗口外必须 miss");
    }
}
