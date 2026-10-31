#!/usr/bin/env python3
"""更新冻结 JSON：science df40 判分结果落库（重启后重建）。

口径与官方 score.py 一致：all_rec = 该 case 全部 perspectives 判 yes 的比例（case 级）。
其余子集（parallel/conditional/policy）保留冻结原值；science 用本次 df40 重判结果。
成本从 llm_calls.jsonl 增量统计（label 前缀 science_domain_ 且 ts 晚于判分启动）。
"""
import json
from pathlib import Path

REPO = Path("/Users/fei/code/scx")
RESULT = REPO / "config/bench/results/bench_p19_keepbench.json"
WORK = REPO / "tools/bench_p19_keep/exp_df40"
LLM_CALLS = Path("/tmp/scx_bench_keep/llm_calls.jsonl")
JUDGE_START_TS = "2026-11-01"  # 本次判分启动日期前缀

def read_jsonl(p):
    return [json.loads(l) for l in p.open(encoding="utf-8") if l.strip()]

def agg_verdicts(stem: str, rows: list[dict]) -> dict:
    verdicts = read_jsonl(WORK / f"verdicts_{stem}.jsonl")
    by_case: dict[str, dict[int, str | None]] = {}
    for v in verdicts:
        by_case.setdefault(v["case"], {})[v["position"]] = v["judgement"]
    n_hit = 0
    dom = {"hit": 0, "tot": 0}
    mnr = {"hit": 0, "tot": 0}
    missing = 0
    for row in rows:
        js = by_case.get(row["id"], {})
        roles = row.get("perspective_roles", [])
        all_yes = True
        for pos, role in enumerate(roles):
            j = js.get(pos)
            if j is None:
                missing += 1
                all_yes = False
            elif j != "yes":
                all_yes = False
            if role == "dominant":
                dom["tot"] += 1
                if j == "yes":
                    dom["hit"] += 1
            else:
                mnr["tot"] += 1
                if j == "yes":
                    mnr["hit"] += 1
        if all_yes:
            n_hit += 1
    n = len(rows)
    return {
        "n": n,
        "all_rec": round(100.0 * n_hit / n, 6),
        "hit_cases": n_hit,
        "dom_rec": round(100.0 * dom["hit"] / dom["tot"], 6) if dom["tot"] else None,
        "min_rec": round(100.0 * mnr["hit"] / mnr["tot"], 6) if mnr["tot"] else None,
        "missing_judgements": missing,
    }

def agg_compression(stem: str) -> dict:
    rows = read_jsonl(WORK / f"{stem}.jsonl")
    orig = sum(len(r["context"]) for r in rows)
    comp = sum(len(r["compressed_context"]) for r in rows)
    kept = sum(len(r["compressed_context"].split("\n")) for r in rows)
    return {
        "n": len(rows),
        "orig_chars": orig,
        "comp_chars": comp,
        "char_ratio": round(comp / orig, 6),
        "kept_lines_mean": round(kept / len(rows), 6),
        "compressed_cases": len(rows),
    }

d = json.loads(RESULT.read_text(encoding="utf-8"))

# ---- 判分聚合 ----
three = agg_verdicts("science_domain_three_perspectives",
                     read_jsonl(WORK / "science_domain_three_perspectives.jsonl"))
four = agg_verdicts("science_domain_four_perspectives",
                    read_jsonl(WORK / "science_domain_four_perspectives.jsonl"))
print("three:", three)
print("four:", four)

# ---- 压缩统计 ----
c_three = agg_compression("science_domain_three_perspectives")
c_four = agg_compression("science_domain_four_perspectives")
print("c_three:", c_three)
print("c_four:", c_four)

# ---- 冻结值（其他子集原值保留）----
s = d["metrics"]["setting"]
SCIENCE_PREV = 22.07920792079208  # 冻结原值（science df 0.25）
science_hit = three["hit_cases"] + four["hit_cases"]
science_n = three["n"] + four["n"]
science_all = 100.0 * science_hit / science_n
s["science"] = {"n": science_n, "all_rec": round(science_all, 6), "hit_cases": science_hit,
                "dom_rec_three": three["dom_rec"], "min_rec_three": three["min_rec"],
                "dom_rec_four": four["dom_rec"], "min_rec_four": four["min_rec"]}

n_over = 4102
sum_hit = (900 * 22.88888888888889 + 1364 * 36.36363636363637
           + (three["n"] + four["n"]) * (three["all_rec"] / 100.0 if three["all_rec"] < 1 else three["all_rec"] / 100.0)
           + 828 * 34.29951690821256)
# all_rec 已换算成百分比，需除以 100 求 hit 数
hit_total = (900 * 22.88888888888889 / 100.0
             + 1364 * 36.36363636363637 / 100.0
             + (three["hit_cases"] + four["hit_cases"])
             + 828 * 34.29951690821256 / 100.0)
d["metrics"]["setting"]["overall"] = {
    "n": n_over, "all_rec": round(100.0 * hit_total / n_over, 6),
    "science_prev_all_rec": SCIENCE_PREV}

sub = d["metrics"]["subset"]
sub["science_domain_three_perspectives"] = {
    "n": three["n"], "all_rec": three["all_rec"], "hit_cases": three["hit_cases"]}
sub["science_domain_four_perspectives"] = {
    "n": four["n"], "all_rec": four["all_rec"], "hit_cases": four["hit_cases"]}

# ---- compression ----
cp = d["compression"]["per_subset"]
cp["science_domain_three_perspectives"] = c_three
cp["science_domain_four_perspectives"] = c_four
other_comp = sum(v["comp_chars"] for k, v in cp.items() if k not in
                 ("science_domain_three_perspectives", "science_domain_four_perspectives"))
d["compression"]["total"] = {
    "orig_chars": 59137837,
    "comp_chars": other_comp + c_three["comp_chars"] + c_four["comp_chars"],
    "char_ratio": round((other_comp + c_three["comp_chars"] + c_four["comp_chars"]) / 59137837, 6),
    "compressed_cases": 4102,
}

# ---- run 记录 ----
d["run"]["df_max_frac"] = 0.25
d["run"]["science_df_max_frac_override"] = 0.40
d["run"]["note"] = ("parallel/conditional/policy 冻结值保留（df 0.25）；science 由本机重判：df_max_frac 0.25→0.40，"
                    "seed 42 全量 1010 cases（science_domain_three/four_perspectives）")

# ---- findings ----
settings = {k: v["all_rec"] for k, v in s.items() if k != "overall"}
weak_name = min(settings, key=settings.get)
weak_val = settings[weak_name]
strong_name = max(settings, key=settings.get)
strong_val = settings[strong_name]
d["findings"]["overall_all_rec"] = d["metrics"]["setting"]["overall"]["all_rec"]
d["findings"]["strongest_setting"] = [strong_name, strong_val]
d["findings"]["weakest_setting"] = [weak_name, weak_val]

# ---- cost（增量：science 本次判分）----
sc_rows = [r for r in read_jsonl(LLM_CALLS)
           if r.get("label", "").startswith("science_domain_")
           and r.get("ts", "").startswith(JUDGE_START_TS)
           and r.get("usage")]
in_tok = sum(r["usage"].get("prompt_tokens", 0) for r in sc_rows)
out_tok = sum(r["usage"].get("completion_tokens", 0) for r in sc_rows)
price = 0.15 / 1e6 * in_tok + 0.60 / 1e6 * out_tok
d["cost"]["science_df40_calls_with_usage"] = len(sc_rows)
d["cost"]["science_df40_input_tokens"] = in_tok
d["cost"]["science_df40_output_tokens"] = out_tok
d["cost"]["science_df40_est_usd"] = round(price, 4)
d["cost"]["estimated_cost_usd"] = round(d["cost"].get("estimated_cost_usd", 8.0831) + price, 4)

# ---- verdict_audit（science 重判 31 条重试后仍失败的 error 如实记录）----
va = d["verdict_audit"]
va["verdicts"] = 15953
va["valid"] = 15953 - 31
va["shortcut_empty_context"] = va.get("shortcut_empty_context", 0)
va["error_rows"] = 31
va["per_group"]["science_domain_four_perspectives"] = {
    "n": 3424, "valid": 3393, "shortcut": 0, "error": 31}

# ---- generated_at / run / reproduce ----
import datetime
d["generated_at"] = datetime.datetime.now().astimezone().strftime("%Y-%m-%dT%H:%M:%S%z")
d["run"]["workers"] = 12
d["reproduce"]["command"] = "/tmp/scx-eval/bin/python tools/bench_p19_keep/exp_df40.py"
d["reproduce"]["update_command"] = "/tmp/scx-eval/bin/python tools/bench_p19_keep/update_p19_v3.py"

# ---- 写回（tmp + replace，防写坏）----
bak = RESULT.with_suffix(".json.bak")
if not bak.exists():
    bak.write_text(RESULT.read_text(encoding="utf-8"), encoding="utf-8")
tmp = RESULT.with_suffix(".json.tmp")
tmp.write_text(json.dumps(d, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
tmp.replace(RESULT)
print("updated:", RESULT)
