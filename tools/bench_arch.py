#!/usr/bin/env python3
"""P6n 架构文档/设计开放题 rubric 评测。

被测者 = 本地模型（千问 3B，chat 模板）在 full / scx 压缩 prompt 下产出
结构化架构方案；评判者 = 在线强模型（DeepSeek V4.1-Flash）按 5 维 rubric
打分（0-5）。用于回答「压缩对开放题/生成型任务是否无损」——判别型评测
（编译/断言）测不了产出型任务，rubric judge 补齐这一层。

用法示例：
  DEEPSEEK_API_KEY=... python tools/bench_arch.py --csv bench_arch_out.csv
  # 一致性校验（抽 2 题复打 judge）：
  DEEPSEEK_API_KEY=... python tools/bench_arch.py --rejudge 2 --seed 42
"""
import argparse
import csv
import json
import os
import random
import re
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from bench import (  # noqa: E402
    load_registry, resolve_model, load_model_mlx, scx_compress, wrap_chat,
)

RUBRIC = """You are a rigorous software architecture reviewer. Score this design answer on 5 criteria, each 0.0 to 5.0 (halves allowed), based ONLY on the answer content:
- structure: whether all required sections are present and organized (module tree, interfaces, data flow, tradeoffs, extension points)
- accuracy: technical correctness of the design; no errors, sound reasoning, realistic numbers
- tradeoff: quality of tradeoff analysis; both sides considered, chosen side justified
- feasibility: specificity toward implementation (interfaces, pseudocode, concrete data structures, quantitative estimates)
- clarity: readability, logical flow, signal density vs fluff

Question (Chinese): {task}
Question (EN): {en_task}
Required output format: {answer_spec}

Answer:
{answer}

Reply with ONLY JSON, no markdown fences, no commentary:
{{"scores": {{"structure": x, "accuracy": x, "tradeoff": x, "feasibility": x, "clarity": x}}, "summary": "one-sentence assessment"}}"""

# 在线 judge 配置（双模型交叉验证）：key 一律走环境变量，不进 git。
JUDGE_CFG = {
    "deepseek": {
        "url": "https://api.deepseek.com/chat/completions",
        "model": "deepseek-flash",
        "env": "DEEPSEEK_API_KEY",
        "reasoning": True,   # flash 带思考：评分任务会先输出 1000+ token 推理
        "max_new": 2048,
    },
    "qwen": {
        "url": "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions",
        "model": "qwen-flash",   # 百炼最便宜档
        "env": "QWEN_API_KEY",
        "reasoning": False,
        "max_new": 1024,
    },
}


def build_prompt(item: dict) -> str:
    """任务 + 英文工程上下文（拉长触发压缩）+ 输出格式要求。"""
    return (
        f"You are a senior software architect. Write a complete, structured design document "
        f"for the following request. Be concrete: name data structures, give interface "
        f"signatures and pseudocode, include quantitative estimates where applicable. "
        f"Output ONLY the document in structured Markdown, no preamble.\n\n"
        f"Task: {item['task']}\n"
        f"Task (EN): {item['en_task']}\n\n"
        f"Output format: {item['answer_spec']}\n\n"
        f"Verification contract: your document will be graded by an expert reviewer on five "
        f"axes (structure, accuracy, tradeoff analysis, feasibility, clarity). Missing required "
        f"sections, vague hand-waving, and unfounded numbers reduce the score. A tight, "
        f"specific, well-organized document scores highest."
    )


def parse_score(raw: str) -> dict | None:
    """解析 judge 的 JSON（容忍围栏/杂文本/单双引号）。"""
    s = raw.strip()
    m = re.search(r"\{.*\}", s, re.S)
    if not m:
        return None
    try:
        data = json.loads(m.group(0))
    except json.JSONDecodeError:
        # 容错：单引号 → 双引号
        try:
            data = json.loads(m.group(0).replace("'", '"'))
        except json.JSONDecodeError:
            return None
    scores = data.get("scores")
    if not isinstance(scores, dict):
        return None
    out = {}
    for k in ("structure", "accuracy", "tradeoff", "feasibility", "clarity"):
        v = scores.get(k)
        try:
            out[k] = float(v)
        except (TypeError, ValueError):
            return None
    return out


def judge_answer(task: dict, answer: str, api_run, judge: str) -> tuple[dict, str]:
    """在线 judge 打分；解析失败重试一次，仍失败返回 None。"""
    prompt = RUBRIC.format(
        task=task["task"], en_task=task["en_task"], answer_spec=task["answer_spec"], answer=answer
    )
    cfg = JUDGE_CFG[judge]
    for _ in range(2):
        # deepseek-flash 带 reasoning：评分任务会先输出 1000+ token 推理，
        # max_tokens 太小会被思考吃光导致 content 为空/截断。给足额度。
        raw = api_run(prompt, max_new=cfg["max_new"], judge=judge)
        sc = parse_score(raw)
        if sc is not None:
            return sc, raw
    return None, raw


def api_call(prompt: str, max_new: int, judge: str) -> str:
    """OpenAI 兼容调用，按 judge 选择端点/模型/key 环境变量。"""
    cfg = JUDGE_CFG[judge]
    key = os.environ.get(cfg["env"], "")
    if not key:
        raise SystemExit(f"{cfg['env']} env missing for judge={judge}")
    import urllib.request
    body = {"model": cfg["model"], "messages": [{"role": "user", "content": prompt}],
            "max_tokens": max_new, "temperature": 0.1}
    req = urllib.request.Request(
        cfg["url"],
        data=json.dumps(body).encode(),
        headers={"Authorization": f"Bearer {key}", "Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=240) as r:
        data = json.loads(r.read())
    return data["choices"][0]["message"]["content"]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", default="bench_arch.csv")
    ap.add_argument("--model", default="qwen2.5-3b-instruct")
    ap.add_argument("--scx-bin", default="target/release/scx")
    ap.add_argument("--quantize", type=int, default=8)
    ap.add_argument("--scx-extra",
                    default="--trim 1 --trim-ratio 0.3 --structure-anchors md --sym-line-keep 1",
                    metavar="ARGS")
    ap.add_argument("--csv", default="", help="逐题明细导出")
    ap.add_argument("--judge", default="deepseek", choices=["deepseek", "qwen", "both"],
                    help="在线 judge 模型：deepseek / qwen / both（双模型交叉验证）")
    ap.add_argument("--rejudge", type=int, default=0, help="随机抽 N 题复打 judge 校验一致性")
    ap.add_argument("--seed", type=int, default=0)
    args = ap.parse_args()

    scx_extra = tuple(args.scx_extra.split()) if args.scx_extra else ()
    with open(args.data, encoding="utf-8") as fh:
        qa = list(csv.DictReader(fh))

    registry = load_registry()
    spec = resolve_model(registry, args.model)
    print(f"[load] {spec.display()} ...", flush=True)
    model, tok = load_model_mlx(spec, quantize=args.quantize)
    from mlx_lm import generate as mlx_generate

    def gen(prompt: str, max_new: int = 900) -> str:
        return mlx_generate(model, tok, wrap_chat(tok, prompt), max_tokens=max_new, verbose=False)

    # 1) 本地生成 full/scx 方案（主线程串行，mlx_lm 不支持子线程）。
    plans = {}
    for i, item in enumerate(qa):
        prompt = build_prompt(item)
        t0 = time.time()
        full_plan = gen(prompt)
        scx_out = scx_compress(args.scx_bin, prompt, item["task"], extra=scx_extra)
        scx_plan = gen(scx_out)
        full_tok = len(tok.encode(prompt))
        scx_tok = len(tok.encode(scx_out))
        plans[i] = (item, full_plan, scx_plan, full_tok, scx_tok)
        print(f"[gen {i+1}/{len(qa)}] {item['domain']:12s} cut={100*(1-scx_tok/full_tok):.0f}% {time.time()-t0:.0f}s",
              flush=True)

    # 2) 在线 judge 打分（并发 3，防限流）。both = 双模型交叉验证。
    judges = ["deepseek", "qwen"] if args.judge == "both" else [args.judge]
    import concurrent.futures as cf
    import threading
    _lock = threading.Lock()
    jobs = [(i, tr, j) for i in range(len(qa)) for tr in ("full", "scx") for j in judges]
    results = {}
    with cf.ThreadPoolExecutor(max_workers=3) as ex:
        futs = {ex.submit(judge_answer, plans[i][0], plans[i][1 if tr == "full" else 2], api_call, j): (i, tr, j)
                for i, tr, j in jobs}
        for f in cf.as_completed(futs):
            i, tr, j = futs[f]
            sc, raw = f.result()
            results[(i, tr, j)] = (sc, raw)
            with _lock:
                print(f"[judge] {j:8s} {i}/{tr}: {sc['structure'] if sc else 'PARSE-FAIL'}", flush=True)

    # 3) 一致性校验：随机抽 --rejudge 题复打（对首个 judge）。
    judge_delta = {}
    if args.rejudge > 0:
        rng = random.Random(args.seed)
        j0 = judges[0]
        for i in rng.sample(range(len(qa)), min(args.rejudge, len(qa))):
            for tr in ("full", "scx"):
                plan = plans[i][1 if tr == "full" else 2]
                sc2, _ = judge_answer(plans[i][0], plan, api_call, j0)
                sc1 = results[(i, tr, j0)][0]
                if sc1 and sc2:
                    d = max(abs(sc2[k] - sc1[k]) for k in sc1)
                    judge_delta[(i, tr)] = d
                    print(f"[rejudge] {j0} {i}/{tr} max-delta={d:.1f}", flush=True)

    # 4) 汇总。
    dims = ("structure", "accuracy", "tradeoff", "feasibility", "clarity")
    agg = {j: {tr: {d: [] for d in dims} for tr in ("full", "scx")} for j in judges}
    per_row = []
    for i in range(len(qa)):
        r = {"domain": plans[i][0]["domain"]}
        r["cut_pct"] = round(100 * (1 - plans[i][4] / plans[i][3]), 1)
        for j in judges:
            for tr in ("full", "scx"):
                sc = results[(i, tr, j)][0]
                for d in dims:
                    v = sc[d] if sc else None
                    r[f"{j}_{tr}_{d}"] = v
                    if v is not None:
                        agg[j][tr][d].append(v)
                r[f"{j}_{tr}_total"] = (sum(sc[d] for d in dims) / 5) if sc else None
        per_row.append(r)

    for j in judges:
        jname = {"deepseek": "DeepSeek-V4.1-Flash", "qwen": "Qwen-flash"}[j]
        print(f"\n=== P6n 架构文档 rubric 评测（{spec.display()} 生成本地 / judge={jname} 在线, {len(qa)} 题, {scx_extra}）===")
        print("维度均分（0-5）：")
        print(f"{'维度':10s} {'full':>6s} {'scx':>6s} {'scx/full':>9s}")
        for d in dims:
            a = agg[j]["full"][d]; b = agg[j]["scx"][d]
            ma = sum(a) / len(a) if a else 0
            mb = sum(b) / len(b) if b else 0
            print(f"{d:10s} {ma:6.2f} {mb:6.2f} {mb/ma if ma else 0:8.2f}x")
        ta = sum(sum(agg[j]["full"][d]) for d in dims)
        tb = sum(sum(agg[j]["scx"][d]) for d in dims)
        n_full = sum(len(agg[j]["full"][d]) for d in dims)
        n_scx = sum(len(agg[j]["scx"][d]) for d in dims)
        print(f"{'TOTAL':10s} {ta/n_full:6.2f} {tb/n_scx:6.2f} {tb/ta if ta else 0:8.2f}x")
        print(f"保持率（scx 总分 / full 总分）: {tb/n_scx:.2f} / {ta/n_full:.2f} = {tb/n_scx/(ta/n_full) if ta else 0:.3f}")
    print(f"压缩率（均值）: {100*(1-sum(plans[i][4] for i in range(len(qa)))/sum(plans[i][3] for i in range(len(qa)))):.1f}%")

    # 双 judge 交叉一致性（same 答案两模型打分对比）。
    if len(judges) == 2:
        print("\n=== 双 judge 交叉一致性（同答案两模型打分差）===")
        j0, j1 = judges
        pairs = {}
        for i in range(len(qa)):
            for tr in ("full", "scx"):
                s0 = results[(i, tr, j0)][0]
                s1 = results[(i, tr, j1)][0]
                if s0 and s1:
                    for d in dims:
                        pairs.setdefault(d, []).append(abs(s0[d] - s1[d]))
        print(f"{'维度':10s} {'均值差':>7s} {'最大差':>7s}")
        all_d = []
        for d in dims:
            v = pairs.get(d, [])
            if v:
                all_d += v
                print(f"{d:10s} {sum(v)/len(v):7.2f} {max(v):7.1f}")
        if all_d:
            print(f"{'综合':10s} {sum(all_d)/len(all_d):7.2f} {max(all_d):7.1f}")

    if judge_delta:
        print(f"\n单 judge 复打一致性（{len(judge_delta)} 次, max 分差）: 均值 {sum(judge_delta.values())/len(judge_delta):.2f} / 最大 {max(judge_delta.values()):.1f}")
    fails = [r["domain"] for r in per_row
             if all(r.get(f"{j}_scx_total") is None or r.get(f"{j}_full_total") is None for j in judges)]
    if fails:
        print("judge 解析失败题:", fails)

    if args.csv:
        with open(args.csv, "w", newline="", encoding="utf-8") as fh:
            keys = ["domain", "cut_pct"]
            for j in judges:
                for d in dims:
                    keys += [f"{j}_full_{d}", f"{j}_scx_{d}"]
                keys += [f"{j}_full_total", f"{j}_scx_total"]
            w = csv.DictWriter(fh, fieldnames=keys)
            w.writeheader()
            for r in per_row:
                w.writerow({k: r.get(k) for k in keys})
        print(f"[csv] {args.csv}", flush=True)


if __name__ == "__main__":
    main()
