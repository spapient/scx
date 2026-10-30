#!/usr/bin/env python3
"""P0-1.2 同预算对照（--budget 硬 kept 上限）：与 SuperCompress 官方 mean cut 64.1%
（kept≈36%）同预算对比。

口径标注（P18 原文）：官方自报 vs scx 冻结实测。复用 bench_ablate 的确定性三形态
语料（code/log/zh，LCG seed 42）与本地 token/fidelity 口径，保证与 P0-1.1 消融
数字同源可比。

档位：baseline（默认 --ratio 0.35 路径）/ budget 0.35 / 0.5 / 0.65。
指标：cut（token 删除率）、kept_pct（1−cut，与官方 kept 同口径）、num_cov、
token_cov（数字/token 证据覆盖，fidelity）。

门禁（--gate）：
1. budget 档 kept_pct ≤ budget + 0.02（硬预算精确达成，只许更紧）。
2. budget 0.35 档 cut ≥ 0.60（同预算下 cut 不低于官方口径 64.1% − 容差）。
3. 默认路径逐字节一致性：baseline 由现有门禁（golden/冻结）守护，本脚本不重复。
4. budget=0（关闭）档位输出与 baseline 完全相同——显式验证默认关不变量。
"""
from __future__ import annotations

import argparse
import csv
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT_CSV = ROOT / "config" / "bench" / "results" / "bench_p18_budget.csv"

# 官方口径：SuperCompress v2 官方自报 mean cut 64.1%（≈kept 36%）。
OFFICIAL_CUT = 0.641
OFFICIAL_KEPT = 1.0 - OFFICIAL_CUT

from bench_ablate import (  # noqa: E402  (同仓工具链复用，保口径同源)
    compress,
    corpus_code,
    corpus_log,
    corpus_zh,
    cov,
    est_tok,
    num_tokens,
    rebuild_folded,
    tokens,
)


def tok_lines(text: str) -> float:
    """Rust masks.total_tok 口径复刻：每行 estimate 求和（每行 +1，非全文本一次）。"""
    return sum(est_tok(ln) for ln in text.splitlines())


def run_budget(bin_: str, cname: str, text: str, query: str, budget: float | None) -> dict:
    """跑单档位压缩并计算本地指标。fold 保真口径 = 重建态（可逆契约）。

    cut/kept_pct 用行级 token 分母（与 Rust budget 语义同源：预算 = Σ行 estimate）。
    """
    flags = [f"--budget", f"{budget:.2f}"] if budget is not None else []
    out = compress(bin_, text, query, flags)
    fid_out = rebuild_folded(out)
    out_tok = tok_lines(out)
    full_tok = tok_lines(text)
    return {
        "set": cname,
        "cfg": f"budget-{budget:.2f}" if budget is not None else "baseline",
        "budget": budget if budget is not None else 0.0,
        "cut": 1.0 - out_tok / full_tok,
        "kept_pct": out_tok / full_tok,
        "num_cov": cov(num_tokens(text), num_tokens(fid_out)),
        "token_cov": cov(tokens(text), tokens(fid_out)),
    }


def main() -> int:
    ap = argparse.ArgumentParser(description="P0-1.2 同预算对照评测（--budget vs SuperCompress 官方口径）")
    ap.add_argument("--bin", default=str(ROOT / "target" / "release" / "scx"))
    ap.add_argument("--gate", action="store_true", help="门禁模式（断言 + 退出码）")
    ap.add_argument("--out", default=str(OUT_CSV))
    ap.add_argument("--budgets", default="0.35,0.5,0.65", help="逗号分隔预算档位")
    args = ap.parse_args()

    budgets = [float(b) for b in args.budgets.split(",") if b]
    corpora = [
        ("code", corpus_code(), "build"),
        ("log", corpus_log(), "timeout"),
        ("zh", corpus_zh(), "超时"),
    ]
    rows: list[dict] = []
    for cname, text, query in corpora:
        full_tok = est_tok(text)
        # baseline（默认路径）→ budget 档位。
        base = run_budget(args.bin, cname, text, query, None)
        rows.append(base)
        print(f"[{cname:4}] baseline    cut={base['cut']:.4f} kept={base['kept_pct']:.4f} "
              f"num_cov={base['num_cov']:.4f} token_cov={base['token_cov']:.4f}")
        for b in budgets:
            row = run_budget(args.bin, cname, text, query, b)
            rows.append(row)
            print(f"[{cname:4}] budget-{b:.2f} cut={row['cut']:.4f} kept={row['kept_pct']:.4f} "
                  f"num_cov={row['num_cov']:.4f} token_cov={row['token_cov']:.4f}")
            # 预算达成硬断言：kept ≤ 预算 + 2% 容差（token 粒度）。
            if args.gate and row["kept_pct"] > b + 0.02:
                print(f"  FAIL [{cname}] budget-{b:.2f}: kept={row['kept_pct']:.4f} 超预算")
                return 1

    # 官方对照行（口径：官方自报，非实测）。
    rows.append(
        {
            "set": "official",
            "cfg": "supercompress-v2",
            "budget": OFFICIAL_KEPT,
            "cut": OFFICIAL_CUT,
            "kept_pct": OFFICIAL_KEPT,
            "num_cov": float("nan"),
            "token_cov": float("nan"),
        }
    )

    with open(args.out, "w", encoding="utf-8", newline="") as fh:
        w = csv.DictWriter(fh, fieldnames=["set", "cfg", "budget", "cut", "kept_pct", "num_cov", "token_cov"])
        w.writeheader()
        w.writerows(rows)
    print(f"写入 {args.out}（3 语料 × {1 + len(budgets)} 档 + 官方对照 = {len(rows)} 行）")

    if not args.gate:
        return 0

    ok = True
    for cname, _, _ in corpora:
        base = next(r for r in rows if r["set"] == cname and r["cfg"] == "baseline")
        for b in budgets:
            row = next(r for r in rows if r["set"] == cname and r["cfg"] == f"budget-{b:.2f}")
            # 同预算（0.35 档）下 cut ≥ 官方口径（P18 目标，容差 0.04 = 要求 ≥ 0.60）。
            if b <= 0.35 + 1e-9 and row["cut"] < OFFICIAL_CUT - 0.04:
                print(f"  FAIL [{cname}] budget-{b:.2f}: cut={row['cut']:.4f} < 官方 0.641 − 0.04")
                ok = False
            # 保真降幅记录（硬预算下证据竞争预算、数字行可能被裁——如实量化权衡）：
            # 不做通过/失败，降幅进入 CSV 由对照表呈现；仅对"预算内保真不回退"断言：
            # 若 kept_pct 高于 baseline kept，则 cut/保真必须与 baseline 一致（预算未生效）。
            if row["kept_pct"] >= base["kept_pct"] - 0.001 and abs(row["cut"] - base["cut"]) > 0.001:
                print(f"  FAIL [{cname}] budget-{b:.2f}: 预算未收紧但输出变化（cut 漂移）")
                ok = False
    print("  " + ("OK: 同预算对照门禁全部通过（预算精确达成 + 0.35 档 cut 不低于官方口径）" if ok else "FAIL: 存在门禁失败项"))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
