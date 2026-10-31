#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""P21 Memora（arXiv:2604.20006, ACL 2026 Findings）scx 记忆体系接入 + 冻结基线。

官方协议（evals/README.md + agent_eval/memory_to_answer.py，commit a6493188）：
- 两阶段：先按会话顺序把对话摄入记忆系统（user_id=<persona>_<period> 隔离），
  再对每题召回记忆 → LLM 作答 → LLM 裁判按 yes/no 子问题逐条判分。
- 指标（paper §4.2，fama_score() 逐字对齐）：
    MPA = correct memory_presence / total memory_presence
    FAA = correct forgetting_absence / total forgetting_absence（无遗忘子题时 FAA=1.0）
    λ   = N_forget / (N_presence + N_forget)
    FAMA = max(0, MPA − λ·(1−FAA))；reasoning 任务无遗忘子题，λ=0，FAMA=MPA。
  桶内 FAMA = 桶内逐题 FAMA 均值 ×100（[0,100]，对齐官方 by_task_type.fama）。

治理对照（本基准核心实验）：
- nogov：P2 仲裁关闭（conflict_sim=1.0）；update/delete 不 revise/retire，
  过时/失效记忆保持 active 可召回。
- gov：P2 仲裁默认开启；update → --revise；delete → --retire。

用法（见 README.md 复现命令）：
  python run_p21.py ingest [--periods weekly monthly quarterly] [--personas ...]
  python run_p21.py eval [--sample-frac 1.0] [--seed 21] [--recall-k 50]
  python run_p21.py report
  python run_p21.py run
"""
from __future__ import annotations

import argparse
import json
import random
import re
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path
from typing import Any, Optional

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from p21_provider import DeepSeekProvider  # noqa: E402
from p21_scx import ScxMem  # noqa: E402

# --------------------------------------------------------------------- 路径
MEMORA_ROOT = Path("/tmp/scx_bench_memora/Memora")
WORK = Path("/tmp/scx_bench_memora")
MEM_ROOT = WORK / "mem"
RESULTS_ROOT = WORK / "results"
LLM_LOG = WORK / "llm_calls.jsonl"
REPORT_JSON = Path("/Users/fei/code/scx/config/bench/results/bench_p21_memora.json")

PERIODS = ("weekly", "monthly", "quarterly")
TASKS = ("remembering", "reasoning", "recommending")
TASK_LABEL = {"remembering": "Remembering", "reasoning": "Reasoning",
              "recommending": "Recommending"}
BRANCHES = ("nogov", "gov")
RECALL_K = 20
SEED = 21
ANSWER_MAX_TOKENS = 500
JUDGE_MAX_TOKENS = 400
MAX_MEM_CHARS = 12000
PER_MEM_CHARS = 500

# --------------------------------------------------------------------- 官方 prompt
ANSWER_SYSTEM = """You are a helpful AI assistant with access to a user's personal memory system.

Your task is to answer questions based ONLY on the user's stored memories and preferences.

Guidelines:
1. Use ONLY the information from the provided memories
2. Be specific and reference actual items/preferences from memory
3. If the question is about recommendations, suggest based on similar items in memory
4. Be conversational and helpful
5. Don't make up information not in the memories"""

ANSWER_USER = """User's Question: {question}

User's Relevant Memories:
{memory_text}

Please provide a helpful answer based on these memories."""

JUDGE_SYSTEM = """You are an expert evaluator assessing AI assistant responses. Your task is to answer a YES/NO evaluation question about a given response.

You must provide your answer in the following JSON format:
{{
    "answer": "yes" or "no",
    "confidence": 0.0 to 1.0,
    "explanation": "Brief explanation of your reasoning"
}}

Be objective and thorough in your evaluation."""

JUDGE_USER = """Please evaluate the following AI response against the evaluation question.

AI RESPONSE TO EVALUATE:
{answer}

EVALUATION QUESTION:
{eval_question}

Provide your evaluation in JSON format with answer (yes/no), confidence (0.0-1.0), and explanation."""


# --------------------------------------------------------------------- 数据装载
def list_personas(period: str) -> list[str]:
    d = MEMORA_ROOT / "data" / period
    return sorted(p.name for p in d.iterdir() if p.is_dir())


def load_questions(period: str, persona: str) -> list[dict[str, Any]]:
    qdir = MEMORA_ROOT / "data" / period / persona
    files = list(qdir.glob("evaluation_questions*.json"))
    if not files:
        return []
    data = json.loads(files[0].read_text(encoding="utf-8"))
    out: list[dict[str, Any]] = []
    for task in TASKS:
        for q in data.get("questions", {}).get(task, []):
            q = dict(q)
            q["period"] = period
            q["persona"] = persona
            q["task"] = task
            q["task_label"] = TASK_LABEL[task]
            out.append(q)
    return out


def load_sessions(period: str, persona: str) -> list[dict[str, Any]]:
    sdir = MEMORA_ROOT / "data" / period / persona / "conversations"
    files = sorted(sdir.glob("session_*.json"),
                   key=lambda p: int(re.search(r"session_(\d+)", p.name).group(1)))
    return [json.loads(f.read_text(encoding="utf-8")) for f in files]


# --------------------------------------------------------------------- 指标
def fama_score(p_correct: int, p_total: int, f_correct: int, f_total: int) -> dict[str, float]:
    """对齐官方 fama_score()：返回 MPA/FAA/λ/FAMA/penalty。"""
    if p_total == 0 and f_total == 0:
        return {"mpa": 0.0, "faa": 1.0, "lambda": 0.0, "fama": 0.0, "penalty": 0.0}
    mpa = p_correct / p_total if p_total > 0 else 0.0
    faa = f_correct / f_total if f_total > 0 else 1.0
    lam = f_total / (p_total + f_total) if (p_total + f_total) > 0 else 0.0
    fama = max(0.0, mpa - lam * (1.0 - faa))
    return {"mpa": mpa, "faa": faa, "lambda": lam, "fama": fama,
            "penalty": lam * (1.0 - faa)}


# --------------------------------------------------------------------- 判分解析
def parse_judge(raw: str, expected: str) -> dict[str, Any]:
    """对齐官方 _evaluate_with_single_judge 的 JSON 解析 + 文本兜底。"""
    text = raw.strip()
    if text.startswith("```"):
        lines = text.split("\n")
        text = "\n".join(lines[1:-1]) if len(lines) > 2 else text
        text = text.replace("```json", "").replace("```", "").strip()
    try:
        result = json.loads(text)
        answer = str(result.get("answer", "")).lower().strip()
        confidence = float(result.get("confidence", 0.0))
        explanation = result.get("explanation", "")
        if answer not in ("yes", "no"):
            low = explanation.lower()
            if "yes" in low and "no" not in low:
                answer = "yes"
            elif "no" in low and "yes" not in low:
                answer = "no"
            else:
                answer = "unclear"
        correct = answer == expected.lower()
        return {"llm_answer": answer, "is_correct": correct,
                "confidence": confidence, "explanation": explanation,
                "parse_error": None}
    except json.JSONDecodeError as exc:
        low = text.lower()
        if "yes" in low and "no" not in low:
            answer = "yes"
        elif "no" in low and "yes" not in low:
            answer = "no"
        else:
            answer = "unclear"
        return {"llm_answer": answer,
                "is_correct": answer == expected.lower(),
                "confidence": 0.5, "explanation": f"Parse error fallback: {text[:200]}",
                "parse_error": str(exc)}


# --------------------------------------------------------------------- 摄取
def cmd_ingest(args: argparse.Namespace) -> None:
    periods = args.periods or list(PERIODS)
    branches = args.branches or list(BRANCHES)
    audit: dict[str, Any] = {"branches": {}, "sessions": 0, "skipped": 0}
    for period in periods:
        personas = args.personas or list_personas(period)
        for persona in personas:
            sessions = load_sessions(period, persona)
            for branch in branches:
                root = MEM_ROOT / branch / f"{period}__{persona}"
                mem = ScxMem(root, governance=(branch == "gov"),
                             workdir=WORK / "tmp" / branch / f"{period}__{persona}")
                done = {a["session_id"] for a in mem.registry.get("audits", [])}
                counts: dict[str, int] = {}
                for sess in sessions:
                    if sess.get("session_id") in done:
                        continue
                    a = mem.ingest_session(sess)
                    counts[a["action"]] = counts.get(a["action"], 0) + 1
                stats = mem.stats()
                audit["branches"].setdefault(branch, {})[f"{period}__{persona}"] = {
                    "sessions": len(sessions), "actions": counts, "stats": stats}
                audit["sessions"] += len(sessions)
                print(f"[ingest] {branch} {period}/{persona}: "
                      f"sessions={len(sessions)} actions={counts} "
                      f"cases={stats.get('cases')} {stats.get('life', '')}")
    (RESULTS_ROOT / "ingest_audit.json").write_text(
        json.dumps(audit, ensure_ascii=False, indent=1), encoding="utf-8")


# --------------------------------------------------------------------- 评测
def sample_questions(all_q: list[dict[str, Any]], frac: float, seed: int) -> list[dict[str, Any]]:
    if frac >= 1.0:
        return all_q
    rng = random.Random(seed)
    buckets: dict[tuple[str, str], list[dict[str, Any]]] = {}
    for q in all_q:
        buckets.setdefault((q["period"], q["task"]), []).append(q)
    picked: list[dict[str, Any]] = []
    for key, qs in sorted(buckets.items()):
        n = max(1, round(len(qs) * frac))
        picked += rng.sample(qs, min(n, len(qs)))
    picked.sort(key=lambda q: (q["period"], q["persona"], q["task"], q["question_id"]))
    return picked


def answer_one(provider: DeepSeekProvider, mem: ScxMem, q: dict[str, Any],
               recall_k: int) -> dict[str, Any]:
    hits = mem.recall(q["question"], k=recall_k)
    fallback = ("I don't have enough information in my memory to answer this "
                "question accurately.")
    if not hits:
        return {"answer": fallback, "memories": [], "n_memories": 0,
                "answer_ok": True, "answer_usage": {}, "answer_elapsed_s": 0.0}
    # 上下文用入库时压缩好的 fix（事实摘要），超长截断；口径记录在 README。
    lines: list[str] = []
    used = 0
    for i, h in enumerate(hits, 1):
        body = h.get("fix") or h.get("summary") or h.get("text") or ""
        body = body.replace("\n", " ").strip()[:PER_MEM_CHARS]
        if used + len(body) > MAX_MEM_CHARS:
            break
        used += len(body)
        lines.append(f"{i}. {body} (relevance: {h['score']:.2f})")
    memory_text = "\n".join(lines)
    meta = {"phase": "answer", "period": q["period"], "persona": q["persona"],
            "task": q["task"], "question_id": q["question_id"],
            "n_memories": len(lines)}
    resp = provider.call(ANSWER_SYSTEM,
                         ANSWER_USER.format(question=q["question"], memory_text=memory_text),
                         purpose="memora_answer", meta=meta,
                         max_tokens=ANSWER_MAX_TOKENS)
    return {"answer": resp["content"] or fallback,
            "memories": [{"short": h["short"], "score": h["score"],
                          "sim": h["sim"], "fix": h.get("fix", "")} for h in hits],
            "n_memories": len(lines), "answer_ok": resp["ok"],
            "answer_usage": resp["usage"], "answer_elapsed_s": resp["elapsed_s"]}


def judge_one(provider: DeepSeekProvider, answer: str, sub: dict[str, Any],
              q: dict[str, Any]) -> dict[str, Any]:
    meta = {"phase": "judge", "period": q["period"], "persona": q["persona"],
            "task": q["task"], "question_id": q["question_id"],
            "eval_id": sub["evaluation_question_id"],
            "eval_type": sub["evaluation_type"]}
    resp = provider.call(JUDGE_SYSTEM,
                         JUDGE_USER.format(answer=answer,
                                           eval_question=sub["evaluation_question"]),
                         purpose="memora_judge", meta=meta,
                         max_tokens=JUDGE_MAX_TOKENS, model=provider.judge_model)
    verdict = parse_judge(resp["content"], sub["expected_answer"]) if resp["ok"] else {
        "llm_answer": "error", "is_correct": False, "confidence": 0.0,
        "explanation": "provider failure", "parse_error": "provider_failed"}
    return {"evaluation_question_id": sub["evaluation_question_id"],
            "evaluation_question": sub["evaluation_question"],
            "expected_answer": sub["expected_answer"],
            "evaluation_type": sub["evaluation_type"],
            "judge_model": provider.model, "judge_ok": resp["ok"],
            "usage": resp["usage"], "elapsed_s": resp["elapsed_s"],
            **verdict}


def eval_persona_branch(provider: DeepSeekProvider, branch: str, period: str,
                       persona: str, questions: list[dict[str, Any]],
                       recall_k: int) -> dict[str, Any]:
    root = MEM_ROOT / branch / f"{period}__{persona}"
    mem = ScxMem(root, governance=(branch == "gov"),
                 workdir=WORK / "tmp" / branch / f"{period}__{persona}")

    def process(q: dict[str, Any]) -> dict[str, Any]:
        ans = answer_one(provider, mem, q, recall_k)
        subs = q["evaluation"]["evaluation_questions"]
        judge_results: list[dict[str, Any]] = []
        # 题内裁判小并发（3）；题间由外层 10 并发池驱动，避免瞬时连接风暴。
        with ThreadPoolExecutor(max_workers=3) as pool:
            futs = {pool.submit(judge_one, provider, ans["answer"], sub, q): sub
                    for sub in subs}
            for fut in as_completed(futs):
                judge_results.append(fut.result())
        judge_results.sort(key=lambda r: r["evaluation_question_id"])

        p_total = p_correct = f_total = f_correct = 0
        for r in judge_results:
            if r["evaluation_type"] == "memory_presence":
                p_total += 1
                p_correct += int(r["is_correct"])
            elif r["evaluation_type"] == "forgetting_absence":
                f_total += 1
                f_correct += int(r["is_correct"])
        m = fama_score(p_correct, p_total, f_correct, f_total)
        row = {
            "question_id": q["question_id"], "question": q["question"],
            "question_date": q.get("question_date"), "task": q["task"],
            "task_label": q["task_label"], "memories_retrieved": ans["n_memories"],
            "model_response": ans["answer"], "answer_ok": ans["answer_ok"],
            "answer_usage": ans["answer_usage"],
            "evaluation_questions": judge_results,
            "memory_presence_total": p_total, "memory_presence_correct": p_correct,
            "forgetting_absence_total": f_total,
            "forgetting_absence_correct": f_correct,
            **m,
            "memory_evidence": q.get("memory_evidence", {}),
            "forgetting_evidence": q.get("forgetting_evidence", {}),
        }
        print(f"[eval] {branch} {period}/{persona} {q['task']} "
              f"{q['question_id']}: FAMA={m['fama']:.3f} MPA={m['mpa']:.2f} "
              f"FAA={m['faa']:.2f} penalty={m['penalty']:.3f}", flush=True)
        return row

    detailed: list[dict[str, Any]] = []
    with ThreadPoolExecutor(max_workers=10) as pool:
        futs = {pool.submit(process, q): q for q in questions}
        for fut in as_completed(futs):
            detailed.append(fut.result())
    detailed.sort(key=lambda r: (r["task"], r["question_id"]))
    return {"branch": branch, "period": period, "persona": persona,
            "recall_k": recall_k, "detailed_results": detailed,
            "mem_stats": mem.stats()}


def cmd_eval(args: argparse.Namespace) -> None:
    periods = args.periods or list(PERIODS)
    all_q: list[dict[str, Any]] = []
    for period in periods:
        personas = args.personas or list_personas(period)
        for persona in personas:
            all_q += load_questions(period, persona)
    picked = sample_questions(all_q, args.sample_frac, args.seed)
    print(f"[eval] inventory={len(all_q)} sampled={len(picked)} "
          f"frac={args.sample_frac} seed={args.seed}")

    provider = DeepSeekProvider(LLM_LOG, model=args.model,
                                judge_model=getattr(args, "judge_model", "deepseek-chat"),
                                temperature=0.0, max_tokens=2048,
                                reasoning_off=not args.reasoning)
    branches = args.branches or list(BRANCHES)
    for branch in branches:
        for period in periods:
            personas = args.personas or list_personas(period)
            for persona in personas:
                out = RESULTS_ROOT / branch / f"{period}__{persona}.json"
                out.parent.mkdir(parents=True, exist_ok=True)
                if out.exists() and not args.resume_overwrite:
                    print(f"[eval] skip existing {out}")
                    continue
                qs = [q for q in picked if q["period"] == period and q["persona"] == persona]
                if not qs:
                    continue
                report = eval_persona_branch(provider, branch, period, persona,
                                            qs, args.recall_k)
                out.write_text(json.dumps(report, ensure_ascii=False, indent=1),
                               encoding="utf-8")
    (RESULTS_ROOT / "llm_stats.json").write_text(
        json.dumps(provider.stats, ensure_ascii=False, indent=1), encoding="utf-8")
    print(f"[eval] LLM stats: {provider.stats}")


# --------------------------------------------------------------------- 汇总
def aggregate_reports() -> dict[str, Any]:
    bucket: dict[tuple[str, str, str], dict[str, Any]] = {}
    overall: dict[str, Any] = {b: {"questions": 0, "p_total": 0, "p_correct": 0,
                                   "f_total": 0, "f_correct": 0, "fama_q": [],
                                   "penalty_q": []} for b in BRANCHES}
    reports: list[Path] = []
    for branch in BRANCHES:
        for f in sorted((RESULTS_ROOT / branch).glob("*.json")) if (RESULTS_ROOT / branch).exists() else []:
            reports.append(f)
    for f in reports:
        rep = json.loads(f.read_text(encoding="utf-8"))
        branch, period, persona = rep["branch"], rep["period"], rep["persona"]
        for r in rep["detailed_results"]:
            task = r["task"]
            key = (branch, period, task)
            b = bucket.setdefault(key, {"questions": 0, "p_total": 0, "p_correct": 0,
                                        "f_total": 0, "f_correct": 0, "fama_q": [],
                                        "penalty_q": []})
            b["questions"] += 1
            b["p_total"] += r["memory_presence_total"]
            b["p_correct"] += r["memory_presence_correct"]
            b["f_total"] += r["forgetting_absence_total"]
            b["f_correct"] += r["forgetting_absence_correct"]
            b["fama_q"].append(r["fama"])
            b["penalty_q"].append(r["penalty"])
            o = overall[branch]
            o["questions"] += 1
            o["p_total"] += r["memory_presence_total"]
            o["p_correct"] += r["memory_presence_correct"]
            o["f_total"] += r["forgetting_absence_total"]
            o["f_correct"] += r["forgetting_absence_correct"]
            o["fama_q"].append(r["fama"])
            o["penalty_q"].append(r["penalty"])

    def pack(b: dict[str, Any]) -> dict[str, Any]:
        mpa = b["p_correct"] / b["p_total"] if b["p_total"] else None
        faa = b["f_correct"] / b["f_total"] if b["f_total"] else None
        fama = sum(b["fama_q"]) / len(b["fama_q"]) * 100 if b["fama_q"] else 0.0
        penalty = sum(b["penalty_q"]) / len(b["penalty_q"]) if b["penalty_q"] else 0.0
        return {"questions": b["questions"],
                "memory_presence_total": b["p_total"],
                "memory_presence_correct": b["p_correct"],
                "mpa": round(mpa, 4) if mpa is not None else None,
                "forgetting_absence_total": b["f_total"],
                "forgetting_absence_correct": b["f_correct"],
                "faa": round(faa, 4) if faa is not None else None,
                "fama": round(fama, 2),
                "mean_penalty": round(penalty, 4),
                "mean_penalty_x100": round(penalty * 100, 2)}

    table: dict[str, Any] = {}
    for branch in BRANCHES:
        table[branch] = {"by_period_task": {}, "overall": pack(overall[branch])}
        for period in PERIODS:
            table[branch]["by_period_task"][period] = {}
            for task in TASKS:
                b = bucket.get((branch, period, task))
                table[branch]["by_period_task"][period][task] = pack(b) if b else None

    # 治理对照：gov − nogov 逐桶差值；惩罚项下降是治理的核心证据。
    comparison: dict[str, Any] = {"by_period_task": {}}
    for period in PERIODS:
        comparison["by_period_task"][period] = {}
        for task in TASKS:
            n = bucket.get(("nogov", period, task))
            g = bucket.get(("gov", period, task))
            if not n or not g:
                comparison["by_period_task"][period][task] = None
                continue
            pn, pg = pack(n), pack(g)
            comparison["by_period_task"][period][task] = {
                "delta_fama": round(pg["fama"] - pn["fama"], 2),
                "delta_mpa": round((pg["mpa"] or 0) - (pn["mpa"] or 0), 4),
                "delta_faa": round((pg["faa"] or 0) - (pn["faa"] or 0), 4),
                "delta_penalty": round(pg["mean_penalty"] - pn["mean_penalty"], 4),
                "nogov": pn, "gov": pg,
            }
    pn, pg = pack(overall["nogov"]), pack(overall["gov"])
    comparison["overall"] = {
        "delta_fama": round(pg["fama"] - pn["fama"], 2),
        "delta_mpa": round((pg["mpa"] or 0) - (pn["mpa"] or 0), 4),
        "delta_faa": round((pg["faa"] or 0) - (pn["faa"] or 0), 4),
        "delta_penalty": round(pg["mean_penalty"] - pn["mean_penalty"], 4),
        "nogov": pn, "gov": pg,
    }
    return {"table": table, "comparison": comparison, "reports": [str(r) for r in reports]}


def _rollup_ingest(ingest: dict[str, Any]) -> dict[str, Any]:
    """汇总两分支治理动作计数（store/revise/retire/miss/keep_active 等）。"""
    out: dict[str, Any] = {b: {"actions": {}, "cases": 0, "sessions": 0}
                           for b in BRANCHES}
    branches = ingest.get("branches", {})
    for branch, per in branches.items():
        agg: dict[str, int] = {}
        cases = sessions = 0
        for _, info in per.items():
            sessions += info.get("sessions", 0)
            for action, n in info.get("actions", {}).items():
                agg[action] = agg.get(action, 0) + n
            stats = info.get("stats", {})
            cases += int(stats.get("cases", 0) or 0)
        out[branch] = {"actions": agg, "cases": cases, "sessions": sessions}
    return out


def _inventory_counts() -> dict[str, Any]:
    total = {p: {"questions": 0, "sub_questions": 0} for p in PERIODS}
    for period in PERIODS:
        for persona in list_personas(period):
            for q in load_questions(period, persona):
                total[period]["questions"] += 1
                total[period]["sub_questions"] += len(
                    q["evaluation"]["evaluation_questions"])
    total["all"] = {"questions": sum(v["questions"] for k, v in total.items()
                                    if k != "all"),
                    "sub_questions": sum(v["sub_questions"] for k, v in total.items()
                                          if k != "all")}
    return total


def cmd_report(_: argparse.Namespace) -> None:
    summary = aggregate_reports()
    llm = json.loads((RESULTS_ROOT / "llm_stats.json").read_text(encoding="utf-8")) \
        if (RESULTS_ROOT / "llm_stats.json").exists() else {}
    ingest = json.loads((RESULTS_ROOT / "ingest_audit.json").read_text(encoding="utf-8")) \
        if (RESULTS_ROOT / "ingest_audit.json").exists() else {}
    gov_audit = _rollup_ingest(ingest)
    inv = _inventory_counts()

    # 自动生成双面结论（数值驱动；定性措辞在完成记录中补充人工判断）。
    comp = summary["comparison"]["overall"]
    conclusions: dict[str, Any] = {
        "headline": (
            f"gov − nogov: ΔFAMA={comp['delta_fama']} (points), "
            f"ΔMPA={comp['delta_mpa']}, ΔFAA={comp['delta_faa']}, "
            f"Δmean_penalty={comp['delta_penalty']}."),
        "governance_helps_forgetting": comp["delta_faa"] > 0 and comp["delta_penalty"] < 0,
        "governance_hurts_presence": comp["delta_mpa"] < 0,
        "notes": [
            "MPA 反映事实召回/作答覆盖；FAA 反映是否规避援引过时记忆；"
            "penalty=λ(1−FAA) 是 FAMA 中由过时记忆污染直接扣掉的部分。",
            "两分支共用同一作答/裁判模型与同一问题集，Δ 仅来自记忆治理差异；"
            "绝对分数受模型/裁判口径影响，不与官方 Table 3 直接可比。",
        ],
    }

    report = {
        "schema": "scx-bench-p21-memora-v1",
        "benchmark": {
            "name": "Memora",
            "paper": "From Recall to Forgetting: Benchmarking Long-Term Memory for "
                     "Personalized Agents",
            "arxiv": "2604.20006", "venue": "ACL 2026 Findings",
            "repo": "https://github.com/geniesinc/Memora",
            "commit": "a6493188efc836d6511ed5e4163fe3ba87da30ff",
            "commit_date": "2026-06-03",
            "metric_protocol": "FAMA = max(0, MPA - lambda*(1-FAA)); "
                               "lambda = N_forget/(N_presence+N_forget); "
                               "bucket FAMA = mean(per-question FAMA)*100",
        },
        "scx": {"version": "0.11.0", "binary": "/Users/fei/code/scx/target/release/scx",
                "repo_head": "b507343b6372186a640f04cf433b8935e7b8135c"},
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "seed": SEED,
        "commands": {
            "ingest": "python tools/bench_p21_memora/run_p21.py ingest",
            "eval": "python tools/bench_p21_memora/run_p21.py eval",
            "report": "python tools/bench_p21_memora/run_p21.py report",
        },
        "inventory": inv,
        "config": {"recall_k": RECALL_K, "temperature": 0.0,
                   "answer_model": "deepseek-flash",
                   "judge_model": "deepseek-chat",  # 别名→V4.1-Flash 无推理
                   "reasoning": False,
                   "answer_max_tokens": ANSWER_MAX_TOKENS,
                   "judge_max_tokens": JUDGE_MAX_TOKENS,
                   "sample_frac": 1.0, "sample_scope": "full",
                   "judge": "single deepseek-chat 别名→V4.1-Flash 无推理 (official: 3-judge majority via "
                            "OpenRouter: gpt-4.1 / claude-haiku-4.5 / gemini-2.5-flash)",
                   "answer_model_note": "official Track 2: gpt-4o-mini, "
                                         "temperature 0.7; ours deepseek-flash, temp 0"},
        "governance_audit": gov_audit,
        "ingest": ingest,
        "results": summary,
        "llm": llm,
        "conclusions": conclusions,
    }
    REPORT_JSON.parent.mkdir(parents=True, exist_ok=True)
    REPORT_JSON.write_text(json.dumps(report, ensure_ascii=False, indent=1),
                           encoding="utf-8")
    print(f"[report] wrote {REPORT_JSON}")


def main() -> None:
    ap = argparse.ArgumentParser(description="P21 Memora scx benchmark adapter")
    sub = ap.add_subparsers(dest="cmd", required=True)
    for name, fn in (("ingest", cmd_ingest), ("eval", cmd_eval), ("report", cmd_report)):
        p = sub.add_parser(name)
        p.add_argument("--periods", nargs="+", choices=list(PERIODS))
        p.add_argument("--personas", nargs="+")
        p.add_argument("--branches", nargs="+", choices=list(BRANCHES))
        if name == "eval":
            p.add_argument("--sample-frac", type=float, default=1.0)
            p.add_argument("--seed", type=int, default=SEED)
            p.add_argument("--recall-k", type=int, default=RECALL_K)
            p.add_argument("--model", default="deepseek-flash")
            p.add_argument("--judge-model", default="deepseek-chat")
            p.add_argument("--reasoning", action="store_true")
            p.add_argument("--resume-overwrite", action="store_true")
        p.set_defaults(func=fn)
    p_run = sub.add_parser("run")
    p_run.set_defaults(func=lambda a: (cmd_ingest(a), cmd_eval(a), cmd_report(a)))
    args = ap.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
