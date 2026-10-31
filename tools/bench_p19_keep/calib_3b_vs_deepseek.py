#!/usr/bin/env python3
"""3B↔deepseek-flash 判分一致率校准（混合路线前提，先校准再切换）。

样本：science three/four df40 压缩产物展开 entries，均匀采样 ~100 条。
同一 entry 分别用本机 qwen2.5-3b-instruct（mlx server，本地 HTTP）与在线
deepseek-flash（keepbench judge_call）判分，比较判定一致率。

运行环境：/tmp/scx-eval/bin/python（httpx；official judge 需 stub openai/tqdm）
产物：tools/bench_p19_keep/calib/calib_3b_vs_deepseek.json
"""
import sys
import json
import types
from pathlib import Path
from concurrent.futures import ThreadPoolExecutor, as_completed

# --- stub openai / tqdm（official judge 模块 import 依赖）---
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
CALIB_DIR = KEEP_DIR / "calib"
CALIB_DIR.mkdir(parents=True, exist_ok=True)

SAMPLE_N = int(sys.argv[1]) if len(sys.argv) > 1 else 100
STEMS = ["science_domain_three_perspectives", "science_domain_four_perspectives"]
# 注意：mlx_lm.server 请求体 model 字段也必须用完整本地路径（短名会被当 HF repo_id → 401）
# 8bit 量化权重（质量接近 bf16，内存 5.8→3.1GB）
MLX_MODEL = "/Users/fei/model/qwen2.5-3b-instruct-mlx-8bit"
MLX_URL = "http://127.0.0.1:8123/v1/chat/completions"
official = kb.load_official_judge()

# ---- 均匀采样 entries（覆盖 three/four）----
entries: list[dict] = []
for stem in STEMS:
    rows = kb.read_jsonl(WORK / f"{stem}.jsonl")
    for row in rows:
        ent, idx, _extra = kb.expand_imbalanced(row, row["compressed_context"])[""]
        for e, ix in zip(ent, idx):
            entries.append({"stem": stem, "case": ix["id"], "position": ix["position"],
                            "entry": e})
step = max(1, len(entries) // SAMPLE_N)
sample = entries[::step][:SAMPLE_N]
print(f"sampled {len(sample)}/{len(entries)} entries (stride={step})", flush=True)


def _judge_for(stem: str):
    return official.judge_for(stem)


def mlx_judge(stem: str, entry: dict) -> dict:
    judge = _judge_for(stem)
    request = judge.build({f: entry[f] for f in judge.fields}, None)
    body = {"model": MLX_MODEL, "messages": request,
            "max_tokens": 2048, "temperature": 0}
    try:
        resp = kb._http().post(MLX_URL, json=body)
        resp.raise_for_status()
        data = resp.json()
        raw = (data["choices"][0]["message"].get("content") or "").strip()
        v = judge.parse(official.try_parse_json(raw))
        if v is not None:
            return {"judgement": v.get("judgement"), "raw_note": None}
        return {"judgement": None, "raw_note": f"unparseable: {raw[:120]}"}
    except Exception as exc:  # noqa: BLE001
        return {"judgement": None, "raw_note": f"{type(exc).__name__}: {exc}"}


def ds_judge(stem: str, entry: dict, label: str) -> dict:
    judge = _judge_for(stem)
    v = kb.judge_call(official, judge, entry, None, label)
    if isinstance(v, dict) and "error" not in v:
        return {"judgement": v.get("judgement"), "raw_note": None}
    return {"judgement": None, "raw_note": v.get("error", "unknown")}


def mlx_only(idx: int):
    p = sample[idx]
    m = mlx_judge(p["stem"], p["entry"])
    return idx, m


def ds_only(idx: int):
    p = sample[idx]
    d = ds_judge(p["stem"], p["entry"], f"calib_ds:{p['case']}:{p['position']}")
    return idx, d


mlx_res: list[dict] = [None] * len(sample)  # type: ignore[list-item]
ds_res: list[dict] = [None] * len(sample)   # type: ignore[list-item]
# 阶段 A：3B 判分（mlx server 单实例只能扛低并发 → 3）
with ThreadPoolExecutor(max_workers=3) as ex:
    for f in as_completed([ex.submit(mlx_only, i) for i in range(len(sample))]):
        i, m = f.result()
        mlx_res[i] = m
# 阶段 B：deepseek 判分（在线无内存约束 → 12 并发）
with ThreadPoolExecutor(max_workers=kb.JUDGE_WORKERS) as ex:
    for f in as_completed([ex.submit(ds_only, i) for i in range(len(sample))]):
        i, d = f.result()
        ds_res[i] = d

results: list[dict] = [
    {"case": sample[i]["case"], "position": sample[i]["position"],
     "stem": sample[i]["stem"],
     "mlx": mlx_res[i]["judgement"], "mlx_note": mlx_res[i]["raw_note"],
     "deepseek": ds_res[i]["judgement"], "ds_note": ds_res[i]["raw_note"]}
    for i in range(len(sample))]

# ---- 一致率 ----
valid = [r for r in results if r["mlx"] is not None and r["deepseek"] is not None]
agree = sum(1 for r in valid if r["mlx"] == r["deepseek"])
by_stem: dict[str, dict] = {}
for stem in STEMS:
    vs = [r for r in valid if r["stem"] == stem]
    if vs:
        ag = sum(1 for r in vs if r["mlx"] == r["deepseek"])
        by_stem[stem] = {"n": len(vs), "agree": ag,
                         "rate": round(100.0 * ag / len(vs), 2)}

report = {
    "model_mlx": MLX_MODEL,
    "model_ds": kb.JUDGE_MODEL,
    "sampled": len(sample),
    "valid_both": len(valid),
    "agree": agree,
    "agree_rate_pct": round(100.0 * agree / len(valid), 2) if valid else None,
    "by_stem": by_stem,
    "divergence_examples": [r for r in valid if r["mlx"] != r["deepseek"]][:10],
    "mlx_unparseable": sum(1 for r in results if r["mlx"] is None),
    "ds_errors": sum(1 for r in results if r["deepseek"] is None),
    "mlx_fail_details": [{"case": r["case"], "note": r["mlx_note"]}
                         for r in results if r["mlx"] is None][:15],
}
out = CALIB_DIR / "calib_3b_vs_deepseek.json"
out.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
print(json.dumps({k: v for k, v in report.items() if k != "divergence_examples"},
                 ensure_ascii=False, indent=2))
print(f"wrote {out}")
