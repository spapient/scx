#!/usr/bin/env python3
"""scx --embed-cmd provider（sim.rs 协议）：
stdin 首行 Q\\t<query>，随后每行 L\\t<text>；stdout 每行一个余弦相似度。
批量编码 + 余弦（normalize 后点积）。"""
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
    if not texts:
        return
    qe = model.encode(q, normalize_embeddings=True)
    te = model.encode(texts, normalize_embeddings=True)
    sims = (te @ qe).tolist()
    for s in sims:
        print(f"{s:.6f}")


if __name__ == "__main__":
    main()
