#!/usr/bin/env python3
"""bench_long.py — 真实工程长代码压缩问答评测（P6j，B 方向）。

范式：每个单元 = 一段真实工程风格长代码 + 若干代码事实题。gold 全部用
真实工具链执行/静态确认（见 bench_long/*.c/rs/py/cpp/java 的 main 输出）。
prompt = 代码全文 + 问题；scx 以问题文本为 query 压缩（query 命中机制保
相关行/词，其余按重要性裁剪）；对照 full（原码）vs scx（压缩码）。
压缩率 = 压缩后 token / 原 prompt token；保持率 = full 答对中 scx 仍答对。
"""
import argparse
import csv
import os
import sys
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from bench import (  # noqa: E402
    load_registry,
    resolve_model,
    load_model_mlx,
    wrap_chat,
    scx_compress,
    model_value,
    kind_of,
    norm_num,
    norm_token,
)

DEFAULT_UNITS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "bench_long")


def normalize_gold(gold: str) -> str:
    """题表 gold 归一：数字去尾零；字符串去引号小写；布尔统一。"""
    k = kind_of(gold)
    if k == "num":
        return norm_num(gold)
    if k == "bool":
        return "true" if gold.lower().startswith("t") else "false"
    return norm_token(gold)


def build_prompt(code: str, q: str, fence: str, fmt_hint: str = "") -> str:
    """单文档问答 prompt：代码围栏 + 问题。答案要短，3B 可判。"""
    hint = f"\n{fmt_hint}" if fmt_hint else ""
    return (
        "以下是一段真实工程代码，请阅读代码后回答后面的问题。\n\n"
        f"```{fence}\n{code}\n```\n\n"
        f"问题：{q}\n"
        f"回答（只输出答案）{hint}："
    )


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--qa", default="config/bench/datasets/bench_long_qa.csv")
    ap.add_argument("--units", default=DEFAULT_UNITS)
    ap.add_argument("--model", default="qwen2.5-3b-instruct")
    ap.add_argument("--scx-bin", default="target/release/scx")
    ap.add_argument("--quantize", type=int, default=8)
    ap.add_argument("--scx-extra", default="--trim 1 --trim-ratio 0.3 --trim-op-keep 1",
                    metavar="ARGS", help="scx 压缩参数；默认 query 引导档（无 code-keep）")
    ap.add_argument("--csv", default="", help="逐题明细导出")
    ap.add_argument("--fmt-hint", default="", metavar="HINT",
                    help="答案格式提示（如 '复杂度请写成 O(1) 形式'），追加在『回答』前")
    args = ap.parse_args()

    scx_extra = tuple(args.scx_extra.split()) if args.scx_extra else ()
    fence = {"rust_cache": "rust", "scx_trim": "rust", "c_pool": "c", "c_sched": "c",
             "py_config": "python", "cpp_matrix": "cpp", "java_queue": "java"}
    ext = {"rust_cache": "rs", "scx_trim": "rs", "c_pool": "c", "c_sched": "c",
           "py_config": "py", "cpp_matrix": "cpp", "java_queue": "java"}

    with open(args.qa, encoding="utf-8") as fh:
        qa = list(csv.DictReader(fh))

    registry = load_registry()
    spec = resolve_model(registry, args.model)
    print(f"[load] MLX 加载模型 {spec.display()} ({spec.path}) ...", flush=True)
    model, tok = load_model_mlx(spec, quantize=args.quantize)

    from mlx_lm import generate as mlx_generate

    def run(prompt: str, max_new: int = 96) -> str:
        return mlx_generate(model, tok, prompt, max_tokens=max_new, verbose=False)

    rows = []
    csv_fh = None
    csv_w = None
    if args.csv:
        csv_fh = open(args.csv, "w", newline="")
        csv_w = csv.DictWriter(csv_fh, fieldnames=["unit", "question", "full", "scx", "gold", "full_tok", "scx_tok", "compressed"])
        csv_w.writeheader()
        csv_fh.flush()

    n = len(qa)
    for i, item in enumerate(qa):
        unit = item["unit"]
        q = item["question"]
        gold = normalize_gold(item["answer"])
        code = (Path(args.units) / f"{unit}.{ext[unit]}").read_text(encoding="utf-8")
        prompt = build_prompt(code, q, fence.get(unit, "text"), args.fmt_hint)

        ans_full = run(wrap_chat(tok, prompt))
        scx_out = scx_compress(args.scx_bin, prompt, q, extra=scx_extra)
        scx_tok = len(tok.encode(scx_out))
        full_tok = len(tok.encode(prompt))
        ans_scx = run(wrap_chat(tok, scx_out))

        row = {
            "unit": unit, "question": q,
            "full": model_value(ans_full, gold), "scx": model_value(ans_scx, gold),
            "gold": gold, "full_tok": full_tok, "scx_tok": scx_tok,
            "compressed": scx_out != prompt,
        }
        rows.append(row)
        if csv_w is not None:
            csv_w.writerow(row)
            csv_fh.flush()
        if (i + 1) % 5 == 0 or i == 0:
            print(f"[{i+1}/{n}] {unit}: full={row['full']} scx={row['scx']} gold={row['gold']} cut={(1 - scx_tok / full_tok) * 100:.1f}%", flush=True)
    if csv_fh is not None:
        csv_fh.close()
        print(f"[csv] 明细已导出: {args.csv}", flush=True)

    full_acc = sum(1 for r in rows if r["full"] == r["gold"]) / n
    scx_acc = sum(1 for r in rows if r["scx"] == r["gold"]) / n
    tok_ratio = sum(r["scx_tok"] for r in rows) / sum(r["full_tok"] for r in rows)
    passed = sum(1 for r in rows if r["full"] == r["gold"])
    kept = sum(1 for r in rows if r["full"] == r["gold"] and r["scx"] == r["gold"])

    print(f"\n=== 长代码压缩问答（{spec.display()} via MLX, {n} 题, {scx_extra}）===")
    print(f"full : acc={full_acc:.3f} ({full_acc * n:.0f}/{n})")
    print(f"scx  : acc={scx_acc:.3f} ({scx_acc * n:.0f}/{n})  tok_ratio={tok_ratio:.3f}  cut={100 * (1 - tok_ratio):.1f}%")
    print(f"保持率（full 答对中 scx 仍答对）: {kept}/{passed} = {kept / max(passed, 1):.3f}")
    not_compressed = sum(1 for r in rows if not r["compressed"])
    if not_compressed:
        print(f"注：{not_compressed}/{n} 题 scx 未压缩（透传/失败），按 fail-open 计")
    by_unit = {}
    for r in rows:
        u = by_unit.setdefault(r["unit"], {"full": 0, "scx": 0, "n": 0})
        u["n"] += 1
        u["full"] += 1 if r["full"] == r["gold"] else 0
        u["scx"] += 1 if r["scx"] == r["gold"] else 0
    for u, v in by_unit.items():
        print(f"  {u:12s} full {v['full']}/{v['n']}  scx {v['scx']}/{v['n']}")


if __name__ == "__main__":
    main()
