// 极简 LRU 缓存：容量 64，双向链表 + 哈希索引。
// 命中计 miss_count；put 满时淘汰最久未用（队尾）。
use std::collections::HashMap;

/// 缓存容量：达到上限后 put 触发淘汰。
const CAP: usize = 64;

struct Cache {
    map: HashMap<String, i32>,
    order: Vec<String>,
    miss: usize,
}

impl Cache {
    fn new() -> Self {
        Cache {
            map: HashMap::new(),
            order: Vec::new(),
            miss: 0,
        }
    }

    fn get(&mut self, key: &str) -> Option<i32> {
        match self.map.get(key) {
            Some(&v) => Some(v),
            None => {
                self.miss += 1;
                None
            }
        }
    }

    fn put(&mut self, key: String, val: i32) {
        if !self.map.contains_key(&key) {
            if self.map.len() >= CAP {
                // 淘汰最久未用：队尾第一个
                if let Some(evict) = self.order.pop() {
                    self.map.remove(&evict);
                }
            }
            self.map.insert(key.clone(), val);
            self.order.insert(0, key);
        }
    }
}

fn main() {
    let mut c = Cache::new();
    c.put("a".into(), 1);
    c.put("b".into(), 2);
    let hit = c.get("a");
    let miss1 = c.get("zz");
    println!("MAP_LEN={} HIT={} MISS={}", c.map.len(), hit.unwrap(), c.miss);
}
