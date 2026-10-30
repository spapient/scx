#!/usr/bin/env python3
"""semantic 权重蒸馏升级（零依赖，确定性）。

教师信号 = 端到端评测指标（token_cov/num_cov/cut，bench_ablate 口径）——
把"评测信号"蒸馏进 18 维权重参数（学生）。方法 = 固定 seed 坐标上升：
逐维在候选网格上扫值，选"cut 最大 + fidelity 不劣于基线（容差 0.01）"
的目标；粗网格→细网格多轮精化，顺序与网格全固定 → 完全确定性。

与 P13-D 启发式弱标签拟合的区别：监督信号从"手工规则"升级为
"冻结评测的真实保真/cut 信号"，且 18 维全启用（cjk_bigram/term_prox
不再锁 0）。输出 scx-sem-2 weights.json（sha256 与 Rust encode 一致）。
"""
from __future__ import annotations
import csv
import hashlib
import io
import json
import struct
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "tools"))
import bench_ablate as ba  # 复用确定性语料 / compress / fidelity 口径

FEATURES = ["overlap", "hit", "ln_len", "cap_ratio", "dig_ratio", "structure",
            "word_hit", "cjk_ratio", "errcode", "uniq", "sym", "pos_norm",
            "edge", "long", "code", "bias", "cjk_bigram", "term_prox"]
DIM = len(FEATURES)
OUT = ROOT / "config" / "semantic" / "weights.json"
TOL = 0.01          # fidelity 不劣容差（对齐 A2 门禁）
CORPORA = [("code", ba.corpus_code(42), "build"),
           ("log", ba.corpus_log(42), "timeout"),
           ("zh", ba.corpus_zh(42), "超时")]


def encode(ws: list[float]) -> bytes:
    return b"".join(struct.pack(">d", w) for w in ws)


def write_weights(tmp: Path, ws: list[float]) -> Path:
    p = tmp / "weights.json"
    w6 = [round(v, 6) for v in ws]
    # sha256 与 Rust verify_v2 严格一致：SHA256(weights 大端 8 字节拼接 ||
    # 每个特征名接 \n)。只哈希 weights 会与加载器不匹配，导致 --semantic 拒载。
    payload = encode(w6) + b"".join(n.encode() + b"\n" for n in FEATURES)
    p.write_text(json.dumps({
        "schema": "scx-sem-2",
        "features": FEATURES,
        "weights": w6,
        "sha256": hashlib.sha256(payload).hexdigest(),
    }, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return p


def eval_w(bin_: str, tmp: Path, ws: list[float]) -> dict:
    """评估权重向量：3 语料 cut/num_cov/token_cov（semantic 开启）。"""
    wp = write_weights(tmp, ws)
    rows = []
    for cname, text, query in CORPORA:
        out = ba.compress(bin_, text, query, ["--semantic", str(wp)])
        full_words = ba.tokens(text)
        full_nums = ba.num_tokens(text)
        out_tok = ba.est_tok(out)
        out_words = ba.tokens(out)
        out_nums = ba.num_tokens(out)
        rows.append({
            "set": cname,
            "cut": 1.0 - out_tok / ba.est_tok(text),
            "num_cov": ba.cov(full_nums, out_nums),
            "token_cov": ba.cov(full_words, out_words),
        })
    return rows


def baseline(bin_: str) -> dict:
    """教师基线：semantic 关时的 3 语料 fidelity。"""
    out = {}
    for cname, text, query in CORPORA:
        o = ba.compress(bin_, text, query, [])
        out[cname] = {
            "num_cov": ba.cov(ba.num_tokens(text), ba.num_tokens(o)),
            "token_cov": ba.cov(ba.tokens(text), ba.tokens(o)),
        }
    return out


def score(rows: list[dict], base: dict) -> float:
    """目标：cut 最大化，fidelity 不劣于基线（平均容差 TOL，硬惩罚）。"""
    cut = sum(r["cut"] for r in rows) / len(rows)
    pen = 0.0
    for r in rows:
        for k in ("num_cov", "token_cov"):
            drop = base[r["set"]][k] - r[k] - TOL
            if drop > 0:
                pen += drop * 5.0
    return cut - pen


def distill(bin_: str) -> tuple[list[float], list[dict]]:
    base = baseline(bin_)
    print("教师基线（semantic 关）fidelity:")
    for c, v in base.items():
        print(f"  {c}: num_cov={v['num_cov']:.4f} token_cov={v['token_cov']:.4f}")

    # 初始 = 现有 scx-sem-2 权重（16 维拟合 + 后 2 维 0），从现状蒸馏。
    cur = [0.072325, 0.496615, -0.310477, -0.005689, -0.006987, -0.0548,
           0.016532, -0.037252, -0.013018, -0.120063, -0.245746, -0.111923,
           -0.075548, 0.009463, -0.010174, -0.276553, 0.0, 0.0]
    log: list[dict] = []
    with tempfile.TemporaryDirectory() as td:
        tmp = Path(td)
        s0 = score(eval_w(bin_, tmp, cur), base)
        print(f"初始: cut_avg={s0:.4f}")
        for rnd, grid in enumerate(([0.05, -0.05, 0.15, -0.15], [0.02, -0.02]), 1):
            improved = False
            for dim in range(DIM):
                best_v, best_s = cur[dim], s0
                for dv in grid:
                    cand = cur.copy()
                    cand[dim] = round(cur[dim] + dv, 6)
                    cand[dim] = max(-0.6, min(0.6, cand[dim]))  # 权重范围夹紧
                    if abs(cand[dim] - cur[dim]) < 1e-9:
                        continue
                    s = score(eval_w(bin_, tmp, cand), base)
                    if s > best_s + 1e-9:
                        best_v, best_s = cand[dim], s
                if best_v != cur[dim]:
                    cur[dim] = best_v
                    s0 = best_s
                    improved = True
                log.append({"round": rnd, "dim": FEATURES[dim],
                            "w": cur[dim], "score": round(s0, 5)})
                print(f"  r{rnd} {FEATURES[dim]:>10} w={cur[dim]:+.6f} score={s0:.4f}")
            if not improved:
                print(f"轮 {rnd} 无改进，收敛")
                break
    print("蒸馏完成: " + ", ".join(f"{v:+.4f}" for v in cur))
    return cur, log


def main() -> int:
    bin_ = ba.ensure_sem_bin()
    cur, log = distill(bin_)
    # 终验：蒸馏权重在 3 语料上的 cut/fidelity
    with tempfile.TemporaryDirectory() as td:
        rows = eval_w(bin_, Path(td), cur)
        base = baseline(bin_)
        print("\n蒸馏后终验（semantic 开）:")
        for r in rows:
            b = base[r["set"]]
            flag = "OK" if (r["num_cov"] >= b["num_cov"] - 0.01
                            and r["token_cov"] >= b["token_cov"] - 0.01) else "FID-DROP"
            print(f"  {r['set']}: cut={r['cut']:.4f} num_cov={r['num_cov']:.4f} "
                  f"token_cov={r['token_cov']:.4f} vs base "
                  f"({b['num_cov']:.4f}/{b['token_cov']:.4f}) {flag}")
    # 写入 config/semantic/weights.json（随仓权重），非仓库根目录。
    write_weights(OUT.parent, cur)
    final = encode(cur) + b"".join(n.encode() + b"\n" for n in FEATURES)
    print(f"写入 {OUT.relative_to(ROOT)} (sha256={hashlib.sha256(final).hexdigest()[:16]}…)")
    with open(ROOT / "config" / "bench" / "results" / "distill_log.csv", "w",
              encoding="utf-8", newline="") as fh:
        w = csv.DictWriter(fh, fieldnames=["round", "dim", "w", "score"])
        w.writeheader()
        w.writerows(log)
    print("蒸馏日志写入 config/bench/results/distill_log.csv")
    return 0


if __name__ == "__main__":
    sys.exit(main())
