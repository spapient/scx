#!/usr/bin/env python3
"""GSM8K 8-shot 三方对照评测（MLX 版）：full vs scx-compressed vs head-truncation。

复刻 LLMLingua-2 论文的 GSM8K 评测协议（8-shot、greedy 解码、exact-match 最终数字）。
推理后端用 Apple MLX（mlx_lm）加载本地模型，不走 transformers Auto*（避免联网）。

模型一律从 config/models.json 注册表按 id 解析（--model 传 id 或本地路径），
不在脚本内散落任何模型路径或特殊 token。

用法：
  bench.py --n 50 --model qwen2.5-3b-instruct [--scx-bin target/release/scx]
"""
from __future__ import annotations

import argparse
import csv
import json
import os
import random
import re
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

os.environ.setdefault("HF_HUB_OFFLINE", "1")
os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")

import torch  # noqa: E402  (datasets 依赖)
from datasets import load_dataset  # noqa: E402
from transformers import PreTrainedTokenizerFast  # noqa: E402

SEED = 20261027
FINAL_NUM = re.compile(r"[-+]?\d{1,3}(?:,\d{3})*(?:\.\d+)?|\d+(?:\.\d+)?")
REGISTRY = Path(__file__).resolve().parent.parent / "config" / "models.json"


@dataclass(frozen=True)
class ModelSpec:
    """注册表中的一个模型条目。"""

    id: str
    name: str
    path: Path
    tokens: dict
    alias_cn: Optional[str] = None
    arch: Optional[str] = None

    def display(self) -> str:
        return f"{self.name}" + (f"（{self.alias_cn}）" if self.alias_cn else "")


def load_registry() -> dict[str, ModelSpec]:
    """读取模型注册表，按 id 建立索引。"""
    with open(REGISTRY, encoding="utf-8") as fh:
        raw = json.load(fh)
    out: dict[str, ModelSpec] = {}
    for item in raw["registry"]:
        out[item["id"]] = ModelSpec(
            id=item["id"],
            name=item["name"],
            path=Path(item["path"]),
            tokens=item.get("tokens") or {},
            alias_cn=item.get("alias_cn"),
            arch=item.get("arch"),
        )
    return out


def resolve_model(registry: dict[str, ModelSpec], model_arg: str) -> ModelSpec:
    """按 id 或本地路径解析模型。id 优先查注册表，路径则校验目录存在。"""
    if model_arg in registry:
        spec = registry[model_arg]
    elif Path(model_arg).is_dir():
        spec = ModelSpec(
            id=Path(model_arg).name,
            name=Path(model_arg).name,
            path=Path(model_arg),
            tokens={},
        )
    else:
        known = ", ".join(sorted(registry))
        sys.exit(f"[模型] 未注册的模型：{model_arg!r}。可用 id：{known}")
    if not spec.path.is_dir():
        sys.exit(f"[模型] 目录不存在：{spec.path}")
    return spec


def build_prompt(demos, question):
    parts = []
    for d in demos:
        parts.append(f"Q: {d['question']}\nA: {d['answer']}")
    parts.append(f"Q: {question}\nA:")
    return "\n\n".join(parts)


def scx_compress(scx_bin: str, prompt: str, query: str, extra: tuple = ()) -> str:
    """调用 scx 压缩 prompt。小输入透传 / 失败均 fail-open 返回原文。

    注意：scx 会在 stdout 末尾追加一行 "compressed: ..." 元数据摘要，
    必须剥离，否则污染压缩结果并虚增 token 数。

    extra 为附加 scx 参数（如 --fill-floor 2），可空。
    """
    with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as fh:
        fh.write(prompt)
        path = fh.name
    try:
        proc = subprocess.run(
            [scx_bin, path, "--query", query, "--ratio", "0.35", "--context", "1", *extra],
            capture_output=True, text=True, timeout=120,
        )
        if proc.returncode != 0:
            return prompt
        out = proc.stdout
        if not out.strip():
            return prompt
        lines = out.splitlines()
        if lines and lines[-1].lstrip().startswith("compressed:"):
            out = "\n".join(lines[:-1])
        return out if out.strip() else prompt
    finally:
        os.unlink(path)


def last_number(text: str) -> Optional[str]:
    nums = FINAL_NUM.findall(text)
    if not nums:
        return None
    return norm_num(nums[-1].replace(",", ""))


def norm_num(s: str) -> str:
    """数字归一化：'34.00'→'34'、'34.50'→'34.5'、'1.25'→'1.25'。

    EM 匹配前统一去尾零，避免同值异形（34 vs 34.00）被误判为答错。
    """
    if "." in s:
        s = s.rstrip("0").rstrip(".")
    return s


def extract_answer(raw: str) -> Optional[str]:
    m = re.search(r"####\s*([-+]?\d+(?:\.\d+)?)", raw)
    return norm_num(m.group(1)) if m else None


def load_model_mlx(spec: ModelSpec, quantize: int = 0):
    """MLX 加载模型 + 本地 tokenizer（绕开 AutoTokenizer 的联网检查）。

    特殊 token 优先用注册表声明（tokens），缺省回退到模型自身的
    tokenizer_config.json 自动读取。chat_template 一并加载：
    instruct 模型必须走 chat 格式才会在答完后输出 eos 停止，
    否则每题白白生成满 max_tokens（评测慢一倍且准确率下降）。

    quantize=8：权重 6GB→3GB，长 prompt 下 decode 约 1.6x 提速（实测
    37→60 tok/s），准确率接近无损；quantize=4 提速 2.7x 但对准确率
    伤害大（实测 0.6→0.2），不推荐。
    """
    from mlx_lm.utils import load_model, quantize_model

    model, _ = load_model(spec.path)
    if quantize:
        model, _ = quantize_model(model, {}, group_size=64, bits=quantize)
        print(f"[load] {quantize}-bit 量化完成（{spec.display()}）", flush=True)
    tok_cfg = spec.tokens
    tcfg = {}
    cfg_path = spec.path / "tokenizer_config.json"
    if cfg_path.exists():
        with open(cfg_path, encoding="utf-8") as fh:
            tcfg = json.load(fh)
    if not tok_cfg:
        tok_cfg = {
            "bos": tcfg.get("bos_token"),
            "eos": tcfg.get("eos_token"),
            "pad": tcfg.get("pad_token"),
            "unk": tcfg.get("unk_token"),
        }
    tok = PreTrainedTokenizerFast(
        tokenizer_file=str(spec.path / "tokenizer.json"),
        bos_token=tok_cfg.get("bos"),
        eos_token=tok_cfg.get("eos"),
        pad_token=tok_cfg.get("pad") or tok_cfg.get("eos"),
        unk_token=tok_cfg.get("unk") or "<unk>",
    )
    tok.chat_template = tcfg.get("chat_template")
    return model, tok


def wrap_chat(tok, content: str) -> str:
    """用模型 chat 模板包裹单轮 user 内容（add_generation_prompt）。

    instruct 模型必须走此格式才会在答完后输出 eos 停止。
    """
    return tok.apply_chat_template(
        [{"role": "user", "content": content}],
        tokenize=False, add_generation_prompt=True,
    )


def summarize(rows, spec: ModelSpec, n: int, no_trunc: bool = False):
    full_acc = sum(1 for r in rows if r["full"] == r["gold"]) / n
    scx_acc = sum(1 for r in rows if r["scx"] == r["gold"]) / n
    trunc_acc = sum(1 for r in rows if r["trunc"] == r["gold"]) / n
    tok_ratio = sum(r["scx_tok"] for r in rows) / sum(r["full_tok"] for r in rows)

    print(f"\n=== GSM8K 8-shot 三方对照（{spec.display()} via MLX, n={n}, seed={SEED}）===")
    print(f"full : acc={full_acc:.3f} ({full_acc*n:.0f}/{n})  ctx_tok={sum(r['full_tok'] for r in rows)//n}")
    print(f"scx  : acc={scx_acc:.3f} ({scx_acc*n:.0f}/{n})  ctx_tok={sum(r['scx_tok'] for r in rows)//n}  tok_ratio={tok_ratio:.3f}")
    if no_trunc:
        print("trunc: 已跳过（--no-trunc）")
    else:
        print(f"trunc: acc={trunc_acc:.3f} ({trunc_acc*n:.0f}/{n})  ctx_tok=同 scx")
    passed = sum(1 for r in rows if r["full"] == r["gold"])
    kept_of_passed = sum(1 for r in rows if r["full"] == r["gold"] and r["scx"] == r["gold"])
    print(f"scx 保持率（full 答对中 scx 仍答对）: {kept_of_passed}/{passed} = {kept_of_passed/max(passed,1):.3f}")
    buckets = {}
    for r in rows:
        cut = 1 - r["scx_tok"] / r["full_tok"] if r["full_tok"] else 0
        b = "cut<2%" if cut < 0.02 else ("2-10%" if cut < 0.10 else ("10-35%" if cut < 0.35 else ">35%"))
        buckets[b] = buckets.get(b, 0) + 1
    print("压缩率分桶（cut = 1 - scx_tok/full_tok）:")
    for b in ["cut<2%", "2-10%", "10-35%", ">35%"]:
        if buckets.get(b):
            print(f"  {b}: {buckets[b]} 样本")
    not_compressed = sum(1 for r in rows if not r["compressed"])
    if not_compressed:
        print(f"注：{not_compressed}/{n} 样本 scx 未压缩（透传/失败），已按 fail-open 计")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--n", type=int, default=100)
    ap.add_argument("--model", default="qwen2.5-3b-instruct",
                    help="模型 id（注册表）或本地路径；默认 qwen2.5-3b-instruct")
    ap.add_argument("--scx-bin", default="target/release/scx")
    ap.add_argument("--csv", default="", help="逐样本明细导出路径（CSV）")
    ap.add_argument("--quantize", type=int, default=0, metavar="BITS",
                    help="权重量化位宽（4/8）：长 prompt 下 4bit≈2.7x、8bit≈1.6x 提速；"
                         "4bit 对准确率伤害大（实测 0.6→0.2），8bit 更稳")
    ap.add_argument("--scx-extra", default="", metavar="ARGS",
                    help="附加 scx 参数（空格分隔，如 '--fill-floor 2'），透传给压缩调用")
    ap.add_argument("--data", default="", metavar="CSV",
                    help="自定义评测集 CSV（列 question,answer；默认 openai/gsm8k）。"
                         "前 --demos 行作 demo，其余作测试集")
    ap.add_argument("--demos", type=int, default=8, metavar="N",
                    help="自定义集的 demo 行数（默认 8）；中文/长 prompt 可调大以超过 scx 透传阈值")
    ap.add_argument("--no-trunc", action="store_true",
                    help="跳过 trunc 第三轨（省 1/3 生成时间）；trunc 基线仅在完整对照需要")
    args = ap.parse_args()

    scx_extra = tuple(args.scx_extra.split()) if args.scx_extra else ()

    random.seed(SEED)
    registry = load_registry()
    spec = resolve_model(registry, args.model)

    print(f"[load] MLX 加载模型 {spec.display()} ({spec.path}) ...", flush=True)
    model, tok = load_model_mlx(spec, quantize=args.quantize)

    if args.data:
        with open(args.data, encoding="utf-8") as fh:
            rows = list(csv.DictReader(fh))
        demos = [{"question": r["question"], "answer": r["answer"]} for r in rows[: args.demos]]
        test = [{"question": r["question"], "answer": r["answer"]} for r in rows[args.demos :]]
        random.shuffle(test)
        test = test[: min(args.n, len(test))]
        print(f"[data] 自定义集 {args.data}: {len(rows)} 行（demo {args.demos} / test {len(test)}）", flush=True)
    else:
        ds = load_dataset("openai/gsm8k", "main")
        # 8-shot demo 用完整答案（含推理链）——LLMLingua-2 论文口径；
        # 只有超过 scx 400-token 透传阈值，压缩才会真正生效。
        demos = [{"question": d["question"], "answer": d["answer"]}
                 for d in ds["train"].select(range(8))]
        test = ds["test"]
        test = test.shuffle(seed=SEED).select(range(min(args.n, len(test))))

    from mlx_lm import generate as mlx_generate

    def run(prompt: str, max_new: int = 192) -> str:
        """greedy 解码，返回生成的文本（不含 prompt 本身）。"""
        return mlx_generate(model, tok, prompt, max_tokens=max_new, verbose=False)

    rows = []
    csv_fh = None
    csv_w = None
    if args.csv:
        csv_fh = open(args.csv, "w", newline="")
        csv_w = csv.DictWriter(csv_fh, fieldnames=["idx", "full", "scx", "trunc", "gold", "full_tok", "scx_tok", "compressed"])
        csv_w.writeheader()
        csv_fh.flush()
    # demo 前缀缓存：8-shot demo 对每题固定，只拼一次（省重复字符串构造）。
    demo_prefix = "".join(f"Q: {d['question']}\nA: {d['answer']}\n\n" for d in demos)
    for i, item in enumerate(test):
        q = item["question"]
        gold = extract_answer(item["answer"])
        prompt = demo_prefix + f"Q: {q}\nA:"

        # 三方统一走 chat 格式：instruct 模型答完即输出 eos 停止，
        # 不会生成多余续文（速度与准确率同时受益）。
        # token 统计口径基于纯文本 prompt（压缩率 = 压缩文本 token 比，
        # trunc 基线按同一口径截断），chat 标记对三方固定、不影响对比。
        ans_full = run(wrap_chat(tok, prompt))
        scx_out = scx_compress(args.scx_bin, prompt, f"Q: {q}", extra=scx_extra)
        scx_tok = len(tok.encode(scx_out))
        full_tok = len(tok.encode(prompt))
        ans_scx = run(wrap_chat(tok, scx_out))
        if args.no_trunc:
            ans_trunc = ""
        else:
            trunc = tok.decode(tok.encode(prompt)[:scx_tok], skip_special_tokens=True)
            ans_trunc = run(wrap_chat(tok, trunc))

        row = {
            "idx": i + 1,
            "full": last_number(ans_full), "scx": last_number(ans_scx),
            "trunc": last_number(ans_trunc), "gold": gold,
            "full_tok": full_tok, "scx_tok": scx_tok,
            "compressed": scx_out != prompt,
        }
        rows.append(row)
        if csv_w is not None:
            csv_w.writerow(row)
            csv_fh.flush()
        if (i + 1) % 5 == 0 or i == 0:
            print(f"[{i+1}/{args.n}]", flush=True)
    if csv_fh is not None:
        csv_fh.close()
        print(f"[csv] 明细已导出: {args.csv}", flush=True)

    summarize(rows, spec, len(rows), no_trunc=args.no_trunc)


if __name__ == "__main__":
    main()
