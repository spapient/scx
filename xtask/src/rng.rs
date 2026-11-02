//! CPython `random.Random` 兼容的 MT19937。
//!
//! 仅用于迁移期 parity：同一 seed 与同一调用序列必须与原 Python 工具
//! 逐位一致，才能对既有冻结基线做字节级对拍。Rust 侧已有的 xorshift
//! （`memory::eval`）算法不同，不能直接替换已冻结的统计输出。

pub(crate) struct PyRng {
    mt: [u32; 624],
    index: usize,
}

impl PyRng {
    pub(crate) fn new(seed: i64) -> Self {
        let key = seed_key(seed);
        let mut rng = Self {
            mt: [0; 624],
            index: 624,
        };
        rng.init_by_array(&key);
        rng
    }

    /// CPython `Random(str)`：`int.from_bytes(s + sha512(s), "big")` 后按
    /// 32 位小端词喂给 `init_by_array`。
    pub(crate) fn from_seed_bytes(data: &[u8]) -> Self {
        let digest = super::sha512::digest(data);
        let mut joined = data.to_vec();
        joined.extend_from_slice(&digest);
        let key = bytes_key(&joined);
        let mut rng = Self {
            mt: [0; 624],
            index: 624,
        };
        rng.init_by_array(&key);
        rng
    }

    /// CPython `random.shuffle`：从尾部向前，`j = randrange(i + 1)` 交换。
    pub(crate) fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.randrange(i + 1);
            items.swap(i, j);
        }
    }

    fn init_genrand(&mut self, seed: u32) {
        self.mt[0] = seed;
        for i in 1..624 {
            let prev = self.mt[i - 1];
            self.mt[i] = 1_812_433_253u32
                .wrapping_mul(prev ^ (prev >> 30))
                .wrapping_add(i as u32);
        }
        self.index = 624;
    }

    fn init_by_array(&mut self, key: &[u32]) {
        self.init_genrand(19_650_218);
        let mut i = 1usize;
        let mut j = 0usize;
        let mut k = 624.max(key.len());
        while k > 0 {
            let prev = self.mt[i - 1];
            self.mt[i] = (self.mt[i] ^ (prev ^ (prev >> 30)).wrapping_mul(1_664_525))
                .wrapping_add(key[j])
                .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= 624 {
                self.mt[0] = self.mt[623];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
            k -= 1;
        }
        k = 623;
        while k > 0 {
            let prev = self.mt[i - 1];
            self.mt[i] = (self.mt[i] ^ (prev ^ (prev >> 30)).wrapping_mul(1_566_083_941))
                .wrapping_sub(i as u32);
            i += 1;
            if i >= 624 {
                self.mt[0] = self.mt[623];
                i = 1;
            }
            k -= 1;
        }
        self.mt[0] = 0x8000_0000;
        self.index = 624;
    }

    fn twist(&mut self) {
        for i in 0..624 {
            let y = (self.mt[i] & 0x8000_0000) | (self.mt[(i + 1) % 624] & 0x7fff_ffff);
            let mut next = self.mt[(i + 397) % 624] ^ (y >> 1);
            if y & 1 != 0 {
                next ^= 0x9908_b0df;
            }
            self.mt[i] = next;
        }
        self.index = 0;
    }

    fn next_u32(&mut self) -> u32 {
        if self.index >= 624 {
            self.twist();
        }
        let mut y = self.mt[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^= y >> 18;
        y
    }

    /// CPython `random()`：两次 32 位输出拼 53 位尾数。
    pub(crate) fn random(&mut self) -> f64 {
        let a = (self.next_u32() >> 5) as f64;
        let b = (self.next_u32() >> 6) as f64;
        (a * 67_108_864.0 + b) / 9_007_199_254_740_992.0
    }

    /// CPython `randrange(n)`：`getrandbits(bit_length(n))` + 拒绝采样。
    pub(crate) fn randrange(&mut self, n: usize) -> usize {
        debug_assert!(n > 0);
        let n = n as u64;
        let bits = 64 - n.leading_zeros();
        loop {
            let r = self.getrandbits(bits);
            if r < n {
                return r as usize;
            }
        }
    }

    fn getrandbits(&mut self, k: u32) -> u64 {
        assert!(k <= 64, "getrandbits 仅支持 <=64 位");
        if k == 0 {
            return 0;
        }
        if k <= 32 {
            return (self.next_u32() >> (32 - k)) as u64;
        }
        let words = (k as usize).div_ceil(32);
        let shift = (words as u32 * 32) - k;
        let mut out = 0u64;
        for i in 0..words {
            let mut word = self.next_u32() as u64;
            if i + 1 == words {
                word >>= shift;
            }
            out |= word << (32 * i);
        }
        out
    }
}

/// CPython 对整数 seed 取绝对值后按 32 位小端切词。
fn seed_key(seed: i64) -> Vec<u32> {
    let mut v = seed.unsigned_abs();
    let mut key = Vec::new();
    loop {
        key.push((v & 0xffff_ffff) as u32);
        v >>= 32;
        if v == 0 {
            break;
        }
    }
    key
}

/// 大端字节串 → CPython `init_by_array` 需要的 32 位小端词（低字在前），
/// 并去掉最高位为零的词（对应 CPython 按 bit_length 计算 keyused）。
fn bytes_key(data: &[u8]) -> Vec<u32> {
    if data.is_empty() {
        return vec![0];
    }
    let mut padded = vec![0u8; (4 - data.len() % 4) % 4];
    padded.extend_from_slice(data);
    let mut key: Vec<u32> = padded
        .rchunks(4)
        .map(|chunk| u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();
    while key.len() > 1 && *key.last().expect("non-empty") == 0 {
        key.pop();
    }
    key
}

#[cfg(test)]
mod tests {
    use super::PyRng;

    #[test]
    fn random_matches_cpython() {
        // python3 -c "import random; r=random.Random(20261023); print([repr(r.random()) for _ in range(5)])"
        let want = [
            0.2529479731947105f64,
            0.7788603842413495,
            0.5336638342796356,
            0.2407135559114315,
            0.36699767427812235,
        ];
        let mut rng = PyRng::new(20_261_023);
        for expected in want {
            assert_eq!(rng.random().to_bits(), expected.to_bits());
        }
    }

    #[test]
    fn randrange_matches_cpython() {
        // python3 -c "import random; r=random.Random(20261023); print([r.randrange(150) for _ in range(10)])"
        let want = [64usize, 67, 51, 136, 90, 61, 146, 93, 5, 104];
        let mut rng = PyRng::new(20_261_023);
        for expected in want {
            assert_eq!(rng.randrange(150), expected);
        }
    }

    #[test]
    fn string_seed_shuffle_matches_cpython() {
        // python3 -c "import random; x=list(range(8)); random.Random('contain-rand-v1:build-01').shuffle(x); print(x)"
        let mut items: Vec<usize> = (0..8).collect();
        PyRng::from_seed_bytes(b"contain-rand-v1:build-01").shuffle(&mut items);
        assert_eq!(items, vec![5, 7, 6, 3, 4, 2, 1, 0]);
    }

    #[test]
    fn shared_stream_matches_cpython() {
        // bootstrap 后继续用同一 rng 做 permutation（Python 版就是这样共享的）。
        // python3 -c "import random; r=random.Random(7); print([r.randrange(10) for _ in range(3)], [r.random() for _ in range(3)])"
        let mut rng = PyRng::new(7);
        let draws: Vec<usize> = (0..3).map(|_| rng.randrange(10)).collect();
        assert_eq!(draws, vec![5, 2, 6]);
        let next = [rng.random(), rng.random(), rng.random()];
        let want = [
            0.6509344730398537f64,
            0.07243628666754276,
            0.5358820043066892,
        ];
        for (got, expected) in next.into_iter().zip(want) {
            assert_eq!(got.to_bits(), expected.to_bits());
        }
    }
}
