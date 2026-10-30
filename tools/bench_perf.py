#!/usr/bin/env python3
"""P12-A 性能基线冻结：多档位 × 三档压缩，输出 JSON（3 次取最优 real 秒 + 吞吐 MB/s）"""
import json, os, subprocess, sys, time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BIN = ROOT / "target/release/scx"
PERF_DIR = ROOT / "config/bench/perf"
OUT = PERF_DIR / "perf_baseline.json"

CORPUS = ["corpus_p1m.txt", "corpus_p10m.txt", "corpus_p30m.txt"]
RATIOS = [("lossless", 1.0), ("r0.3", 0.3), ("r0.2", 0.2)]

def run_once(path, ratio):
    with open(path, "rb") as f:
        data = f.read()
    t0 = time.perf_counter()
    out = subprocess.run([str(BIN), "--query", "fn compress", "--ratio", str(ratio),
                          "--trim", "1", "--trim-ratio", str(ratio),
                          "--trim-op-keep", "1", "--num-line-keep", "1",
                          str(path)], capture_output=True)
    dt = time.perf_counter() - t0
    return dt, out.stdout

def main():
    results = {}
    for c in CORPUS:
        p = PERF_DIR / c
        size = os.path.getsize(p)
        row = {"chars": size}
        for label, ratio in RATIOS:
            best = min(run_once(p, ratio)[0] for _ in range(3))
            mbps = size / best / 1024 / 1024
            row[label] = {"best_s": round(best, 4), "mbps": round(mbps, 1)}
        results[c] = row
        print(c, row, flush=True)
    OUT.write_text(json.dumps({"tool": "tools/bench_perf.py", "date": "2026-10-29",
                               "corpus_dir": str(PERF_DIR), "results": results},
                              ensure_ascii=False, indent=1))
    print("WROTE", OUT, flush=True)

if __name__ == "__main__":
    main()
