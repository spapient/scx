//! CPython `random.Random` 兼容的 MT19937。
//!
//! 仅用于迁移期 parity：同一 seed 与同一调用序列必须与 `tools/scx.py`
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
        let digest = sha512_digest(data);
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

const K: [u64; 80] = [
    0x428a2f98d728ae22,
    0x7137449123ef65cd,
    0xb5c0fbcfec4d3b2f,
    0xe9b5dba58189dbbc,
    0x3956c25bf348b538,
    0x59f111f1b605d019,
    0x923f82a4af194f9b,
    0xab1c5ed5da6d8118,
    0xd807aa98a3030242,
    0x12835b0145706fbe,
    0x243185be4ee4b28c,
    0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f,
    0x80deb1fe3b1696b1,
    0x9bdc06a725c71235,
    0xc19bf174cf692694,
    0xe49b69c19ef14ad2,
    0xefbe4786384f25e3,
    0x0fc19dc68b8cd5b5,
    0x240ca1cc77ac9c65,
    0x2de92c6f592b0275,
    0x4a7484aa6ea6e483,
    0x5cb0a9dcbd41fbd4,
    0x76f988da831153b5,
    0x983e5152ee66dfab,
    0xa831c66d2db43210,
    0xb00327c898fb213f,
    0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2,
    0xd5a79147930aa725,
    0x06ca6351e003826f,
    0x142929670a0e6e70,
    0x27b70a8546d22ffc,
    0x2e1b21385c26c926,
    0x4d2c6dfc5ac42aed,
    0x53380d139d95b3df,
    0x650a73548baf63de,
    0x766a0abb3c77b2a8,
    0x81c2c92e47edaee6,
    0x92722c851482353b,
    0xa2bfe8a14cf10364,
    0xa81a664bbc423001,
    0xc24b8b70d0f89791,
    0xc76c51a30654be30,
    0xd192e819d6ef5218,
    0xd69906245565a910,
    0xf40e35855771202a,
    0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8,
    0x1e376c085141ab53,
    0x2748774cdf8eeb99,
    0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63,
    0x4ed8aa4ae3418acb,
    0x5b9cca4f7763e373,
    0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc,
    0x78a5636f43172f60,
    0x84c87814a1f0ab72,
    0x8cc702081a6439ec,
    0x90befffa23631e28,
    0xa4506cebde82bde9,
    0xbef9a3f7b2c67915,
    0xc67178f2e372532b,
    0xca273eceea26619c,
    0xd186b8c721c0c207,
    0xeada7dd6cde0eb1e,
    0xf57d4f7fee6ed178,
    0x06f067aa72176fba,
    0x0a637dc5a2c898a6,
    0x113f9804bef90dae,
    0x1b710b35131c471b,
    0x28db77f523047d84,
    0x32caab7b40c72493,
    0x3c9ebe0a15c9bebc,
    0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6,
    0x597f299cfc657e2a,
    0x5fcb6fab3ad6faec,
    0x6c44198c4a475817,
];

const H0: [u64; 8] = [
    0x6a09e667f3bcc908,
    0xbb67ae8584caa73b,
    0x3c6ef372fe94f82b,
    0xa54ff53a5f1d36f1,
    0x510e527fade682d1,
    0x9b05688c2b3e6c1f,
    0x1f83d9abfb41bd6b,
    0x5be0cd19137e2179,
];

fn sha512_digest(data: &[u8]) -> [u8; 64] {
    let mut h = H0;
    let mut message = data.to_vec();
    let bits = (data.len() as u128) * 8;
    message.push(0x80);
    while message.len() % 128 != 112 {
        message.push(0);
    }
    message.extend_from_slice(&bits.to_be_bytes());
    for block in message.chunks_exact(128) {
        compress(&mut h, block);
    }
    let mut out = [0u8; 64];
    for (chunk, word) in out.chunks_exact_mut(8).zip(h) {
        chunk.copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn compress(h: &mut [u64; 8], block: &[u8]) {
    let mut w = [0u64; 80];
    for (i, chunk) in block.chunks_exact(8).enumerate() {
        w[i] = u64::from_be_bytes(chunk.try_into().expect("8-byte chunk"));
    }
    for i in 16..80 {
        let s0 = w[i - 15].rotate_right(1) ^ w[i - 15].rotate_right(8) ^ (w[i - 15] >> 7);
        let s1 = w[i - 2].rotate_right(19) ^ w[i - 2].rotate_right(61) ^ (w[i - 2] >> 6);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
    for i in 0..80 {
        let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
        let ch = (e & f) ^ (!e & g);
        let t1 = hh
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        hh = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (slot, value) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
        *slot = slot.wrapping_add(value);
    }
}

#[cfg(test)]
mod tests {
    use super::{sha512_digest, PyRng};

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
    fn sha512_known_vectors() {
        let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert_eq!(
            hex(&sha512_digest(b"abc")),
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
             2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        );
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
