"""本机 8bit judge 端到端探索（混合路线日常通道验证）
用法: /tmp/scx-eval/bin/python explore_mlx_8bit.py [n_cases]
纯本机判分（不依赖 deepseek），统计 case 级 all_rec，对比在线冻结口径。
"""
import json, sys, time, types
from pathlib import Path
from concurrent.futures import ThreadPoolExecutor, as_completed

sys.path.insert(0, "/Users/fei/code/scx/tools/bench_p19_keep")

# stub openai/tqdm（keepbench 顶部 import 依赖）
class _Dummy:
    def __init__(self, *a, **k): pass
    def __getattr__(self, n): return _Dummy
    def __call__(self, *a, **k): return _Dummy()
    def __enter__(self): return self
    def __exit__(self, *a): return False
_oa = types.ModuleType("openai"); _oa.OpenAI = _Dummy; _oa.AsyncOpenAI = _Dummy
sys.modules["openai"] = _oa
_tq = types.ModuleType("tqdm"); _tq.tqdm = _Dummy
sys.modules["tqdm"] = _tq

import keepbench as kb
import httpx

N = int(sys.argv[1]) if len(sys.argv) > 1 else 30
MLX_URL = "http://127.0.0.1:8123/v1/chat/completions"
MLX_MODEL = "/Users/fei/model/qwen2.5-3b-instruct-mlx-8bit"

official = kb.load_official_judge()
rows = kb.read_jsonl(Path("/Users/fei/code/scx/tools/bench_p19_keep/exp_df40/science_domain_four_perspectives.jsonl"))
rows = rows[:N]
judge = official.judge_for("science_domain_four_perspectives")

def judge_one(entry):
    msgs = judge.build({f: entry[f] for f in judge.fields}, None)
    body = {"model": MLX_MODEL, "messages": msgs, "max_tokens": 2048, "temperature": 0}
    r = httpx.post(MLX_URL, json=body, timeout=600)
    out = r.json()["choices"][0]["message"]["content"]
    # 与 keepbench.judge_call 同口径：try_parse_json 剥 <think> 前缀 + 最外层 {}，judge.parse 出 yes/no
    v = judge.parse(official.try_parse_json(out))  # {'judgement':'yes'|'no'} or None
    return v["judgement"] if v else None  # 字符串 yes/no/None

tasks = []
for row in rows:
    try:
        ent, _, _ = kb.expand_imbalanced(row, row["compressed_context"])[""]
    except Exception as e:
        print(f"skip {row['id']}: {e}"); continue
    for e in ent:
        tasks.append((row["id"], e))

t0 = time.time()
verdicts = [None] * len(tasks)
with ThreadPoolExecutor(max_workers=3) as ex:
    futs = {ex.submit(judge_one, e): i for i, (_, e) in enumerate(tasks)}
    for f in as_completed(futs):
        verdicts[futs[f]] = f.result()
dt = time.time() - t0

# case 级 all_rec：一个 case 的全部视角全 yes 才算 hit
by_case = {}
for (cid, _), v in zip(tasks, verdicts):
    by_case.setdefault(cid, []).append(v)
hits = 0; unparse = 0
for cid, vs in by_case.items():
    if any(v is None for v in vs): unparse += 1
    if all(v == "yes" for v in vs): hits += 1
n_case = len(by_case)
all_rec = hits / n_case * 100 if n_case else 0

rep = {
    "model": MLX_MODEL, "n_cases": n_case, "n_judgements": len(tasks),
    "elapsed_s": round(dt, 1), "per_judge_s": round(dt / max(len(tasks), 1), 2),
    "unparseable": unparse, "all_rec_pct": round(all_rec, 2),
    "hits": hits, "online_frozen_four_all_rec_pct": 26.05,
    "note": "本机 8bit 探索样本（exp_df40 four 前 N case），在线冻结 four all_rec=26.05 为参照"
}
print(json.dumps(rep, ensure_ascii=False, indent=2))
Path("/Users/fei/code/scx/tools/bench_p19_keep/calib/explore_mlx_8bit.json").write_text(
    json.dumps(rep, ensure_ascii=False, indent=2), encoding="utf-8")
