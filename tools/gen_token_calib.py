#!/usr/bin/env python3
"""生成 `src/token_calib.rs` 的冻结校准系数（离线，仅改 tokenizer 后重跑）。

对代表性语料采样字符特征，用真实 tokenizer（tiktoken cl100k_base）取真值 token
数，做最小二乘线性回归，把系数写入 `src/token_calib.rs` 的 `CALIB_COEFFS` 常量。

用法：
    python3 tools/gen_token_calib.py [corpus.json] [out.rs]

- corpus.json：JSON 字符串数组（缺省用内置代表性样本）。
- 依赖：`tiktoken`（离线，MIT）。缺失时输出「复现启发式」的占位系数。
- 特征序与 `src/token.rs::features` 对齐：
  [ascii_alnum, ascii_punct, ascii_space, cjk, other, lines]，截距为第 7 项。
"""

import json
import sys

FEATURE_NAMES = ["ascii_alnum", "ascii_punct", "ascii_space", "cjk", "other", "lines"]


def ideograph(ch):
    u = ord(ch)
    return (
        0x3400 <= u <= 0x4DBF
        or 0x4E00 <= u <= 0x9FFF
        or 0x3040 <= u <= 0x30FF
        or 0xAC00 <= u <= 0xD7AF
        or 0x20000 <= u <= 0x2FA1F
    )


def full(ch):
    u = ord(ch)
    return ideograph(ch) or 0x3000 <= u <= 0x303F or 0xFF00 <= u <= 0xFFEF


def features(text):
    a_alnum = a_punct = a_space = cjk = other = 0
    for ch in text:
        if ch.isascii() and ch.isalnum():
            a_alnum += 1
        elif ch.isascii() and ch in "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~":
            a_punct += 1
        elif ch.isascii() and ch.isspace():
            a_space += 1
        elif full(ch):
            cjk += 1
        else:
            other += 1
    return [a_alnum, a_punct, a_space, cjk, other, len(text.splitlines())]


def default_corpus():
    """代表性样本：日志/trace/自然语言/JSON 混合（与运行时输入同分布）。"""
    samples = []
    for i in range(50):
        samples.append(
            "INFO heartbeat seq=%d ok pool=3 latency=1ms connection established" % i
        )
        samples.append(
            "ERROR db timeout at 12:01 pool exhausted (conn_id=9f2a) retry=3"
        )
    for i in range(50):
        samples.append("The quick brown fox jumps over the lazy dog " * (i % 5 + 1))
        samples.append("数据库连接超时 连接池耗尽 磁盘空间不足 errno=28")
    for i in range(50):
        samples.append(
            '{"type":"object","properties":{"id":{"type":"integer"},"name":{"type":"string"}}}'
        )
    return samples


def least_squares(xs, ys):
    """普通最小二乘：返回 (系数[6], 截距)。"""
    import numpy as np

    x = np.array(xs, dtype=float)
    y = np.array(ys, dtype=float)
    x = np.hstack([x, np.ones((x.shape[0], 1))])
    coef, *_ = np.linalg.lstsq(x, y, rcond=None)
    return coef[:6].tolist(), float(coef[6])


def heuristic_coeffs():
    """复现 `terms::estimate` 的占位系数（ASCII 4 字/tok、CJK 0.6 tok/字、
    other 2.5 字/tok、截距 +1）。"""
    return [0.25, 0.25, 0.25, 0.6, 0.4, 0.0], 1.0


def main():
    corpus_path = sys.argv[1] if len(sys.argv) > 1 else None
    out_path = sys.argv[2] if len(sys.argv) > 2 else "src/token_calib.rs"
    samples = json.load(open(corpus_path, encoding="utf-8")) if corpus_path else default_corpus()

    xs = [features(s) for s in samples]
    try:
        import tiktoken  # noqa: F401

        enc = tiktoken.get_encoding("cl100k_base")
        ys = [len(enc.encode(s)) for s in samples]
        coef, intercept = least_squares(xs, ys)
        note = "由 tools/gen_token_calib.py 对 cl100k_base 最小二乘回归生成"
        if len(sys.argv) > 1:
            note += f"（语料 {corpus_path}）"
    except Exception as exc:  # tiktoken/numpy 缺失 → 占位系数
        coef, intercept = heuristic_coeffs()
        note = f"占位系数（复现启发式；未校准：{exc}）"

    coeffs = coef + [intercept]
    body = f"""//! 冻结的校准系数（`TokenMode::Calibrated` 口径）。
//!
//! {note}。
//! 改 tokenizer 后重跑 `python3 tools/gen_token_calib.py` 再提交。

#![forbid(unsafe_code)]

/// 线性系数：特征序 `[ascii_alnum, ascii_punct, ascii_space, cjk, other, lines]` + 截距。
pub const CALIB_COEFFS: [f64; 7] = {coeffs:?};
"""
    with open(out_path, "w", encoding="utf-8") as f:
        f.write(body)
    print(f"wrote {out_path}: {coeffs}")


if __name__ == "__main__":
    main()
