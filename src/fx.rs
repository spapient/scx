//! 确定性快速哈希（FxHash 风格），供查找/计数型 `HashMap` 使用。
//!
//! std 默认 SipHash 抗哈希碰撞 DoS，但代价是随机种子——同一进程内两次运行
//! 迭代顺序都不同，与本工具“同输入同输出”的确定性承诺相悖。本工具是本地、
//! 无网络攻击面的确定性工具，改用无随机种子的 FxHash：
//! - 更快（乘法散列，无 SipHash 多轮混淆）；
//! - 跨进程迭代顺序稳定（进一步强化确定性）；
//! - 使用方均“先查表计数、按键引用或排序输出”，不依赖裸迭代顺序。

#![forbid(unsafe_code)]

use std::hash::{BuildHasherDefault, Hasher};

/// rustc 内部 FxHasher 的等价实现（乘法散列，零依赖）。
#[derive(Default)]
pub(crate) struct FxHasher {
    hash: u64,
}

const SEED: u64 = 0x517c_c1b7_2722_0a95;

impl FxHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            self.add(u64::from_ne_bytes(c.try_into().unwrap()));
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut last = [0u8; 8];
            last[..rest.len()].copy_from_slice(rest);
            self.add(u64::from_ne_bytes(last));
        }
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(i as u64);
    }

    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add(i as u64);
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(i as u64);
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }
}

pub(crate) type FxBuildHasher = BuildHasherDefault<FxHasher>;
pub(crate) type FxHashMap<K, V> = std::collections::HashMap<K, V, FxBuildHasher>;
