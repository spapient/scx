#!/usr/bin/env python3
"""逐候选盲审：生成冻结包、导入裁决、校验覆盖。

只用标准库。三个子命令：
  make   从 baseline + 记忆库生成盲审 TSV（不含任何 A/B 分数或既有 rel）
  import 把填好的 TSV 写回成带 judge_items 的 baseline（逐候选溯源）
  check  校验 baseline 的逐候选覆盖是否完整

裁决口径（预注册，不可中途改）：
  2 = 同一事故/同一根因，或能直接给出修复
  1 = 同一故障族/相邻问题，但不能直接解决
  0 = 无关
"""

import argparse
import hashlib
import json
import os
import sys
from collections import OrderedDict

GRADE_HELP = "2=同事故/同根因或直接修复  1=同故障族/部分有用  0=无关"


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def sha256_text(text):
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def clean(text, limit=220):
    text = " ".join(str(text).replace("\t", " ").replace("\r", " ").split())
    return text[:limit]


def case_index(mem_dir):
    """hash 后缀 -> (full_id, query, fix)；记忆库缺失时抛错。"""
    out = {}
    root = os.path.join(mem_dir, "cases")
    if not os.path.isdir(root):
        sys.exit(f"blind-review: missing memory cases dir: {root}")
    for name in sorted(os.listdir(root)):
        path = os.path.join(root, name, "case.json")
        if not os.path.isfile(path):
            continue
        with open(path, encoding="utf-8") as f:
            c = json.load(f)
        full = str(c.get("id") or name)
        tail = full.split("_")[-1]
        out[tail] = (full, str(c.get("query") or ""), str(c.get("fix") or ""))
    return out


def tail_of(key):
    return key.split("_")[-1]


def cmd_make(args):
    with open(args.baseline, encoding="utf-8") as f:
        base = json.load(f, object_pairs_hook=OrderedDict)
    cases = base["cases"]
    by_layer = OrderedDict()
    for i, c in enumerate(cases):
        by_layer.setdefault(c["layer"], []).append(i)
    picks = []
    for layer, idxs in by_layer.items():
        want = args.per_layer.get(layer, 0)
        if want > len(idxs):
            sys.exit(f"blind-review: layer {layer} has {len(idxs)} cases < {want}")
        ranked = sorted(idxs, key=lambda i: sha256_text(f"{args.seed}:{layer}:{i}"))
        picks.extend(ranked[:want])
    picks.sort(key=lambda i: sha256_text(f"{args.seed}:case:{i}"))

    mem = case_index(args.mem_dir)
    rows = []
    manifest_cases = []
    for ci in picks:
        c = cases[ci]
        keys = list(c.get("rel", {}).keys())
        if not keys:
            sys.exit(f"blind-review: case {ci} has empty rel pool")
        ordered = sorted(keys, key=lambda k: sha256_text(f"{args.seed}:{ci}:{k}"))
        filled = []
        for key in ordered:
            hit = mem.get(tail_of(key))
            if hit is None:
                sys.exit(f"blind-review: candidate {key} (case {ci}) missing in memory index")
            full, q, fix = hit
            hint = q + (f" || fix: {fix}" if fix else "")
            filled.append((key, full, clean(hint)))
        manifest_cases.append(
            {"case_index": ci, "layer": c["layer"], "candidates": len(filled)}
        )
        for key, _full, hint in filled:
            rows.append((ci, c["layer"], c["query"], key, hint))

    os.makedirs(os.path.dirname(os.path.abspath(args.out)) or ".", exist_ok=True)
    with open(args.out, "w", encoding="utf-8", newline="") as f:
        f.write("row\tcase_index\tlayer\tquery\tcandidate\thint\tgrade\n")
        for n, (ci, layer, query, key, hint) in enumerate(rows, 1):
            f.write(
                f"{n}\t{ci}\t{layer}\t{clean(query)}\t{key}\t{hint}\t\n"
            )
    manifest = {
        "name": args.name,
        "seed": args.seed,
        "baseline": os.path.abspath(args.baseline),
        "baseline_sha256": sha256_file(args.baseline),
        "memory_dir": os.path.abspath(args.mem_dir),
        "packet": os.path.abspath(args.out),
        "packet_sha256": sha256_file(args.out),
        "protocol": GRADE_HELP,
        "cases": manifest_cases,
        "sample_cases": len(manifest_cases),
        "total_cases": len(cases),
        "judgments": len(rows),
    }
    with open(args.manifest, "w", encoding="utf-8") as f:
        json.dump(manifest, f, ensure_ascii=False, indent=1)
        f.write("\n")
    print(
        f"blind-review: cases={len(manifest_cases)}/{len(cases)} "
        f"judgments={len(rows)} packet={args.out}"
    )
    print(f"blind-review: packet_sha256={manifest['packet_sha256']}")
    print(f"blind-review: {GRADE_HELP}")


def read_packet(path):
    with open(path, encoding="utf-8") as f:
        head = f.readline().rstrip("\n").split("\t")
        need = ["row", "case_index", "layer", "query", "candidate", "hint", "grade"]
        if head[: len(need)] != need:
            sys.exit(f"blind-review: bad header: {head}")
        rows = [line.rstrip("\n").split("\t") for line in f if line.strip()]
    return [dict(zip(head, r)) for r in rows]


def cmd_import(args):
    rows = read_packet(args.packet)
    if not rows:
        sys.exit("blind-review: empty packet")
    grades = {}
    for r in rows:
        g = (r.get("grade") or "").strip()
        if g not in {"0", "1", "2"}:
            if args.allow_partial:
                continue
            sys.exit(
                f"blind-review: row {r['row']} grade={g!r} (need 0/1/2; "
                "fill every row or pass --allow-partial for diagnostics)"
            )
        key = (int(r["case_index"]), r["candidate"])
        if key in grades:
            sys.exit(f"blind-review: duplicate row for case {key[0]} candidate {key[1]}")
        grades[key] = float(g)
    if not grades:
        sys.exit("blind-review: no graded rows")

    with open(args.baseline, encoding="utf-8") as f:
        base = json.load(f, object_pairs_hook=OrderedDict)
    cases = base["cases"]
    by_case = {}
    for (ci, cand), g in grades.items():
        if not 0 <= ci < len(cases):
            sys.exit(f"blind-review: case_index out of range: {ci}")
        by_case.setdefault(ci, {})[cand] = g

    out_cases = []
    stamped = 0
    now = args.date
    violations = []
    for ci in sorted(by_case):
        c = cases[ci]
        pool = set(c.get("rel", {}).keys())
        got = set(by_case[ci])
        if got != pool:
            missing = sorted(pool - got)
            extra = sorted(got - pool)
            sys.exit(
                f"blind-review: case {ci} coverage mismatch "
                f"missing={missing[:3]} extra={extra[:3]}"
            )
        rel = OrderedDict((k, by_case[ci][k]) for k in c["rel"].keys())
        target = c["expected"][0].split("_")[-1]
        target_grade = next(
            (v for k, v in rel.items() if k.split("_")[-1] == target), None
        )
        if c["layer"] == "literal" and target_grade != 2.0:
            violations.append((ci + 1, target[:8], target_grade))
        items = OrderedDict(
            (k, {"by": args.judge, "date": now}) for k in rel.keys()
        )
        nc = OrderedDict()
        nc["query"] = c["query"]
        nc["layer"] = c["layer"]
        nc["expected"] = c["expected"]
        nc["rel"] = rel
        nc["judge"] = {
            "by": args.judge,
            "date": now,
            "method": args.method,
            "packet_sha256": sha256_file(args.packet),
            "packet": os.path.abspath(args.packet),
        }
        nc["judge_items"] = items
        nc["note"] = f"per-item blind review; packet={os.path.basename(args.packet)}"
        if args.note:
            nc["note"] += f"; {args.note}"
        out_cases.append(nc)
        stamped += len(items)

    if violations:
        for ci, target, grade in violations[:8]:
            print(
                f"blind-review: literal case {ci} target {target} graded {grade} (must be 2)",
                file=sys.stderr,
            )
        sys.exit(
            f"blind-review: {len(violations)} literal self-match rows are not 2 - "
            "grades are misaligned or invalid; nothing written"
        )

    out = {"name": args.name, "provenance": "per-item-blind-v1", "cases": out_cases}
    tmp = args.out + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(out, f, ensure_ascii=False, indent=1)
        f.write("\n")
    os.replace(tmp, args.out)
    rel2 = sum(1 for c in out_cases for v in c["rel"].values() if v == 2)
    rel1 = sum(1 for c in out_cases for v in c["rel"].values() if v == 1)
    rel0 = sum(1 for c in out_cases for v in c["rel"].values() if v == 0)
    print(
        f"blind-review: wrote {args.out} cases={len(out_cases)} items={stamped} "
        f"rel2={rel2} rel1={rel1} rel0={rel0}"
    )


def cmd_check(args):
    with open(args.baseline, encoding="utf-8") as f:
        base = json.load(f)
    bad = 0
    items = 0
    for i, c in enumerate(base["cases"]):
        pool = set(c.get("rel", {}).keys())
        got = set(c.get("judge_items", {}).keys())
        items += len(got)
        if pool != got:
            bad += 1
            print(
                f"blind-review: case {i} uncovered={len(pool - got)} "
                f"extra={len(got - pool)}"
            )
    print(f"blind-review: cases={len(base['cases'])} items={items} incomplete={bad}")
    return 1 if bad else 0


def cmd_progress(args):
    rows = read_packet(args.packet)
    done = sum(1 for r in rows if (r.get("grade") or "").strip() in {"0", "1", "2"})
    print(f"blind-review: graded={done}/{len(rows)} remaining={len(rows) - done}")
    return 0 if done == len(rows) else 1


def cmd_review(args):
    """逐行提示 + 立即落盘：避免表格批量粘贴造成行错位。"""
    with open(args.packet, encoding="utf-8") as f:
        head = f.readline().rstrip("\n").split("\t")
        rows = [line.rstrip("\n").split("\t") for line in f if line.strip()]
    cols = {k: i for i, k in enumerate(head)}
    grade_i = cols["grade"]
    todo = [r for r in rows if not r[grade_i].strip()]
    done = len(rows) - len(todo)
    print(f"blind-review: {done}/{len(rows)} already graded; {len(todo)} to go")
    print(f"blind-review: {GRADE_HELP}  (s=skip  q=save&quit)")
    dirty = False
    try:
        for r in todo:
            print(
                f"\n[row {r[cols['row']]}/{len(rows)}] layer={r[cols['layer']]}"
                f"\nquery: {r[cols['query']]}\nhint:  {r[cols['hint']]}"
            )
            while True:
                ans = input("grade? [2/1/0/s/q] > ").strip().lower()
                if ans == "q":
                    raise KeyboardInterrupt
                if ans in {"2", "1", "0"}:
                    r[grade_i] = ans
                    dirty = True
                    break
                if ans == "s":
                    break
            if dirty:
                tmp = args.packet + ".tmp"
                with open(tmp, "w", encoding="utf-8", newline="") as f:
                    f.write("\t".join(head) + "\n")
                    f.write("".join("\t".join(x) + "\n" for x in rows))
                os.replace(tmp, args.packet)
                dirty = False
    except (KeyboardInterrupt, EOFError):
        pass
    graded = sum(1 for r in rows if r[grade_i].strip() in {"0", "1", "2"})
    print(f"\nblind-review: graded={graded}/{len(rows)} (saved)")
    return 0 if graded == len(rows) else 1


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    sub = ap.add_subparsers(dest="cmd", required=True)

    m = sub.add_parser("make")
    m.add_argument("--baseline", required=True)
    m.add_argument("--mem-dir", required=True)
    m.add_argument("--out", required=True)
    m.add_argument("--manifest", required=True)
    m.add_argument("--name", default="confirm5-blind40")
    m.add_argument("--seed", default="confirm5-blind-v1")
    m.add_argument(
        "--per-layer",
        default="literal=14,paraphrase=13,semantic=13",
        help="每层抽样条数，例如 literal=14,paraphrase=13,semantic=13",
    )
    m.set_defaults(func=cmd_make)

    i = sub.add_parser("import")
    i.add_argument("--baseline", required=True)
    i.add_argument("--packet", required=True)
    i.add_argument("--out", required=True)
    i.add_argument("--judge", required=True)
    i.add_argument("--date", required=True, help="UTC 时间戳，例如 2026-10-24T12:00:00Z")
    i.add_argument("--name", default="confirm5-blind40")
    i.add_argument(
        "--method",
        default="per-item-blind-v1",
        help="per-item-* 方法名；AI 裁决用 per-item-ai-v1",
    )
    i.add_argument("--note", default="", help="写入每个 case 的 note（审计说明）")
    i.add_argument("--allow-partial", action="store_true")
    i.set_defaults(func=cmd_import)

    c = sub.add_parser("check")
    c.add_argument("--baseline", required=True)
    c.set_defaults(func=cmd_check)

    p = sub.add_parser("progress")
    p.add_argument("--packet", required=True)
    p.set_defaults(func=cmd_progress)

    r = sub.add_parser("review")
    r.add_argument("--packet", required=True)
    r.set_defaults(func=cmd_review)

    args = ap.parse_args()
    if args.cmd == "make":
        parts = dict(p.split("=", 1) for p in args.per_layer.split(","))
        args.per_layer = {k: int(v) for k, v in parts.items()}
    sys.exit(args.func(args) or 0)


if __name__ == "__main__":
    main()
