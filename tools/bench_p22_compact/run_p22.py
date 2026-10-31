#!/usr/bin/env python3
"""P22 CompactBench 驱动脚本（官方 harness 驱动，非自写评分脚本）。

职责：
1. 把 DeepSeek OpenAI 兼容 provider 注册进 compactbench 0.1.0 内置注册表；
2. 用官方 ``run_experiment`` 逐方法跑套件（scx 适配器 + 内置基线），
   结果写官方事件流 results.jsonl（harness 自带机械指标 + 目标模型作答判分）；
3. 汇总官方 RunResult 指标，外加 scx 机械保留率审计（ground-truth 短语在
   压缩产物中的整串存活，口径同 P17 contain5 审计），产出自描述汇总 JSON。

复现：见同目录 README.md。
"""
from __future__ import annotations

import argparse
import asyncio
import json
import os
import statistics
import sys
import time
from collections import defaultdict
from pathlib import Path

ADAPTER_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(ADAPTER_DIR))

import compactbench.providers as providers_mod  # noqa: E402
from compactbench.dsl import DifficultyLevel, load_suite, validate_template  # noqa: E402
from compactbench.engine import derive_case_seed, generate_case  # noqa: E402
from compactbench.providers import MockProvider  # noqa: E402
from compactbench.runner import RunArgs, run_experiment, to_run_result  # noqa: E402
from compactbench.scoring.checks import _normalize  # noqa: E402
from compactbench.scoring.compression import artifact_tokens, transcript_tokens  # noqa: E402

from deepseek_provider import DeepSeekProvider  # noqa: E402
from p22_logger import reset_logger  # noqa: E402

SCX_COMPACTOR_PATH = ADAPTER_DIR / "scx_compactor.py"

METHODS: dict[str, str] = {
    "scx": f"{SCX_COMPACTOR_PATH}:ScxCompactor",
    "naive-summary": "built-in:naive-summary",
    "structured-state": "built-in:structured-state",
    "hybrid-ledger": "built-in:hybrid-ledger",
    "hierarchical-summary": "built-in:hierarchical-summary",
}


def register_deepseek() -> None:
    providers_mod._REGISTRY["deepseek"] = DeepSeekProvider  # type: ignore[attr-defined]


async def run_one(
    *,
    method_key: str,
    method_spec: str,
    suite: str,
    difficulty: DifficultyLevel,
    slots: int,
    drift_cycles: int,
    seed_group: str,
    benchmarks_dir: Path,
    output: Path,
    model: str,
    resume: bool,
) -> Path:
    args = RunArgs(
        method_spec=method_spec,
        suite_key=suite,
        provider_key="deepseek",
        model=model,
        difficulty=difficulty,
        drift_cycles=drift_cycles,
        case_count_per_template=slots,
        seed_group=seed_group,
        benchmarks_dir=benchmarks_dir,
        output_path=output,
        resume=resume,
    )
    await run_experiment(args)
    return output


def aggregate(result) -> dict:
    """从官方 RunResult 抽取汇总指标 + 家族/题型分层。"""
    family: dict[str, list[float]] = defaultdict(list)
    item_type: dict[str, list[float]] = defaultdict(list)
    cycle_comp: list[float] = []
    latencies: list[int] = []
    for case in result.cases:
        fam = case.template_key.split("_v1")[0]
        family[fam].append(case.case_score)
        for cyc in case.cycles:
            cycle_comp.append(cyc.scorecard.compression_ratio)
            if cyc.latency_ms is not None:
                latencies.append(cyc.latency_ms)
            for iscore in cyc.scorecard.item_scores:
                item_type[iscore.item_type].append(iscore.score)

    def mean(xs: list[float]) -> float:
        return statistics.fmean(xs) if xs else 0.0

    return {
        "overall_score": round(result.overall_score, 6),
        "drift_resistance": round(result.drift_resistance, 6),
        "constraint_retention": round(result.constraint_retention, 6),
        "contradiction_rate": round(result.contradiction_rate, 6),
        "compression_ratio": round(result.compression_ratio, 6),
        "cycles_compression_ratio_mean": round(mean(cycle_comp), 6),
        "cases": len(result.cases),
        "per_family_case_score": {k: round(mean(v), 6) for k, v in sorted(family.items())},
        "per_item_type_score": {k: round(mean(v), 6) for k, v in sorted(item_type.items())},
        "latency_ms_mean": round(mean([float(x) for x in latencies]), 1) if latencies else None,
    }


# ---- scx 机械保留率审计（离线重放同一确定性 case，不触网）----

async def mechanical_audit(
    *,
    suite: str,
    difficulty: DifficultyLevel,
    slots: int,
    seed_group: str,
    benchmarks_dir: Path,
) -> dict:
    from scx_compactor import ScxCompactor

    templates = load_suite(benchmarks_dir / suite)
    suite_version = next(iter({t.version for t in templates}))
    compactor = ScxCompactor(MockProvider(), "offline-scx")

    probes_total = 0
    probes_hit = 0
    per_family: dict[str, list[float]] = defaultdict(list)
    token_src: list[int] = []
    token_dst: list[int] = []
    scx_meta: list[dict] = []
    cases_seen = 0

    skipped: list[str] = []
    for template in templates:
        validate_template(template)
        for slot in range(slots):
            seed = derive_case_seed(f"{suite}@{suite_version}", seed_group, slot)
            try:
                case = generate_case(template, seed, difficulty)
            except Exception:
                skipped.append(f"{template.key}:{difficulty.value}")
                continue
            artifact = await compactor.compact(case.transcript)
            rendered = (artifact.summary_text + "\n"
                        + json.dumps(artifact.structured_state.model_dump(), ensure_ascii=False))
            norm = _normalize(rendered)

            # 机械保留率审计口径：以 harness 实际判分探针为准（evaluation_items
            # 里 contains_normalized / exact / set_match 的目标值 + entity_map 人名）；
            # forbidden_absent 是「不得说出」的规避检查，不计入保留探针。
            probes: list[str] = []
            for item in case.evaluation_items:
                check = item.expected.get("check")
                if check in ("contains_normalized", "exact"):
                    probes.append(item.expected.get("value", ""))
                elif check == "set_match":
                    probes.extend(item.expected.get("values", []))
            probes += list(case.ground_truth.entity_map.keys())
            probes = [p for p in probes if p]

            hits = 0
            for p in probes:
                probes_total += 1
                if _normalize(p) in norm:
                    probes_hit += 1
                    hits += 1
            family = case.template_key.split("_v1")[0]
            per_family[family].append(hits / len(probes) if probes else 1.0)

            token_src.append(transcript_tokens(case.transcript))
            token_dst.append(artifact_tokens(artifact))
            scx_meta.append(artifact.method_metadata.get("scx", {}))
            cases_seen += 1

    modes = defaultdict(int)
    passthrough = 0
    cut_pcts: list[float] = []
    kept_lines: list[int] = []
    for m in scx_meta:
        modes[m.get("mode", "?")] += 1
        if str(m.get("mode", "")).startswith("passthrough"):
            passthrough += 1
        if m.get("cut_pct") is not None:
            cut_pcts.append(float(m["cut_pct"]))
        if m.get("kept") is not None:
            kept_lines.append(float(m["kept"]) / float(m["total"] or 1))

    return {
        "cases": cases_seen,
        "skipped_templates": sorted(set(skipped)),
        "probes_total": probes_total,
        "probes_hit": probes_hit,
        "retention": round(probes_hit / probes_total, 6) if probes_total else None,
        "per_family_retention": {k: round(statistics.fmean(v), 6) for k, v in sorted(per_family.items())},
        "token_compression_ratio": round(
            statistics.fmean(s / d for s, d in zip(token_src, token_dst)), 6
        ) if token_src else None,
        "scx_passthrough_cases": passthrough,
        "scx_mode_counts": dict(modes),
        "scx_cut_pct_mean": round(statistics.fmean(cut_pcts), 3) if cut_pcts else None,
        "scx_kept_line_ratio_mean": round(statistics.fmean(kept_lines), 4) if kept_lines else None,
    }


async def main_async(args: argparse.Namespace) -> None:
    register_deepseek()
    reset_logger(args.llm_log)
    benchmarks_dir = Path(args.benchmarks_dir)
    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    difficulty = DifficultyLevel(args.difficulty)

    methods = args.methods.split(",")
    runs: dict[str, dict] = {}
    started = time.time()

    for method_key in methods:
        spec = METHODS[method_key]
        output = out_dir / f"{method_key}_{args.suite}_{args.difficulty}.jsonl"
        print(f"[P22] run {method_key} -> {output}", flush=True)
        await run_one(
            method_key=method_key, method_spec=spec, suite=args.suite,
            difficulty=difficulty, slots=args.case_count, drift_cycles=args.drift_cycles,
            seed_group=args.seed_group, benchmarks_dir=benchmarks_dir,
            output=output, model=args.model, resume=args.resume,
        )
        result = to_run_result(output)
        runs[method_key] = {
            "results_jsonl": str(output),
            "run_id": result.run_id,
            "method_version": result.method_version,
            "suite_version": result.suite_version,
            "scorer_version": result.scorer_version,
            "started_at": result.started_at.isoformat(),
            "completed_at": result.completed_at.isoformat(),
            "aggregate": aggregate(result),
        }

    audit: dict = {}
    if "scx" in methods:
        audit = await mechanical_audit(
            suite=args.suite, difficulty=difficulty, slots=args.case_count,
            seed_group=args.seed_group, benchmarks_dir=benchmarks_dir,
        )

    summary = {
        "elapsed_seconds": round(time.time() - started, 2),
        "args": {
            "suite": args.suite,
            "difficulty": args.difficulty,
            "case_count_per_template": args.case_count,
            "drift_cycles": args.drift_cycles,
            "seed_group": args.seed_group,
            "model": args.model,
            "provider": "deepseek",
            "benchmarks_dir": str(benchmarks_dir),
        },
        "runs": runs,
        "scx_mechanical_audit": audit,
    }
    Path(args.summary_json).parent.mkdir(parents=True, exist_ok=True)
    Path(args.summary_json).write_text(
        json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    print(f"[P22] summary -> {args.summary_json}", flush=True)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--suite", default="elite_practice")
    ap.add_argument("--difficulty", default="elite", choices=["easy", "medium", "hard", "elite"])
    ap.add_argument("--case-count", type=int, default=3)
    ap.add_argument("--drift-cycles", type=int, default=2)
    ap.add_argument("--seed-group", default="default")
    ap.add_argument("--model", default="deepseek-flash")
    ap.add_argument("--methods", default="scx")
    ap.add_argument("--benchmarks-dir", default="/tmp/scx_bench_compact/repo/benchmarks/public")
    ap.add_argument("--out-dir", default="/tmp/scx_bench_compact/results")
    ap.add_argument("--llm-log", default="/tmp/scx_bench_compact/llm_calls.jsonl")
    ap.add_argument("--resume", action="store_true",
                    help="harness resume：跳过已完成 case，追加续跑")
    ap.add_argument("--summary-json", required=True)
    args = ap.parse_args()
    asyncio.run(main_async(args))


if __name__ == "__main__":
    main()
