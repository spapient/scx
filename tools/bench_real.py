#!/usr/bin/env python3
"""Run scx over extracted real-session dumps and report cut / retention.

Reusable baseline for the real-dump compression corpus (the SuperCompress
counterpart corpus). For every .dump.txt under --dumps it runs the release
binary across a config matrix and prints a table. Deterministic: files are
sorted; the JSON stats line (scxjson: on stderr) is parsed directly.
"""
import argparse
import glob
import json
import os
import subprocess
import sys


def scx_json(binary, path, query, ctx, gate, ratio):
    cmd = [
        binary, path, "--query", query,
        "--context", str(ctx), "--risk-gate", str(gate),
        "--ratio", str(ratio), "--json",
    ]
    proc = subprocess.run(cmd, capture_output=True, text=True)
    for line in proc.stderr.splitlines():
        if line.startswith("scxjson:"):
            return json.loads(line[len("scxjson:"):])
    return None


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--dumps", required=True, help="dir with *.dump.txt files")
    ap.add_argument("--binary", required=True, help="path to release scx binary")
    ap.add_argument("--query", default="compress dedup evidence")
    ap.add_argument("--ratio", default="0.35")
    args = ap.parse_args()

    files = sorted(glob.glob(os.path.join(args.dumps, "*.dump.txt")))
    if not files:
        sys.exit(f"no .dump.txt under {args.dumps}")

    print(f"{'dump':12s} {'ctx':>3s} {'gate':>4s} {'cut%':>6s} {'kept':>6s} "
          f"{'seeds':>6s} {'guard':>5s} {'risk':>5s}")
    for path in files:
        base = os.path.basename(path).replace(".dump.txt", "")[:12]
        for ctx in (0, 1):
            for gate in (0, 1):
                data = scx_json(args.binary, path, args.query, ctx, gate, args.ratio)
                if not data:
                    print(f"{base} ctx={ctx} gate={gate}  ERROR: no scxjson")
                    continue
                print(f"{base:12s} {ctx:3d} {gate:4d} "
                      f"{data['cut_pct']:5.1f}% {data['kept']:6d} {data['seeds']:6d} "
                      f"{data['guarded']:5d} {data['risk']:>5s}")


if __name__ == "__main__":
    main()
