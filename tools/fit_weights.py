#!/usr/bin/env python3
"""P13-D: 离线拟合 semantic 权重（scx-sem-1 schema）。

无第三方依赖（纯 stdlib）。监督信号为启发式弱监督：
  正样本 = 错误行(error/panic/fail/exception/crash) | 结构行(import/use/#include/fn/pub)
         | query 命中行（代表 query 集）
  负样本 = 其余行
训练语料 = 仓库内真实文本（README/docs + src 抽样，seed 固定保证可复现）。

输出：config/semantic/weights.json（schema scx-sem-1，含 dim/weights/sha256）。
sha256 规范与 Rust `encode()` 一致：每 f64 大端 8 字节拼接。
"""
from __future__ import annotations
import hashlib
import math
import re
import struct
import sys
from pathlib import Path

DIM = 16
ROOT = Path(__file__).resolve().parent.parent

# ---- 特征复刻（与 src/semantic.rs 逐维一致） ----
PREFIXES = ("```", "~~~", "import ", "use ", "#include", "at ", "caused by:",
            "->", "=>", "panic! ", "error:")
WORD_CHARS = set("abcdefghijklmnopqrstuvwxyz0123456789_-./:@\\")


def tokens(lower: str) -> list[str]:
    out: list[str] = []
    cur: list[str] = []
    i = 0
    b = lower.encode()
    n = len(b)
    while i < n:
        c = b[i]
        is_word = chr(c).isalnum() or chr(c) in WORD_CHARS if c < 128 else False
        is_cjk = 0xE4 <= c <= 0xE9 and i + 2 < n
        if is_word or is_cjk:
            cur.append(lower[i:i + (3 if is_cjk else 1)])
        elif cur:
            out.append("".join(cur))
            cur = []
        i += 3 if is_cjk else 1
    if cur:
        out.append("".join(cur))
    return out


def stem(w: str) -> str | None:
    if len(w) < 4 or not w.isascii() or not w.islower() or not w.isalpha():
        return None
    if w.endswith("ing") and len(w) - 3 >= 4:
        return w[:-3]
    if w.endswith("ed") and len(w) - 2 >= 4:
        return w[:-2]
    if w.endswith("ies"):
        return w[:-3] + "y"
    if w.endswith("es") and w[:-2].endswith(("ch", "sh", "s", "x", "z")):
        return w[:-2]
    if w.endswith("s") and not w.endswith("ss") and len(w) - 1 >= 3:
        return w[:-1]
    return None


def root_match(a: str, b: str) -> bool:
    if a == b:
        return True
    sa, sb = stem(a), stem(b)
    if sa and sb:
        return sa == sb
    if sa:
        return sa == b
    if sb:
        return a == sb
    return False


def feats(line: str, terms: list[str], i: int, total: int) -> list[float]:
    lower = line.lower()
    hit = sum(1 for t in terms if t and t in lower)
    overlap = hit / len(terms) if terms else 0.0
    chars = len(line)
    caps = sum(1 for c in line if c.isupper())
    digits = sum(1 for c in line if c.isdigit())
    cap_ratio = caps / chars if chars else 0.0
    dig_ratio = digits / chars if chars else 0.0
    t = line.lstrip()
    tl = t.lower()
    structure = 1.0 if any(t.startswith(p) or tl.startswith(p) for p in PREFIXES) else 0.0
    toks = tokens(lower)
    n_hit = 0
    for q in terms:
        if not q:
            continue
        plain = q.isascii() and q.isalpha()
        if any(root_match(tok, q) for tok in toks) or (not plain and q in lower):
            n_hit += 1
    w_hit = n_hit / len(terms) if terms else 0.0
    cjk = sum(1 for c in line if 0x4E00 <= ord(c) <= 0x9FFF)
    cjk_r = cjk / chars if chars else 0.0
    err = sum(1 for tok in toks if any(c.isalpha() for c in tok) and any(c.isdigit() for c in tok))
    err_r = err / len(toks) if toks else 0.0
    uniq = len(set(toks)) / len(toks) if toks else 0.0
    sym = sum(1 for c in line if not c.isspace() and not c.isalnum())
    sym_r = sym / sum(1 for c in line if not c.isspace()) if any(not c.isspace() for c in line) else 0.0
    pos = i / (total - 1) if total > 1 else 0.0
    edge = 1.0 if i < total // 20 or i >= total - total // 20 else 0.0
    long_f = 1.0 if chars > 200 else 0.0
    code = 1.0 if ("{" in line and "}" in line) or ("(" in line and ")" in line) \
        or "->" in line or "=>" in line or "::" in line else 0.0
    return [overlap, min(hit, 1), math.log1p(chars), cap_ratio, dig_ratio, structure,
            w_hit, cjk_r, err_r, uniq, sym_r, pos, edge, long_f, code, 1.0]


# ---- 语料与弱监督标签 ----
QUERIES = ["error", "compress", "token", "memory", "performance", "crash", "panic"]


def load_lines() -> list[str]:
    files = [ROOT / "README.md", ROOT / "docs" / "benchmarks.md",
             ROOT / "docs" / "release-checklist.md", ROOT / "src" / "lib.rs",
             ROOT / "src" / "trim.rs", ROOT / "src" / "semantic.rs"]
    lines: list[str] = []
    for f in files:
        if f.exists():
            lines.extend(f.read_text(encoding="utf-8").splitlines())
    return [ln for ln in lines if ln.strip()]


def label(line: str) -> int:
    l = line.lower()
    if any(k in l for k in ("error", "panic", "fail", "exception", "crash", "fatal")):
        return 1
    if line.startswith(("import ", "use ", "#include", "fn ", "pub ", "struct ", "impl ")):
        return 1
    if any(q in l for q in QUERIES):
        return 1
    return 0


# ---- 逻辑回归（纯 Python 梯度下降 + L2） ----
def sigmoid(z: float) -> float:
    if z >= 0:
        return 1.0 / (1.0 + math.exp(-z))
    e = math.exp(z)
    return e / (1.0 + e)


def fit(X: list[list[float]], y: list[int], lr: float = 0.05, epochs: int = 120,
        l2: float = 1e-3) -> list[float]:
    n = len(X)
    d = len(X[0])
    w = [0.0] * d
    for _ in range(epochs):
        grad = [0.0] * d
        for xi, yi in zip(X, y):
            p = sigmoid(sum(wi * xj for wi, xj in zip(w, xi)))
            err = p - yi
            for j in range(d):
                grad[j] += err * xi[j]
        for j in range(d):
            w[j] -= lr * (grad[j] / n + l2 * w[j])
    return w


# ---- 输出（scx-sem-1，sha256 与 Rust encode 一致） ----
def encode(ws: list[float]) -> bytes:
    return b"".join(struct.pack(">d", w) for w in ws)


def main() -> int:
    lines = load_lines()
    if len(lines) < 200:
        print(f"语料不足: {len(lines)} 行", file=sys.stderr)
        return 1
    # 行数裁剪 + seed 固定抽样（可复现）。
    step = max(1, len(lines) // 1500)
    sampled = lines[::step][:1500]
    X, y = [], []
    total = len(sampled)
    for i, ln in enumerate(sampled):
        X.append(feats(ln, QUERIES, i, total))
        y.append(label(ln))
    pos = sum(y)
    print(f"语料 {total} 行, 正样本 {pos} ({pos / total:.1%})")
    w = fit(X, y)
    print("权重: " + ", ".join(f"{v:+.3f}" for v in w))
    obj = {
        "schema": "scx-sem-1",
        "dim": DIM,
        "weights": [round(v, 6) for v in w],
    }
    ws = obj["weights"]
    obj["sha256"] = hashlib.sha256(encode(ws)).hexdigest()
    out = ROOT / "config" / "semantic"
    out.mkdir(parents=True, exist_ok=True)
    p = out / "weights.json"
    p.write_text(
        "{\n"
        f'  "schema": "scx-sem-1",\n'
        f'  "dim": {DIM},\n'
        f'  "weights": [{", ".join(str(v) for v in ws)}],\n'
        f'  "sha256": "{obj["sha256"]}"\n'
        "}\n",
        encoding="utf-8",
    )
    print(f"写入 {p.relative_to(ROOT)} (sha256={obj['sha256'][:16]}…)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
