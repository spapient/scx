#!/usr/bin/env python3
"""生成 `src/calib.rs` 的冻结校准密度表，并报告 vs 启发式的 MAE。

对代表性语料采样字符特征（words/ascii_alnum/ascii_punct/newline/cjk/other），用真实
tokenizer（tiktoken cl100k_base）取真值 token 数，按「长度桶」分片做无截距最小二乘
回归，得到每桶每特征的密度，写入 `src/calib.rs` 的 `CALIB_DENSITY`
（[[f64; 5]; 6]）。同时打印校准 vs 启发式的平均绝对误差（MAE，样本内，偏乐观）。

用法：
    python3 tools/calibrate.py [corpus.json] [out.rs]

- corpus.json：JSON 字符串数组（缺省用内置代表性样本）。
- 依赖：`tiktoken`、`numpy`（离线）。缺失时输出「复现启发式」的占位密度。
"""

import json
import sys

BUCKET_BOUNDS = [16, 64, 256, 1024]  # 长度桶上界（非空白字符数）
HEURISTIC = [0.9, 0.05, 0.25, 1.0, 0.6, 0.4]  # 词数主导 + 换行 1 token 的占位密度
FEATURES = ["words", "ascii_alnum", "ascii_punct", "newline", "cjk", "other"]


def full(ch):
    u = ord(ch)
    return (
        0x3400 <= u <= 0x4DBF
        or 0x4E00 <= u <= 0x9FFF
        or 0x3040 <= u <= 0x30FF
        or 0xAC00 <= u <= 0xD7AF
        or 0x20000 <= u <= 0x2FA1F
        or 0x3000 <= u <= 0x303F
        or 0xFF00 <= u <= 0xFFEF
    )


def features(text):
    """字符特征计数：`[words, alnum, punct, newline, cjk, other]`（与 src/token.rs 同口径）。"""
    words = alnum = punct = newline = cjk = other = 0
    in_word = False
    for ch in text:
        if ch.isascii() and ch.isalnum():
            alnum += 1
            if not in_word:
                words += 1
                in_word = True
        else:
            in_word = False
            if ch.isascii() and ch in "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~":
                punct += 1
            elif ch == "\n":
                newline += 1
            elif ch.isascii() and ch.isspace():
                pass  # 普通空白不计入
            elif full(ch):
                cjk += 1
            else:
                other += 1
    return [words, alnum, punct, newline, cjk, other]


def char_count(feat):
    """非空白字符总数（用于长度分桶，不含 words 这类的 run 计数）。"""
    return feat[1] + feat[2] + feat[3] + feat[4] + feat[5]


def bucket_index(chars):
    for i, b in enumerate(BUCKET_BOUNDS):
        if chars <= b:
            return i
    return len(BUCKET_BOUNDS)


def heuristic_estimate(text):
    """复现 `terms::estimate`：ASCII 4 字/tok、CJK 0.6 tok/字、other 2.5 字/tok、截距 +1。"""
    ascii_n = cjk = other = 0
    for ch in text:
        if ch.isascii():
            ascii_n += 1
        elif full(ch):
            cjk += 1
        else:
            other += 1
    return ascii_n / 4.0 + cjk * 0.6 + other / 2.5 + 1.0


def default_corpus():
    """多脚本多行长度的代表性样本（避免单行导致回归退化）。"""
    samples = []
    for i in range(40):
        lines = [
            "INFO heartbeat seq=%d ok pool=3 latency=1ms" % (i * 10 + j)
            for j in range(5 + i % 6)
        ]
        lines.append("ERROR db timeout at 12:01 pool exhausted (conn_id=9f2a) retry=3")
        samples.append("\n".join(lines))
    for i in range(40):
        samples.append("The quick brown fox jumps over the lazy dog " * (i % 10 + 1))
        samples.append("数据库连接超时 连接池耗尽 磁盘空间不足 errno=28")
        samples.append("short")
        samples.append("hello world")
    for i in range(40):
        samples.append(
            "fn main() {\n    let x = 1 + 2;\n    println!(\"{}\", x);\n}\n"
        )
        samples.append(
            '{\n  "type": "object",\n  "properties": {\n    "id": {"type": "integer"}\n  }\n}'
        )
    for i in range(30):
        samples.append("Привет мир как дела " * (i % 6 + 1))
        samples.append("مرحبا بالعالم كيف الحال " * (i % 6 + 1))
        samples.append("🚀 emoji ✨ mixed 中文")
    return samples


def fit_global(samples, ys):
    """全样本无截距最小二乘，返回 6 特征系数；失败回退启发式。"""
    import numpy as np

    x = np.array(samples, dtype=float)
    y = np.array(ys, dtype=float)
    try:
        coef, *_ = np.linalg.lstsq(x, y, rcond=None)
        coef = coef.tolist()
        if all(c >= 0 for c in coef):
            return coef
    except Exception:
        pass
    return list(HEURISTIC)


def fit_bucket(xs, ys, global_coef):
    """对单桶联合回归「有样本的特征」；稀疏特征（该桶计数为 0）回退全局密度。"""
    import numpy as np

    x = np.array(xs, dtype=float)
    y = np.array(ys, dtype=float)
    coef = list(global_coef)
    if x.shape[0] < 3:
        return coef
    col_sum = x.sum(axis=0)
    active = [s for s in range(6) if col_sum[s] > 0]
    if not active:
        return coef
    try:
        c, *_ = np.linalg.lstsq(x[:, active], y, rcond=None)
        c = c.tolist()
        for idx, s in enumerate(active):
            if c[idx] >= 0:
                coef[s] = c[idx]
    except Exception:
        pass
    return coef


def generate_table(samples, ys):
    global_coef = fit_global(samples, ys)
    table = [[0.0] * 5 for _ in range(6)]
    for b in range(5):
        xs_b, ys_b = [], []
        for feat, tok in zip(samples, ys):
            if bucket_index(char_count(feat)) == b:
                xs_b.append(feat)
                ys_b.append(tok)
        coef = fit_bucket(xs_b, ys_b, global_coef)
        for s in range(6):
            table[s][b] = coef[s]
    return table


def estimate(feat, table):
    b = bucket_index(char_count(feat))
    return sum(feat[s] * table[s][b] for s in range(6))


def mae(estimates, ys):
    return sum(abs(e - y) for e, y in zip(estimates, ys)) / len(ys)


def main():
    corpus_path = sys.argv[1] if len(sys.argv) > 1 else None
    out_path = sys.argv[2] if len(sys.argv) > 2 else "src/calib.rs"
    samples = json.load(open(corpus_path, encoding="utf-8")) if corpus_path else default_corpus()

    xs = [features(s) for s in samples]
    try:
        import tiktoken  # noqa: F401

        enc = tiktoken.get_encoding("cl100k_base")
        ys = [len(enc.encode(s)) for s in samples]
        note = "由 tools/calibrate.py 对 cl100k_base 分桶最小二乘回归生成"
    except Exception as exc:  # tiktoken/numpy 缺失 → 占位密度
        ys = None
        note = f"占位密度（复现启发式；未校准：{exc}）"

    if ys is None:
        table = [[d] * 5 for d in HEURISTIC]
        mae_calib = mae_heur = None
    else:
        table = generate_table(xs, ys)
        mae_calib = mae([estimate(f, table) for f in xs], ys)
        mae_heur = mae([heuristic_estimate(s) for s in samples], ys)
        # 留出评估：80/20 分（固定 seed），拟合训练集、评估测试集，得诚实 MAE。
        import random

        random.seed(42)
        idx = list(range(len(samples)))
        random.shuffle(idx)
        split = int(len(samples) * 0.8)
        tr, te = idx[:split], idx[split:]
        held = generate_table([xs[i] for i in tr], [ys[i] for i in tr])
        mae_held = mae([estimate(xs[i], held) for i in te], [ys[i] for i in te])
        mae_held_heur = mae(
            [heuristic_estimate(samples[i]) for i in te], [ys[i] for i in te]
        )
        note += (
            f"（MAE 校准={mae_calib:.2f} 留出={mae_held:.2f} "
            f"启发式={mae_heur:.2f} 留出启发式={mae_held_heur:.2f}）"
        )

    rows = "\n".join(
        "    [%s], // %s" % (", ".join("%r" % v for v in table[s]), FEATURES[s])
        for s in range(6)
    )
    body = f"""//! 冻结的校准密度表（`TokenMode::Calibrated` 口径）。
//!
//! {note}。
//! 改 tokenizer 后重跑 `python3 tools/calibrate.py` 再提交。

#![forbid(unsafe_code)]

/// 特征序：`[words, ascii_alnum, ascii_punct, newline, cjk, other]`；长度桶序见
/// [`crate::token`]（1-16/17-64/65-256/257-1024/1025+）。
/// `CALIB_DENSITY[feature][bucket]` = 每单位 token 数。
pub const CALIB_DENSITY: [[f64; 5]; 6] = [
{rows}
];
"""
    with open(out_path, "w", encoding="utf-8") as f:
        f.write(body)
    print("wrote %s:" % out_path)
    for s in range(6):
        print("  %s: %s" % (FEATURES[s], table[s]))
    if mae_calib is not None:
        print("MAE 校准=%.2f 启发式=%.2f（样本内）" % (mae_calib, mae_heur))


if __name__ == "__main__":
    main()
