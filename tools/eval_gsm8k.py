#!/usr/bin/env python3
"""GSM8K 8-shot 三方对照评测：full vs scx-compressed vs head-truncation。

复刻 LLMLingua-2 论文的 GSM8K 评测协议（8-shot、exact-match 最终数字）。
scx 以 release 二进制压缩完整 prompt（query = 待答问题全文）；
trunc 基线把 prompt 头截断到与 scx 输出相同的 token 数（论文同口径，
从头保留会丢失问题本体——这恰是 truncation 的失败方式，对照用）。
确定性：固定 seed、greedy 解码、固定 8 个 few-shot 演示。

用法：
  eval_gsm8k.py --n 100 --model Qwen/Qwen2.5-3B-Instruct [--scx-bin target/release/scx]
"""
import argparse
import re
import subprocess
import tempfile
import torch
from datasets import load_dataset
from transformers import AutoModelForCausalLM, AutoTokenizer

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
        import os
        os.unlink(path)


def last_number(text):
    nums = FINAL_NUM.findall(text)
    if not nums:
        return None
    return nums[-1].replace(",", "")


def extract_answer(raw):
    """GSM8K 标准答案：'#### 42' 后的数字。"""
    m = re.search(r"####\s*([-+]?\d+(?:\.\d+)?)", raw)
    return m.group(1) if m else None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--n", type=int, default=100)
    ap.add_argument("--model", default="Qwen/Qwen2.5-3B-Instruct")
    ap.add_argument("--scx-bin", default="target/release/scx")
    ap.add_argument("--shards", type=int, default=3, help="并行分片数（按样本分片）")
    args = ap.parse_args()

    torch.manual_seed(SEED)
    ds = load_dataset("openai/gsm8k", "main")
    demos = [{"question": d["question"], "answer": extract_answer(d["answer"])}
             for d in ds["train"].select(range(8))]  # 固定 8 演示
    test = ds["test"]
    test = test.shuffle(seed=SEED).select(range(min(args.n, len(test))))

    tok = AutoTokenizer.from_pretrained(args.model)
    model = AutoModelForCausalLM.from_pretrained(
        args.model, torch_dtype=torch.float16, device_map="auto")

    def run(prompt):
        inputs = tok(prompt, return_tensors="pt").to(model.device)
        with torch.no_grad():
            out = model.generate(
                **inputs, max_new_tokens=192, do_sample=False, pad_token_id=tok.eos_token_id)
        return tok.decode(out[0][inputs.input_ids.shape[1]:], skip_special_tokens=True)

    rows = []
    for i, item in enumerate(test):
        q = item["question"]
        gold = extract_answer(item["answer"])
        prompt = build_prompt(demos, q)

        ans_full = run(prompt)
        scx_out = scx_compress(args.scx_bin, prompt, f"Q: {q}")
        # 若 scx 因小输入透传导致未压缩，如实记录
        scx_tok = len(tok.encode(scx_out))
        full_tok = len(tok.encode(prompt))
        ans_scx = run(scx_out)
        # trunc：头截断到与 scx 输出同 token 数
        trunc = tok.decode(tok.encode(prompt)[:scx_tok], skip_special_tokens=True)
        ans_trunc = run(trunc)

        rows.append({
            "full": last_number(ans_full), "scx": last_number(ans_scx),
            "trunc": last_number(ans_trunc), "gold": gold,
            "full_tok": full_tok, "scx_tok": scx_tok,
            "compressed": scx_out != prompt,
        })
        if (i + 1) % 20 == 0 or i == 0:
            print(f"[{i+1}/{args.n}]", flush=True)

    n = len(rows)
    full_acc = sum(1 for r in rows if r["full"] == r["gold"]) / n
    scx_acc = sum(1 for r in rows if r["scx"] == r["gold"]) / n
    trunc_acc = sum(1 for r in rows if r["trunc"] == r["gold"]) / n
    tok_ratio = sum(r["scx_tok"] for r in rows) / sum(r["full_tok"] for r in rows)

    print("\n=== GSM8K 8-shot 三方对照（Qwen2.5-3B-Instruct, n=%d, seed=%d）===" % (n, SEED))
    print(f"full : acc={full_acc:.3f} ({full_acc*n:.0f}/{n})  ctx_tok={sum(r['full_tok'] for r in rows)//n}")
    print(f"scx  : acc={scx_acc:.3f} ({scx_acc*n:.0f}/{n})  ctx_tok={sum(r['scx_tok'] for r in rows)//n}  tok_ratio={tok_ratio:.3f}")
    print(f"trunc: acc={trunc_acc:.3f} ({trunc_acc*n:.0f}/{n})  ctx_tok=同 scx")
    passed = sum(1 for r in rows if r["full"] == r["gold"])
    kept_of_passed = sum(1 for r in rows if r["full"] == r["gold"] and r["scx"] == r["gold"])
    print(f"scx 保持率（full 答对中 scx 仍答对）: {kept_of_passed}/{passed} = {kept_of_passed/max(passed,1):.3f}")
    not_compressed = sum(1 for r in rows if not r["compressed"])
    if not_compressed:
        print(f"注：{not_compressed}/{n} 样本 scx 未压缩（透传/失败），已按 fail-open 计")


if __name__ == "__main__":
    main()
