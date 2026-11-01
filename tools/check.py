#!/usr/bin/env python3
"""scx 评测结果验证工具（二合一）。

子命令：
  fidelity  A2 round-trip 保真断言（接入 check_bench.sh）：读 p11_a_results.json
            按 fidelity_thresholds.json 分档阈值断言 num_cov/token_cov/bleu4_prec。
  subset    C2 可压缩子集口径重算：剔除 fail-open（compressed!=True）后两套口径。
"""
import collections
import csv
import json
import os
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
RES = ROOT / "config" / "bench" / "results"


def cmd_fidelity() -> int:
    a2 = ROOT / "config" / "bench" / "results" / "p11_a_results.json"
    thresh = ROOT / "config" / "bench" / "fidelity_thresholds.json"
    res = json.load(open(a2, encoding="utf-8"))
    cfg = json.load(open(thresh, encoding="utf-8"))
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


def stat_one(name: str) -> dict:
    rows = list(csv.DictReader(open(RES / name, encoding="utf-8")))
    bym = collections.defaultdict(list)
    for r in rows:
        bym[r["model"]].append(r)
    out = {"file": name, "n": len(rows), "models": {}}
    for m, rs in sorted(bym.items()):
        fo = [r for r in rs if r["compressed"].strip() != "True"]
        cp = [r for r in rs if r["compressed"].strip() == "True"]
        p_all = sum(1 for r in rs if r["scx"].strip() == "True")
        p_cp = sum(1 for r in cp if r["scx"].strip() == "True")
        out["models"][m] = {
            "all": {"pass": p_all, "n": len(rs), "rate": round(p_all / len(rs), 4)},
            "subset": {"pass": p_cp, "n": len(cp), "rate": round(p_cp / len(cp), 4)},
            "fail_open": {"n": len(fo), "pass_in_fo": sum(1 for r in fo if r["scx"].strip() == "True")},
        }
        print(f"  {name} {m}: 全量 {p_all}/{len(rs)} ({p_all/len(rs):.1%}) "
              f"| 剔除fail-open {p_cp}/{len(cp)} ({p_cp/len(cp):.1%}) | fail-open {len(fo)}")
    return out


def cmd_subset(names: list[str]) -> int:
    print("== C2 可压缩子集口径重算（剔除 fail-open）==")
    rows = [stat_one(n) for n in names]
    out = ROOT / "config" / "bench" / "results" / "p12_6_c2_subset.json"
    json.dump({
        "pipeline": "P12.6 C2 可压缩子集口径重算（剔除 fail-open 未压缩题）",
        "method": "fail-open = compressed!=True；口径列示 全量 vs 剔除后 两套",
        "files": rows,
    }, open(out, "w", encoding="utf-8"), ensure_ascii=False, indent=2)
    print("WROTE", out)
    return 0


DEFAULT_SUBSET = ["bench_code_shape50_v4pro.csv", "bench_code_layers50_v4pro.csv"]


def main() -> int:
    if len(sys.argv) < 2:
        sys.exit("usage: python tools/check.py fidelity|subset [csv...]")
    cmd = sys.argv[1]
    if cmd == "fidelity":
        return cmd_fidelity()
    if cmd == "subset":
        return cmd_subset(sys.argv[2:] or DEFAULT_SUBSET)
    sys.exit(f"unknown subcommand: {cmd}")


if __name__ == "__main__":
    sys.exit(main())
