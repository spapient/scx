#!/usr/bin/env python3
"""Paired bootstrap + sign-flip permutation for scx --eval-verbose output.

Replaces the PowerShell-only significance path on macOS/Linux. The input is the
archived verbose report (A/B top-10), the frozen baseline (expected target), and
the optional strata CSV (case_index,tier). Primary metric: MRR@10.

Usage:
  python3 eval-real/significance.py \
    --baseline eval-real/v2-baseline-arb.json \
    --verbose eval-real/v2-human-gate-final-2026-10-24.txt \
    --strata eval-real/v2-strata.csv \
    --out eval-real/v2-human-significance.csv
"""

import argparse
import csv
import json
import random
import re


def parse_verbose(path, n):
    text = open(path, encoding="utf-8").read().splitlines()
    rows = []
    current = None
    for line in text:
        match = re.match(r'case (\d+)/\d+ q="(.*)"', line)
        if match:
            current = {"A": [], "B": []}
            rows.append(current)
            continue
        match = re.match(r"  ([AB]): (.*)", line)
        if match and current is not None:
            current[match.group(1)] = re.findall(r"\) (\S+) sim=", match.group(2))
    if len(rows) != n:
        raise SystemExit(f"verbose cases={len(rows)} != baseline cases={n}")
    return rows


def target_rank(ids, target):
    tail = target.rsplit("_", 1)[-1]
    for i, case_id in enumerate(ids[:10]):
        if case_id == target or case_id == tail or case_id[-8:] == tail[-8:]:
            return i + 1
    return 0


def mrr(rank):
    return 1.0 / rank if rank else 0.0


def analyze(name, rows, boot, perm, seed, mde):
    deltas = [r["a"] - r["b"] for r in rows]
    n = len(deltas)
    if not n:
        return None
    observed = sum(deltas) / n
    rng = random.Random(seed)
    means = sorted(
        sum(deltas[rng.randrange(n)] for _ in range(n)) / n for _ in range(boot)
    )
    lo = means[int(0.025 * (boot - 1))]
    hi = means[int(0.975 * (boot - 1))]
    hits = sum(
        1
        for _ in range(perm)
        if abs(sum(x if rng.random() >= 0.5 else -x for x in deltas) / n)
        >= abs(observed)
    )
    p = (hits + 1.0) / (perm + 1.0)
    return {
        "group": name,
        "n": n,
        "A_mrr": round(sum(r["a"] for r in rows) / n, 4),
        "B_mrr": round(sum(r["b"] for r in rows) / n, 4),
        "delta": round(observed, 4),
        "ci_lo": round(lo, 4),
        "ci_hi": round(hi, 4),
        "perm_p": round(p, 4),
        "mde": mde,
        "advantage": int(lo > 0 and observed >= mde and p < 0.05),
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--baseline", required=True)
    parser.add_argument("--verbose", required=True)
    parser.add_argument("--strata")
    parser.add_argument("--out", required=True)
    parser.add_argument("--boot", type=int, default=10000)
    parser.add_argument("--perm", type=int, default=10000)
    parser.add_argument("--seed", type=int, default=20261023)
    parser.add_argument("--mde", type=float, default=0.05)
    args = parser.parse_args()

    cases = json.load(open(args.baseline, encoding="utf-8"))["cases"]
    verbose = parse_verbose(args.verbose, len(cases))
    strata = {}
    if args.strata:
        for row in csv.DictReader(open(args.strata, encoding="utf-8-sig")):
            strata[int(row["case_index"])] = row.get("tier", "")

    rows = []
    for i, (case, hits) in enumerate(zip(cases, verbose)):
        target = case["expected"][0]
        a = mrr(target_rank(hits["A"], target))
        b = mrr(target_rank(hits["B"], target))
        rows.append({"i": i, "layer": case["layer"], "tier": strata.get(i, ""), "a": a, "b": b})

    results = []
    for layer in ["literal", "paraphrase", "semantic"]:
        group = [r for r in rows if r["layer"] == layer]
        if group:
            results.append(analyze(layer, group, args.boot, args.perm, args.seed, args.mde))
    for tier in sorted({r["tier"] for r in rows if r["tier"]}):
        results.append(
            analyze(f"tier:{tier}", [r for r in rows if r["tier"] == tier], args.boot, args.perm, args.seed, args.mde)
        )
    results.append(analyze("all", rows, args.boot, args.perm, args.seed, args.mde))

    with open(args.out, "w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=list(results[0].keys()))
        writer.writeheader()
        writer.writerows(results)
    for row in results:
        print(
            f"{row['group']:14s} n={row['n']:3d} A={row['A_mrr']:.4f} B={row['B_mrr']:.4f} "
            f"d={row['delta']:+.4f} CI=[{row['ci_lo']:+.4f},{row['ci_hi']:+.4f}] p={row['perm_p']:.4f}"
        )


if __name__ == "__main__":
    main()
