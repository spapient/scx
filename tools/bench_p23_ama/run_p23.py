#!/usr/bin/env python3
"""P23 · AMA-Bench (arXiv 2602.22769) scx 记忆体系冻结基线编排器。

协议对齐（只读引用官方 repo /tmp/scx_bench_ama/repo，commit ddfd319e）：
- offline per-query：episode 轨迹摄取一次；每个 QA 独立对固定记忆作答，QA 之间不更新记忆。
- 两通道同语料同 K=10：`scx`（scx memory turn 级 case，RRF 融合召回）；
  `bm25`（官方 BM25 基线零依赖复刻，同 turn 文档粒度）。
- 答案 prompt / judge prompt / 答案抽取 / F1 口径逐字对齐官方（见 p23_provider.py 注释）。

用法见 README.md；全量优先，`--sample-per-type N` 固定种子分层抽样（标注抽样口径）。
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import random
import statistics
import subprocess
import sys
import time
from collections import Counter, defaultdict
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

from p23_provider import DeepSeekProvider, compute_f1_score  # noqa: E402
from p23_scx import SCXMemory  # noqa: E402
from p23_bm25 import BM25Episode  # noqa: E402

# ---------------------------------------------------------------------------
# 官方资源元数据（冻结时写入结果 JSON；来源见 README.md 资源清单）
# ---------------------------------------------------------------------------
OFFICIAL = {
    "paper": "arXiv:2602.22769v4 (2026-05-27) AMA-Bench: Evaluating Long-Horizon "
             "Memory for Agentic Applications",
    "website": "https://ama-bench.github.io/",
    "hf_dataset": "AMA-bench/AMA-bench",
    "hf_file": "test/open_end_qa_set.jsonl",
    "hf_file_sha256": "45c36052e1520d87ad9de4114f71c9df42d4aac9cf158c0c353e800b653d65ff",
    "github_repo": "https://github.com/AMA-Bench/AMA-Hub",
    "github_commit": "ddfd319e0be33424288c13806f1eafc63e625b59",
    "leaderboard_space": "https://huggingface.co/spaces/AMA-bench/AMA-bench-Leaderboard",
    "official_answer_model": "Qwen3-32B (paper §6.1 / Table 5 backbone)",
    "official_judge_model": "Qwen3-32B binary yes/no (paper Appendix C.1)",
}

DEFAULT_DATA = "/tmp/scx_bench_ama/open_end_qa_set.jsonl"
DEFAULT_WORK = "/tmp/scx_bench_ama"
DEFAULT_RESULTS = "/Users/fei/code/scx/config/bench/results/bench_p23_amabench.json"
SCX_BINARY = "/Users/fei/code/scx/target/release/scx"
K = 10
SEED = 42

# 官方论文参考基线（Table 5，backbone=Qwen3-32B，judge=Qwen3-32B；仅作对照，
# 与本运行不同 backbone/judge，不可直接比较绝对值）。列序 = QA 类型 A/B/C/D。
# A=Recall, B=Causal Inference, C=State Updating, D=State Abstraction。
OFFICIAL_REFERENCE = {
    "source": "AMA-Bench arXiv:2602.22769v4 Table 5 (real-world subset, Qwen3-32B backbone)",
    "note": "本运行答案/judge 均经 deepseek-chat 别名调用（API 实测路由到 deepseek-flash"
            "=官方 DeepSeek-V4.1-Flash，无推理模式，temp 0）；backbone、judge、上下文构造均不同，"
            "下列数字只用于定位量级，不构成同条件对比。",
    "columns": {"A": "Recall", "B": "Causal Inference",
                "C": "State Updating", "D": "State Abstraction"},
    "table5_accuracy_f1": {
        "RAG/BM25": {"A": [0.3301, 0.1465], "B": [0.4264, 0.1549],
                     "C": [0.3450, 0.1325], "D": [0.2498, 0.1623],
                     "Average": [0.3436, 0.1475]},
        "RAG/Qwen3-Emb-4B": {"Average": [0.4227, 0.1522]},
        "RAG/HippoRAG2": {"Average": [0.4480, 0.2048]},
        "Memory/MemoRAG": {"Average": [0.4606, 0.1822]},
        "Memory/EMem": {"Average": [0.4610, 0.2095]},
        "AMA-Agent(official)": {"Average": [0.5722, 0.2992]},
    },
    "table4_longcontext_note": "Table 4 长上下文模型（Qwen3-32B/8B 等 backbone 直塞轨迹）"
                               "Average accuracy 区间 0.4361–0.7226，backbone 差异主导绝对值。",
}

JUDGE_PROTOCOL = {
    "source": "AMA-Hub utils/evaluation_metrics.py L153-169; L143-151",
    "decision": "judge 收到 (task_type, episode_id, task, question, reference, predicted)，"
                "仅输出 yes/no；取末个 yes/no；无法解析记 incorrect。",
    "fallback": "judge 不可用/空响应 → 记 incorrect（repair 轮重试后仍失败列入 residual_failures）。",
    "official_judge": "Qwen3-32B（paper Appendix C.1；300 例人工标注一致率 92.67%）",
    "this_run_judge": "deepseek-chat 别名→DeepSeek-V4.1-Flash 无推理，temperature 0，max_tokens=16；口径差异已在 models 节记录。",
    "f1_metric": "官方第二指标 F1：离线机械计算（分类别 token P/R 调和平均），不经 LLM；"
                 "移植自 evaluation_metrics.py compute_f1_score。",
}

SYNTHETIC_STATUS = {
    "target": "Synthetic subset：BabyAI + TextWorld，1,200 QA，轨迹长度 8K/16K/32K/64K/128K",
    "status": "NOT_OBTAINABLE_PUBLICLY —— 本运行未使用、未自造任何 synthetic 数据",
    "attempted_paths": [
        "HF dataset AMA-bench/AMA-bench 递归文件树：仅 test/open_end_qa_set.jsonl（real-world），"
        "无 synthetic / mcq 文件；refs 无其他分支。",
        "GitHub AMA-Bench/AMA-Hub commit ddfd319e：仅含 BabyAI/TextWorld/Crafter 生成代码，"
        "产物目录 .gitignore 排除，仓库未提交预生成 synthetic jsonl。",
        "Leaderboard space AMA-bench/AMA-bench-Leaderboard：submission.py 地面真值仅加载 "
        "open_end_qa_set.jsonl；无 synthetic 标注。",
        "Google Drive 1Fn-gTiOJoSAXgXdnHa3F0-lRfYVsrQyJ（ama_agent_log）：仅 AMA-Agent 自身 "
        "answer.jsonl + evaluation_result.json（模型输出，非基准语料）。",
        "HF mirror Pettingllms/AMA-bench：307 重定向/不可用；内部数据集 "
        "AMA-bench/AMA_submissions_internal 返回 401（提交记录，非地面真值）。",
        "第三方 S-Agents/AMA-Bench 引用仓库：GitHub 404，不存在。",
    ],
    "why_not_reproduced": "复现 synthetic 需要在本地跑 BabyAI/TextWorld 环境并使用官方未公开的"
                          "种子、难度配置与 QA 生成参数；依赖（babyai/textworld/crafter）未安装，"
                          "参数不可得。按任务约束：禁止自造数据冒充官方，故该子集标记为受阻。",
    "mcq_note": "论文 §6.1：所有 QA 均为 open-ended generation；run.py 内 mcq_set.jsonl 为"
                "遗留代码路径，公开数据中不存在 MCQ 子集。",
}


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def scx_version() -> str:
    p = subprocess.run([SCX_BINARY, "--version"], capture_output=True, text=True, timeout=30)
    return p.stdout.strip()


def load_episodes(path: Path) -> list[dict[str, Any]]:
    episodes = []
    with path.open(encoding="utf-8") as f:
        for ln in f:
            ln = ln.strip()
            if ln:
                episodes.append(json.loads(ln))
    return episodes


def select_sample(episodes: list[dict[str, Any]], per_type: int, seed: int) -> set[int]:
    """按 QA 类型分层、类型内按 domain 轮询平衡地选 episode；返回 episode_id 集合。

    episode 主类型 = 其 QA 的多数类型；选中 episode 的全部 QA 进入评测
    （与官方 per-episode 记忆协议一致）。固定种子，可复现。
    """
    rng = random.Random(seed)
    by_type: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for e in episodes:
        main_type = Counter(q["type"] for q in e["qa_pairs"]).most_common(1)[0][0]
        by_type[main_type].append(e)
    selected: set[int] = set()
    for main_type, eps in by_type.items():
        by_domain: dict[str, list[dict[str, Any]]] = defaultdict(list)
        for e in eps:
            by_domain[e["domain"]].append(e)
        for lst in by_domain.values():
            rng.shuffle(lst)
        queues = list(by_domain.values())
        qi = 0
        picked = 0
        while picked < per_type and qi < len(eps) * 4:
            bucket = queues[qi % len(queues)]
            if qi < len(bucket):
                eid = bucket[qi]["episode_id"]
                if eid not in selected:
                    selected.add(eid)
                    picked += 1
            qi += 1
    return selected


def aggregate(rows: list[dict[str, Any]]) -> dict[str, Any]:
    """按 channel / domain / qa_type 聚合 accuracy（judge yes）与离线 F1。"""
    out: dict[str, Any] = {}
    groups: dict[tuple[str, str, str], list[dict[str, Any]]] = defaultdict(list)
    for r in rows:
        groups[(r["channel"], "ALL", "ALL")].append(r)
        groups[(r["channel"], r["domain"], "ALL")].append(r)
        groups[(r["channel"], "ALL", r["qa_type"])].append(r)
    for (channel, domain, qtype), items in sorted(groups.items()):
        n = len(items)
        correct = sum(1 for x in items if x["judge_decision"])
        f1 = [x["f1"] for x in items]
        acc = correct / n if n else 0.0
        f1_mean = statistics.fmean(f1) if f1 else 0.0
        out.setdefault(channel, {}).setdefault(domain, {})[qtype] = {
            "n": n, "accuracy": round(acc, 4), "f1": round(f1_mean, 4),
            "correct": correct,
        }
    return out


def retrieval_diagnostics(rows: list[dict[str, Any]]) -> dict[str, Any]:
    diag: dict[str, Any] = {}
    for channel in sorted({r["channel"] for r in rows}):
        cr = [r for r in rows if r["channel"] == channel]
        zero = sum(1 for r in cr if r["n_hits"] == 0)
        diag[channel] = {
            "n": len(cr),
            "zero_hit_rate": round(zero / len(cr), 4) if cr else 0.0,
            "mean_hits": round(statistics.fmean(r["n_hits"] for r in cr), 3) if cr else 0.0,
            "mean_context_chars": round(statistics.fmean(r["context_chars"] for r in cr), 1) if cr else 0.0,
            "mean_score": round(statistics.fmean(r["top_score"] for r in cr if r["top_score"] is not None), 4)
            if any(r["top_score"] is not None for r in cr) else None,
            "mean_sim": round(statistics.fmean(r["top_sim"] for r in cr if r["top_sim"] is not None), 4)
            if any(r["top_sim"] is not None for r in cr) else None,
        }
    return diag


def main() -> None:
    ap = argparse.ArgumentParser(description="P23 AMA-Bench scx frozen baseline")
    ap.add_argument("--data", default=DEFAULT_DATA)
    ap.add_argument("--work", default=DEFAULT_WORK)
    ap.add_argument("--results", default=DEFAULT_RESULTS)
    ap.add_argument("--channels", default="scx,bm25")
    ap.add_argument("--k", type=int, default=K)
    ap.add_argument("--seed", type=int, default=SEED)
    ap.add_argument("--sample-per-type", type=int, default=0,
                    help=">0 时按 QA 类型分层抽 N 个 episode（固定种子）；0=全量")
    ap.add_argument("--answer-workers", type=int, default=8)
    ap.add_argument("--ingest-workers", type=int, default=4)
    ap.add_argument("--answer-model", default=DeepSeekProvider.ANSWER_MODEL)
    ap.add_argument("--judge-model", default=DeepSeekProvider.JUDGE_MODEL)
    ap.add_argument("--limit-episodes", type=int, default=0)
    ap.add_argument("--pilot", action="store_true",
                    help="仅跑前 2 个 episode，用于时延/成本估算")
    args = ap.parse_args()

    work = Path(args.work)
    work.mkdir(parents=True, exist_ok=True)
    data = Path(args.data)
    if not data.exists():
        sys.exit(f"missing official data: {data}")
    actual_sha = sha256_file(data)
    if actual_sha != OFFICIAL["hf_file_sha256"]:
        print(f"[warn] data sha256 mismatch: {actual_sha}", file=sys.stderr)

    episodes = load_episodes(data)
    if args.limit_episodes:
        episodes = episodes[: args.limit_episodes]
    if args.pilot:
        episodes = episodes[:2]

    scope = "full"
    selected_ids: set[int] | None = None
    if args.sample_per_type > 0:
        selected_ids = select_sample(episodes, args.sample_per_type, args.seed)
        episodes = [e for e in episodes if e["episode_id"] in selected_ids]
        scope = f"sampled_per_type={args.sample_per_type}_seed={args.seed}"
    episodes_by_id = {e["episode_id"]: e for e in episodes}

    print(f"[p23] episodes={len(episodes)} scope={scope} channels={args.channels} k={args.k}")

    provider = DeepSeekProvider(work / "llm_calls.jsonl",
                                answer_model=args.answer_model,
                                judge_model=args.judge_model)
    scx = SCXMemory(mem_root=work / "mem")
    bm25_indexes: dict[int, BM25Episode] = {}

    # ---- Phase 1: 摄取（scx 落库；bm25 内存索引）----
    ingest_audit: list[dict[str, Any]] = []
    t0 = time.time()

    def ingest_one(ep: dict[str, Any]) -> dict[str, Any]:
        audit = scx.ingest_episode(ep)
        audit["scx_cases"] = scx.episode_stats(ep["episode_id"]).get("cases")
        bm25_indexes[ep["episode_id"]] = BM25Episode(ep, k=args.k)
        return audit

    with ThreadPoolExecutor(max_workers=args.ingest_workers) as ex:
        futs = {ex.submit(ingest_one, ep): ep for ep in episodes}
        for fut in as_completed(futs):
            audit = fut.result()
            ingest_audit.append(audit)
    ingest_s = time.time() - t0
    (work / "ingest_audit.jsonl").write_text(
        "\n".join(json.dumps(a, ensure_ascii=False) for a in ingest_audit), encoding="utf-8")

    # ---- Phase 2: QA 评测（断点续跑 + 失败修复轮）----
    answers_path = work / "answers.jsonl"
    done: set[tuple[str, str]] = set()
    existing_rows: list[dict[str, Any]] = []
    if answers_path.exists():
        for ln in answers_path.read_text(encoding="utf-8").splitlines():
            if ln.strip():
                row = json.loads(ln)
                existing_rows.append(row)
                done.add((row["question_uuid"], row["channel"]))

    channels = [c.strip() for c in args.channels.split(",") if c.strip()]

    def recall_for(ep: dict[str, Any], qa: dict[str, Any], channel: str):
        if channel == "scx":
            hits = scx.recall(ep, qa["question"], k=args.k)
            context = SCXMemory.context_from_hits(hits)
        else:
            bm = bm25_indexes[ep["episode_id"]]
            hits = bm.search(qa["question"])
            context = BM25Episode.context_from_hits(hits)
        return hits, context

    def build_row(ep: dict[str, Any], qa: dict[str, Any], channel: str,
                  hits: list[dict[str, Any]], context: str,
                  pred: str, ans_call: dict[str, Any],
                  decision: bool, judge_raw: str,
                  judge_call: dict[str, Any]) -> dict[str, Any]:
        return {
            "question_uuid": qa["question_uuid"],
            "episode_id": ep["episode_id"],
            "domain": ep["domain"],
            "task_type": ep["task_type"],
            "qa_type": qa["type"],
            "channel": channel,
            "question": qa["question"],
            "gold": qa["answer"],
            "pred": pred,
            "judge_decision": decision,
            "judge_raw": judge_raw,
            "f1": round(compute_f1_score(qa["answer"], pred), 4),
            "n_hits": len(hits),
            "hit_turns": [h["turn_index"] for h in hits],
            "context_chars": len(context),
            "top_score": hits[0]["score"] if hits else None,
            "top_sim": hits[0]["sim"] if hits and hits[0].get("sim") is not None else None,
            "answer_call_id": ans_call["call_id"],
            "judge_call_id": judge_call["call_id"],
            "answer_ok": ans_call["ok"],
            "judge_ok": judge_call["ok"],
        }

    def run_task(ep: dict[str, Any], qa: dict[str, Any], channel: str) -> dict[str, Any]:
        hits, context = recall_for(ep, qa, channel)
        pred, ans_call = provider.answer(qa["question"], context, ep, channel,
                                          qa["question_uuid"])
        decision, judge_raw, judge_call = provider.judge(
            qa["question"], qa["answer"], pred, ep, channel, qa["question_uuid"])
        return build_row(ep, qa, channel, hits, context, pred, ans_call,
                         decision, judge_raw, judge_call)

    tasks: list[tuple[dict[str, Any], dict[str, Any], str]] = []
    for ep in episodes:
        for qa in ep["qa_pairs"]:
            for ch in channels:
                if (qa["question_uuid"], ch) not in done:
                    tasks.append((ep, qa, ch))
    print(f"[p23] pending QA-channel tasks={len(tasks)} (resume done={len(done)})")

    rows = list(existing_rows)
    answers_f = answers_path.open("a", encoding="utf-8")
    t1 = time.time()
    if tasks:
        with ThreadPoolExecutor(max_workers=args.answer_workers) as ex:
            futs = {ex.submit(run_task, ep, qa, ch): (ep, qa, ch)
                    for ep, qa, ch in tasks}
            for i, fut in enumerate(as_completed(futs), start=1):
                row = fut.result()
                rows.append(row)
                answers_f.write(json.dumps(row, ensure_ascii=False) + "\n")
                answers_f.flush()
                if i % 50 == 0 or i == len(tasks):
                    print(f"[p23] evaluated {i}/{len(tasks)}")
    answers_f.close()

    # ---- 修复轮：answer/judge 失败的行重试（网络 reset/超时类瞬时故障）----
    repaired = 0
    residual_failures: list[dict[str, Any]] = []
    by_key = {(r["question_uuid"], r["channel"]): r for r in rows}
    bad = [r for r in rows if not r["answer_ok"] or not r["judge_ok"]]
    print(f"[p23] repair pass: {len(bad)} failed rows to retry")
    for row in bad:
        ep = episodes_by_id.get(row["episode_id"])
        if ep is None:
            # 该行属本次未加载的 episode（如 pilot 只加载 2 个、全量后续补跑）：
            # 本轮跳过，待加载该 episode 的运行再处理，避免 KeyError。
            continue
        qa = next(q for q in ep["qa_pairs"] if q["question_uuid"] == row["question_uuid"])
        channel = row["channel"]
        hits, context = recall_for(ep, qa, channel)
        if not row["answer_ok"]:
            pred, ans_call = provider.answer(qa["question"], context, ep, channel,
                                             qa["question_uuid"])
        else:
            pred, ans_call = row["pred"], {"call_id": row["answer_call_id"], "ok": True}
        if not row["judge_ok"]:
            decision, judge_raw, judge_call = provider.judge(
                qa["question"], qa["answer"], pred, ep, channel, qa["question_uuid"])
        else:
            decision, judge_raw, judge_call = row["judge_decision"], row["judge_raw"], \
                {"call_id": row["judge_call_id"], "ok": True}
        new_row = build_row(ep, qa, channel, hits, context, pred, ans_call,
                             decision, judge_raw, judge_call)
        by_key[(row["question_uuid"], channel)] = new_row
        if new_row["answer_ok"] and new_row["judge_ok"]:
            repaired += 1
        else:
            residual_failures.append({
                "question_uuid": row["question_uuid"], "channel": channel,
                "episode_id": row["episode_id"],
                "answer_ok": new_row["answer_ok"], "judge_ok": new_row["judge_ok"],
            })
    rows = list(by_key.values())
    rows.sort(key=lambda r: (r["episode_id"], r["question_uuid"], r["channel"]))
    answers_path.write_text(
        "\n".join(json.dumps(r, ensure_ascii=False) for r in rows) + "\n",
        encoding="utf-8")
    eval_s = time.time() - t1
    print(f"[p23] repaired={repaired} residual_failures={len(residual_failures)}")

    # ---- Phase 3: 聚合 + 冻结结果 JSON ----
    metrics = aggregate(rows)
    diag = retrieval_diagnostics(rows)

    # 摄取审计聚合
    stored = sum(a["stored"] for a in ingest_audit)
    dedup = sum(a["dedup"] for a in ingest_audit)
    truncated = sum(a["truncated"] for a in ingest_audit)
    failed = sum(a["failed"] for a in ingest_audit)
    ingest_errors = [e for a in ingest_audit for e in a["errors"]]

    domain_counts = Counter(e["domain"] for e in episodes)
    type_counts: Counter[str] = Counter()
    for e in episodes:
        for q in e["qa_pairs"]:
            type_counts[q["type"]] += 1

    # 双面结论所需的标量（从聚合结果直接取，不手填）
    def overall(channel: str) -> dict[str, Any]:
        return metrics.get(channel, {}).get("ALL", {}).get("ALL", {})

    def type_acc(channel: str, qtype: str) -> float:
        return metrics.get(channel, {}).get("ALL", {}).get(qtype, {}).get("accuracy", 0.0)

    scx_all, bm_all = overall("scx"), overall("bm25")
    conclusion = {
        "scx_overall": scx_all,
        "bm25_overall": bm_all,
        "scx_minus_bm25_acc": round(scx_all.get("accuracy", 0) - bm_all.get("accuracy", 0), 4),
        "scx_minus_bm25_f1": round(scx_all.get("f1", 0) - bm_all.get("f1", 0), 4),
        "by_type_scx_vs_bm25": {
            t: {"scx": type_acc("scx", t), "bm25": type_acc("bm25", t)}
            for t in ("A", "B", "C", "D")
        },
    }

    result: dict[str, Any] = {
        "schema": "bench_p23_amabench_v1",
        "benchmark": "AMA-Bench (arXiv:2602.22769v4)",
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "scx_version": scx_version(),
        "scx_binary": SCX_BINARY,
        "official": OFFICIAL,
        "official_data_audit": {
            "file": str(data),
            "bytes": data.stat().st_size,
            "sha256": actual_sha,
            "sha256_matches_hf_lfs": actual_sha == OFFICIAL["hf_file_sha256"],
            "episodes": len(episodes),
            "qa_pairs": sum(len(e["qa_pairs"]) for e in episodes),
            "domain_counts": dict(domain_counts),
            "qa_type_counts": dict(type_counts),
        },
        "command": " ".join(sys.argv),
        "scope": scope,
        "seed": args.seed,
        "temperature": 0.0,
        "k": args.k,
        "channels": channels,
        "models": {
            "answer": args.answer_model,
            "judge": args.judge_model,
            "official_answer": OFFICIAL["official_answer_model"],
            "official_judge": OFFICIAL["official_judge_model"],
        },
        "timings_s": {"ingest": round(ingest_s, 2), "evaluate": round(eval_s, 2)},
        "llm_calls": {
            "path": str(work / "llm_calls.jsonl"),
            "totals": provider.totals,
        },
        "ingestion": {
            "turns_stored": stored, "turns_dedup": dedup,
            "turns_truncated_over_1MiB": truncated, "turns_failed": failed,
            "errors": ingest_errors[:50],
            "mem_root": str(work / "mem"),
        },
        "methodology": {
            "protocol": "offline per-query（官方 §6.1）：episode 轨迹摄取一次；每个 QA 对固定"
                        "记忆独立作答，QA 之间不更新记忆；两通道同一语料、同一 K=10。",
            "ingestion_mapping": {
                "granularity": "一个 trajectory turn = 一个 scx case（与官方 BM25 文档粒度一致）",
                "case_text": "官方 _trajectory_to_text 同款块：Step n / Action / Observation",
                "case_query": "domain + task_type + step n + action 头词",
                "case_fix": "确定性规则摘要（动作头 + 观察头 160 字符）；摄取不调用 LLM，可复现",
                "isolation": "每 episode 独立 mem-dir；空 action/observation 记 (null)/(empty)；"
                             "超过 scx MAX_CASE_TEXT=1MiB 的 turn 头尾各留 480KiB 并标记 truncated",
            },
            "channels": {
                "scx": "scx memory turn 级 case，生产 RRF 融合（BM25 秩 + 字段/实体秩 + MinHash 秩）",
                "bm25": "官方 BM25 基线零依赖复刻（rank_bm25 默认 k1=1.5, b=0.75，"
                        "lower().split() 分词，top_k=10），同 turn 文档语料",
            },
            "answer_prompt_source": "AMA-Hub src/memory_interface.py L168-175（逐字）",
            "answer_extract_source": "AMA-Hub utils/extract_final_answer.py（Answer[1]: 正则 + think 剥离）",
        },
        "judge_protocol": JUDGE_PROTOCOL,
        "official_reference": OFFICIAL_REFERENCE,
        "synthetic_subset": SYNTHETIC_STATUS,
        "applicability_boundary": {
            "corpus_shape": "AMA-Bench 是 agent 轨迹语料（Action/Observation 交替、非对话轮次、"
                            "含长工具输出与环境状态转储）；scx 原生面向事故/复盘证据型 case"
                            "（query + fix 结论 + 证据块），本适配器以 turn 为最小证据单元映射，"
                            "未把轨迹压缩成事故结论。",
            "matches": "turn 级事实（对象/位置/动作/状态）可由 scx 词面 + 字段 + MinHash/RRF 召回；"
                       "Recall(A) 类问题与词面检索对齐最好。",
            "mismatches": [
                "因果推断(B)/状态更新(C)/状态抽象(D) 依赖跨 turn 关系与时间序；scx turn 级 case"
                "不建跨 turn 因果图，关系信号弱于官方 AMA-Agent 的 causality graph。",
                "超长工具输出（openworld 单 turn 可达 1.1M 字符）受 1MiB case 上限截断，"
                "截断 turn 的细节不可恢复。",
                "real-world 子集未标注证据 turn 编号，无法机械计算 recall@K 命中率；"
                "本运行只报告答案级 accuracy/F1 与召回诊断（命中数/上下文长度/排序分）。",
                "scx fix 字段在本任务是规则摘要而非“已验证修复结论”，L1 直答语义不适用；"
                "本基线仅使用召回侧能力。",
            ],
        },
        "dual_sided_conclusion": conclusion,
        "limitations": [
            "答案模型与 judge 均经 deepseek-chat 别名调用（物理模型=DeepSeek-V4.1-Flash，无推理），非官方 Qwen3-32B；绝对值不可与 Table 4/5 直接比较。",
            "synthetic 1,200 QA 未取得（见 synthetic_subset），长轨迹 8K–128K 尺度压力测试未覆盖。",
            "未跑长上下文（full trajectory）对照通道：多数 episode 超出本地模型上下文预算，"
            "官方 long-context 基线仅作参考数字。",
        ],
        "cost_note": "仅记录 prompt/completion token 与调用次数，不折算金额；"
                     "reasoning_tokens 不产生（deepseek-chat 别名=无推理模式）。",
        "metrics": metrics,
        "retrieval_diagnostics": diag,
        "repair": {"repaired": repaired, "residual_failures": residual_failures},
        "rows": len(rows),
        "sidecar": {"answers_jsonl": str(answers_path),
                    "ingest_audit_jsonl": str(work / "ingest_audit.jsonl")},
    }

    Path(args.results).parent.mkdir(parents=True, exist_ok=True)
    Path(args.results).write_text(json.dumps(result, ensure_ascii=False, indent=2),
                                  encoding="utf-8")
    print(f"[p23] frozen -> {args.results} rows={len(rows)}")
    print(json.dumps(metrics, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
