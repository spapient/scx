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
        "model": "qwen3.8-flash",   # 百炼 flash 档最便宜（输入1.09/输出3.43 元每百万 token）
        "env": "QWEN_API_KEY",
        "reasoning": True,   # 3.8 系带思考：judge 同样要给足 max_tokens
        "max_new": 2048,
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
    """在线 judge 打分；解析失败或网络异常重试，仍失败返回 None。"""
    prompt = RUBRIC.format(
        task=task["task"], en_task=task["en_task"], answer_spec=task["answer_spec"], answer=answer
    )
    cfg = JUDGE_CFG[judge]
    for _ in range(2):
        # deepseek-flash 带 reasoning：评分任务会先输出 1000+ token 推理，
        # max_tokens 太小会被思考吃光导致 content 为空/截断。给足额度。
        try:
            raw = api_run(prompt, max_new=cfg["max_new"], judge=judge)
        except Exception:  # noqa: BLE001 网络失败不 kill 全进程
            return None, ""
        sc = parse_score(raw)
        if sc is not None:
            return sc, raw
    return None, raw


def api_call(prompt: str, max_new: int, judge: str) -> str:
    """OpenAI 兼容调用，按 judge 选择端点/模型/key 环境变量。网络超时重试 3 次。"""
    cfg = JUDGE_CFG[judge]
    key = os.environ.get(cfg["env"], "")
    if not key:
        raise SystemExit(f"{cfg['env']} env missing for judge={judge}")
    import urllib.request
    body = {"model": cfg["model"], "messages": [{"role": "user", "content": prompt}],
            "max_tokens": max_new, "temperature": 0.1}
    last = None
    for attempt in range(3):
        try:
            req = urllib.request.Request(
                cfg["url"],
                data=json.dumps(body).encode(),
                headers={"Authorization": f"Bearer {key}", "Content-Type": "application/json"},
            )
            with urllib.request.urlopen(req, timeout=240) as r:
                data = json.loads(r.read())
            return data["choices"][0]["message"]["content"]
        except Exception as e:  # noqa: BLE001 网络/超时/限流均重试
            last = e
            time.sleep(2 * (attempt + 1))
    raise last


def main() -> None:
    # 压缩召回 query：完整任务文本当 query 会命中几乎每行（terms=53），
    # 把压缩稀释到 0-42% 且不稳定。正确用法是短领域关键词（terms≈4-6），
    # 才能让 --trim-ratio 档位产生可分辨的梯度。
    Q_HINT = {
        "rust_arch": "模块架构 接口 数据流 权衡 扩展点",
        "perf_opt": "性能优化 瓶颈 基准 测量 缓存",
        "gpu_tiling": "GPU 分块 并行 共享内存 访存",
        "kv_cache": "KV 缓存 显存 淘汰 分页 量化",
        "scheduler": "调度器 优先级 延迟 抢占 公平",
        "allocator": "内存分配器 碎片 缓存行 池化",
        "code_review": "代码评审 缺陷 安全性 性能 可读性",
        "log_sys": "日志系统 异步 缓冲 采样 级别",
        "test_strategy": "测试策略 单元 集成 模糊 覆盖率",
        "kv_batch": "批处理 合并 吞吐 延迟 队列",
        "cache_coherence": "缓存一致性 MESI 目录 伪共享 总线",
        "consensus": "Raft 共识 选举 日志复制 持久化",
        "compiler_ir": "编译器 IR SSA 类型系统 pass",
        "net_stack": "网络栈 零拷贝 环形队列 TCP 吞吐",
        "gpu_mempool": "显存池 分桶 复用 碎片 cudaMalloc",
        "err_arch": "错误处理 类型 上下文 重试 降级",
        "hashmap": "哈希表 开放寻址 扩容 缓存友好",
        "build_sys": "增量构建 依赖图 缓存 并行 失效",
        "lsm_store": "LSM 存储 SSTable 合并 写放大 WAL",
        "observability": "可观测性 trace metric 采样 成本",
    }
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", default="config/bench/datasets/bench_arch.csv")
    ap.add_argument("--model", default="qwen2.5-3b-instruct")
    ap.add_argument("--scx-bin", default="target/release/scx")
    ap.add_argument("--quantize", type=int, default=8)
    ap.add_argument("--scx-extra",
                    default="--trim 1 --trim-ratio 0.3 --structure-anchors md --sym-line-keep 1",
                    metavar="ARGS")
    ap.add_argument("--ratio-scan", default="",
                    help="压缩档位扫描：逗号分隔的 --trim-ratio 列表（如 0.2,0.3,0.4），"
                         "覆盖默认档并逐档生成 scx 方案，绘制质量-压缩曲线")
    ap.add_argument("--resume", action="store_true",
                    help="若 plans 缓存文件存在则跳过本地生成（崩溃续跑）")
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
    #    ratio-scan 时：full 一次 + 每档 scx 一次。生成结果持久化，--resume 可续跑。
    ratios = [r.strip() for r in args.ratio_scan.split(",") if r.strip()] or [None]
    # 档位扫描时从默认 extra 中移除旧的 --trim-ratio 标志及其值（成对跳过），
    # 避免残留的 "0.3" 被 scx 当作第二个输入文件导致 rc!=0 fail-open。
    scx_extra_base = []
    _i = 0
    while _i < len(scx_extra):
        if scx_extra[_i] == "--trim-ratio":
            _i += 2
            continue
        scx_extra_base.append(scx_extra[_i])
        _i += 1
    scx_extra_base = tuple(scx_extra_base)
    plans_cache = "config/bench/cache/bench_arch_plans.json"
    plans = {}
    if args.resume and os.path.exists(plans_cache):
        import json as _json
        with open(plans_cache, encoding="utf-8") as fh:
            cached = _json.load(fh)
        for i, item in enumerate(qa):
            c = cached.get(item["domain"])
            if c and set(c["scx"].keys()) == {str(r) for r in ratios}:
                plans[i] = (item, c["full"], {r: c["scx"][str(r)] for r in ratios},
                            c["full_tok"], {r: c["scx_tok"][str(r)] for r in ratios})
        if len(plans) == len(qa):
            print(f"[resume] 从 {plans_cache} 加载 {len(plans)} 题生成结果", flush=True)
    if not plans:
        for i, item in enumerate(qa):
            prompt = build_prompt(item)
            t0 = time.time()
            full_plan = gen(prompt)
            full_tok = len(tok.encode(prompt))
            scx_plans, scx_toks = {}, {}
            for rt in ratios:
                extra = scx_extra_base + (f"--trim-ratio", rt) if rt else scx_extra
                scx_out = scx_compress(args.scx_bin, prompt, Q_HINT.get(item["domain"], item["domain"]),
                                       extra=extra)
                scx_plans[rt] = gen(scx_out)
                scx_toks[rt] = len(tok.encode(scx_out))
            plans[i] = (item, full_plan, scx_plans, full_tok, scx_toks)
            cut = 100 * (1 - scx_toks[ratios[0]] / full_tok)
            print(f"[gen {i+1}/{len(qa)}] {item['domain']:14s} cut={cut:.0f}% {time.time()-t0:.0f}s", flush=True)
            # 每题完成后即时持久化（崩溃最多丢 1 题）
            import json as _json
            dump = {p[0]["domain"]: {"full": p[1], "scx": {str(r): p[2][r] for r in ratios},
                                     "full_tok": p[3], "scx_tok": {str(r): p[4][r] for r in ratios}}
                    for p in plans.values()}
            with open(plans_cache, "w", encoding="utf-8") as fh:
                _json.dump(dump, fh, ensure_ascii=False)

    # 2) 在线 judge 打分（并发 3，防限流）。both = 双模型交叉验证。
    #    judge 结果即时落盘：崩溃/重跑时 --resume 跳过已完成，避免整批重打。
    judges = ["deepseek", "qwen"] if args.judge == "both" else [args.judge]
    import concurrent.futures as cf
    import threading
    _lock = threading.Lock()
    jobs = [(i, tr, rt, j) for i in range(len(qa)) for tr in ("full", "scx") for rt in ratios for j in judges]
    results = {}
    judge_cache = "config/bench/cache/bench_arch_judge.json"
    if args.resume and os.path.exists(judge_cache):
        import json as _json
        with open(judge_cache, encoding="utf-8") as fh:
            cached = _json.load(fh)
        for k, v in cached.items():
            i, tr, rt, j = k.split("|")
            results[(int(i), tr, rt, j)] = (v, "")
        print(f"[resume] judge 缓存 {len(results)} 条", flush=True)
    pending = [jb for jb in jobs if jb not in results]
    if pending:
        with cf.ThreadPoolExecutor(max_workers=3) as ex:
            def _ans(i, tr, rt):
                return plans[i][1] if tr == "full" else plans[i][2][rt]
            futs = {ex.submit(judge_answer, plans[i][0], _ans(i, tr, rt), api_call, j): (i, tr, rt, j)
                    for i, tr, rt, j in pending}
            for f in cf.as_completed(futs):
                i, tr, rt, j = futs[f]
                sc, raw = f.result()
                results[(i, tr, rt, j)] = (sc, raw)
                with _lock:
                    print(f"[judge] {j:8s} {i}/{tr}@{rt}: {sc['structure'] if sc else 'PARSE-FAIL'}", flush=True)
                    import json as _json
                    dump = {}
                    for (i2, tr2, rt2, j2), (sc2, _) in results.items():
                        dump[f"{i2}|{tr2}|{rt2}|{j2}"] = sc2
                    with open(judge_cache, "w", encoding="utf-8") as fh:
                        _json.dump(dump, fh, ensure_ascii=False)

    # 3) 一致性校验：随机抽 --rejudge 题复打（对首个 judge、首个档位）。
    judge_delta = {}
    if args.rejudge > 0:
        rng = random.Random(args.seed)
        j0 = judges[0]
        rt0 = ratios[0]
        for i in rng.sample(range(len(qa)), min(args.rejudge, len(qa))):
            for tr in ("full", "scx"):
                ans = plans[i][1] if tr == "full" else plans[i][2][rt0]
                sc2, _ = judge_answer(plans[i][0], ans, api_call, j0)
                sc1 = results[(i, tr, rt0, j0)][0]
                if sc1 and sc2:
                    d = max(abs(sc2[k] - sc1[k]) for k in sc1)
                    judge_delta[(i, tr)] = d
                    print(f"[rejudge] {j0} {i}/{tr} max-delta={d:.1f}", flush=True)

    # 4) 汇总。full 轨数据按各档位各存一份（同答案重复存），取 ratios[0] 即可。
    dims = ("structure", "accuracy", "tradeoff", "feasibility", "clarity")
    agg = {j: {tr: {rt: {d: [] for d in dims} for rt in ratios} for tr in ("full", "scx")} for j in judges}
    per_row = []
    for i in range(len(qa)):
        r = {"domain": plans[i][0]["domain"]}
        r["cut_pct"] = round(100 * (1 - plans[i][4][ratios[0]] / plans[i][3]), 1)
        for j in judges:
            for tr in ("full", "scx"):
                for rt in ratios:
                    sc = results[(i, tr, rt, j)][0]
                    for d in dims:
                        v = sc[d] if sc else None
                        r[f"{j}_{tr}_{rt}_{d}"] = v
                        if v is not None:
                            agg[j][tr][rt][d].append(v)
                    r[f"{j}_{tr}_{rt}_total"] = (sum(sc[d] for d in dims) / 5) if sc else None
        per_row.append(r)

    for j in judges:
        jname = {"deepseek": "DeepSeek-V4.1-Flash", "qwen": "Qwen-flash (3.8)"}[j]
        print(f"\n=== P6n 架构文档 rubric 评测（{spec.display()} 生成本地 / judge={jname} 在线, {len(qa)} 题, ratios={ratios}）===")
        print(f"{'轨':16s} {'structure':>9s} {'accuracy':>9s} {'tradeoff':>9s} {'feasibility':>10s} {'clarity':>8s} {'TOTAL':>7s} {'cut':>6s}")
        all_tot = {}
        for label, tr, rt in ([(f"full", "full", ratios[0])] +
                              [(f"scx r={rt}", "scx", rt) for rt in ratios]):
            row = [label]
            for d in dims:
                a = agg[j][tr][rt][d]
                row.append(sum(a) / len(a) if a else 0)
            ta = sum(sum(agg[j][tr][rt][d]) for d in dims)
            n_t = sum(len(agg[j][tr][rt][d]) for d in dims)
            tot = ta / n_t if n_t else 0
            row.append(tot)
            all_tot[label] = tot
            cut = 0.0
            if tr == "scx":
                cut = 100 * (1 - sum(plans[i][4][rt] for i in range(len(qa))) / sum(plans[i][3] for i in range(len(qa))))
            row.append(f"{cut:.0f}%")
            print(f"{row[0]:16s} {row[1]:9.2f} {row[2]:9.2f} {row[3]:9.2f} {row[4]:10.2f} {row[5]:8.2f} {row[6]:7.2f} {row[7]:>6s}")
        ft = all_tot.get("full", 0)
        for rt in ratios:
            st = all_tot.get(f"scx r={rt}", 0)
            print(f"保持率 scx(r={rt})/full = {st:.2f} / {ft:.2f} = {st/ft if ft else 0:.3f}")
    # 压缩率按首个档位展示
    if ratios[0]:
        cut0 = 100 * (1 - sum(plans[i][4][ratios[0]] for i in range(len(qa))) / sum(plans[i][3] for i in range(len(qa))))
        print(f"压缩率（r={ratios[0]}）: {cut0:.1f}%")

    # 双 judge 交叉一致性（same 答案两模型打分对比，按档位）。
    if len(judges) == 2:
        print("\n=== 双 judge 交叉一致性（同答案两模型打分差）===")
        j0, j1 = judges
        for rt in ratios:
            pairs = {}
            for i in range(len(qa)):
                for tr in ("full", "scx"):
                    s0 = results[(i, tr, rt, j0)][0]
                    s1 = results[(i, tr, rt, j1)][0]
                    if s0 and s1:
                        for d in dims:
                            pairs.setdefault(d, []).append(abs(s0[d] - s1[d]))
            print(f"--- 档位 {rt} ---")
            print(f"{'维度':10s} {'均值差':>7s} {'最大差':>7s} {'方向一致率':>10s}")
            all_d = []
            agree = []
            for d in dims:
                v = pairs.get(d, [])
                if v:
                    all_d += v
                    # 方向一致：full→scx 增减方向是否一致
                    for i in range(len(qa)):
                        f0 = results[(i, "full", rt, j0)][0]
                        f1 = results[(i, "full", rt, j1)][0]
                        s0 = results[(i, "scx", rt, j0)][0]
                        s1 = results[(i, "scx", rt, j1)][0]
                        if f0 and f1 and s0 and s1:
                            agree.append((s0[d] >= f0[d]) == (s1[d] >= f1[d]))
                    ar = sum(agree) / len(agree) if agree else 0
                    print(f"{d:10s} {sum(v)/len(v):7.2f} {max(v):7.1f} {ar*100:9.0f}%")
            if all_d:
                print(f"{'综合':10s} {sum(all_d)/len(all_d):7.2f} {max(all_d):7.1f}")

    if judge_delta:
        print(f"\n单 judge 复打一致性（{len(judge_delta)} 次, max 分差）: 均值 {sum(judge_delta.values())/len(judge_delta):.2f} / 最大 {max(judge_delta.values()):.1f}")
    fails = [r["domain"] for r in per_row
             if all(r.get(f"{j}_scx_{ratios[0]}_total") is None and r.get(f"{j}_full_{ratios[0]}_total") is None
                    for j in judges)]
    if fails:
        print("judge 解析失败题:", fails)

    if args.csv:
        with open(args.csv, "w", newline="", encoding="utf-8") as fh:
            keys = ["domain", "cut_pct"]
            for j in judges:
                for rt in ratios:
                    for d in dims:
                        keys += [f"{j}_full_{rt}_{d}", f"{j}_scx_{rt}_{d}"]
                    keys += [f"{j}_full_{rt}_total", f"{j}_scx_{rt}_total"]
            w = csv.DictWriter(fh, fieldnames=keys)
            w.writeheader()
            for r in per_row:
                w.writerow({k: r.get(k) for k in keys})
        print(f"[csv] {args.csv}", flush=True)


if __name__ == "__main__":
    main()
