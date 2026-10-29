#!/usr/bin/env python3
"""C2 可压缩子集口径重算：剔除 fail-open（未压缩题）后的真实压缩通过率。

fail-open 定义：compressed 列 != True（小输入透传/未触发压缩）的题。
它们没经过压缩管线，计入全量通过率会虚高压缩能力；本工具输出
全量 / 剔除 fail-open 两套口径供评测结论使用。

用法：python tools/check_subset.py [csv...]   （默认 shape50/layers50 v4pro）
输出：stdout 统计 + config/bench/results/p12_6_c2_subset.json 冻结
"""
import collections, csv, json, pathlib, sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
RES = ROOT / "config/bench/results"
OUT = RES / "p12_6_c2_subset.json"

DEFAULT = ["bench_code_shape50_v4pro.csv", "bench_code_layers50_v4pro.csv"]


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


def main() -> int:
    names = sys.argv[1:] or DEFAULT
    print("== C2 可压缩子集口径重算（剔除 fail-open）==")
    rows = [stat_one(n) for n in names]
    json.dump({
        "pipeline": "P12.6 C2 可压缩子集口径重算（剔除 fail-open 未压缩题）",
        "method": "fail-open = compressed!=True；口径列示 全量 vs 剔除后 两套",
        "files": rows,
    }, open(OUT, "w", encoding="utf-8"), ensure_ascii=False, indent=2)
    print("WROTE", OUT)
    return 0


if __name__ == "__main__":
    sys.exit(main())
