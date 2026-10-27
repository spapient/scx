#!/usr/bin/env python3
"""GSM8K 8-shot 三方对照评测（MLX 版）：full vs scx-compressed vs head-truncation。

复刻 LLMLingua-2 论文的 GSM8K 评测协议（8-shot、exact-match 最终数字），
推理后端用 Apple MLX（mlx_lm）加载本地模型，不走 transformers Auto*（避免联网）。

用法：
  eval_gsm8k_mlx.py --n 50 --model /Users/fei/model [--scx-bin target/release/scx]
"""
import argparse
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

os.environ.setdefault("HF_HUB_OFFLINE", "1")
os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")

import torch  # noqa: E402  (datasets 依赖)
from datasets import load_dataset  # noqa: E402
from transformers import PreTrainedTokenizerFast  # noqa: E402

SEED = 20261027
FINAL_NUM = re.compile(r"[-+]?\d{1,3}(?:,\d{3})*(?:\.\d+)?|\d+(?:\.\d+)?")


def build_prompt(demos, question):
    parts = []
    for d in demos:
        parts.append(f"Q: {d['question']}\nA: {d['answer']}")
    parts.append(f"Q: {question}\nA:")
    return "\n\n".join(parts)


def scx_compress(scx_bin, prompt, query):
    with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as fh:
        fh.write(prompt)
        path = fh.name
    try:
        proc = subprocess.run(
            [scx_bin, path, "--query", query, "--ratio", "0.35", "--context", "1"],
            capture_output=True, text=True, timeout=120,
        )
        if proc.returncode != 0:
            return prompt  # fail-open：压缩失败用原文
        out = proc.stdout
        if not out.strip():
            return prompt
        # 剥离 scx 追加的元数据摘要行（以 "compressed: " 开头的最后一行）
        lines = out.splitlines()
        if lines and lines[-1].lstrip().startswith("compressed:"):
            out = "\n".join(lines[:-1])
        return out if out.strip() else prompt
    finally:
        os.unlink(path)


def last_number(text):
    nums = FINAL_NUM.findall(text)
    if not nums:
        return None
    return nums[-1].replace(",", "")


def extract_answer(raw):
    m = re.search(r"####\s*([-+]?\d+(?:\.\d+)?)", raw)
    return m.group(1) if m else None


def load_model_mlx(model_dir):
    """MLX 加载模型 + 本地 tokenizer（绕开 AutoTokenizer 的联网检查）。"""
    from mlx_lm.utils import load_model

    model, _ = load_model(Path(model_dir))
    tok = PreTrainedTokenizerFast(
        tokenizer_file=os.path.join(model_dir, "tokenizer.json"),
        bos_token="<|im_start|>", eos_token="<|im_end|>",
        pad_token="<|im_end|>", unk_token="<unk>",
    )
    return model, tok


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--n", type=int, default=100)
    ap.add_argument("--model", default="/Users/fei/model")
    ap.add_argument("--scx-bin", default="target/release/scx")
    ap.add_argument("--csv", default="", help="逐样本明细导出路径（CSV）")
    args = ap.parse_args()

    import random
    random.seed(SEED)

    print(f"[load] MLX 加载模型 {args.model} ...", flush=True)
    model, tok = load_model_mlx(args.model)

    ds = load_dataset("openai/gsm8k", "main")
    # 8-shot demo 用完整答案（含推理链）——LLMLingua-2 论文口径；
    # 只有超过 scx 500-token 透传阈值，压缩才会真正生效。
    demos = [{"question": d["question"], "answer": d["answer"]}
             for d in ds["train"].select(range(8))]
    test = ds["test"]
    test = test.shuffle(seed=SEED).select(range(min(args.n, len(test))))

    from mlx_lm import generate as mlx_generate

    def run(prompt, max_new=192):
        """greedy 解码，返回生成的文本（不含 prompt 本身）。"""
        out = mlx_generate(model, tok, prompt, max_tokens=max_new, verbose=False)
        return out

    rows = []
    for i, item in enumerate(test):
        q = item["question"]
        gold = extract_answer(item["answer"])
        prompt = build_prompt(demos, q)

        ans_full = run(prompt)
        scx_out = scx_compress(args.scx_bin, prompt, f"Q: {q}")
        scx_tok = len(tok.encode(scx_out))
        full_tok = len(tok.encode(prompt))
        ans_scx = run(scx_out)
        trunc = tok.decode(tok.encode(prompt)[:scx_tok], skip_special_tokens=True)
        ans_trunc = run(trunc)

        rows.append({
            "idx": i + 1,
            "full": last_number(ans_full), "scx": last_number(ans_scx),
            "trunc": last_number(ans_trunc), "gold": gold,
            "full_tok": full_tok, "scx_tok": scx_tok,
            "compressed": scx_out != prompt,
        })
        if (i + 1) % 5 == 0 or i == 0:
            print(f"[{i+1}/{args.n}]", flush=True)

    if args.csv:
        import csv as _csv
        with open(args.csv, "w", newline="") as fh:
            w = _csv.DictWriter(fh, fieldnames=list(rows[0].keys()))
            w.writeheader()
            w.writerows(rows)
        print(f"[csv] 明细已导出: {args.csv}", flush=True)

    n = len(rows)
    full_acc = sum(1 for r in rows if r["full"] == r["gold"]) / n
    scx_acc = sum(1 for r in rows if r["scx"] == r["gold"]) / n
    trunc_acc = sum(1 for r in rows if r["trunc"] == r["gold"]) / n
    tok_ratio = sum(r["scx_tok"] for r in rows) / sum(r["full_tok"] for r in rows)
    # 分桶统计：按压缩率（1 - scx_tok/full_tok）分桶看保持
    buckets = {}
    for r in rows:
        cut = 1 - r["scx_tok"] / r["full_tok"] if r["full_tok"] else 0
        b = "cut<2%" if cut < 0.02 else ("2-10%" if cut < 0.10 else ("10-35%" if cut < 0.35 else ">35%"))
        buckets.setdefault(b, [0, 0])
        buckets[b][0] += 1
        if r["full"] == r["gold"]:
            buckets[b][1] += 1

    print("\n=== GSM8K 8-shot 三方对照（Nanbeige4.2-3B via MLX, n=%d, seed=%d）===" % (n, SEED))
    print(f"full : acc={full_acc:.3f} ({full_acc*n:.0f}/{n})  ctx_tok={sum(r['full_tok'] for r in rows)//n}")
    print(f"scx  : acc={scx_acc:.3f} ({scx_acc*n:.0f}/{n})  ctx_tok={sum(r['scx_tok'] for r in rows)//n}  tok_ratio={tok_ratio:.3f}")
    print(f"trunc: acc={trunc_acc:.3f} ({trunc_acc*n:.0f}/{n})  ctx_tok=同 scx")
    passed = sum(1 for r in rows if r["full"] == r["gold"])
    kept_of_passed = sum(1 for r in rows if r["full"] == r["gold"] and r["scx"] == r["gold"])
    print(f"scx 保持率（full 答对中 scx 仍答对）: {kept_of_passed}/{passed} = {kept_of_passed/max(passed,1):.3f}")
    print("压缩率分桶（cut = 1 - scx_tok/full_tok）:")
    for b in ["cut<2%", "2-10%", "10-35%", ">35%"]:
        cnt = buckets.get(b, [0, 0])[0]
        if cnt:
            print(f"  {b}: {cnt} 样本")
    not_compressed = sum(1 for r in rows if not r["compressed"])
    if not_compressed:
        print(f"注：{not_compressed}/{n} 样本 scx 未压缩（透传/失败），已按 fail-open 计")


if __name__ == "__main__":
    main()
