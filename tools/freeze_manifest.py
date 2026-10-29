#!/usr/bin/env python3
"""冻结 config/bench/results/ 全部结果资产入 eval-manifest.json（frozen）。

口径与 check_bench.sh 完全一致：
  - sha256 = 文件原始字节哈希
  - rows    = 换行计数（末行无 \n 时 +1；CSV 含表头行）
  - bytes   = 文件字节数（审计辅助）

幂等：可重复执行；已冻结条目按当前磁盘状态刷新，新增条目自动并入。
用法: python3 tools/freeze_manifest.py [--results DIR] [--manifest PATH]
"""
import hashlib, json, os, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RESULTS = os.path.join(ROOT, "config", "bench", "results")
MANIFEST = os.path.join(ROOT, "config", "bench", "eval-manifest.json")
EXTS = (".csv", ".json")


def main() -> int:
    args = sys.argv[1:]
    results_dir = RESULTS
    manifest_path = MANIFEST
    if "--results" in args:
        results_dir = args[args.index("--results") + 1]
    if "--manifest" in args:
        manifest_path = args[args.index("--manifest") + 1]

    m = json.load(open(manifest_path, encoding="utf-8"))
    frozen = m.setdefault("frozen", {})

    names = sorted(
        n for n in os.listdir(results_dir)
        if n.endswith(EXTS) and os.path.isfile(os.path.join(results_dir, n))
    )
    if not names:
        print(f"no result files under {results_dir}")
        return 1

    added, updated = 0, 0
    for n in names:
        p = os.path.join(results_dir, n)
        data = open(p, "rb").read()
        h = hashlib.sha256(data).hexdigest()
        rows = data.count(b"\n") + (0 if data.endswith(b"\n") else 1)
        entry = {"sha256": h, "bytes": len(data), "rows": rows}
        if n not in frozen:
            added += 1
        elif frozen[n] != entry:
            updated += 1
        frozen[n] = entry

    m["frozen"] = dict(sorted(frozen.items()))
    m["schema"] = 1
    m["frozen_at"] = "2026-10-29"
    m["note"] = (
        "定稿评测结果冻结（不可变基线）。修改/重跑后必须重新冻结并提交；"
        "check_bench.sh 校验 sha256+行数。"
    )
    json.dump(m, open(manifest_path, "w", encoding="utf-8"), ensure_ascii=False, indent=2)
    print(f"frozen={len(frozen)}  added={added}  updated={updated}  ({results_dir})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
