#!/usr/bin/env python3
"""A2 round-trip 保真断言（接入 check_bench.sh）。

读取 P11-A 冻结结果 p11_a_results.json 的 a2_roundtrip 表，
按 config/bench/fidelity_thresholds.json 分档阈值断言：
  - code 档（r28 激进档）：num_cov/token_cov/bleu4_prec 三锚下界
  - zh_extreme 档（cut 92-94%）：num_cov 硬锚（数字覆盖必须守住）

任一指标低于阈值 → 非零退出（回归门禁）。
"""
import json, os, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
A2 = os.path.join(ROOT, "config", "bench", "results", "p11_a_results.json")
THRESH = os.path.join(ROOT, "config", "bench", "fidelity_thresholds.json")


def main() -> int:
    res = json.load(open(A2, encoding="utf-8"))
    cfg = json.load(open(THRESH, encoding="utf-8"))
    table = {r["set"]: r for r in res["a2_roundtrip"]}
    ok = True
    for band, spec in cfg["bands"].items():
        metrics = [(k, v) for k, v in spec.items() if k in ("num_cov", "token_cov", "bleu4_prec")]
        for s in spec["sets"]:
            row = table.get(s)
            if row is None:
                print(f"  [fidelity:{band}] {s}: MISSING in a2_roundtrip"); ok = False; continue
            for m, thr in metrics:
                got = row.get(m, -1.0)
                if got < thr:
                    print(f"  [fidelity:{band}] {s} {m}={got:.3f} < {thr}  FAIL"); ok = False
                else:
                    print(f"  [fidelity:{band}] {s} {m}={got:.3f} >= {thr}  ok")
    print("  " + ("OK: A2 保真断言全部通过" if ok else "FAIL: A2 保真断言未通过"))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
