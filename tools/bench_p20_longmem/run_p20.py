#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""P20 LongMemEval · scx memory 接入主评测管线。

臂定义：
  baseline  : 全量会话按时间序摄取为 scx cases → --recall-only top-K → 官方 con prompt
              交 reader 作答 → 官方 judge 模板判分。全部 500 问。
  versioned : 仅 knowledge-update 78 问；新证据会话摄取时 --revise 旧证据 case（P3
              版本化），旧 case 标记 revised 并被召回过滤；其余流程同 baseline。
  oracle    : 跳过 scx 检索，直接以官方 answer_session_ids 证据会话构造 reader 上下文
              （官方 oracle 检索上界参考臂，非 scx 成绩）。

用法（复现命令见 README.md）：
  /tmp/scx-eval/bin/python tools/bench_p20_longmem/run_p20.py \
      --dataset S --arm baseline --workers 8 --out-tag s_baseline
  /tmp/scx-eval/bin/python tools/bench_p20_longmem/run_p20.py \
      --dataset S --arm versioned --workers 8 --out-tag s_versioned
"""
from __future__ import annotations

import argparse
import json
import re
import shutil
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import common  # noqa: E402
import llm  # noqa: E402

# 弃权表达（仅作诊断统计，不参与官方判分；官方判分由 judge prompt 决定）。
_ABSTAIN_PAT = re.compile(
    r"(i (don't|do not) (have|know|remember)|not (mentioned|provided|stated|informed)|"
    r"no (information|record|memory)|unable to (find|recall|determine|answer)|"
    r"cannot (find|recall|determine|answer)|doesn't mention|did not mention|"
    r"you did not mention|information is (incomplete|unavailable)|not enough information|"
    r"no relevant information|I have no record|not recorded)",
    re.I,
)


def session_store_text(sid: str, date: str, turns: list[dict]) -> str:
    lines = [f"### Session: {sid}", f"Session Date: {date}", "Session Content:"]
    for t in turns:
        lines.append(f"{t['role']}: {t['content']}")
    return "\n".join(lines)


def user_query(turns: list[dict]) -> str:
    """store query：与官方 session 粒度 BM25 语料同口径（仅 user 发言，
    见 run_retrieval.py process_item_flat_index L206）；检索时的查询是问题本身。"""
    users = " ".join(t["content"] for t in turns if t["role"] == "user")
    return users[:1800]


def ingest_question(entry: dict, arm: str, mem_dir: str) -> dict:
    """摄取该问全部会话；返回 sid → case_id / date / turns 映射与统计。"""
    sids = entry["haystack_session_ids"]
    dates = entry["haystack_dates"]
    sessions = entry["haystack_sessions"]
    sid2case: dict[str, str] = {}
    case2sid: dict[str, str] = {}
    ingest_stats = {"stored": 0, "dedup": 0, "chars_in": 0, "chars_out": 0,
                     "wall_ms": 0.0, "errors": []}

    # versioned 臂：证据会话按日期排序，新证据 revise 旧证据。
    evidence_sids = list(entry["answer_session_ids"])
    evidence_by_date = sorted(
        [(d, s) for s, d in zip(sids, dates) if s in evidence_sids],
        key=lambda x: x[0],
    )
    revise_map: dict[str, str] = {}  # newer sid -> older case_id
    if arm == "versioned" and entry["question_type"] == "knowledge-update":
        for (_d_old, s_old), (_d_new, s_new) in zip(evidence_by_date, evidence_by_date[1:]):
            revise_map[s_new] = s_old  # 占位，case_id 在旧会话摄取后回填

    old_evidence_case: str | None = None
    for sid, date, turns in zip(sids, dates, sessions):
        text = session_store_text(sid, date, turns)
        fix = f"Session Date: {date} | official_session_id: {sid}"
        revise_case = None
        if arm == "versioned" and sid in revise_map:
            old_sid = revise_map[sid]
            old_evidence_case = sid2case.get(old_sid)
            if old_evidence_case:
                revise_case = old_evidence_case
        try:
            res = common.scx_store(
                mem_dir=mem_dir,
                query=user_query(turns),
                text=text, fix=fix, revise=revise_case,
            )
        except Exception as e:  # noqa: BLE001
            ingest_stats["errors"].append({"sid": sid, "error": repr(e)[:300]})
            continue
        sid2case[sid] = res["case_id"]
        case2sid[res["case_id"]] = sid
        ingest_stats["stored" if res["added"] else "dedup"] += 1
        ingest_stats["wall_ms"] += res["wall_ms"]
        st = res["stats"]
        ingest_stats["chars_in"] += st.get("chars_in", 0)
        ingest_stats["chars_out"] += st.get("chars_out", 0)

    return {"sid2case": sid2case, "case2sid": case2sid, "stats": ingest_stats,
            "evidence_by_date": evidence_by_date}


def process_question(entry: dict, arm: str, recall_k: int, mem_root: Path) -> dict:
    qid = entry["question_id"]
    mem_dir = str(mem_root / arm / qid)
    if Path(mem_dir).exists():
        shutil.rmtree(mem_dir, ignore_errors=True)
    Path(mem_dir).mkdir(parents=True, exist_ok=True)

    record: dict = {
        "question_id": qid,
        "question_type": entry["question_type"],
        "abstention": common.is_abstention(qid),
        "ability": common.ability_of(entry["question_type"], qid),
        "question": entry["question"],
        "answer": entry["answer"],
        "question_date": entry["question_date"],
        "arm": arm,
        "recall_k": recall_k,
    }

    if arm == "oracle":
        # 官方 oracle：仅证据会话进上下文，不走 scx 库。
        recalled = []
        for sid, date, turns in zip(entry["haystack_session_ids"],
                                    entry["haystack_dates"],
                                    entry["haystack_sessions"]):
            if sid in entry["answer_session_ids"]:
                recalled.append((sid, date, turns))
        record["retrieval"] = {"source": "oracle", "n_sessions": len(recalled)}
    else:
        ing = ingest_question(entry, arm, mem_dir)
        record["ingest"] = ing["stats"]
        record["evidence_by_date"] = [s for _, s in ing["evidence_by_date"]]

        hits = common.scx_recall(mem_dir, entry["question"], recall_k)
        record["recall"] = [
            {"rank": i + 1, "case": h["full_id"], "sim": h["sim"], "score": h["score"]}
            for i, h in enumerate(hits)
        ]
        recalled = []
        for h in hits:
            sid = ing["case2sid"].get(h["full_id"])
            if sid is None:
                continue
            idx = entry["haystack_session_ids"].index(sid)
            recalled.append((sid, entry["haystack_dates"][idx],
                             entry["haystack_sessions"][idx]))

        # 检索诊断（session 粒度；弃权题官方跳过，见 run_retrieval.py L396）。
        if not record["abstention"]:
            corpus_ids = entry["haystack_session_ids"]
            rankings = [corpus_ids.index(sid) for sid, _, _ in recalled]
            correct = list(entry["answer_session_ids"])
            diag: dict = {}
            for k in (1, 3, 5, 10, 30):
                ra, _, _ = common.evaluate_retrieval(rankings, correct, corpus_ids, k)
                diag[f"recall_any@{k}"] = ra
            _, ra_all, ndcg10 = common.evaluate_retrieval(rankings, correct,
                                                          corpus_ids, 10)
            diag["recall_all@10"] = ra_all
            diag["ndcg@10"] = ndcg10
            diag["n_recalled"] = len(recalled)
            record["retrieval"] = diag

        # knowledge-update 专项：旧/新证据是否进入召回（baseline 臂统计陈旧泄漏）。
        if entry["question_type"] == "knowledge-update" and not record["abstention"]:
            ev = ing["evidence_by_date"]
            old_sid, new_sid = (ev[0][1], ev[-1][1]) if len(ev) >= 2 else (None, None)
            recalled_sids = [sid for sid, _, _ in recalled]
            record["ku_special"] = {
                "evidence_sids": [s for _, s in ev],
                "old_evidence_sid": old_sid,
                "new_evidence_sid": new_sid,
                "old_in_topk": old_sid in recalled_sids if old_sid else None,
                "new_in_topk": new_sid in recalled_sids if new_sid else None,
                "old_rank": (recalled_sids.index(old_sid) + 1 if old_sid in recalled_sids
                             else None) if old_sid else None,
                "new_rank": (recalled_sids.index(new_sid) + 1 if new_sid in recalled_sids
                             else None) if new_sid else None,
            }

    history = common.render_history(recalled)
    record["history_chars"] = len(history)
    prompt = common.reader_prompt(history, entry["question_date"], entry["question"])
    try:
        answer, usage, lat, retries = llm.reader_call(
            prompt, qid, arm, common.READER_MAX_TOKENS)
        record["hypothesis"] = answer
        record["reader"] = {"usage": usage, "latency_ms": lat, "retries": retries,
                             "history_chars": len(history)}
    except Exception as e:  # noqa: BLE001
        record["hypothesis"] = ""
        record["reader"] = {"error": repr(e)[:500]}

    # 弃权表达诊断（不影响官方判分）。
    record["abstention_phrase"] = bool(_ABSTAIN_PAT.search(record["hypothesis"]))

    # 官方 judge 判分。
    jprompt = common.judge_prompt(
        entry["question_type"], entry["question"], entry["answer"],
        record["hypothesis"], record["abstention"])
    try:
        jresp, jusage, jlat, jretries = llm.judge_call(jprompt, qid, arm,
                                                       common.JUDGE_MAX_TOKENS)
        record["judge"] = {"response": jresp, "label": "yes" in jresp.lower(),
                           "usage": jusage, "latency_ms": jlat, "retries": jretries}
    except Exception as e:  # noqa: BLE001
        record["judge"] = {"error": repr(e)[:500], "label": False}

    # 清理 per-question 库（审计需要时可改 keep）。
    if arm != "oracle":
        shutil.rmtree(mem_dir, ignore_errors=True)
    return record


def repair_judges(path: Path, workers: int) -> None:
    """复用已落盘 hypothesis，仅对 judge 失败/缺失的记录重跑官方判分，原地回写。"""
    records = [json.loads(l) for l in path.read_text(encoding="utf-8").splitlines() if l.strip()]
    targets = [r for r in records if r.get("judge", {}).get("error")
               or "label" not in r.get("judge", {})]
    print(f"[p20-rejudge] {path.name}: total={len(records)} need_rejudge={len(targets)}",
          flush=True)

    def fix(r: dict) -> dict:
        jprompt = common.judge_prompt(
            r["question_type"], r["question"], r["answer"],
            r["hypothesis"], r["abstention"])
        try:
            jresp, jusage, jlat, jretries = llm.judge_call(
                jprompt, r["question_id"], r["arm"], common.JUDGE_MAX_TOKENS)
            r["judge"] = {"response": jresp, "label": "yes" in jresp.lower(),
                          "usage": jusage, "latency_ms": jlat, "retries": jretries}
        except Exception as e:  # noqa: BLE001
            r["judge"] = {"error": repr(e)[:500], "label": False}
        return r

    by_id = {r["question_id"]: r for r in records}
    with ThreadPoolExecutor(max_workers=workers) as pool:
        for out in pool.map(fix, targets):
            by_id[out["question_id"]] = out
    fixed = [by_id[r["question_id"]] for r in records]
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text("\n".join(json.dumps(r, ensure_ascii=False) for r in fixed) + "\n",
                   encoding="utf-8")
    tmp.replace(path)
    still_bad = sum(1 for r in fixed if r["judge"].get("error"))
    print(f"[p20-rejudge] done; still_failed={still_bad}", flush=True)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--dataset", default="S", choices=list(common.DATA_FILES))
    ap.add_argument("--arm", default="baseline",
                   choices=["baseline", "versioned", "oracle"])
    ap.add_argument("--workers", type=int, default=common.LLM_WORKERS)
    ap.add_argument("--recall-k", type=int, default=common.RECALL_K)
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--qtype", default="", help="仅跑指定题型（调试用）")
    ap.add_argument("--abstention-only", action="store_true",
                    help="仅跑 _abs 弃权题（调试用）")
    ap.add_argument("--out-tag", default="")
    ap.add_argument("--out-dir", default=str(common.WORK_DIR / "runs"))
    ap.add_argument("--repair-judge", default="",
                    help="仅重判指定 jsonl 中 judge 失败的记录（复用已存 hypothesis）")
    args = ap.parse_args()

    if args.repair_judge:
        repair_judges(Path(args.repair_judge), args.workers)
        return

    data = common.load_dataset(args.dataset)
    if args.arm == "versioned":
        data = [e for e in data if e["question_type"] == "knowledge-update"]
    if args.qtype:
        data = [e for e in data if e["question_type"] == args.qtype]
    if args.abstention_only:
        data = [e for e in data if common.is_abstention(e["question_id"])]
    if args.limit:
        data = data[:args.limit]

    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    out_jsonl = out_dir / f"p20_{args.dataset}_{args.arm}_{args.out_tag}.jsonl"
    done_qids: set[str] = set()
    if out_jsonl.exists():  # 断点续跑
        for line in out_jsonl.read_text(encoding="utf-8").splitlines():
            if line.strip():
                done_qids.add(json.loads(line)["question_id"])
    todo = [e for e in data if e["question_id"] not in done_qids]
    print(f"[p20] dataset={args.dataset} arm={args.arm} total={len(data)} "
          f"done={len(done_qids)} todo={len(todo)} workers={args.workers}", flush=True)

    mem_root = common.MEM_ROOT / f"{args.dataset}_{args.arm}_{args.out_tag}"
    t0 = time.monotonic()
    n_done = 0
    with out_jsonl.open("a", encoding="utf-8") as fout, \
            ThreadPoolExecutor(max_workers=args.workers) as pool:
        futures = {pool.submit(process_question, e, args.arm, args.recall_k,
                               mem_root): e for e in todo}
        for fut in as_completed(futures):
            e = futures[fut]
            try:
                rec = fut.result()
            except Exception as ex:  # noqa: BLE001
                rec = {"question_id": e["question_id"], "question_type": e["question_type"],
                       "arm": args.arm, "fatal_error": repr(ex)[:500]}
            fout.write(json.dumps(rec, ensure_ascii=False) + "\n")
            fout.flush()
            n_done += 1
            if n_done % 10 == 0 or n_done == len(todo):
                el = time.monotonic() - t0
                print(f"[p20] {n_done}/{len(todo)} elapsed={el:.0f}s", flush=True)

    print(f"[p20] wrote {out_jsonl}", flush=True)


if __name__ == "__main__":
    main()
