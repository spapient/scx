#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""P20 LongMemEval 结果聚合 → 自描述冻结 JSON。

输入：
  runs/p20_S_baseline_<tag>.jsonl      全量 500 问（scx memory baseline 臂）
  runs/p20_S_versioned_<tag>.jsonl     knowledge-update 78 问（P3 版本化臂）
  runs/p20_S_oracle_<tag>.jsonl         可选 oracle 上界臂
  /tmp/scx_bench_longmem/llm_calls.jsonl  逐条 LLM 审计（token / 延迟 / 重试）

输出：config/bench/results/bench_p20_longmemeval.json
指标口径与官方 print_qa_metrics.py 对齐：
  - 六类非弃权题型各自 accuracy；
  - task-averaged = 六类均值；overall = 全部非弃权实例均值；
  - abstention = 30 道 _abs 单独 accuracy。
五能力分组见 common.ABILITY_MAP。
"""
from __future__ import annotations

import argparse
import hashlib
import json
import statistics
import subprocess
from datetime import datetime, timezone
from pathlib import Path

import common  # noqa: E402

RESULTS_DIR = Path("/Users/fei/code/scx/config/bench/results")
RUNS = common.WORK_DIR / "runs"


def load_jsonl(path: Path) -> list[dict]:
    if not path.exists():
        return []
    out = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.strip():
            out.append(json.loads(line))
    return out


def mean(xs: list[float]) -> float:
    return round(statistics.fmean(xs), 4) if xs else None


def by_type(records: list[dict]) -> dict[str, list[int]]:
    d: dict[str, list[int]] = {t: [] for t in common.NON_ABSTENTION_TYPES}
    d["abstention"] = []
    for r in records:
        if r.get("fatal_error"):
            continue
        key = "abstention" if r["abstention"] else r["question_type"]
        d.setdefault(key, []).append(1 if r["judge"]["label"] else 0)
    return d


def accuracy_table(records: list[dict]) -> dict:
    d = by_type(records)
    table = {}
    for k, vals in d.items():
        if vals:
            table[k] = {"n": len(vals), "accuracy": mean([float(v) for v in vals])}
    non_abs = [v for t in common.NON_ABSTENTION_TYPES for v in d[t]]
    task_means = [table[t]["accuracy"] for t in common.NON_ABSTENTION_TYPES if t in table]
    out = {
        "by_question_type": table,
        "task_averaged_accuracy": mean(task_means),
        "overall_non_abstention_accuracy": mean([float(v) for v in non_abs]),
        "overall_non_abstention_n": len(non_abs),
    }
    if "abstention" in table:
        out["abstention_accuracy"] = table["abstention"]["accuracy"]
        out["abstention_n"] = table["abstention"]["n"]
    return out


def ability_table(records: list[dict]) -> dict:
    groups: dict[str, list[int]] = {a: [] for a in common.ABILITY_ORDER}
    for r in records:
        if r.get("fatal_error"):
            continue
        groups[r["ability"]].append(1 if r["judge"]["label"] else 0)
    return {a: {"n": len(v), "accuracy": mean([float(x) for x in v])}
            for a, v in groups.items()}


def retrieval_diag(records: list[dict]) -> dict:
    keys = ["recall_any@1", "recall_any@3", "recall_any@5", "recall_any@10",
            "recall_any@30", "recall_all@10", "ndcg@10"]
    agg: dict[str, list[float]] = {k: [] for k in keys}
    for r in records:
        rd = r.get("retrieval")
        if not rd or r.get("abstention"):
            continue
        for k in keys:
            if k in rd:
                agg[k].append(float(rd[k]))
    return {k: mean(v) for k, v in agg.items() if v}


def ingest_diag(records: list[dict]) -> dict:
    stored = dedup = 0
    errors = 0
    chars_in = chars_out = 0
    walls: list[float] = []
    for r in records:
        ig = r.get("ingest")
        if not ig:
            continue
        stored += ig.get("stored", 0)
        dedup += ig.get("dedup", 0)
        errors += len(ig.get("errors", []))
        chars_in += ig.get("chars_in", 0)
        chars_out += ig.get("chars_out", 0)
        walls.append(ig.get("wall_ms", 0))
    return {
        "sessions_stored": stored, "sessions_dedup": dedup, "ingest_errors": errors,
        "chars_in": chars_in, "chars_out": chars_out,
        "char_retention": round(chars_out / chars_in, 6) if chars_in else None,
        "mean_ingest_wall_ms_per_question": mean(walls) if walls else None,
    }


def ku_special(baseline: list[dict], versioned: list[dict]) -> dict:
    base = {r["question_id"]: r for r in baseline
            if r["question_type"] == "knowledge-update" and not r.get("fatal_error")}
    ver = {r["question_id"]: r for r in versioned if not r.get("fatal_error")}
    paired = sorted(set(base) & set(ver))
    base_correct = sum(1 for q in paired if base[q]["judge"]["label"])
    ver_correct = sum(1 for q in paired if ver[q]["judge"]["label"])

    def leak(recs: dict[str, dict]) -> dict:
        old_topk = new_topk = 0
        old_ranks, new_ranks = [], []
        for q in paired:
            sp = recs[q].get("ku_special") or {}
            if sp.get("old_in_topk"):
                old_topk += 1
                old_ranks.append(sp["old_rank"])
            if sp.get("new_in_topk"):
                new_topk += 1
                new_ranks.append(sp["new_rank"])
        n = len(paired)
        return {
            "old_evidence_in_topk": old_topk, "old_evidence_in_topk_rate": round(old_topk / n, 4),
            "new_evidence_in_topk": new_topk, "new_evidence_in_topk_rate": round(new_topk / n, 4),
            "old_median_rank": statistics.median(old_ranks) if old_ranks else None,
            "new_median_rank": statistics.median(new_ranks) if new_ranks else None,
        }

    return {
        "n_paired": len(paired),
        "baseline_accuracy": round(base_correct / len(paired), 4) if paired else None,
        "versioned_accuracy": round(ver_correct / len(paired), 4) if paired else None,
        "accuracy_delta_versioned_minus_baseline": (
            round(ver_correct / len(paired) - base_correct / len(paired), 4)
            if paired else None),
        "baseline_retrieval": leak(base),
        "versioned_retrieval": leak(ver),
        "switched_qids": sorted(q for q in paired
                                if bool(base[q]["judge"]["label"]) != bool(ver[q]["judge"]["label"])),
        "versioned_old_case_excluded_verified": all(
            not (ver[q].get("ku_special") or {}).get("old_in_topk") for q in paired),
    }


def abstention_special(records: list[dict]) -> dict:
    abs_recs = [r for r in records if r.get("abstention") and not r.get("fatal_error")]
    correct = [r for r in abs_recs if r["judge"]["label"]]
    phrase = [r for r in abs_recs if r.get("abstention_phrase")]
    sims: list[float] = []
    hist: list[int] = []
    for r in abs_recs:
        hits = r.get("recall") or []
        if hits:
            sims.append(float(hits[0]["sim"]))
        hist.append(r.get("history_chars", 0))
    return {
        "n": len(abs_recs),
        "judge_correct": len(correct),
        "accuracy": mean([1 if r["judge"]["label"] else 0 for r in abs_recs]),
        "abstention_phrase_rate": mean([1.0 if r.get("abstention_phrase") else 0
                                        for r in abs_recs]),
        "mean_top1_sim": mean(sims),
        "mean_history_chars": mean(hist),
    }


def llm_audit(log_path: Path) -> dict:
    calls = {"reader": [], "judge": []}
    errors = 0
    for line in log_path.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        c = json.loads(line)
        if c.get("error"):
            errors += 1
        calls[c["role"]].append(c)
    # deepseek-flash 峰谷价（2026-10-31 周末=闲时；USD / 1M tokens，
    # 来源 https://api-docs.deepseek.com/quick_start/pricing）：
    # 输入 cache hit 0.003 / cache miss 0.15；输出 0.60。
    PRICE = {"in_hit": 0.003, "in_miss": 0.15, "out": 0.60}
    summary: dict = {"pricing_note": "deepseek-flash off-peak weekend rates, USD/1M tokens; "
                    "source: https://api-docs.deepseek.com/quick_start/pricing "
                    "(fetched 2026-10-31)", "rates_per_1m_usd": PRICE}
    grand_cost = 0.0
    for role, rows in calls.items():
        pt = sum((r["usage"] or {}).get("prompt_tokens", 0) for r in rows)
        ct = sum((r["usage"] or {}).get("completion_tokens", 0) for r in rows)
        hit = sum((r["usage"] or {}).get("prompt_cache_hit_tokens", 0) for r in rows)
        miss = sum((r["usage"] or {}).get("prompt_cache_miss_tokens", 0) for r in rows)
        cost = (hit * PRICE["in_hit"] + miss * PRICE["in_miss"] + ct * PRICE["out"]) / 1e6
        grand_cost += cost
        lat = [r["latency_ms"] for r in rows if r["latency_ms"] is not None]
        summary[role] = {
            "calls": len(rows), "prompt_tokens": pt, "completion_tokens": ct,
            "total_tokens": pt + ct, "cache_hit_tokens": hit, "cache_miss_tokens": miss,
            "estimated_cost_usd": round(cost, 4),
            "mean_latency_ms": round(statistics.fmean(lat), 1) if lat else None,
            "total_latency_ms": round(sum(lat), 1),
            "retries": sum(r["retries"] for r in rows),
            "model": sorted({r["model"] for r in rows}),
            "max_tokens": sorted({r["max_tokens"] for r in rows}),
        }
    summary["estimated_total_cost_usd"] = round(grand_cost, 4)
    summary["failed_calls"] = errors
    return summary


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--baseline-tag", default="full")
    ap.add_argument("--versioned-tag", default="full")
    ap.add_argument("--oracle-tag", default="")
    ap.add_argument("--out", default=str(RESULTS_DIR / "bench_p20_longmemeval.json"))
    args = ap.parse_args()

    baseline = load_jsonl(RUNS / f"p20_S_baseline_{args.baseline_tag}.jsonl")
    versioned = load_jsonl(RUNS / f"p20_S_versioned_{args.versioned_tag}.jsonl")
    oracle = load_jsonl(RUNS / f"p20_S_oracle_{args.oracle_tag}.jsonl") if args.oracle_tag else []

    fatal = [r["question_id"] for r in baseline if r.get("fatal_error")]

    scx_version = subprocess.run([common.SCX_BIN, "--version"], capture_output=True,
                                 text=True).stdout.strip()

    report = {
        "schema": "scx-bench-p20-longmemeval-v1",
        "generated_at": datetime.now(timezone.utc).astimezone().isoformat(),
        "benchmark": "LongMemEval (arXiv 2410.10813, ICLR 2025)",
        "scx": {
            "binary": common.SCX_BIN,
            "version": scx_version,
            "source_commit": "b507343b6372186a640f04cf433b8935e7b8135c",
        },
        "official": {
            "repo": "https://github.com/xiaowu0162/LongMemEval",
            "repo_local": "/tmp/scx_bench_longmem/repo",
            "repo_commit": "9e0b455f4ef0e2ab8f2e582289761153549043fc",
            "dataset_hf": "xiaowu0162/longmemeval-cleaned",
            "dataset_file": "longmemeval_s_cleaned.json",
            "dataset_sha256": sha256_file(common.DATA_DIR / "longmemeval_s_cleaned.json"),
            "oracle_file": "longmemeval_oracle.json",
            "oracle_sha256": sha256_file(common.DATA_DIR / "longmemeval_oracle.json"),
            "source_files": {
                "judge_prompts": "src/evaluation/evaluate_qa.py get_anscheck_prompt",
                "reader_prompt": "src/generation/run_generation.py prepare_prompt (con/json/useronly=false)",
                "retrieval_metrics": "src/retrieval/eval_utils.py",
                "aggregate_metrics": "src/evaluation/print_qa_metrics.py",
            },
        },
        "protocol": {
            "dataset": "LongMemEval-S (longmemeval_s_cleaned.json, 500 questions)",
            "recall_k": common.RECALL_K,
            "reader_prompt_variant": "con (step-by-step extract then reason), json history, useronly=false",
            "temperature": 0,
            "seed": common.SEED,
            "ingest_recipe": (
                "per-question fresh mem-dir; each haystack session = one --store 1 case; "
                "store query = user turns joined (official session BM25 corpus口径); "
                "--ratio 1.0 --context 1 --kind session-message; fix = session date + "
                "official session id; chronological ingest; recall --recall-only top-K "
                "by question; history rendered official json format sorted by date."
            ),
            "judge_model_note": (
                "Official judge = gpt-4o; environment substitutes deepseek-flash with the "
                "verbatim official judge prompt and temperature=0; prompt/answer protocol "
                "unchanged; model-judge caliber difference recorded here as a口径 caveat."
            ),
            "reader_model": "deepseek-flash",
            "judge_model": "deepseek-flash",
            "reader_max_tokens": common.READER_MAX_TOKENS,
            "judge_max_tokens": common.JUDGE_MAX_TOKENS,
            "workers": common.LLM_WORKERS,
        },
        "counts": {
            "baseline_questions": len(baseline),
            "versioned_questions": len(versioned),
            "oracle_questions": len(oracle),
            "fatal_errors": fatal,
        },
        "baseline": {
            "official_metrics": accuracy_table(baseline),
            "five_ability": ability_table(baseline),
            "retrieval_diagnostics": retrieval_diag(baseline),
            "ingest": ingest_diag(baseline),
        },
        "knowledge_updates_special": ku_special(baseline, versioned),
        "abstention_special": abstention_special(baseline),
        "llm_audit": llm_audit(common.LLM_LOG),
    }

    if oracle:
        report["oracle_arm"] = {
            "official_metrics": accuracy_table(oracle),
            "five_ability": ability_table(oracle),
        }

    # 逐题明细行（冻结 rows 口径）：保留判分与诊断关键字段。
    def slim(r: dict) -> dict:
        return {
            "question_id": r["question_id"], "question_type": r["question_type"],
            "ability": r["ability"], "abstention": r["abstention"],
            "question": r["question"], "answer": r["answer"],
            "hypothesis": r["hypothesis"],
            "judge_label": r["judge"].get("label"),
            "judge_response": r["judge"].get("response"),
            "retrieval": r.get("retrieval"),
            "ku_special": r.get("ku_special"),
            "history_chars": r.get("history_chars"),
            "arm": r["arm"],
        }

    report["rows"] = [slim(r) for r in baseline]
    if versioned:
        report["rows_versioned"] = [slim(r) for r in versioned]

    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(report, ensure_ascii=False, indent=1), encoding="utf-8")
    print(f"wrote {out} rows={len(report['rows'])} "
          f"versioned={len(report.get('rows_versioned', []))}")

    # 控制台摘要
    m = report["baseline"]["official_metrics"]
    print("task-averaged:", m["task_averaged_accuracy"],
          "| overall non-abs:", m["overall_non_abstention_accuracy"],
          "| abstention:", m.get("abstention_accuracy"))
    print(json.dumps(report["baseline"]["five_ability"], indent=1, ensure_ascii=False))
    print("KU special:", json.dumps(report["knowledge_updates_special"],
                                     indent=1, ensure_ascii=False)[:1200])


if __name__ == "__main__":
    main()
