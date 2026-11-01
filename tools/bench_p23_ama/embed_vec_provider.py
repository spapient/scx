#!/usr/bin/env python3
"""scx --embed-vec provider（sim.rs 向量协议）：
stdin 首行 Q\\t<query>，随后每行 L\\t<text>；stdout 每个输入行一个原始向量
（空格分隔 dim 个 f32，首行 = query 向量，随后 = 各 text 向量，顺序对应）。
归一化与余弦由 scx 在库内完成（f64 确定性）。"""
import sys

from sentence_transformers import SentenceTransformer

MODEL = "all-MiniLM-L6-v2"


def main() -> None:
    model = SentenceTransformer(MODEL)
    q = None
    texts: list[str] = []
    for ln in sys.stdin:
        ln = ln.rstrip("\n")
        if ln.startswith("Q\t"):
            q = ln[2:]
        elif ln.startswith("L\t"):
            texts.append(ln[2:])
    if q is None:
        print("ERROR: no query line", file=sys.stderr)
        sys.exit(2)
    qe = model.encode(q)
    out = [" ".join(f"{x:.6f}" for x in qe)]
    if texts:
        for v in model.encode(texts):
            out.append(" ".join(f"{x:.6f}" for x in v))
    sys.stdout.write("\n".join(out) + "\n")


if __name__ == "__main__":
    main()
