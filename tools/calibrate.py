#!/usr/bin/env python3
"""计算 `--max-tokens`/预算的保守上界系数（`BUDGET_SAFETY_FACTOR`），并报告 vs 启发式的分位数比率。

对代表性语料采样，用真实 tokenizer（tiktoken cl100k_base）取真值 token 数，与
`terms::estimate` 启发式（ASCII 4 字/tok、CJK 0.6 tok/字、other 2.5 字/tok、截距 +1）
对比，输出「真值 / 启发式」比率的 P50/P90/P95/P99/P100 分位数。取 P95（或 P99）作为
保守上界系数：`estimate × 系数 ≥ 真值` 在对应分位数上成立，宁高估勿低估——避免启发式
在 CJK/emoji/other 脚本上低估导致 `--max-tokens` 超预算。

用法：
    python3 tools/calibrate.py [corpus.json]

- corpus.json：JSON 字符串数组（缺省用内置代表性样本）。
- 依赖：`tiktoken`（离线）。缺失时输出「无真值，无法定系数」。
- 输出：分位数表 + 建议的 `BUDGET_SAFETY_FACTOR` 值（需手动写入 src/terms.rs）。
"""

import json
import sys


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
    """多脚本多行长度的代表性样本（含 ASCII/CJK/other/emoji，覆盖低估风险面）。"""
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


def quantiles(ratios, qs):
    """最近秩分位数（对排序后的比率取 int(q*(n-1)) 下标）。"""
    ratios = sorted(ratios)
    n = len(ratios)
    return [ratios[min(n - 1, int(q * (n - 1)))] for q in qs]


def main():
    corpus_path = sys.argv[1] if len(sys.argv) > 1 else None
    samples = json.load(open(corpus_path, encoding="utf-8")) if corpus_path else default_corpus()

    try:
        import tiktoken  # noqa: F401

        enc = tiktoken.get_encoding("cl100k_base")
        ys = [len(enc.encode(s)) for s in samples]
    except Exception as exc:  # tiktoken 缺失 → 无真值，无法定系数
        print("无真值（tiktoken 不可用）：无法定安全系数。%s" % exc)
        return 1

    ratios = [y / heuristic_estimate(s) for s, y in zip(samples, ys)]
    qs = [0.50, 0.90, 0.95, 0.99, 1.00]
    vals = quantiles(ratios, qs)
    print("样本数=%d  比率（真值 / 启发式）分位数：" % len(samples))
    for q, v in zip(qs, vals):
        print("  P%-4d = %.3f" % (int(q * 100), v))
    p95, p99 = vals[2], vals[3]
    print("\n建议 BUDGET_SAFETY_FACTOR = %.3f  （P95；更保守可改用 P99 = %.3f）" % (p95, p99))
    print("写入 src/terms.rs：`pub(crate) const BUDGET_SAFETY_FACTOR: f64 = %.3f;`" % p95)
    return 0


if __name__ == "__main__":
    sys.exit(main())
