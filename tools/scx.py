#!/usr/bin/env python3
"""scx 评测/运维命令委托器。

所有确定性/离线命令的实现已迁到 Rust `xtask`（feature=eval，见 src/evaltool）。
本脚本只保留命令行契约（build_parser 用于 --help 与参数校验）并把参数原样
转发给 `target/{release,debug}/xtask`。在线模型评测与在线蒸馏保留在
tools/bench.py（Python），不在此脚本内。

构建：cargo build --features eval --bin xtask
"""

import argparse
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FM_RESULTS = os.path.join(ROOT, "config", "bench", "results")
FM_MANIFEST = os.path.join(ROOT, "config", "bench", "eval-manifest.json")


def xtask_binary():
    """定位已构建的 xtask 二进制；未构建返回 None。"""
    for profile in ("release", "debug"):
        for name in ("xtask.exe", "xtask"):
            path = ROOT / "target" / profile / name
            if path.is_file():
                return str(path)
    return None


def build_parser():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="cmd", required=True)

    seed = sub.add_parser("seed", help="corpus JSON -> memory cases + eval CSV")
    seed.add_argument("corpus")
    seed.add_argument("--mem-dir", required=True)
    seed.add_argument("--csv")
    seed.add_argument("--binary", default="")
    seed.add_argument(
        "--jobs", type=int, default=4,
        help="parallel store processes (default: 4)",
    )
    seed.add_argument("--force", action="store_true")

    baseline = sub.add_parser("baseline", help="eval CSV -> target baseline")
    baseline.add_argument("--csv", required=True)
    baseline.add_argument("--out", required=True)
    baseline.add_argument("--judge-by", default="mechanical-target-v1")
    baseline.add_argument("--judge-date", default="2026-10-23T00:00:00Z")

    stats = sub.add_parser("stats", help="verbose + baseline -> significance CSV")
    stats.add_argument("--baseline", required=True)
    stats.add_argument("--verbose", required=True)
    stats.add_argument("--out", required=True)
    stats.add_argument("--mem-dir")
    stats.add_argument("--strata")
    stats.add_argument("--metric", choices=("mrr", "ndcg", "recall@1", "recall@3", "recall@5", "recall@10"), default="mrr")
    stats.add_argument("--boot", type=int, default=10000)
    stats.add_argument("--perm", type=int, default=10000)
    stats.add_argument("--seed", type=int, default=20261023)
    stats.add_argument("--mde", type=float, default=0.05)
    stats.add_argument("--cluster-size", type=int, default=0)
    stats.add_argument("--loo", action="store_true",
                       help="leave-one-incident-out stability of tier:hard")
    stats.add_argument("--loo-group", type=int, default=5,
                       help="cases per incident for LOO grouping")
    stats.add_argument("--loo-gate-delta", type=float, default=0.05,
                       help="min tier:hard delta after dropping any incident")
    stats.add_argument("--gate", action="store_true",
                       help="with --loo: fail if any dropped incident loses "
                            "the frozen advantage")

    blind = sub.add_parser("blind", help="frozen per-candidate blind review")
    blind_sub = blind.add_subparsers(dest="blind_cmd", required=True)

    make = blind_sub.add_parser("make")
    make.add_argument("--baseline", required=True)
    make.add_argument("--mem-dir", required=True)
    make.add_argument("--out", required=True)
    make.add_argument("--manifest", required=True)
    make.add_argument("--name", default="gauge-blind40")
    make.add_argument("--seed", default="gauge-blind-v1")
    make.add_argument(
        "--per-layer", default="literal=14,paraphrase=13,semantic=13"
    )

    imp = blind_sub.add_parser("import")
    imp.add_argument("--baseline", required=True)
    imp.add_argument("--packet", required=True)
    imp.add_argument("--out", required=True)
    imp.add_argument("--judge", required=True)
    imp.add_argument("--date", required=True)
    imp.add_argument("--name", default="gauge-blind40")
    imp.add_argument("--method", default="per-item-blind-v1")
    imp.add_argument("--note", default="")
    imp.add_argument("--allow-partial", action="store_true")

    check = blind_sub.add_parser("check")
    check.add_argument("--baseline", required=True)
    check.add_argument("--packet", help="reconstruct the filled packet")
    check.add_argument("--out", help="write the reconstructed filled packet")

    progress = blind_sub.add_parser("progress")
    progress.add_argument("--packet", required=True)

    agree = blind_sub.add_parser("agree")
    agree.add_argument("--a", required=True)
    agree.add_argument("--b", required=True)

    review = blind_sub.add_parser("review")
    review.add_argument("--packet", required=True)

    verify = sub.add_parser("verify", help="packet hash and row count")
    verify.add_argument("--packet", required=True)
    verify.add_argument("--sha256")

    contain = sub.add_parser(
        "contain", help="evidence containment vs head-truncation baseline"
    )
    contain.add_argument("corpus", help="contain*.corpus.json")
    contain.add_argument("--binary", default="")
    contain.add_argument("--ratio", type=float, default=0.35)
    contain.add_argument("--context", type=int, default=1)
    contain.add_argument("--out", help="write per-slice detail JSON")
    contain.add_argument(
        "--seed", default="contain-rand-v1",
        help="deterministic seed for the random-sampling baseline",
    )
    contain.add_argument(
        "--timing-rounds", type=int, default=5,
        help="repeat each method per slice, take median wall time",
    )

    fidelity = sub.add_parser(
        "fidelity", help="compression fidelity: answerability + token retention"
    )
    fidelity.add_argument("corpus", help="fidelity*.corpus.json")
    fidelity.add_argument("--binary", default="")
    fidelity.add_argument("--ratio", type=float, default=0.35)
    fidelity.add_argument("--context", type=int, default=1)
    fidelity.add_argument("--seed", default="fidelity-rand-v1")
    fidelity.add_argument("--out", help="write per-slice detail JSON")
    fidelity.add_argument("--gate", action="store_true",
                         help="enforce frozen thresholds: ev/ans >= thresholds "
                              "and > trunc; token recall is report-only")
    fidelity.add_argument("--gate-ev", type=float, default=0.95)
    fidelity.add_argument("--gate-ans", type=float, default=0.90)
    fidelity.add_argument("--domain-boot", type=int, default=10000,
                          help="bootstrap replicates for domain cluster test")
    fidelity.add_argument("--domain-gate-p", type=float, default=0.05,
                          help="one-sided p threshold for cross-domain advantage")

    manifest = sub.add_parser(
        "manifest", help="freeze/verify evaluation-input hashes"
    )
    manifest.add_argument("--files", nargs="+", default=[],
                          help="paths to freeze; prefix 'date:' to normalize "
                               "YYYY-MM-DD_ -> <date>_")
    manifest.add_argument("--out", help="write frozen manifest JSON")
    manifest.add_argument("--expect", help="verify against frozen manifest")

    report = sub.add_parser(
        "report", help="compose frozen metrics markdown from gate artifacts"
    )
    report.add_argument("--stats", required=True, help="mrr stats CSV")
    report.add_argument("--stats-recall3", required=True,
                        help="recall@3 stats CSV")
    report.add_argument("--loo-json", required=True, help="LOO detail JSON")
    report.add_argument("--fidelity", required=True,
                        help="fidelity detail JSON")
    report.add_argument("--out", help="write generated markdown")
    report.add_argument("--check", help="verify byte-identical frozen file")

    test = sub.add_parser("selftest")
    check = sub.add_parser("check", help="A2 保真断言 + C2 可压缩子集口径")
    check_sub = check.add_subparsers(dest="check_cmd", required=True)
    a2 = check_sub.add_parser("a2", help="A2 round-trip 保真断言（原 check.py fidelity）")
    subset = check_sub.add_parser("subset", help="C2 可压缩子集口径重算（剔除 fail-open）")
    subset.add_argument("csvs", nargs="*")

    audit = sub.add_parser("audit", help="Rust 生产路径 unwrap/panic 审计门禁")

    freeze = sub.add_parser("freeze", help="冻结 results/ 全部结果资产入 eval-manifest.json")
    freeze.add_argument("--results", default=FM_RESULTS)
    freeze.add_argument("--manifest", default=FM_MANIFEST)

    extract = sub.add_parser("extract", help="提取 Claude Code 会话转储为压缩语料")
    extract.add_argument("--dir",
                         default=os.path.expanduser("~/.claude/projects/-Users-fei-code-scx"))
    extract.add_argument("--scan", action="store_true")
    extract.add_argument("--extract", metavar="N", type=int)
    extract.add_argument("--out", default=os.environ.get("TMPDIR", "/tmp"))

    return parser


def main():
    parser = build_parser()
    args = parser.parse_args()
    xtask = xtask_binary()
    if not xtask:
        sys.exit(
            "xtask not built; run `cargo build --features eval --bin xtask` first"
        )
    if args.cmd == "check":
        if getattr(args, "check_cmd", None) == "a2":
            return subprocess.run([xtask, "a2"]).returncode
        return subprocess.run([xtask, "subset", *sys.argv[3:]]).returncode
    if args.cmd in ("selftest", "audit"):
        return subprocess.run([xtask, args.cmd]).returncode
    return subprocess.run([xtask, args.cmd, *sys.argv[2:]]).returncode


if __name__ == "__main__":
    sys.exit(main())
