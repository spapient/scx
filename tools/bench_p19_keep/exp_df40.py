#!/usr/bin/env python3
"""science df40 恢复实验：压缩 + 判分（重启后 /tmp 丢失，重建链路）。

产物（持久于仓库，避免 /tmp 被清）：
  tools/bench_p19_keep/exp_df40/{stem}.jsonl            压缩产物（含 compressed_context）
  tools/bench_p19_keep/exp_df40/verdicts_{stem}.jsonl   判分落盘 {case, position, judgement, raw}

判分前提（勿再踩）：
  - env PYTHONPATH=/tmp/scx-eval/lib/python3.12/site-packages（httpx 等）
  - 本脚本顶部 stub openai/tqdm（official judge 模块只会 import，从不真正调用）
  - llm_calls.jsonl 只用于成本审计，不得重建 verdicts（response 是字符串、行序≠position）
"""
import sys
import json
import types
from pathlib import Path
from concurrent.futures import ThreadPoolExecutor, as_completed

# --- stub openai / tqdm（official judge 模块 import 依赖；空实现，不真正调用）---
class _Dummy:
    def __init__(self, *a, **k):
        pass
    def __getattr__(self, n):
        return _Dummy
    def __call__(self, *a, **k):
        return _Dummy()
    def __enter__(self):
        return self
    def __exit__(self, *a):
        return False
    def update(self, *a, **k):
        pass

_oa = types.ModuleType("openai")
_oa.AsyncOpenAI = _Dummy
_oa.OpenAI = _Dummy
sys.modules["openai"] = _oa
_tq = types.ModuleType("tqdm")
_tq.tqdm = _Dummy
sys.modules["tqdm"] = _tq

KEEP_DIR = Path("/Users/fei/code/scx/tools/bench_p19_keep")
sys.path.insert(0, str(KEEP_DIR))
import keepbench as kb  # noqa: E402

WORK = KEEP_DIR / "exp_df40"
WORK.mkdir(parents=True, exist_ok=True)
STEMS = ["science_domain_three_perspectives", "science_domain_four_perspectives"]
official = kb.load_official_judge()

# ---- 阶段 1：压缩（df40 只作用于 science 组；seed 42 全量）----
for stem in STEMS:
    spec = next(s for s in kb.SUBSETS if s["stem"] == stem)
    rows = kb.load_subset(Path(spec["data"]), None, kb.SEED)
    out = WORK / f"{stem}.jsonl"
    if out.exists():
        prev = kb.read_jsonl(out)
        if len(prev) == len(rows) and "compressed_context" in prev[0]:
            print(f"  reuse compressed {stem}: {len(rows)}", flush=True)
            continue
    done = [None] * len(rows)  # type: ignore[list-item]

    def compress_one(idx: int, row: dict) -> tuple[int, dict]:
        docs = row["context"].split("\n")
        qc = kb.build_query(row["question"], docs, kb.SCIENCE_DF_OVERRIDE)
        comp, _m = kb.scx_compress(kb.DEFAULT_SCX_BIN, row["context"], qc, [])
        o = dict(row)
        o["compressed_context"] = comp
        return idx, o

    with ThreadPoolExecutor(max_workers=12) as ex:
        futs = [ex.submit(compress_one, i, r) for i, r in enumerate(rows)]
        for f in as_completed(futs):
            i, o = f.result()
            done[i] = o
    kb.write_jsonl(out, done)
    print(f"compressed {stem}: {len(done)}", flush=True)

# ---- 阶段 2：判分（官方协议；verdicts 逐子集落盘，可续跑）----
for stem in STEMS:
    rows = kb.read_jsonl(WORK / f"{stem}.jsonl")
    items: list[tuple[str, int, dict, dict | None]] = []
    for row in rows:
        ent, idx, extra = kb.expand_imbalanced(row, row["compressed_context"])[""]
        for e, ix in zip(ent, idx):
            items.append((ix["id"], ix["position"], e, extra))
    out_path = WORK / f"verdicts_{stem}.jsonl"
    results: list[dict | None] = [None] * len(items)
    if out_path.exists():
        prev = kb.read_jsonl(out_path)
        if len(prev) == len(items):
            bad = [i for i, r in enumerate(prev)
                   if not (isinstance(r.get("raw"), dict)
                           and r["raw"].get("judgement") in ("yes", "no")
                           and "error" not in r["raw"])]
            if not bad:
                print(f"  reuse judge {stem}: {len(prev)} rows", flush=True)
                continue
            print(f"  re-judge {stem}: {len(bad)}/{len(prev)} invalid", flush=True)
            results = list(prev)

            def judge_one(pos: int, entry: dict, extra: dict | None, label: str):
                judge = official.judge_for(stem)
                sc = official.shortcut(judge, entry)
                if sc is not None:
                    return pos, {"judgement": sc.get("judgement"),
                                 "raw": {"note": sc.get("note", "shortcut")}}
                return pos, kb.judge_call(official, judge, entry, extra, label)

            with ThreadPoolExecutor(max_workers=kb.JUDGE_WORKERS) as ex:
                futs = [ex.submit(judge_one, pos, items[pos][2],
                                  items[pos][3], f"{stem}:{pos + 1}") for pos in bad]
                for f in as_completed(futs):
                    pos, v = f.result()
                    case, position = items[pos][0], items[pos][1]
                    results[pos] = {"case": case, "position": position,
                                    "judgement": v.get("judgement") if isinstance(v, dict) else None,
                                    "raw": v}
            kb.write_jsonl(out_path, results)
            ok = sum(1 for r in results
                     if isinstance(r.get("raw"), dict) and r["raw"].get("judgement") in ("yes", "no")
                     and "error" not in r["raw"])
            err = sum(1 for r in results if isinstance(r.get("raw"), dict) and "error" in r["raw"])
            print(f"judged {stem}: {len(results)} rows, ok={ok}, err={err}", flush=True)
            continue

    def judge_one(pos: int, entry: dict, extra: dict | None, label: str):
        judge = official.judge_for(stem)
        sc = official.shortcut(judge, entry)
        if sc is not None:
            return pos, {"judgement": sc.get("judgement"),
                         "raw": {"note": sc.get("note", "shortcut")}}
        return pos, kb.judge_call(official, judge, entry, extra, label)

    with ThreadPoolExecutor(max_workers=kb.JUDGE_WORKERS) as ex:
        futs = [ex.submit(judge_one, pos, items[pos][2],
                          items[pos][3], f"{stem}:{pos + 1}") for pos in range(len(items))]
        for f in as_completed(futs):
            pos, v = f.result()
            case, position = items[pos][0], items[pos][1]
            results[pos] = {"case": case, "position": position,
                            "judgement": v.get("judgement") if isinstance(v, dict) else None,
                            "raw": v}
    kb.write_jsonl(out_path, results)
    ok = sum(1 for r in results
             if isinstance(r.get("raw"), dict) and r["raw"].get("judgement") in ("yes", "no")
             and "error" not in r["raw"])
    err = sum(1 for r in results if isinstance(r.get("raw"), dict) and "error" in r["raw"])
    print(f"judged {stem}: {len(results)} rows, ok={ok}, err={err}", flush=True)
