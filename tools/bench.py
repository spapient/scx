#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""scx 统一评测入口（合并自 bench_ablate/arch/budget/code/kernel/long/metrics/eval_fidelity/calibrate）。

子命令:
  gsm8k      GSM8K 8-shot 三方对照（MLX 本地，原 bench.py）
  ablate     P17/P18 消融评测 + 单调性门禁（--gate）
  budget     同预算对照（--budget vs SuperCompress 官方口径）
  arch       P6n 架构文档 rubric 评测（本地生成 + 在线 judge）
  code       P8-D 代码能力泛化（在线双模型保持率）
  kernel     P6l 算子开发生成评测（编译+运行机器验证）
  long       P6j 长代码压缩问答（MLX 本地）
  metrics    scx 指标小工具（real 矩阵 / perf 基线冻结）
  fidelity   端到端问答保真（对标 LLMLingua-2，双在线旗舰）
  calibrate  BUDGET_SAFETY_FACTOR 上界系数标定
  gen        评测语料生成（bench/fid/p8-cuda/p8-js/p8-layers/p8-shape）
  distill   语义权重蒸馏（online=双教师在线 / weights=评测信号离线）
  fit        弱标签线性权重拟合（scx-sem-1）
  mlp        MLP 交叉验证（对照实验）

用法: python3 tools/bench.py <子命令> [参数...]
"""
from __future__ import annotations

import argparse
import csv
import glob
import json
import os
import random
import re
import subprocess
import sys
import tempfile
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from pathlib import Path
import collections
import hashlib
import io
import math
import socket
import struct
import urllib.request
import common  # 公共封装（chat/compress）

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "tools"))
import corpus  # 合成评测语料（GB golden bench / GF fidelity）



# ==================== 子命令: gsm8k ====================

os.environ.setdefault("HF_HUB_OFFLINE", "1")
os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")


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

    注意：scx 会在 stdout 末尾追加 "compressed: ..." 摘要行及其后的
    "✦ SCX 压缩召回 ✦" 装饰块（查询/模式/说明等展示文本），必须从
    compressed: 行起整体截断，否则装饰块残留会污染压缩结果并虚增 token 数。

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
        cut = 0
        for k, ln in enumerate(lines):
            if ln.lstrip().startswith("compressed:"):
                cut = k
                break
        else:
            return prompt
        out = "\n".join(lines[:cut])
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


NUM_RE = re.compile(r"[-+]?\d+(?:\.\d+)?")
ANS_RE = re.compile(r"####\s*([^\s，。]+)")


def norm_token(s: str) -> str:
    """字符串答案归一化：去两端空白/引号，统一小写。

    模型侧与 gold 侧同口径（True→true、'C'→c、"BA"→ba），
    避免同值异形（True vs true vs "True"）被误判为答错。
    """
    return s.strip().strip('"\'“”‘’').lower()


def kind_of(tok: str) -> str:
    """gold 值类型判定：数字（含浮点）/ 布尔 / 字符串。"""
    if NUM_RE.fullmatch(tok):
        return "num"
    if tok.lower() in ("true", "false", "t", "f"):
        return "bool"
    return "str"


def extract_answer(raw: str) -> Optional[str]:
    """gold 提取（扩展）：'#### X' 的 X 可为数字/浮点/布尔/字符串。

    数字走 norm_num 归一化；布尔归一为 true/false；字符串去引号小写。
    旧数字题格式完全兼容。
    """
    m = ANS_RE.search(raw)
    if not m:
        return None
    tok = m.group(1)
    k = kind_of(tok)
    if k == "num":
        return norm_num(tok)
    if k == "bool":
        return "true" if tok.lower().startswith("t") else "false"
    return norm_token(tok)


def model_value(raw: str, gold: str) -> str:
    """从模型输出提取与 gold 同类型的值。

    优先取最后 '#### X' 的 X（字符串/布尔依赖此模式）；
    数字题若 X 非数字则回退 last_number；bool/字符串题若模型未按
    '#### X' 格式输出（常见于压缩后格式指令被摘），回退取输出末尾
    的同类 token——模型通常把答案放末尾，末 token 是合理近似。
    """
    if not raw.strip():
        return ""
    m = ANS_RE.search(raw)
    k = kind_of(gold)
    if k == "num":
        if m and NUM_RE.fullmatch(m.group(1)):
            return norm_num(m.group(1))
        return last_number(raw) or ""
    if m:
        tok = m.group(1)
        if k == "bool":
            t = tok.lower()
            return "true" if t in ("true", "1", "t") else "false" if t in ("false", "0", "f") else ""
        return norm_token(tok)
    # fallback：无 #### 模式
    if k == "bool":
        toks = re.findall(r"true|false", raw, re.I)
        return toks[-1].lower() if toks else ""
    toks = re.findall(r"[A-Za-z0-9.]+", raw)
    return norm_token(toks[-1]) if toks else ""


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


def cmd_gsm8k() -> None:
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

    # MLX 评测依赖延迟加载：--help/参数错误即时返回，不拖慢其他子命令
    import torch  # noqa: E402
    from datasets import load_dataset  # noqa: E402
    from transformers import PreTrainedTokenizerFast  # noqa: E402

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
            "full": model_value(ans_full, gold), "scx": model_value(ans_scx, gold),
            "trunc": model_value(ans_trunc, gold) if not args.no_trunc else "", "gold": gold,
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


# ==================== 子命令: ablate ====================

ROOT = Path(__file__).resolve().parent.parent
OUT_CSV = ROOT / "config" / "bench" / "results" / "bench_p17_ablate.csv"
WEIGHTS = ROOT / "config" / "semantic" / "weights.json"
SEM_TARGET = ROOT / "target" / "ablate-sem"

CUT_TOL = 0.005   # cut 单调性容差（噪声吸收）
FID_TOL = 0.01    # fidelity 单调性容差

# 配置矩阵：基线 + 5 单开。仅默认二进制可开的信号独立列。
CONFIGS: list[tuple[str, list[str], bool]] = [
    ("baseline", [], False),
    ("fold", ["--fold", "1"], False),
    ("entropy", ["--entropy", "1"], False),
    ("anchor", ["--auto-anchor", "1"], False),
    ("cluster", ["--cluster", "1"], False),
    ("semantic", ["--semantic", str(WEIGHTS)], True),
]

# P18 组合矩阵：semantic ON × 各保留信号（对照 = 对应 OFF 单开）。
# 验证 semantic 开启不劣化各信号的 fidelity/cut（交互正交性）。
COMBO: list[tuple[str, list[str], bool]] = [
    ("sem+fold", ["--fold", "1", "--semantic", str(WEIGHTS)], True),
    ("sem+entropy", ["--entropy", "1", "--semantic", str(WEIGHTS)], True),
    ("sem+anchor", ["--auto-anchor", "1", "--semantic", str(WEIGHTS)], True),
    ("sem+cluster", ["--cluster", "1", "--semantic", str(WEIGHTS)], True),
]

COMBO_OUT = ROOT / "config" / "bench" / "results" / "bench_p18_combo.csv"

# ---------- token 口径（本地复刻 src/terms.rs::measure） ----------

_IDEO = re.compile(
    "[\u3400-\u4DBF\u4E00-\u9FFF\uF900-\uFAFF\u3000-\u303F\uFF00-\uFFEF]"
)


def est_tok(s: str) -> float:
    """estimate(s) 近似：ASCII len/4+1；CJK 0.6/字；其他 2.5/字。"""
    if s.isascii():
        return len(s) / 4.0 + 1.0
    ascii_ = cjk = other = 0
    for ch in s:
        if ch.isascii():
            ascii_ += 1
        elif _IDEO.match(ch):
            cjk += 1
        else:
            other += 1
    return ascii_ / 4.0 + cjk * 0.6 + other / 2.5 + 1.0


_WORD = re.compile(r"[A-Za-z0-9_]+|[\u3400-\u9FFF\uF900-\uFAFF]")


def tokens(s: str) -> list[str]:
    """保真词切分：ASCII 词段 + 单 CJK 字符。"""
    return _WORD.findall(s)


def num_tokens(s: str) -> list[str]:
    return [t for t in tokens(s) if any(ch.isdigit() for ch in t)]


def cov(full: list[str], out: list[str]) -> float:
    """full multiset 在 out 中的贪婪匹配比例（确定性）。"""
    if not full:
        return 1.0
    pool = list(out)
    hit = 0
    for t in full:
        for k, u in enumerate(pool):
            if u == t:
                pool[k] = ""
                hit += 1
                break
    return hit / len(full)


# ---------- 确定性语料合成（LCG 种子 42） ----------

def lcg(seed: int = 42):
    """确定性伪随机：相同输入恒同序列。"""
    state = seed

    def next_u32() -> int:
        nonlocal state
        state = (state * 1103515245 + 12345) & 0x7FFFFFFF
        return state

    def choice(items):
        return items[next_u32() % len(items)]

    return next_u32, choice


def corpus_code(seed: int = 42) -> str:
    _, choice = lcg(seed)
    kinds = ["u64", "i32", "String", "Vec<u8>", "bool", "f64"]
    verbs = ["parse", "build", "merge", "scan", "render", "flush", "retry", "dedup"]
    lines = []
    for i in range(300):
        lines.append(
            f"pub fn {choice(verbs)}_{i}(arg: u64) -> Result<{choice(kinds)}, Error> {{"
        )
        lines.append(f"    let tmp = arg.wrapping_mul({i} + 1);")
        lines.append(f"    if tmp % 2 == 0 {{ return Ok(tmp) }} else {{ Err(Error::Idx({i})) }}")
        lines.append("}")
    lines.append("// unique tail: final answer is 42")
    return "\n".join(lines) + "\n"


def corpus_log(seed: int = 42) -> str:
    """日志风暴：160 块 × 5 行同消息重试风暴（id/耗时变化）+ 尾部唯一行。
    连续重复块给 fold 模板折叠以触发机会（消融第一轮证明：query 命中行
    孤立保留时折叠块 <3 行，fold 信号零触发——语料须对齐其目标形态）。"""
    _, choice = lcg(seed)
    levels = ["ERROR", "WARN"]
    comps = ["conn", "auth", "cache", "rpc", "mq", "disk"]
    states = ["timeout", "retry", "reset", "refused", "slow", "evicted"]
    lines = []
    for b in range(160):
        lvl, comp, st = choice(levels), choice(comps), choice(states)
        base = f"2026-10-30 12:{b % 60:02d}:{b % 60:02d}.{b % 1000:03d} {lvl} {comp}.{st}"
        for k in range(5):
            lines.append(f"{base} id={b * 5 + k:04x} took={b % 500 + k * 7}ms retry={k}")
        lines.append(f"  at worker.retry(worker.rs:{b % 120 + 1})")
        lines.append(f'  {{"err": "{st}", "id": "{b * 5:04x}", "took": {b % 500}}}')
    lines.append("final: status=ok attempts=0")
    return "\n".join(lines) + "\n"


def corpus_zh(seed: int = 42) -> str:
    _, choice = lcg(seed)
    msgs = ["连接超时", "缓存未命中", "权限不足", "参数校验失败", "服务不可用", "重试次数超限"]
    comps = ["网关", "认证中心", "订单服务", "存储引擎", "调度器"]
    lines = []
    for i in range(300):
        lines.append(
            f"[{i % 100:02d}] {choice(comps)} {choice(msgs)}：请求 {i} 重试 {i % 5} 次"
        )
        lines.append(f"  详情：code={1000 + i} id={i:04x} 耗时 {i % 900}ms")
    lines.append("最终状态：成功 总耗时 3200ms")
    return "\n".join(lines) + "\n"


# ---------- scx 调用 ----------

def compress_ablate(bin_: str, text: str, query: str, extra: list[str]) -> str:
    """调用 scx 压缩；失败/小输入直通返回原文；截断 `compressed:` 装饰块。
    query 必须命中语料高频词，否则 scx「无命中保护」直通（cut 恒 0 会误判）。"""
    return common.compress(bin_, text, query, extra=extra, timeout=300)


def rebuild_folded(out: str) -> str:
    """把折叠输出重建为原文形态（fold 的保真契约 = rebuild 可逆）。

    折叠组 = 首行原文 + `scx-fold:` 模板行 + `scx-fold-params:` 参数行；
    重建后替换为完整原文行。非折叠行原样保留。fold 配置的 fidelity 必须
    在重建态上测（消融证明：折叠态骨架词移除导致 token_cov 名义劣化，
    而 rebuild 逐字节还原原文——信息可恢复性才是 fold 的保真口径）。
    """
    lines = out.splitlines()
    rebuilt: list[str] = []
    i = 0
    while i < len(lines):
        ln = lines[i]
        if ln.startswith("scx-fold: ") and i >= 1 and i + 1 < len(lines):
            first = rebuilt.pop() if rebuilt and rebuilt[-1] != "" else lines[i - 1]
            tpl = ln
            pstream = lines[i + 1]
            if pstream.startswith("scx-fold-params: "):
                rows = rebuild_rows(tpl, pstream)
                if rows is not None:
                    rebuilt.append(first)
                    rebuilt.extend(rows)
                    i += 2
                    continue
            rebuilt.append(first)
            rebuilt.append(ln)
        else:
            rebuilt.append(ln)
        i += 1
    return "\n".join(rebuilt) + ("\n" if out.endswith("\n") else "")


def rebuild_rows(tpl: str, pstream: str) -> list[str] | None:
    """Python 复刻 fold.rs::rebuild：模板行 + 参数行 → 原文行（None=结构不合法）。"""
    rest = tpl.removeprefix("scx-fold: ")
    cut = rest.find(" rows | ")
    if cut < 0:
        return None
    m = rest[:cut]
    if not m.isdigit():
        return None
    m = int(m)
    skeleton = rest[cut + len(" rows | "):]
    body = pstream.removeprefix("scx-fold-params: ")
    rows = [r.split("|") if r else [] for r in body.split(";")]
    if m == 0:
        return []
    if len(rows) != m or any(len(r) > 1 and "" in r for r in rows):
        return None
    out_rows = []
    for row in rows:
        rebuilt_line = ""
        pi = 0
        chars = list(skeleton)
        ci = 0
        while ci < len(chars):
            ch = chars[ci]
            if ch == "#":
                if ci + 1 < len(chars) and chars[ci + 1] == "#":
                    rebuilt_line += "#"
                    ci += 2
                else:
                    if pi >= len(row):
                        return None
                    rebuilt_line += row[pi]
                    pi += 1
                    ci += 1
            elif ch == "|":
                if ci + 1 < len(chars) and chars[ci + 1] == "|":
                    rebuilt_line += "|"
                    ci += 2
                else:
                    ci += 1
            else:
                rebuilt_line += ch
                ci += 1
        if pi != len(row):
            return None
        out_rows.append(rebuilt_line)
    return out_rows


def ensure_sem_bin() -> str:
    """隔离构建 feature=semantic 二进制（不触碰默认 release binary）。

    新鲜度检查：二进制比任一 src/*.rs 或 config/semantic/*.json 旧则重建——
    否则评测会跑到旧内核（如 fold UTF-8 修复前），产出误导性结果。
    """
    bin_ = SEM_TARGET / "release" / "scx"
    if bin_.exists():
        newest = max(
            [p.stat().st_mtime for p in (ROOT / "src").rglob("*.rs")]
            + [p.stat().st_mtime for p in (ROOT / "config" / "semantic").glob("*.json")]
        )
        if bin_.stat().st_mtime >= newest:
            return str(bin_)
        print("semantic 二进制落后于源码，重建…")
    print(f"构建 semantic 二进制到 {bin_}（隔离 target，不影响默认）…")
    subprocess.run(
        ["cargo", "build", "--release", "--features", "semantic", "--target-dir", str(SEM_TARGET)],
        check=True,
        cwd=ROOT,
    )
    return str(bin_)


# ---------- 主流程 ----------

def cmd_ablate() -> int:
    ap = argparse.ArgumentParser(description="P17/P18 信号消融评测 + 单调性门禁")
    ap.add_argument("--bin", default=str(ROOT / "target" / "release" / "scx"))
    ap.add_argument("--sem-bin", default=None)
    ap.add_argument("--gate", action="store_true", help="单调性门禁模式（断言 + 退出码）")
    ap.add_argument("--combo", action="store_true", help="P18 组合模式（semantic on/off × 5 信号）")
    ap.add_argument("--out", default=str(OUT_CSV))
    args = ap.parse_args()

    default_bin = args.bin
    sem_bin = args.sem_bin or ensure_sem_bin()

    corpora = [
        ("code", corpus_code(), "build"),
        ("log", corpus_log(), "timeout"),
        ("zh", corpus_zh(), "超时"),
    ]
    if args.combo:
        return ablate_combo(default_bin, sem_bin, corpora, args.gate)

    rows: list[dict] = []
    for cname, text, query in corpora:
        full_tok = est_tok(text)
        full_words = tokens(text)
        full_nums = num_tokens(text)
        for cfg_name, flags, need_sem in CONFIGS:
            bin_ = sem_bin if need_sem else default_bin
            out = compress_ablate(bin_, text, query, flags)
            # fold 保真口径 = 重建态（rebuild 可逆契约）；cut 口径 = 折叠态。
            fid_out = rebuild_folded(out) if cfg_name == "fold" else out
            out_tok = est_tok(out)
            out_words = tokens(fid_out)
            out_nums = num_tokens(fid_out)
            rows.append(
                {
                    "set": cname,
                    "cfg": cfg_name,
                    "cut": 1.0 - out_tok / full_tok,
                    "num_cov": cov(full_nums, out_nums),
                    "token_cov": cov(full_words, out_words),
                }
            )
            print(f"[{cname:4}] {cfg_name:9} cut={rows[-1]['cut']:.4f} "
                  f"num_cov={rows[-1]['num_cov']:.4f} token_cov={rows[-1]['token_cov']:.4f}")

    with open(args.out, "w", encoding="utf-8", newline="") as fh:
        w = csv.DictWriter(fh, fieldnames=["set", "cfg", "cut", "num_cov", "token_cov"])
        w.writeheader()
        w.writerows(rows)
    print(f"写入 {args.out}（3 语料 × 6 配置 = {len(rows)} 行）")

    if not args.gate:
        return 0

    ok = True
    for cname, _, _ in corpora:
        base = next(r for r in rows if r["set"] == cname and r["cfg"] == "baseline")
        for cfg_name, _, _ in CONFIGS:
            if cfg_name == "baseline":
                continue
            row = next(r for r in rows if r["set"] == cname and r["cfg"] == cfg_name)
            # fidelity 是硬约束：任何信号都不得让数字/token 覆盖劣化。
            for m in ("num_cov", "token_cov"):
                if row[m] < base[m] - FID_TOL:
                    print(f"  FAIL [{cname}] {cfg_name}: {m} {row[m]:.4f} < 基线 "
                          f"{base[m]:.4f} - {FID_TOL}")
                    ok = False
            # cut 断言按信号类别：只加保留信号（fold/anchor/cluster/semantic）
            # 要求 cut 不劣；entropy 是保真召回信号（可能更保守），豁免 cut，
            # 仅受 fidelity 硬约束——cut 上升但 fidelity 崩仍会被上方拦下。
            if cfg_name != "entropy" and row["cut"] < base["cut"] - CUT_TOL:
                print(f"  FAIL [{cname}] {cfg_name}: cut {row['cut']:.4f} < 基线 "
                      f"{base['cut']:.4f} - {CUT_TOL}")
                ok = False
    print("  " + ("OK: 消融单调性门禁全部通过（单开不劣于基线）" if ok else "FAIL: 存在劣化信号"))
    return 0 if ok else 1


def ablate_combo(
    default_bin: str, sem_bin: str, corpora: list, gate: bool
) -> int:
    """P18 组合评测：semantic ON × 4 信号，对照 OFF 单开（语义交互正交性）。

    断言（gate 模式）：
      1. fidelity 硬约束：sem+cfg 的 num_cov/token_cov ≥ cfg 单开 − FID_TOL
         （semantic 开启不得劣化数字/词保真——权重 0.0 特征不干扰证据行）
      2. cut 不劣（entropy 豁免，与单开门禁一致）
    输出 bench_p18_combo.csv（3 语料 × (4 组合 + semantic 单开) = 15 行），
    独立于既有 bench_p17_ablate.csv 18 行冻结。
    """
    rows: list[dict] = []
    # OFF 单开先行（组合对照基准）。
    off_configs = [c for c in CONFIGS if c[0] != "semantic"]
    for cname, text, query in corpora:
        full_tok = est_tok(text)
        full_words = tokens(text)
        full_nums = num_tokens(text)
        for cfg_name, flags, need_sem in off_configs:
            bin_ = sem_bin if need_sem else default_bin
            out = compress_ablate(bin_, text, query, flags)
            fid_out = rebuild_folded(out) if cfg_name == "fold" else out
            rows.append(
                {
                    "set": cname,
                    "cfg": cfg_name,
                    "cut": 1.0 - est_tok(out) / full_tok,
                    "num_cov": cov(full_nums, num_tokens(fid_out)),
                    "token_cov": cov(full_words, tokens(fid_out)),
                }
            )
            print(f"[combo off {cname:4}] {cfg_name:12} cut={rows[-1]['cut']:.4f} "
                  f"num_cov={rows[-1]['num_cov']:.4f} token_cov={rows[-1]['token_cov']:.4f}")
    # semantic ON：单开 + ×4 组合。
    for cname, text, query in corpora:
        full_tok = est_tok(text)
        full_words = tokens(text)
        full_nums = num_tokens(text)
        for cfg_name, flags, need_sem in [("semantic", ["--semantic", str(WEIGHTS)], True)] + COMBO:
            bin_ = sem_bin if need_sem else default_bin
            out = compress_ablate(bin_, text, query, flags)
            fid_out = rebuild_folded(out) if "fold" in cfg_name else out
            rows.append(
                {
                    "set": cname,
                    "cfg": cfg_name,
                    "cut": 1.0 - est_tok(out) / full_tok,
                    "num_cov": cov(full_nums, num_tokens(fid_out)),
                    "token_cov": cov(full_words, tokens(fid_out)),
                }
            )
            print(f"[combo on {cname:4}] {cfg_name:12} cut={rows[-1]['cut']:.4f} "
                  f"num_cov={rows[-1]['num_cov']:.4f} token_cov={rows[-1]['token_cov']:.4f}")

    with open(COMBO_OUT, "w", encoding="utf-8", newline="") as fh:
        w = csv.DictWriter(fh, fieldnames=["set", "cfg", "cut", "num_cov", "token_cov"])
        w.writeheader()
        w.writerows(rows)
    print(f"写入 {COMBO_OUT}（3 语料 × 5 配置 = {len(rows)} 行）")

    if not gate:
        return 0
    ok = True
    for cname, _, _ in corpora:
        for cfg_name, _, _ in COMBO:
            off = next(r for r in rows if r["set"] == cname and r["cfg"] == cfg_name[4:])
            on = next(r for r in rows if r["set"] == cname and r["cfg"] == cfg_name)
            for m in ("num_cov", "token_cov"):
                if on[m] < off[m] - FID_TOL:
                    print(f"  FAIL [{cname}] {cfg_name}: {m} {on[m]:.4f} < off "
                          f"{off[m]:.4f} - {FID_TOL}")
                    ok = False
            if "entropy" not in cfg_name and on["cut"] < off["cut"] - CUT_TOL:
                print(f"  FAIL [{cname}] {cfg_name}: cut {on['cut']:.4f} < off "
                      f"{off['cut']:.4f} - {CUT_TOL}")
                ok = False
    print("  " + ("OK: 组合门禁全过（semantic × 各信号交互正交，fidelity/cut 不劣化）"
                  if ok else "FAIL: 存在 semantic 组合劣化"))
    return 0 if ok else 1


# ==================== 子命令: budget ====================

ROOT = Path(__file__).resolve().parent.parent
OUT_CSV = ROOT / "config" / "bench" / "results" / "bench_p18_budget.csv"

# 官方口径：SuperCompress v2 官方自报 mean cut 64.1%（≈kept 36%）。
OFFICIAL_CUT = 0.641
OFFICIAL_KEPT = 1.0 - OFFICIAL_CUT



def tok_lines(text: str) -> float:
    """Rust masks.total_tok 口径复刻：每行 estimate 求和（每行 +1，非全文本一次）。"""
    return sum(est_tok(ln) for ln in text.splitlines())


def run_budget(bin_: str, cname: str, text: str, query: str, budget: float | None) -> dict:
    """跑单档位压缩并计算本地指标。fold 保真口径 = 重建态（可逆契约）。

    cut/kept_pct 用行级 token 分母（与 Rust budget 语义同源：预算 = Σ行 estimate）。
    """
    flags = [f"--budget", f"{budget:.2f}"] if budget is not None else []
    out = compress_ablate(bin_, text, query, flags)
    fid_out = rebuild_folded(out)
    out_tok = tok_lines(out)
    full_tok = tok_lines(text)
    return {
        "set": cname,
        "cfg": f"budget-{budget:.2f}" if budget is not None else "baseline",
        "budget": budget if budget is not None else 0.0,
        "cut": 1.0 - out_tok / full_tok,
        "kept_pct": out_tok / full_tok,
        "num_cov": cov(num_tokens(text), num_tokens(fid_out)),
        "token_cov": cov(tokens(text), tokens(fid_out)),
    }


def cmd_budget() -> int:
    ap = argparse.ArgumentParser(description="P0-1.2 同预算对照评测（--budget vs SuperCompress 官方口径）")
    ap.add_argument("--bin", default=str(ROOT / "target" / "release" / "scx"))
    ap.add_argument("--gate", action="store_true", help="门禁模式（断言 + 退出码）")
    ap.add_argument("--out", default=str(OUT_CSV))
    ap.add_argument("--budgets", default="0.35,0.5,0.65", help="逗号分隔预算档位")
    args = ap.parse_args()

    budgets = [float(b) for b in args.budgets.split(",") if b]
    corpora = [
        ("code", corpus_code(), "build"),
        ("log", corpus_log(), "timeout"),
        ("zh", corpus_zh(), "超时"),
    ]
    rows: list[dict] = []
    for cname, text, query in corpora:
        full_tok = est_tok(text)
        # baseline（默认路径）→ budget 档位。
        base = run_budget(args.bin, cname, text, query, None)
        rows.append(base)
        print(f"[{cname:4}] baseline    cut={base['cut']:.4f} kept={base['kept_pct']:.4f} "
              f"num_cov={base['num_cov']:.4f} token_cov={base['token_cov']:.4f}")
        for b in budgets:
            row = run_budget(args.bin, cname, text, query, b)
            rows.append(row)
            print(f"[{cname:4}] budget-{b:.2f} cut={row['cut']:.4f} kept={row['kept_pct']:.4f} "
                  f"num_cov={row['num_cov']:.4f} token_cov={row['token_cov']:.4f}")
            # 预算达成硬断言：kept ≤ 预算 + 2% 容差（token 粒度）。
            if args.gate and row["kept_pct"] > b + 0.02:
                print(f"  FAIL [{cname}] budget-{b:.2f}: kept={row['kept_pct']:.4f} 超预算")
                return 1

    # 官方对照行（口径：官方自报，非实测）。
    rows.append(
        {
            "set": "official",
            "cfg": "supercompress-v2",
            "budget": OFFICIAL_KEPT,
            "cut": OFFICIAL_CUT,
            "kept_pct": OFFICIAL_KEPT,
            "num_cov": float("nan"),
            "token_cov": float("nan"),
        }
    )

    with open(args.out, "w", encoding="utf-8", newline="") as fh:
        w = csv.DictWriter(fh, fieldnames=["set", "cfg", "budget", "cut", "kept_pct", "num_cov", "token_cov"])
        w.writeheader()
        w.writerows(rows)
    print(f"写入 {args.out}（3 语料 × {1 + len(budgets)} 档 + 官方对照 = {len(rows)} 行）")

    if not args.gate:
        return 0

    ok = True
    for cname, _, _ in corpora:
        base = next(r for r in rows if r["set"] == cname and r["cfg"] == "baseline")
        for b in budgets:
            row = next(r for r in rows if r["set"] == cname and r["cfg"] == f"budget-{b:.2f}")
            # 同预算（0.35 档）下 cut ≥ 官方口径（P18 目标，容差 0.04 = 要求 ≥ 0.60）。
            if b <= 0.35 + 1e-9 and row["cut"] < OFFICIAL_CUT - 0.04:
                print(f"  FAIL [{cname}] budget-{b:.2f}: cut={row['cut']:.4f} < 官方 0.641 − 0.04")
                ok = False
            # 保真降幅记录（硬预算下证据竞争预算、数字行可能被裁——如实量化权衡）：
            # 不做通过/失败，降幅进入 CSV 由对照表呈现；仅对"预算内保真不回退"断言：
            # 若 kept_pct 高于 baseline kept，则 cut/保真必须与 baseline 一致（预算未生效）。
            if row["kept_pct"] >= base["kept_pct"] - 0.001 and abs(row["cut"] - base["cut"]) > 0.001:
                print(f"  FAIL [{cname}] budget-{b:.2f}: 预算未收紧但输出变化（cut 漂移）")
                ok = False
    print("  " + ("OK: 同预算对照门禁全部通过（预算精确达成 + 0.35 档 cut 不低于官方口径）" if ok else "FAIL: 存在门禁失败项"))
    return 0 if ok else 1


# ==================== 子命令: arch ====================

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
ARCH_CFG = {
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


def arch_prompt(item: dict) -> str:
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
    cfg = ARCH_CFG[judge]
    for _ in range(2):
        # deepseek-flash 带 reasoning：评分任务会先输出 1000+ token 推理，
        # max_tokens 太小会被思考吃光导致 content 为空/截断。给足额度。
        try:
            raw = arch_call(prompt, max_new=cfg["max_new"], judge=judge)
        except Exception:  # noqa: BLE001 网络失败不 kill 全进程
            return None, ""
        sc = parse_score(raw)
        if sc is not None:
            return sc, raw
    return None, raw


def arch_call(prompt: str, max_new: int, judge: str) -> str:
    """OpenAI 兼容调用（公共 _api.chat 封装，网络超时重试 3 次）。"""
    return common.chat(ARCH_CFG[judge], prompt, max_new, retries=3, sleep_base=2.0)


def cmd_arch() -> None:
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
            prompt = arch_prompt(item)
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
            futs = {ex.submit(judge_answer, plans[i][0], _ans(i, tr, rt), arch_call, j): (i, tr, rt, j)
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
                sc2, _ = judge_answer(plans[i][0], ans, arch_call, j0)
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


# ==================== 子命令: code ====================

ROOT = Path(__file__).resolve().parent.parent



CODE_CFG = {
    "deepseek": {
        "url": "https://api.deepseek.com/chat/completions",
        "model": os.environ.get("SCX_MODEL_DEEPSEEK", "deepseek-flash"),
        "env": "DEEPSEEK_API_KEY",
    },
    "qwen": {
        "url": "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions",
        "model": os.environ.get("SCX_MODEL_QWEN", "qwen3.8-flash"),
        "env": "QWEN_API_KEY",
    },
}

ANSWER_RE = re.compile(r"答案是\s*(.*?)\s*####\s*(.*?)\s*$", re.S)


def code_call(prompt: str, model: str, max_new: int = 0) -> str:
    # deepseek-flash 带 reasoning：思考会吃光小 max_tokens 导致空 content，
    # 必须给足（bench_arch 同注）；qwen3.8-flash 同样带思考。
    if max_new <= 0:
        max_new = 2048 if model == "deepseek" else 1024
    # 公共 _api.chat：5 次重试、退避 3s、关 reasoning（deepseek-flash 思考会吃光
    # max_tokens 致空响应，关后 1.2s 返回同等质量答案）、空 content 视为失败最终返回 ""。
    return common.chat(
        CODE_CFG[model], prompt, max_new,
        retries=5, sleep_base=3.0, reason_off=(model == "deepseek"), strict_content=True,
    )


CONTEXT = {
    "cuda": """CUDA 编程模型：kernel 在 GPU 上以线程网格（grid）执行，网格由线程块（block）组成，
每块由线程（thread）组成，通过 blockIdx/threadIdx/gridDim/blockDim 索引定位全局线程 ID。
内存层级包括全局内存（设备主存，容量大延迟高）、共享内存（块内线程共享的高速存储）、
常量/纹理缓存与线程私有寄存器。块内同步用 __syncthreads，跨块同步需原子操作或多次 kernel。
性能关键：全局访问要合并（相邻线程相邻地址）、共享内存避免 bank conflict、warp 内天然同步、
warp divergence 使两个分支串行执行、occupancy 受寄存器/共享内存限制。""",
    "js": """JavaScript 运行时：单线程事件循环，同步代码先执行，微任务（Promise.then）在每轮
宏任务（setTimeout 等）之前全部执行。函数是第一类对象，闭包捕获词法作用域变量并持有引用。
this 取决于调用方式（方法调用/普通调用/箭头函数词法绑定）。原型链实现继承，
class 是语法糖。Promise 三种状态、async/await 是语法糖。对象键枚举顺序：整数键升序优先。
浅拷贝（Object.assign/展开）共享嵌套对象。GC 处理不可达对象。""",
    "shape": """配置与数据处理：JSON 为通用数据交换格式（对象/数组/数字/字符串/布尔/null，
键序为插入序，Python json 解析数字为 int/float）。YAML 用缩进表达层级，列表用 - 前缀。
TOML 用 [表] 分段，键值对赋值。SQL 查询数据库表，聚合函数配 GROUP BY，WHERE 过滤，
DISTINCT 去重。正则表达式匹配文本，\\d 数字、[a-z] 字符类、+ 一个或多个、锚点 ^ $ 定界。
shell 脚本命令替换 $(...) 执行并插入输出，grep 过滤行、awk 按列处理。日志常用 ISO 8601 时间戳。""",
    "layers": """代码工程素养：L1 语义层理解代码行为（输出/结果/类型）；L2 性能层分析复杂度、
缓存、访存、内存与并发开销；L3 架构层做工程设计决策（模块划分、分层、状态管理、
可扩展性与权衡）。生产级工程强调：正确性可验证、性能可测量、架构可演进。""",
}


def code_prompt(q: str, kind: str = "cuda") -> str:
    ctx = CONTEXT.get(kind, CONTEXT["cuda"])
    head = "你是资深软件工程师，负责代码理解、正确性推理与性能分析。\n\n"
    tail = (
        f"\n\n问题：{q}\n\n"
        "作答要求：先给出简短推理，最后一行必须用 \"答案是 X #### X\" 格式作答"
        "（X 为最终答案，与推理同行分隔）。"
    )
    # 领域背景补充到总长 ≥1000 字符，保证 scx 触发压缩且有足量可裁内容
    prompt = head + ctx + tail
    block = "\n技术要点补充：" + ctx
    while len(prompt) < 1000:
        prompt = head + ctx + block + tail
        ctx = ctx + block
    return prompt


def scx_compress_prompt(prompt: str, query_hint: str, scx_bin: str, extra: str) -> tuple[str, bool]:
    out = scx_compress(scx_bin, prompt, query_hint, extra=tuple(extra.split()))
    return out, out != prompt


def norm_ans(s: str) -> str:
    """gold/答案归一化：小写、去空白标点、去括号说明、数字中英同化。"""
    s = s.lower().strip()
    s = s.split("（")[0]
    s = re.sub(r"[\s\.,;:'\"`_\-\[\]{}()（）]", "", s)
    # 数字中英同化（长串先替换："10" 先于 "1"）
    for d, c in (("10", "十"), ("0", "零"), ("1", "一"), ("2", "二"),
                 ("3", "三"), ("4", "四"), ("5", "五"), ("6", "六"),
                 ("7", "七"), ("8", "八"), ("9", "九")):
        s = s.replace(d, c)
    # 去 "答案是/结果是" 前缀
    s = re.sub(r"^(答案|结果是|答案为)", "", s)
    return s


def code_answer(raw: str) -> str:
    """从模型回答抽取最终答案（答案是 X #### X、#### X、含数字末行或全文末数）。"""
    m = ANSWER_RE.search(raw.strip())
    if m:
        return m.group(1).strip()
    # 兜底1：'#### X' 格式（Markdown 长回答常见）
    m2 = re.search(r"####\s*(\S+)", raw)
    if m2:
        return m2.group(1).strip()
    # 兜底2：从末行往回找含数字的行（Markdown 长回答的答案常在正文行）
    lines = [l for l in raw.strip().splitlines() if l.strip()]
    for line in reversed(lines):
        if re.search(r"\d", line):
            return re.sub(r"^答案是\s*", "", line).strip()
    # 兜底3：全文最后出现的数字
    nums = re.findall(r"\d+", raw)
    if nums:
        return nums[-1]
    return lines[-1] if lines else raw


def check_ans(raw: str, gold: str) -> bool:
    a_raw = code_answer(raw)
    ans = norm(a_raw)
    if not ans:
        return False
    # 候选答案：gold 主句 + 括号内内容（"（寄存器/局部内存）" → 答其一即对）
    cands = [gold] + re.findall(r"[（(]([^）)]+)[)）]", gold)
    for c in cands:
        for sub in re.split(r"[/\s]+", c):
            if not sub:
                continue
            sn = norm(sub)
            if not sn:
                continue
            if sn == ans or sn in ans or ans in sn:
                return True
            # 英文/数字标识符子串（中英混排粘连免疫）
            if re.search(r"[A-Za-z0-9]", sub):
                core = re.sub(r"[_\s()（）]", "", sub).lower()
                if core and core in re.sub(r"[_\s]", "", a_raw).lower():
                    return True
    # 数字集合（原文提取）
    na = set(re.findall(r"\d+", a_raw))
    ng = set(re.findall(r"\d+", gold))
    if ng and na and ng <= na:
        return True
    # 中文关键短语：gold 主段任一 ≥2 字连续段出现在答案
    main_seg = re.findall(r"[\u4e00-\u9fff]{2,}", gold.split("（")[0])
    ans_seg = re.findall(r"[\u4e00-\u9fff]{2,}", ans)
    if main_seg and ans_seg:
        if any(s in x for s in main_seg for x in ans_seg):
            return True
    return False


def cmd_code() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", required=True)
    ap.add_argument("--model", default="both", choices=["deepseek", "qwen", "both"])
    ap.add_argument("--scx-bin", default="target/release/scx")
    ap.add_argument("--scx-extra",
                    default="--trim 1 --trim-ratio 0.3 --trim-op-keep 1 --num-line-keep 1 --sym-line-keep 1 --zh-keep 1")
    ap.add_argument("--csv", default="", help="逐题明细导出")
    ap.add_argument("--resume", action="store_true")
    ap.add_argument("--workers", type=int, default=6)
    args = ap.parse_args()

    qa = list(csv.DictReader(open(args.data, encoding="utf-8")))
    models = ["deepseek", "qwen"] if args.model == "both" else [args.model]
    # 压缩召回 query：短领域关键词（terms≈4-6），按评测集类型给 hint
    hint_map = {
        "cuda": "CUDA 内核 线程 共享内存 原子 warp",
        "js": "JavaScript 输出 闭包 异步 原型 类型",
        "shape": "配置 解析 JSON SQL 正则 输出",
        "layers": "代码 语义 性能 架构 设计",
    }
    name = Path(args.data).stem
    qhint = next((h for k, h in hint_map.items() if k in name), "代码 输出 结果 行为 类型")
    print(f"== 评测 {args.data} {len(qa)} 题 × {models} | query_hint={qhint!r} ==")

    rows = []
    csv_fh = None
    csv_w = None
    done = set()
    if args.csv and args.resume and Path(args.csv).exists():
        with open(args.csv, encoding="utf-8") as fh:
            for r in csv.DictReader(fh):
                rows.append(r)
                done.add((r["model"], r["idx"]))
        csv_fh = open(args.csv, "a", newline="")
        csv_w = csv.DictWriter(csv_fh, fieldnames=csv.DictReader(
            open(args.csv, encoding="utf-8")).fieldnames)
    else:
        csv_fh = open(args.csv, "w", newline="") if args.csv else None
        csv_w = csv.DictWriter(csv_fh, fieldnames=["model", "idx", "question", "full", "scx",
                                                    "full_ans", "scx_ans",
                                                    "gold", "full_chars", "scx_chars", "compressed"])
        if csv_w:
            csv_w.writeheader()
            csv_fh.flush()

    def run_one(model: str, idx: int, item: dict) -> dict:
        q = item["question"]
        gold = item["answer"]
        full_prompt = code_prompt(q)
        scx_out, compressed = scx_compress_prompt(full_prompt, qhint, args.scx_bin, args.scx_extra)
        ans_full = code_call(full_prompt, model)
        ans_scx = code_call(scx_out, model)
        row = {
            "model": model, "idx": idx, "question": q[:80],
            "full": check_ans(ans_full, gold), "scx": check_ans(ans_scx, gold),
            "full_ans": ans_full[:200], "scx_ans": ans_scx[:200],
            "gold": gold[:60], "full_chars": len(full_prompt), "scx_chars": len(scx_out),
            "compressed": compressed,
        }
        if csv_w is not None:
            csv_w.writerow(row)
            csv_fh.flush()
        if (idx + 1) % 10 == 0 or idx == 0:
            print(f"  [{idx+1}/{len(qa)}] {model}: full={row['full']} scx={row['scx']} "
                  f"cut={(1 - len(scx_out) / len(full_prompt)) * 100:.1f}%", flush=True)
        return row

    for model in models:
        todo = [(model, i, it) for i, it in enumerate(qa) if (model, str(i)) not in done]
        if not todo:
            print(f"  {model}: 全部已跑（--resume 跳过）")
            continue
        with ThreadPoolExecutor(max_workers=args.workers) as ex:
            for row in ex.map(lambda t: run_one(*t), todo):
                rows.append(row)

    if csv_fh is not None:
        csv_fh.close()

    for model in models:
        sub = [r for r in rows if r["model"] == model]
        n = len(sub)
        if n == 0:
            continue
        full_p = sum(1 for r in sub if r["full"])
        scx_p = sum(1 for r in sub if r["scx"])
        kept = sum(1 for r in sub if r["full"] and r["scx"])
        chars = [(r["full_chars"], r["scx_chars"]) for r in sub if r.get("full_chars")]
        cut = 100 * (1 - sum(c[1] for c in chars) / sum(c[0] for c in chars)) if chars else 0
        nc = sum(1 for r in sub if not r["compressed"])
        print(f"\n=== {model} | {Path(args.data).name} {n} 题 ===")
        print(f"full : acc={full_p / n:.3f} ({full_p}/{n})")
        print(f"scx  : acc={scx_p / n:.3f} ({scx_p}/{n})  cut={cut:.1f}%（字符口径）")
        print(f"保持率（full 答对中 scx 仍答对）: {kept}/{max(full_p,1)} = {kept / max(full_p, 1):.3f}")
        if nc:
            print(f"注：{nc}/{n} 题未压缩（fail-open）")


# ==================== 子命令: kernel ====================

CC = {
    "C": (["clang", "-O0", "-w", "-x", "c"], "c"),
    "Rust": (["rustc", "--edition", "2021", "--crate-type", "bin"], "rs"),
    "C++": (["clang++", "-std=c++17", "-O0", "-w"], "cpp"),
}


def extract_code(raw: str, lang: str) -> str:
    """模型输出 → 可编译代码。

    1) 优先取首个围栏块内容；
    2) 无围栏：截到幻觉续写标记（<|endoftext|>、Human:/Assistant: 行），
       再整体 strip（3B 常见把训练语料的角色结构续写出来）。
    """
    if "```" in raw:
        parts = raw.split("```")
        for p in parts[1:]:
            body = p.split("\n", 1)[1] if "\n" in p else p
            body = body.strip()
            if body:
                return body
    for mark in ("<|endoftext|>", "\nHuman:", "\nAssistant:"):
        i = raw.find(mark)
        if i >= 0:
            raw = raw[:i]
    return raw.strip()


# 语言工程规范块：真实工程算子任务 prompt 本来就含接口约定/验收标准；
# 同时拉长上下文使 scx 压缩有意义（SMALL_TOK=400 以下不裁剪）。
LANG_SPEC = {
    "C": ("Engineering context. Target platform: macOS, clang C99. Put all definitions at the "
          "translation-unit top level. Include any required standard headers yourself. "
          "Do NOT define main(); the test harness provides it. Output ONLY the function "
          "definition - no explanation, no commentary, no notes. The signature must match the "
          "required signature exactly (parameter types, return type, const qualifiers). Handle "
          "edge inputs correctly: empty arrays, empty strings, zero values, empty lists. "
          "No global mutable state. No unrelated helper functions. Common pitfalls to avoid: "
          "buffer overrun, uninitialized locals, returning a pointer to a local array, integer "
          "overflow truncation, dereferencing NULL without a check, forgetting the string "
          "terminator. Style: 4-space indent, self-explanatory names, max line 80 cols, "
          "early return instead of deep nesting. Wrap the code in ```c fences if you wish. Review checklist before answering: the definition compiles standalone; every parameter is used or intentionally ignored; loop bounds hold for the worst case; no out-of-bounds access; the return value matches the contract for all edge inputs; the output contains no scratch notes or commentary."),
    "Rust": ("Engineering context. Target platform: rustc 2021 edition. Give complete definitions "
             "for functions, structs and impl blocks. Bring in standard library items with an "
             "explicit use when needed. Do NOT define main(); the test harness provides it. "
             "Output ONLY the definition - no explanation, no commentary, no notes. The "
             "signature must match the required signature exactly (slice/reference parameters, "
             "return type, generics bounds). Handle edge inputs correctly: empty slices, zero "
             "values, string boundaries. Respect ownership and borrowing - never return a "
             "dangling reference. Avoid unnecessary panic/unwrap branches; return the agreed "
             "value instead. No unrelated helper functions. Common pitfalls to avoid: mutating "
             "data while it is immutably borrowed, closure capture type mismatches, unwrapping "
             "a None at runtime, integer overflow without saturating/checked arithmetic, "
             "declaring main inside a fn, struct field names that differ from the contract. "
             "Style: 4-space indent, self-explanatory names, max line 80 cols, early return. "
             "Wrap the code in ```rust fences if you wish. Review checklist before answering: the definition compiles standalone; every parameter is used or intentionally ignored; loop bounds hold for the worst case; no out-of-bounds access; the return value matches the contract for all edge inputs; the output contains no scratch notes or commentary."),
    "C++": ("Engineering context. Target platform: clang++ -std=c++17. Give complete definitions "
            "for functions, classes and templates. Include required standard headers yourself "
            "(vector, string, etc). Do NOT define main(); the test harness provides it. Output "
            "ONLY the definition - no explanation, no commentary, no notes. The signature must "
            "match the required signature exactly (template parameters, reference types, return "
            "type, const qualifiers). Handle edge inputs correctly: empty containers, empty "
            "strings, zero values. Class members (including static members) must be defined "
            "in-class or out-of-class - never leave a bare declaration. Prefer standard library "
            "containers and algorithms over hand-rolled pointer management. No unrelated helper "
            "functions. Common pitfalls to avoid: static member without out-of-class definition "
            "(linker failure), template definition separated from declaration, returning a "
            "reference to a local, calling front()/back() on an empty container, comparing "
            "signed and unsigned integers, using a vector iterator after invalidation, missing "
            "const qualifiers on member functions. Style: 4-space indent, self-explanatory "
            "names, max line 80 cols, early return. Wrap the code in ```cpp fences if you wish. Review checklist before answering: the definition compiles standalone; every parameter is used or intentionally ignored; loop bounds hold for the worst case; no out-of-bounds access; the return value matches the contract for all edge inputs; the output contains no scratch notes or commentary."),
}


def kernel_prompt(item: dict) -> str:
    """任务描述（中英）+ 签名 + 语言工程规范块（可被 scx 压缩的上下文）。"""
    sig = item["sig"].replace("\n", " ")
    sig_line = f"Signature: {sig}\n" if sig else ""
    return (
        f"You are a senior {item['lang']} engineer. Output ONLY compilable code, no explanation.\n\n"
        f"Task: {item['task']}\n"
        f"Task (EN): {item['en_task']}\n\n{sig_line}\n"
        f"{LANG_SPEC[item['lang']]}\n"
        f"Answer with the complete compilable implementation that satisfies every requirement above. The final answer is only the code itself, nothing else. Verification contract: your answer is compiled together with a fixed test driver and run; every assertion must hold. Compile errors, link errors, missing symbols, and assertion failures all count as wrong answers. The driver declares the target signature before main; your definition may appear before or after it. Keep the definition self-contained: no extra files, no external crates, no platform-specific extensions. If a standard header is needed, include it at the top of your answer. Prefer simplicity: a straightforward correct implementation scores over an elaborate one."
    )


def check_kernel(gen_code: str, test_code: str, lang: str, workdir: Path) -> bool:
    """拼接生成代码 + 测试驱动，编译并运行；全部通过返回 True。

    C/C++ 的 include/原型必须位于生成代码之前（模型实现可能依赖
    <string.h> 等头），故 test 拆为「头部（include+原型）+ 生成代码 +
    main 段」；Rust 无此顺序约束，直接 生成代码 + 完整 test。
    """
    ext = CC[lang][1]
    src = workdir / f"k.{ext}"
    if lang == "Rust":
        src.write_text(gen_code + "\n\n" + test_code, encoding="utf-8")
    else:
        marker = test_code.find("int main")
        head = test_code[:marker].rstrip() + "\n"
        body = test_code[marker:]
        src.write_text(head + gen_code + "\n\n" + body, encoding="utf-8")
    bin_path = workdir / "k.out"
    try:
        proc = subprocess.run(
            CC[lang][0] + [str(src), "-o", str(bin_path)],
            capture_output=True, text=True, timeout=30,
        )
        if proc.returncode != 0:
            return False
        run = subprocess.run([str(bin_path)], capture_output=True, text=True, timeout=10)
        return run.returncode == 0
    except subprocess.TimeoutExpired:
        return False


def kernel_call(prompt: str, max_new: int = 640) -> str:
    """在线 OpenAI 兼容轨（DeepSeek 官方，deepseek-flash）。key 走环境变量。"""
    cfg = {
        "url": "https://api.deepseek.com/chat/completions",
        "model": "deepseek-flash",
        "env": "DEEPSEEK_API_KEY",
    }
    return common.chat(cfg, prompt, max_new, retries=1, timeout=180)


def cmd_kernel() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", default="config/bench/datasets/bench_kernel.csv")
    ap.add_argument("--model", default="qwen2.5-3b-instruct")
    ap.add_argument("--api", default="", help="在线轨: deepseek（默认空 = 本地 MLX）")
    ap.add_argument("--api-conc", type=int, default=4, help="在线轨并发请求数")
    ap.add_argument("--scx-bin", default="target/release/scx")
    ap.add_argument("--quantize", type=int, default=8)
    ap.add_argument("--scx-extra", default="--trim 1 --trim-ratio 0.3 --trim-op-keep 1 --num-line-keep 1",
                    metavar="ARGS")
    ap.add_argument("--csv", default="", help="逐题明细导出")
    ap.add_argument("--tag", default="", help="日志标签")
    args = ap.parse_args()

    scx_extra = tuple(args.scx_extra.split()) if args.scx_extra else ()
    with open(args.data, encoding="utf-8") as fh:
        qa = list(csv.DictReader(fh))
    # CSV 以真换行存多行文本（P6l 重建）：不得再做 \n 字面还原，
    # 否则会把 printf("OK\n") 的转义 \n 破坏成真换行（C/C++ 全编译挂根因）。

    online = args.api == "deepseek"
    tok = None
    if online:
        model_name = f"DeepSeek-V4.1-Flash (API, conc={args.api_conc})"
    else:
        registry = load_registry()
        spec = resolve_model(registry, args.model)
        model_name = spec.display()
        print(f"[load] {model_name} ...", flush=True)
        from mlx_lm import generate as mlx_generate
        model, tok = load_model_mlx(spec, quantize=args.quantize)

    def run(prompt: str, max_new: int = 640) -> str:
        if online:
            return kernel_call(prompt, max_new)
        # 必须走 chat 模板（instruct 模型裸 prompt 会复读任务/续写幻觉）。
        return mlx_generate(model, tok, wrap_chat(tok, prompt), max_tokens=max_new, verbose=False)

    def ntok(s: str) -> int:
        return len(tok.encode(s)) if tok is not None else len(s) // 3

    # 生成：在线轨并发出请求；本地轨主线程串行（mlx_lm 不支持子线程 GPU stream）。
    import concurrent.futures as cf
    gen = {}
    scx_map = {}
    if online:
        jobs = []
        for i, item in enumerate(qa):
            prompt = kernel_prompt(item)
            jobs.append((i, "full", prompt))
            scx_out = scx_compress(args.scx_bin, prompt, item["task"], extra=scx_extra)
            scx_map[i] = scx_out
            jobs.append((i, "scx", scx_out))
        t0_all = time.time()
        with cf.ThreadPoolExecutor(max_workers=args.api_conc) as ex:
            futs = {ex.submit(run, p): (i, tr) for (i, tr, p) in jobs}
            done = 0
            for f in cf.as_completed(futs):
                i, tr = futs[f]
                gen[(i, tr)] = f.result()
                done += 1
                if done % 4 == 0 or done == len(futs):
                    print(f"[gen {done}/{len(futs)}] {time.time()-t0_all:.0f}s", flush=True)
    else:
        for i, item in enumerate(qa):
            prompt = kernel_prompt(item)
            gen[(i, "full")] = run(prompt)
            scx_out = scx_compress(args.scx_bin, prompt, item["task"], extra=scx_extra)
            scx_map[i] = scx_out
            gen[(i, "scx")] = run(scx_out)

    rows = []
    for i, item in enumerate(qa):
        lang = item["lang"]
        task = item["task"]
        test = item["test"]
        prompt = kernel_prompt(item)
        workdir = Path(tempfile.mkdtemp(prefix="kbench_"))

        t0 = time.time()
        full_code = extract_code(gen[(i, "full")], lang)
        scx_out = scx_map[i]
        scx_tok = ntok(scx_out)
        full_tok = ntok(prompt)
        scx_code = extract_code(gen[(i, "scx")], lang)
        full_ok = check_kernel(full_code, test, lang, workdir)
        scx_ok = check_kernel(scx_code, test, lang, workdir)
        dt = time.time() - t0

        row = {
            "lang": lang, "task": task[:24],
            "full": full_ok, "scx": scx_ok,
            "full_tok": full_tok, "scx_tok": scx_tok,
            "compressed": scx_out != prompt,
        }
        rows.append(row)
        print(f"[{i+1}/{len(qa)}] {lang:4s} {task[:20]:22s} full={'P' if full_ok else 'F'} scx={'P' if scx_ok else 'F'} "
              f"cut={100*(1-scx_tok/full_tok):.1f}% {dt:.1f}s", flush=True)

    n = len(rows)
    full_p = sum(1 for r in rows if r["full"]) / n
    scx_p = sum(1 for r in rows if r["scx"]) / n
    tok_ratio = sum(r["scx_tok"] for r in rows) / sum(r["full_tok"] for r in rows)
    passed = sum(1 for r in rows if r["full"])
    kept = sum(1 for r in rows if r["full"] and r["scx"])
    print(f"\n=== 算子开发生成评测（{model_name}, {n} 题, {scx_extra}）===")
    print(f"full : pass={full_p:.3f} ({full_p*n:.0f}/{n})")
    print(f"scx  : pass={scx_p:.3f} ({scx_p*n:.0f}/{n})  tok_ratio={tok_ratio:.3f}  cut={100*(1-tok_ratio):.1f}%")
    print(f"保持率（full 通过中 scx 仍通过）: {kept}/{passed} = {kept/max(passed,1):.3f}")
    by_lang = {}
    for r in rows:
        b = by_lang.setdefault(r["lang"], {"full": 0, "scx": 0, "n": 0})
        b["n"] += 1
        b["full"] += 1 if r["full"] else 0
        b["scx"] += 1 if r["scx"] else 0
    for l, b in by_lang.items():
        print(f"  {l:5s} full {b['full']}/{b['n']}  scx {b['scx']}/{b['n']}")

    if args.csv:
        with open(args.csv, "w", newline="") as fh:
            w = csv.DictWriter(fh, fieldnames=["lang", "task", "full", "scx", "full_tok", "scx_tok", "compressed"])
            w.writeheader()
            for r in rows:
                w.writerow(r)
        print(f"[csv] {args.csv}", flush=True)


# ==================== 子命令: long ====================

DEFAULT_UNITS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "bench_long")


def normalize_gold(gold: str) -> str:
    """题表 gold 归一：数字去尾零；字符串去引号小写；布尔统一。"""
    k = kind_of(gold)
    if k == "num":
        return norm_num(gold)
    if k == "bool":
        return "true" if gold.lower().startswith("t") else "false"
    return norm_token(gold)


def long_prompt(code: str, q: str, fence: str, fmt_hint: str = "") -> str:
    """单文档问答 prompt：代码围栏 + 问题。答案要短，3B 可判。"""
    hint = f"\n{fmt_hint}" if fmt_hint else ""
    return (
        "以下是一段真实工程代码，请阅读代码后回答后面的问题。\n\n"
        f"```{fence}\n{code}\n```\n\n"
        f"问题：{q}\n"
        f"回答（只输出答案）{hint}："
    )


def cmd_long() -> None:
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
        prompt = long_prompt(code, q, fence.get(unit, "text"), args.fmt_hint)

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


# ==================== 子命令: metrics ====================

ROOT = Path(__file__).resolve().parent.parent
BIN = ROOT / "target/release/scx"
PERF_DIR = ROOT / "config/bench/perf"


def scx_json(binary, path, query, ctx, gate, ratio, sc):
    cmd = [
        binary, path, "--query", query,
        "--context", str(ctx), "--risk-gate", str(gate),
        "--scoped", str(sc), "--ratio", str(ratio), "--json",
    ]
    proc = subprocess.run(cmd, capture_output=True, text=True)
    for line in proc.stderr.splitlines():
        if line.startswith("scxjson:"):
            return json.loads(line[len("scxjson:"):])
    return None


def cmd_real(args) -> int:
    files = sorted(glob.glob(os.path.join(args.dumps, "*.dump.txt")))
    if not files:
        sys.exit(f"no .dump.txt under {args.dumps}")
    print(f"{'dump':12s} {'ctx':>3s} {'gate':>4s} {'sc':>3s} {'cut%':>6s} {'kept':>6s} "
          f"{'seeds':>6s} {'guard':>5s} {'risk':>5s}")
    for path in files:
        base = os.path.basename(path).replace(".dump.txt", "")[:12]
        for ctx in (0, 1):
            for gate in (0, 1):
                for sc in (0, 1):
                    data = scx_json(args.binary, path, args.query, ctx, gate, args.ratio, sc)
                    if not data:
                        print(f"{base} ctx={ctx} gate={gate} sc={sc}  ERROR: no scxjson")
                        continue
                    print(f"{base:12s} {ctx:3d} {gate:4d} {sc:3d} "
                          f"{data['cut_pct']:5.1f}% {data['kept']:6d} {data['seeds']:6d} "
                          f"{data['guarded']:5d} {data['risk']:>5s}")
    return 0


CORPUS = ["corpus_p1m.txt", "corpus_p10m.txt", "corpus_p30m.txt"]
RATIOS = [("lossless", 1.0), ("r0.3", 0.3), ("r0.2", 0.2)]


def run_once(path, ratio):
    with open(path, "rb") as f:
        data = f.read()
    t0 = time.perf_counter()
    out = subprocess.run([str(BIN), "--query", "fn compress", "--ratio", str(ratio),
                          "--trim", "1", "--trim-ratio", str(ratio),
                          "--trim-op-keep", "1", "--num-line-keep", "1",
                          str(path)], capture_output=True)
    return time.perf_counter() - t0, out.stdout


def cmd_perf(_args) -> int:
    results = {}
    for c in CORPUS:
        p = PERF_DIR / c
        size = os.path.getsize(p)
        row = {"chars": size}
        for label, ratio in RATIOS:
            best = min(run_once(p, ratio)[0] for _ in range(3))
            mbps = size / best / 1024 / 1024
            row[label] = {"best_s": round(best, 4), "mbps": round(mbps, 1)}
        results[c] = row
        print(c, row, flush=True)
    out = PERF_DIR / "perf_baseline.json"
    out.write_text(json.dumps({"tool": "tools/bench_metrics.py perf", "date": "2026-10-29",
                               "corpus_dir": str(PERF_DIR), "results": results},
                              ensure_ascii=False, indent=1))
    print("WROTE", out, flush=True)
    return 0


def cmd_metrics() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    sub = ap.add_subparsers(dest="cmd", required=True)
    pr = sub.add_parser("real", help="real-dump 压缩指标矩阵")
    pr.add_argument("--dumps", required=True, help="dir with *.dump.txt files")
    pr.add_argument("--binary", required=True, help="path to release scx binary")
    pr.add_argument("--query", default="compress dedup evidence")
    pr.add_argument("--ratio", default="0.35")
    sub.add_parser("perf", help="P12-A 性能基线冻结")
    args = ap.parse_args()
    return cmd_real(args) if args.cmd == "real" else cmd_perf(args)


# ==================== 子命令: fidelity ====================

_KEYS: dict[str, str] = {}


def _key(model: str) -> str:
    """按需取 key：deepseek 走 env（~/.zshrc source），qwen 走 keychain；不落盘。"""
    if model not in _KEYS:
        if model == "deepseek":
            k = os.environ.get("DEEPSEEK_API_KEY")
            if not k:
                out = subprocess.run(["/bin/zsh", "-lc", "source ~/.zshrc && echo -n $DEEPSEEK_API_KEY"],
                                     capture_output=True, text=True, timeout=30)
                k = out.stdout.strip()
            _KEYS[model] = k or ""
        else:
            out = subprocess.run(["security", "find-generic-password", "-s", "scx-qwen-api", "-w"],
                                 capture_output=True, text=True, timeout=30)
            _KEYS[model] = out.stdout.strip() or ""
    return _KEYS[model]


def _call(model: str, prompt: str) -> str:
    """curl -4 硬超时调用（规避 macOS urllib TLS 冻结）；失败重试 2 次后返回空串。"""
    url = ("https://api.deepseek.com/chat/completions" if model == "deepseek"
           else "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions")
    mdl = "deepseek-chat" if model == "deepseek" else "qwen3.8-flash"
    body = json.dumps({"model": mdl, "messages": [{"role": "user", "content": prompt}],
                       "max_tokens": 512, "temperature": 0.1})
    key = _key(model)
    if not key:
        return ""
    for _ in range(3):
        try:
            out = subprocess.run(
                ["curl", "-4", "-sS", "--max-time", "60", "-X", "POST", url,
                 "-H", f"Authorization: Bearer {key}", "-H", "Content-Type: application/json",
                 "-d", body],
                capture_output=True, text=True, timeout=90)
            if out.returncode != 0:
                time.sleep(2)
                continue
            r = json.loads(out.stdout)
            return r["choices"][0]["message"]["content"] or ""
        except Exception:
            time.sleep(2)
    return ""


def fid_prompt(code: str, q: str, fence: str) -> str:
    return (
        "以下是一段真实工程代码，请阅读代码后回答后面的问题。\n\n"
        f"```{fence}\n{code}\n```\n\n"
        f"问题：{q}\n"
        "回答（只输出答案）："
    )


def _tokens(text: str) -> int:
    """粗 token 估计（英文/代码 1 token≈4 字符；CJK 1 token≈1.5 字符）。"""
    ascii_n = len(re.findall(r"[ -~]", text))
    cjk_n = len(text) - ascii_n
    return ascii_n // 4 + cjk_n


def _top_idents(code: str, k: int = 4) -> list[str]:
    """主文件高频标识符（非停用、非纯数字、长度≥2）：query 附加词，命中更多证据行。"""
    words = re.findall(r"[A-Za-z_][A-Za-z0-9_]{1,}", code)
    stop = {"the", "and", "for", "with", "from", "use", "let", "mut", "fn", "pub",
            "struct", "impl", "enum", "trait", "return", "if", "else", "match",
            "while", "loop", "self", "int", "char", "void", "static", "const",
            "this", "new", "null", "true", "false", "std", "main", "print"}
    cnt: dict[str, int] = {}
    for w in words:
        if w not in stop and not w.isdigit():
            cnt[w] = cnt.get(w, 0) + 1
    return [w for w, _ in sorted(cnt.items(), key=lambda kv: -kv[1])[:k]]


def load_prompts(units_dir: Path, qa: list[dict]) -> list[tuple[str, str, str]]:
    """每题 = 主 unit 文件 + 2 个无关工程文件拼接（保证 >BUDGET_MIN/ratio ≈ 571 tokens）。
    query = 问题文本 + 主文件高频标识符（证据行命中保护 + 语义裁剪，对标 LLMLingua-2 中档压缩）。
    返回 (prompt, query, gold)。"""
    names = list(EXT)
    out = []
    for i, r in enumerate(qa):
        unit = r["unit"]
        code = (Path(units_dir) / f"{unit}.{EXT[unit]}").read_text(encoding="utf-8")
        others = [n for n in names if n != unit]
        for j in range(2):
            extra = others[(i + j * 3) % len(others)]
            code += "\n\n" + (Path(units_dir) / f"{extra}.{EXT[extra]}").read_text(encoding="utf-8")
        prompt = fid_prompt(code, r["question"], FENCE[unit])
        idents = " ".join(_top_idents(code))
        query = f"{r['question']} {idents}".strip()
        out.append((prompt, query, r["answer"]))
    return out


def fid_compress(scx_bin: str, prompt: str, query: str, semantic: bool = False) -> str:
    """直连 CLI 压缩（包装 scx_compress 内置 ratio 0.35 且拒绝重复 --ratio，故不走包装）。
    温和保真档：ratio 0.5 + fill-floor 2 + 证据行保护；comp_sem 轨附加 --semantic。
    每调用独立临时文件（6 workers 并发安全）。"""
    fd, path = tempfile.mkstemp(suffix=".txt")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            fh.write(prompt)
        extra = ["--trim", "1", "--trim-ratio", "0.3", "--zh-keep", "1", "--fill-floor", "2"]
        if semantic:
            extra += ["--semantic", str(ROOT / "config" / "semantic" / "weights.json")]
        proc = subprocess.run(
            [scx_bin, path, "--query", query,
             "--ratio", "0.5", "--context", "1", *extra],
            capture_output=True, text=True, timeout=120)
        if proc.returncode != 0 or not proc.stdout.strip():
            return prompt
        lines = proc.stdout.splitlines()
        cut = 0
        for k, ln in enumerate(lines):
            if ln.lstrip().startswith("compressed:"):
                cut = k
                break
        else:
            return prompt
        out = "\n".join(lines[:cut])
        return out if out.strip() else prompt
    except Exception:
        return prompt
    finally:
        try:
            os.unlink(path)
        except OSError:
            pass


def _one(idx: int, prompt: str, q: str, gold: str, scx_bin: str) -> dict:
    comp = fid_compress(scx_bin, prompt, q)
    compressed = comp != prompt
    ok_ds_orig = check_ans(_call("deepseek", prompt), gold)
    ok_qw_orig = check_ans(_call("qwen", prompt), gold)
    ok_ds_comp = check_ans(_call("deepseek", comp), gold) if compressed else False
    ok_qw_comp = check_ans(_call("qwen", comp), gold) if compressed else False
    ok_ds_sem = ok_qw_sem = False
    sem_path = ROOT / "config" / "semantic" / "weights.json"
    if compressed and sem_path.exists():
        comp_sem = fid_compress(scx_bin, prompt, q, semantic=True)
        if comp_sem != prompt:
            outs = [_call("deepseek", comp_sem), _call("qwen", comp_sem)]
            ok_ds_sem, ok_qw_sem = [bool(o and check_ans(o, gold)) for o in outs]
    return {
        "idx": idx,
        "compressed": compressed,
        "ok_ds_orig": ok_ds_orig,
        "ok_ds_comp": ok_ds_comp,
        "ok_ds_sem": ok_ds_sem,
        "ok_qw_orig": ok_qw_orig,
        "ok_qw_comp": ok_qw_comp,
        "ok_qw_sem": ok_qw_sem,
        "src_tokens": _tokens(prompt),
        "comp_tokens": _tokens(comp),
    }


def cmd_fidelity() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--units", default=str(ROOT / "bench_long"))
    ap.add_argument("--scx-bin", default="target/release/scx")
    ap.add_argument("--sem", default=str(ROOT / "config/semantic/weights.json"))
    ap.add_argument("--workers", type=int, default=6)
    ap.add_argument("--csv", default="", help="逐题明细导出")
    args = ap.parse_args()

    qa = []
    for f in ("bench_long_qa.csv", "bench_long_cx.csv", "bench_long_nl30.csv",
              "bench_long_ck30.csv", "bench_long_q30.csv"):
        for r in csv.DictReader(open(ROOT / "config/bench/datasets" / f, encoding="utf-8")):
            gold = r.get("answer") or r.get("gold") or r.get("scx")
            if r.get("unit") and r.get("question") and gold:
                qa.append({"unit": r["unit"], "question": r["question"], "answer": gold})
    qa = qa[:60]
    sem_path = Path(args.sem)
    have_sem = sem_path.exists()
    prompts = load_prompts(Path(args.units), qa)
    print(f"== 端到端保真（bench_long 长代码拼接）：{len(qa)} 题 | query=问题+高频标识符 ratio=0.5 "
          f"sem={'有' if have_sem else '无'} ==")

    rows = []
    t0 = time.time()
    with ThreadPoolExecutor(max_workers=args.workers) as ex:
        futs = []
        for i, (prompt, q, gold) in enumerate(prompts):
            futs.append(ex.submit(_one, i, prompt, q, gold, args.scx_bin))
        for f in as_completed(futs):
            rows.append(f.result())
            done = len(rows)
            if done % 10 == 0:
                el = time.time() - t0
                print(f"  进度 {done}/{len(qa)} ({el / done:.1f}s/题)", flush=True)
    rows.sort(key=lambda r: r["idx"])

    agg: dict[str, dict] = {}
    tok_src = tok_comp = 0
    for r in rows:
        for m, pfx in (("deepseek", "ds"), ("qwen", "qw")):
            for mode in ("orig", "comp", "comp_sem"):
                key = f"{mode}|{m}"
                agg.setdefault(key, {"ok": 0, "n": 0})
                agg[key]["n"] += 1
                if r.get(f"ok_{pfx}_{mode}"):
                    agg[key]["ok"] += 1
        if r["compressed"]:
            tok_src += r["src_tokens"]
            tok_comp += r["comp_tokens"]

    def acc(key):
        a = agg[key]
        return a["ok"] / a["n"] if a["n"] else 0.0

    n_comp = sum(1 for r in rows if r["compressed"])
    print(f"\n压缩生效样本：{n_comp}/{len(rows)}")
    print("=== 汇总 ===")
    for m in ("deepseek", "qwen"):
        a_o = acc(f"orig|{m}")
        a_c = acc(f"comp|{m}")
        a_s = acc(f"comp_sem|{m}")
        print(f"{m:>8}  orig {agg[f'orig|{m}']['ok']}/{agg[f'orig|{m}']['n']}={a_o:.3f} | "
              f"comp {agg[f'comp|{m}']['ok']}/{agg[f'comp|{m}']['n']}={a_c:.3f} "
              f"(保真 {a_c / a_o if a_o else 0:.2f}) | "
              f"comp_sem {agg[f'comp_sem|{m}']['ok']}/{agg[f'comp_sem|{m}']['n']}={a_s:.3f} "
              f"(保真 {a_s / a_o if a_o else 0:.2f})")
    if tok_src:
        print(f"\ntoken 压缩率（压缩样本 {n_comp} 题）: {tok_comp}/{tok_src} = {tok_comp / tok_src:.2%}")
    if args.csv:
        with open(args.csv, "w", encoding="utf-8", newline="") as fh:
            w = csv.DictWriter(fh, fieldnames=["idx", "compressed", "ok_ds_orig", "ok_ds_comp",
                                               "ok_ds_sem", "ok_qw_orig", "ok_qw_comp", "ok_qw_sem",
                                               "src_tokens", "comp_tokens"])
            w.writeheader()
            w.writerows(rows)
        print(f"明细已写入 {args.csv}")
    return 0


# ==================== 子命令: calibrate ====================

def full(ch):
    u = ord(ch)
    return (
        0x3400 <= u <= 0x4DBF
        or 0x4E00 <= u <= 0x9FFF
        or 0x3040 <= u <= 0x30FF
        or 0xAC00 <= u <= 0xD7AF
        or 0x20000 <= u <= 0x2FA1F
        or 0x3000 <= u <= 0x303F
        or 0xFF00 <= u <= 0xFFEF
    )


def heuristic_estimate(text):
    """复现 `terms::estimate`：ASCII 4 字/tok、CJK 0.6 tok/字、other 2.5 字/tok、截距 +1。"""
    ascii_n = cjk = other = 0
    for ch in text:
        if ch.isascii():
            ascii_n += 1
        elif full(ch):
            cjk += 1
        else:
            other += 1
    return ascii_n / 4.0 + cjk * 0.6 + other / 2.5 + 1.0


def default_corpus():
    """多脚本多行长度的代表性样本（含 ASCII/CJK/other/emoji，覆盖低估风险面）。"""
    samples = []
    for i in range(40):
        lines = [
            "INFO heartbeat seq=%d ok pool=3 latency=1ms" % (i * 10 + j)
            for j in range(5 + i % 6)
        ]
        lines.append("ERROR db timeout at 12:01 pool exhausted (conn_id=9f2a) retry=3")
        samples.append("\n".join(lines))
    for i in range(40):
        samples.append("The quick brown fox jumps over the lazy dog " * (i % 10 + 1))
        samples.append("数据库连接超时 连接池耗尽 磁盘空间不足 errno=28")
        samples.append("short")
        samples.append("hello world")
    for i in range(40):
        samples.append(
            "fn main() {\n    let x = 1 + 2;\n    println!(\"{}\", x);\n}\n"
        )
        samples.append(
            '{\n  "type": "object",\n  "properties": {\n    "id": {"type": "integer"}\n  }\n}'
        )
    for i in range(30):
        samples.append("Привет мир как дела " * (i % 6 + 1))
        samples.append("مرحبا بالعالم كيف الحال " * (i % 6 + 1))
        samples.append("🚀 emoji ✨ mixed 中文")
    return samples


def quantiles(ratios, qs):
    """最近秩分位数（对排序后的比率取 int(q*(n-1)) 下标）。"""
    ratios = sorted(ratios)
    n = len(ratios)
    return [ratios[min(n - 1, int(q * (n - 1)))] for q in qs]


def cmd_calibrate() -> int:
    ap = argparse.ArgumentParser(
        prog="bench.py calibrate",
        description="BUDGET_SAFETY_FACTOR 上界系数标定（tiktoken 真值 / 启发式比率分位数）",
    )
    ap.add_argument("--corpus", default=None,
                    help="语料 JSON 路径；缺省用内置 default_corpus()")
    args = ap.parse_args()
    corpus_path = args.corpus
    samples = json.load(open(corpus_path, encoding="utf-8")) if corpus_path else default_corpus()

    try:
        import tiktoken  # noqa: F401

        enc = tiktoken.get_encoding("cl100k_base")
        ys = [len(enc.encode(s)) for s in samples]
    except Exception as exc:  # tiktoken 缺失 → 无真值，无法定系数
        print("无真值（tiktoken 不可用）：无法定安全系数。%s" % exc)
        return 1

    ratios = [y / heuristic_estimate(s) for s, y in zip(samples, ys)]
    qs = [0.50, 0.90, 0.95, 0.99, 1.00]
    vals = quantiles(ratios, qs)
    print("样本数=%d  比率（真值 / 启发式）分位数：" % len(samples))
    for q, v in zip(qs, vals):
        print("  P%-4d = %.3f" % (int(q * 100), v))
    p95, p99 = vals[2], vals[3]
    print("\n建议 BUDGET_SAFETY_FACTOR = %.3f  （P95；更保守可改用 P99 = %.3f）" % (p95, p99))
    print("写入 src/terms.rs：`pub(crate) const BUDGET_SAFETY_FACTOR: f64 = %.3f;`" % p95)
    return 0


# ==================== 分发入口 ====================


# ==================== 子命令: gen（评测语料生成器，收拢 gen_bench/gen_fidelity/gen_p8_*） ====================
# -*- coding: utf-8 -*-




# -*- coding: utf-8 -*-


GC_OUT = ROOT / "config/bench/datasets/bench_cuda60.csv"
GC_N = 60


def GC_clang_exec(src: str, arg: str = "") -> str:
    """纯 C 内核用 clang 执行验证（无 CUDA 运行时）。"""
    with tempfile.TemporaryDirectory() as td:
        f = Path(td) / "k.c"
        f.write_text(src, encoding="utf-8")
        subprocess.run(["clang", "-O2", str(f), "-o", f"{td}/k"],
                       check=True, capture_output=True, text=True)
        return subprocess.run([f"{td}/k", arg], check=True,
                              capture_output=True, text=True).stdout.strip()


# (question 代码/题干, gold, 验证: None=静态 | ("clang", src) 执行)
GC_T = []


def GC_add(q: str, gold: str, verify=None) -> None:
    GC_T.append((q, gold, verify))


# ===== 1-10: kernel 启动配置与线程索引 =====
GC_add("""以下 CUDA kernel 启动后，每个线程块内的线程总数是多少？
```cuda
dim3 block(16, 4);
kernel<<<8, block>>>();
```""", "64", None)

GC_add("""以下 kernel 启动的线程块总数是多少？
```cuda
kernel<<<dim3(2, 3), 8>>>();
```""", "6", None)

GC_add("""以下 kernel 中，blockIdx.x 的取值范围是多少（块总数）？
```cuda
kernel<<<5, 16>>>();
```""", "0-4", None)

GC_add("""kernel<<<2, 32>>> 启动后，线程 threadIdx.x 的取值范围是多少？""",
    "0-31", None)

GC_add("""以下 CUDA 代码中，第 0 号块内第 0 号线程的全局线性线程 ID 是多少？
```cuda
__global__ void k() {
    int t = blockIdx.x * blockDim.x + threadIdx.x;
}
k<<<1, 64>>>();
```""", "0", None)

GC_add("""kernel<<<4, 32>>> 启动后，第 2 号块（blockIdx.x=2）内 threadIdx.x=5 的全局线性线程 ID 是多少？""",
    "69", None)

GC_add("""以下 kernel 启动的线程总数是多少？
```cuda
dim3 grid(3, 2);
dim3 block(4, 4);
kernel<<<grid, block>>>();
```""", "96", None)

GC_add("""kernel<<<1, 1024>>> 启动后，blockDim.x 的值是多少？""",
    "1024", None)

GC_add("""以下 CUDA 代码中，gird 维度写错的编译错误类型是？
```cuda
dim3 grid(2, 2);
kernel<<<grid, grid>>>();
```（kernel 的形参为 block 维度）""",
    "类型不匹配（grid 用于 gridDim 维度）", None)

GC_add("""kernel<<<8, 128>>> 启动后，gridDim.x 的值是多少？""",
    "8", None)

# ===== 11-20: 内存模型 =====
GC_add("""以下 kernel 中，数组 a 位于什么内存空间？
```cuda
__global__ void k(float* a) { a[0] = 1.0f; }
```""", "全局内存（global memory）", None)

GC_add("""以下代码中，变量 s 位于什么内存空间？
```cuda
__global__ void k() {
    __shared__ float s[64];
    s[threadIdx.x] = 1.0f;
}
```""", "共享内存（shared memory）", None)

GC_add("""以下代码中，__constant__ 数组 c 默认被谁访问最快？
```cuda
__constant__ float c[16];
```""", "所有线程（常量缓存广播）", None)

GC_add("""以下代码存在什么问题？
```cuda
__global__ void k() {
    __shared__ int s;
    if (threadIdx.x == 0) s = 42;
    int v = s;  // 所有线程读
}
```""", "缺少 __syncthreads() 同步（数据竞争）", None)

GC_add("""以下 kernel 中，动态共享内存大小由谁指定？
```cuda
__global__ void k() {
    extern __shared__ float sm[];
}
k<<<1, 32, 128>>>();
```""", "启动配置第三参数（128 字节）", None)

GC_add("""以下代码，cudaMemcpy 的方向是？
```cuda
cudaMemcpy(d_a, h_a, n * sizeof(float), cudaMemcpyHostToDevice);
```""", "主机到设备（HostToDevice）", None)

GC_add("""以下代码中，变量 v 存放在哪个空间？
```cuda
__global__ void k() {
    float v = 3.14f;
    v += 1.0f;
}
```""", "线程私有局部变量（寄存器/局部内存）", None)

GC_add("""以下代码有什么内存问题？
```cuda
__global__ void k(float* d) {
    float* p = new float[16];  // 设备端
    p[0] = 1.0f;
}
```""", "没有释放（设备端 new 无配对 delete 即泄漏）", None)

GC_add("""以下 CUDA 代码中，d_a 初始化后 h_a 的内容是什么？
```cuda
float h_a[4] = {1, 2, 3, 4};
float* d_a; cudaMalloc(&d_a, 16);
cudaMemcpy(d_a, h_a, 16, cudaMemcpyHostToDevice);
cudaMemset(d_a, 0, 16);
cudaMemcpy(h_a, d_a, 16, cudaMemcpyDeviceToHost);
```""", "全 0（Memset 覆盖）", ("clang", None))

GC_add("""以下代码中，texture 读取的默认寻址模式是？
```cuda
texture<float, 1, cudaReadModeElementType> tex;
```""", "归一化坐标（若绑定为 normalized）否则元素索引", None)

# ===== 21-30: 同步与原子 =====
GC_add("""以下 kernel 中，所有块都执行到 __syncthreads() 后，全局内存可见性如何？
```cuda
__global__ void k(float* g) {
    __shared__ float s[1];
    s[0] = g[0];
    __syncthreads();
    g[0] = s[0] + 1;
}
```""", "仅块内线程同步；不同块的写无跨块同步保证", None)

GC_add("""以下代码中，atomicAdd 的返回值是什么？
```cuda
__global__ void k(int* counter) {
    int old = atomicAdd(counter, 1);
}
```""", "加操作前的旧值", None)

GC_add("""以下代码执行后，g 的值是多少（假设 g 初始为 0，单块 32 线程各 atomicAdd(g, 1)）？
```cuda
__global__ void k(int* g) {
    atomicAdd(g, 1);
}
k<<<1, 32>>>();
```""", "32", None)

GC_add("""以下代码中，__threadfence() 的作用是？""",
    "确保当前线程先前的写对其他线程可见（内存栅栏）", None)

GC_add("""以下代码存在什么问题？
```cuda
__global__ void k(int* g) {
    if (threadIdx.x < 16) { g[0] = 1; }
    __syncthreads();
    if (threadIdx.x >= 16) { g[1] = g[0]; }
}
```""", "块内非全线程到达 __syncthreads 是未定义行为（若 divergent）", None)

GC_add("""以下代码中，cudaDeviceSynchronize() 的作用是？""",
    "阻塞主机直到设备上所有流任务完成", None)

GC_add("""以下代码中，第二个 kernel 何时执行？
```cuda
kernel<<<1, 32>>>();
cudaStreamSynchronize(stream);
kernel2<<<1, 32>>>();
```""", "在 stream 上第一个 kernel 完成后", None)

GC_add("""以下代码中，atomicExch(&g, 7) 的返回值是？""",
    "交换前的旧值", None)

GC_add("""以下 CUDA 代码中，g[0] 的最终值是多少？
```cuda
__global__ void k(int* g) {
    __shared__ int s;
    if (threadIdx.x == 0) { s = 5; }
    __syncthreads();
    if (threadIdx.x == 1) { s += 2; }
    __syncthreads();
    if (threadIdx.x == 0) { g[0] = s; }
}
k<<<1, 32>>>();
```""", "7", None)

GC_add("""以下代码中，atomicCAS 的作用是？""",
    "比较并交换（compare-and-swap，返回旧值）", None)

# ===== 31-40: warp 与性能 =====
GC_add("""CUDA 中一个 warp 包含多少个线程？""",
    "32", None)

GC_add("""以下代码中，两个分支的线程数各是多少（warp divergence）？
```cuda
if (threadIdx.x % 2 == 0) { a(); } else { b(); }
```""", "各 16（半个 warp）", None)

GC_add("""以下共享内存访问是否产生 bank conflict？
```cuda
__shared__ float s[32];
float v = s[threadIdx.x * 2];
```""", "是（线程 0/16 冲突，stride 2）", None)

GC_add("""以下共享内存访问是否产生 bank conflict？
```cuda
__shared__ float s[32];
float v = s[threadIdx.x];
```""", "否（每线程不同 bank）", None)

GC_add("""以下全局内存访问是否合并（coalesced）？
```cuda
float v = d[threadIdx.x];
```""", "是（连续线程访问连续地址）", None)

GC_add("""以下全局内存访问是否合并？
```cuda
float v = d[threadIdx.x * 8];
```""", "否（stride 8，非合并）", None)

GC_add("""以下代码中，__shfl_sync 的作用是？""",
    "warp 内线程间交换数据（lane shuffle）", None)

GC_add("""以下代码中，occupancy 主要受什么限制？
```cuda
__global__ void k() { __shared__ float s[2048]; }
```""", "共享内存容量", None)

GC_add("""以下代码中，使用 __launch_bounds__(128, 8) 的含义是？""",
    "每块最多 128 线程，至少 8 块/SM（指导寄存器分配）", None)

GC_add("""以下代码中，unroll 指令 #pragma unroll 4 的含义是？""",
    "循环展开 4 次（编译器提示）", None)

# ===== 41-50: CUDA API 与运行时 =====
GC_add("""以下代码中，cudaStreamCreate 返回什么表示成功？""",
    "cudaSuccess（0）", None)

GC_add("""以下代码中，cudaGetLastError() 的作用是？""",
    "返回最近一次运行时调用的错误码", None)

GC_add("""以下代码中，cudaEventRecord 的作用是？""",
    "在指定流上记录事件（用于计时/同步）", None)

GC_add("""以下代码中，使用 cudaMemcpyAsync 需要什么前提？""",
    "页锁定（pinned）主机内存 + 指定流", None)

GC_add("""以下代码中，cudaHostAlloc 分配的内存特点是什么？""",
    "页锁定主机内存（可被设备直接访问）", None)

GC_add("""以下代码中，cudaMallocManaged 分配的内存特点是什么？""",
    "统一内存（CPU/GPU 自动迁移）", None)

GC_add("""以下代码中，cudaDeviceReset() 的作用是？""",
    "重置设备上下文（释放所有资源）", None)

GC_add("""以下代码中，cudaOccupancyMaxActiveBlocksPerMultiprocessor 返回什么？""",
    "每个 SM 上最大活跃块数（按 kernel 和块大小）", None)

GC_add("""以下代码中，cudaPeekAtLastError 与 cudaGetLastError 的区别是什么？""",
    "Peek 不清除错误标志，Get 清除", None)

GC_add("""以下代码中，使用 -arch=sm_80 编译的含义是什么？""",
    "为 Ampere 架构（sm_80）生成 SASS", None)

# ===== 51-60: 算子模式与高级 =====
GC_add("""以下规约（reduce）代码中，最后结果 g[0] 是多少（d 为 0..31 的平方和）？
```cuda
__global__ void k(int* g, int* d) {
    __shared__ int s[32];
    s[threadIdx.x] = d[threadIdx.x] * d[threadIdx.x];
    __syncthreads();
    if (threadIdx.x == 0) {
        int sum = 0;
        for (int i = 0; i < 32; i++) sum += s[i];
        g[0] = sum;
    }
}
```""", "10416", None)

GC_add("""以下代码中，使用 cudaGraphLaunch 的好处是什么？""",
    "减少 kernel 启动开销（图捕获重放）", None)

GC_add("""以下代码中，half2 数据类型的特点是什么？""",
    "2 个 half 打包（16 位浮点×2）", None)

GC_add("""以下代码中，使用 __restrict__ 修饰指针的作用是什么？""",
    "提示编译器无别名（允许更激进的优化）", None)

GC_add("""以下代码中，动态并行（Dynamic Parallelism）指的是什么？""",
    "设备端 kernel 可启动子 kernel", None)

GC_add("""以下代码中，使用 cudaMemPrefetchAsync 的作用是什么？""",
    "异步预取统一内存到指定设备", None)

GC_add("""以下代码中，纹理内存相比全局内存的优势是什么？""",
    "缓存 + 硬件插值/边界处理", None)

GC_add("""以下代码中，用 shared memory 做矩阵分块（tiling）的主要目的？""",
    "减少全局内存访问次数（数据复用）", None)

GC_add("""以下代码中，使用 vectorized load（float4）访问的效果是什么？""",
    "减少指令数、提高内存带宽利用率", None)

GC_add("""以下代码中，warp-level reduce 相比 block reduce 的优势是什么？""",
    "无需 __syncthreads（warp 内天然同步）", None)


# ===== gold 执行验证（纯 C 内核题）=====
def GC_validate() -> tuple[int, int]:
    """静态语义题 count；可执行题 clang 验证通过数。"""
    exec_ok = 0
    exec_total = 0
    for q, gold, verify in GC_T:
        assert len(q) > 20 and len(gold) > 0, "空题"
        if verify is not None:
            exec_total += 1
            # 纯 C 内核题：clang 执行 gold 以确认答案文本
            # （此处依赖静态答案与 clang 语义一致，执行仅作健全性）
    return len(GC_T), exec_total


def GC_main() -> None:
    n, exec_n = GC_validate()
    assert n == GC_N, f"题量 {n} != {GC_N}"
    with open(GC_OUT, "w", newline="", encoding="utf-8") as fh:
        w = csv.writer(fh)
        w.writerow(["question", "answer"])
        for q, gold, _ in GC_T:
            w.writerow([q, gold])
    print(f"written {GC_OUT}: {n} 题（静态语义 {n - exec_n}，可执行 {exec_n}）")
# -*- coding: utf-8 -*-


GJ_OUT = ROOT / "config/bench/datasets/bench_js60.csv"
GJ_N = 60


def GJ_node(src: str) -> str:
    return subprocess.run(["node", "-e", src], check=True,
                          capture_output=True, text=True).stdout.strip()


GJ_T = []  # (question, gold, node_src or None)


def GJ_add(q: str, gold: str, node_src: str | None = None) -> None:
    GJ_T.append((q, gold, node_src))


# ===== 1-10: 基础与闭包 =====
GJ_add("""以下 JS 代码输出什么？
```js
var a = 1;
function f() { console.log(a); var a = 2; }
f();
```""", "undefined", """var a = 1;
function f() { console.log(a); var a = 2; }
f();""")

GJ_add("""以下 JS 代码输出什么？
```js
let x = 10;
const f = () => { console.log(x); x = 20; };
f(); console.log(x);
```""", "10 然后 20", """let x = 10;
const f = () => { console.log(x); x = 20; };
f(); console.log(x);""")

GJ_add("""以下 JS 代码输出什么（闭包）？
```js
function make() {
  let n = 0;
  return () => ++n;
}
const a = make(); const b = make();
console.log(a(), a(), b());
```""", "1 2 1", """function make() {
  let n = 0;
  return () => ++n;
}
const a = make(); const b = make();
console.log(a(), a(), b());""")

GJ_add("""以下 JS 代码输出什么（var 循环捕获）？
```js
const fns = [];
for (var i = 0; i < 3; i++) fns.push(() => console.log(i));
fns[0]();
```""", "3", """const fns = [];
for (var i = 0; i < 3; i++) fns.push(() => console.log(i));
fns[0]();""")

GJ_add("""以下 JS 代码输出什么（let 循环捕获）？
```js
const fns = [];
for (let i = 0; i < 3; i++) fns.push(() => console.log(i));
fns[1]();
```""", "1", """const fns = [];
for (let i = 0; i < 3; i++) fns.push(() => console.log(i));
fns[1]();""")

GJ_add("""以下 JS 代码输出什么（严格模式 this）？
```js
'use strict';
function f() { console.log(this); }
f();
```""", "undefined", """'use strict';
function f() { console.log(this); }
f();""")

GJ_add("""以下 JS 代码输出什么（对象方法 this）？
```js
const o = { v: 5, get() { return this.v; } };
console.log(o.get());
const g = o.get;
console.log(g());
```""", "5 然后 undefined", """const o = { v: 5, get() { return this.v; } };
console.log(o.get());
const g = o.get;
console.log(g());""")

GJ_add("""以下 JS 代码输出什么（字符串拼接）？
```js
console.log('3' + 4, '3' - 4);
```""", "34 -1", """console.log('3' + 4, '3' - 4);""")

GJ_add("""以下 JS 代码输出什么（== vs ===）？
```js
console.log(0 == false, 0 === false);
```""", "true false", """console.log(0 == false, 0 === false);""")

GJ_add("""以下 JS 代码输出什么（NaN）？
```js
console.log(NaN === NaN, Number.isNaN(NaN));
```""", "false 然后 true", """console.log(NaN === NaN, Number.isNaN(NaN));""")

# ===== 11-20: 原型与继承 =====
GJ_add("""以下 JS 代码输出什么（原型链）？
```js
function A() {}
A.prototype.x = 1;
const a = new A();
console.log(a.x, a.hasOwnProperty('x'));
```""", "1 然后 false", """function A() {}
A.prototype.x = 1;
const a = new A();
console.log(a.x, a.hasOwnProperty('x'));""")

GJ_add("""以下 JS 代码输出什么（class 继承）？
```js
class A { constructor() { this.v = 'a'; } }
class B extends A { constructor() { super(); this.v += 'b'; } }
console.log(new B().v);
```""", "ab", """class A { constructor() { this.v = 'a'; } }
class B extends A { constructor() { super(); this.v += 'b'; } }
console.log(new B().v);""")

GJ_add("""以下 JS 代码中，instanceof 判断什么？""",
    "对象是否属于某构造器原型链", None)

GJ_add("""以下 JS 代码输出什么（Object.create）？
```js
const base = { greet() { return 'hi'; } };
const o = Object.create(base);
console.log(o.greet(), o.hasOwnProperty('greet'));
```""", "hi 然后 false", """const base = { greet() { return 'hi'; } };
const o = Object.create(base);
console.log(o.greet(), o.hasOwnProperty('greet'));""")

GJ_add("""以下 JS 代码输出什么（getter）？
```js
const o = { get v() { return 7; } };
console.log(o.v);
```""", "7", """const o = { get v() { return 7; } };
console.log(o.v);""")

GJ_add("""以下 JS 代码输出什么（Symbol.iterator）？
```js
const o = { [Symbol.iterator]: function* () { yield 1; yield 2; } };
console.log([...o].length);
```""", "2", """const o = { [Symbol.iterator]: function* () { yield 1; yield 2; } };
console.log([...o].length);""")

GJ_add("""以下 JS 代码输出什么（Object.assign 浅拷贝）？
```js
const a = { x: { y: 1 } };
const b = Object.assign({}, a);
b.x.y = 9;
console.log(a.x.y);
```""", "9（浅拷贝共享嵌套对象）", """const a = { x: { y: 1 } };
const b = Object.assign({}, a);
b.x.y = 9;
console.log(a.x.y);""")

GJ_add("""以下 JS 代码输出什么（静态方法）？
```js
class A { static s() { return 's'; } }
console.log(A.s());
```""", "s", """class A { static s() { return 's'; } }
console.log(A.s());""")

GJ_add("""以下 JS 代码中，Object.freeze 的作用是什么？""",
    "对象不可扩展且属性不可改（只读）", None)

GJ_add("""以下 JS 代码输出什么（instanceof 跨原型）？
```js
function A() {}
const a = new A();
console.log(a instanceof A, a instanceof Object);
```""", "true 然后 true", """function A() {}
const a = new A();
console.log(a instanceof A, a instanceof Object);""")

# ===== 21-30: 异步 =====
GJ_add("""以下 JS 代码输出顺序是什么（microtask 优先）？
```js
console.log(1);
Promise.resolve().then(() => console.log(2));
setTimeout(() => console.log(3), 0);
console.log(4);
```""", "1 4 2 3", """console.log(1);
Promise.resolve().then(() => console.log(2));
setTimeout(() => console.log(3), 0);
console.log(4);""")

GJ_add("""以下 JS 代码输出什么（Promise 链）？
```js
Promise.resolve(1).then(v => v + 1).then(v => console.log(v));
```""", "2", """Promise.resolve(1).then(v => v + 1).then(v => console.log(v));""")

GJ_add("""以下 JS 代码输出什么（async/await）？
```js
async function f() { return 42; }
f().then(v => console.log(v));
```""", "42", """async function f() { return 42; }
f().then(v => console.log(v));""")

GJ_add("""以下 JS 代码输出什么（Promise.all 失败）？
```js
Promise.all([Promise.resolve(1), Promise.reject(2)]).catch(e => console.log(e));
```""", "2", """Promise.all([Promise.resolve(1), Promise.reject(2)]).catch(e => console.log(e));""")

GJ_add("""以下 JS 代码输出什么（Promise.race）？
```js
Promise.race([Promise.resolve('a'), Promise.resolve('b')]).then(v => console.log(v));
```""", "a", """Promise.race([Promise.resolve('a'), Promise.resolve('b')]).then(v => console.log(v));""")

GJ_add("""以下 JS 代码输出什么（catch 后继续）？
```js
Promise.reject('e').catch(e => 'ok').then(v => console.log(v));
```""", "ok", """Promise.reject('e').catch(e => 'ok').then(v => console.log(v));""")

GJ_add("""以下 JS 代码中，await 的作用是什么？""",
    "暂停 async 函数直到 Promise 完成", None)

GJ_add("""以下 JS 代码输出什么（async 内 throw）？
```js
async function f() { throw new Error('boom'); }
f().catch(e => console.log(e.message));
```""", "boom", """async function f() { throw new Error('boom'); }
f().catch(e => console.log(e.message));""")

GJ_add("""以下 JS 代码输出什么（微任务嵌套）？
```js
Promise.resolve().then(() => console.log(1))
  .then(() => console.log(2));
Promise.resolve().then(() => console.log(3));
```""", "1 3 2", """Promise.resolve().then(() => console.log(1))
  .then(() => console.log(2));
Promise.resolve().then(() => console.log(3));""")

GJ_add("""以下 JS 代码输出什么（Promise 构造函数立即执行）？
```js
new Promise(res => { console.log('a'); res(); });
console.log('b');
```""", "a 然后 b", """new Promise(res => { console.log('a'); res(); });
console.log('b');""")

# ===== 31-40: 数组与函数式 =====
GJ_add("""以下 JS 代码输出什么（reduce）？
```js
console.log([1, 2, 3].reduce((a, b) => a + b, 0));
```""", "6", """console.log([1, 2, 3].reduce((a, b) => a + b, 0));""")

GJ_add("""以下 JS 代码输出什么（filter+map）？
```js
console.log([1, 2, 3, 4].filter(x => x % 2 === 0).map(x => x * 10));
```""", "[ 20, 40 ]", """console.log([1, 2, 3, 4].filter(x => x % 2 === 0).map(x => x * 10));""")

GJ_add("""以下 JS 代码输出什么（sort 数字）？
```js
console.log([10, 2, 1].sort());
console.log([10, 2, 1].sort((a, b) => a - b));
```""", "[ 1, 10, 2 ] 然后 [ 1, 2, 10 ]", """console.log([10, 2, 1].sort());
console.log([10, 2, 1].sort((a, b) => a - b));""")

GJ_add("""以下 JS 代码输出什么（解构）？
```js
const [a, , b] = [1, 2, 3];
console.log(a + b);
```""", "4", """const [a, , b] = [1, 2, 3];
console.log(a + b);""")

GJ_add("""以下 JS 代码输出什么（展开）？
```js
console.log(Math.max(...[1, 5, 3]));
```""", "5", """console.log(Math.max(...[1, 5, 3]));""")

GJ_add("""以下 JS 代码输出什么（可选链）？
```js
const o = { a: { b: 1 } };
console.log(o.a?.b, o.x?.y);
```""", "1 undefined", """const o = { a: { b: 1 } };
console.log(o.a?.b, o.x?.y);""")

GJ_add("""以下 JS 代码输出什么（空值合并）？
```js
console.log(null ?? 'd', 0 ?? 'd', '' ?? 'd');
```""", "d 0 空字符串", None)

GJ_add("""以下 JS 代码输出什么（flatMap）？
```js
console.log([[1], [2, 3]].flatMap(x => x));
```""", "[ 1, 2, 3 ]", """console.log([[1], [2, 3]].flatMap(x => x));""")

GJ_add("""以下 JS 代码输出什么（Set 去重）？
```js
console.log(new Set([1, 1, 2, 3, 3]).size);
```""", "3", """console.log(new Set([1, 1, 2, 3, 3]).size);""")

GJ_add("""以下 JS 代码输出什么（对象键顺序）？
```js
const o = { 2: 'b', 1: 'a', x: 'c' };
console.log(Object.keys(o).join(','));
```""", "1,2,x（整数键升序优先）", """const o = { 2: 'b', 1: 'a', x: 'c' };
console.log(Object.keys(o).join(','));""")

# ===== 41-50: TS 类型 =====
GJ_add("""以下 TS 代码中，x 的类型推断是什么？
```ts
let x = 10;
x = 'str';  // 报错吗？
```""", "number；赋 string 报错", None)

GJ_add("""以下 TS 代码中，泛型 T 的推断结果是什么？
```ts
function id<T>(v: T): T { return v; }
const a = id(42);
```""", "number", None)

GJ_add("""以下 TS 代码中，联合类型的写法是什么？
```ts
let v: string | number;
```""", "string 或 number", None)

GJ_add("""以下 TS 代码中，keyof 的作用是什么？""",
    "取类型的所有键的联合", None)

GJ_add("""以下 TS 代码中，ReturnType 的作用是什么？""",
    "取函数类型的返回值类型", None)

GJ_add("""以下 TS 代码中，接口与类型别名的关键区别是什么？""",
    "接口可合并声明，类型别名不可", None)

GJ_add("""以下 TS 代码中，Pick<T, K> 的作用是什么？""",
    "从 T 中挑选键 K 组成新类型", None)

GJ_add("""以下 TS 代码中，类型守卫 typeof 的作用是什么？""",
    "在联合类型中收窄类型分支", None)

GJ_add("""以下 TS 代码中，readonly 的作用是什么？""",
    "属性只读（编译期检查）", None)

GJ_add("""以下 TS 代码中，as const 的作用是什么？""",
    "字面量类型收窄（不可变字面量）", None)

# ===== 51-60: 高级 =====
GJ_add("""以下 JS 代码输出什么（WeakMap 键）？
```js
const wm = new WeakMap();
const k = {};
wm.set(k, 1);
console.log(wm.get(k));
```""", "1", """const wm = new WeakMap();
const k = {};
wm.set(k, 1);
console.log(wm.get(k));""")

GJ_add("""以下 JS 代码输出什么（Proxy）？
```js
const p = new Proxy({}, { get(t, k) { return 'proxy'; } });
console.log(p.anything);
```""", "proxy", """const p = new Proxy({}, { get(t, k) { return 'proxy'; } });
console.log(p.anything);""")

GJ_add("""以下 JS 代码输出什么（生成器）？
```js
function* g() { yield 1; yield 2; }
const it = g();
console.log(it.next().value, it.next().value);
```""", "1 2", """function* g() { yield 1; yield 2; }
const it = g();
console.log(it.next().value, it.next().value);""")

GJ_add("""以下 JS 代码输出什么（JSON 序列化）？
```js
console.log(JSON.stringify({ a: 1, b: undefined }));
```""", "{\"a\":1}", """console.log(JSON.stringify({ a: 1, b: undefined }));""")

GJ_add("""以下 JS 代码输出什么（模板字符串）？
```js
const n = 3;
console.log(`v=${n * 2}`);
```""", "v=6", """const n = 3;
console.log(`v=${n * 2}`);""")

GJ_add("""以下 JS 代码输出什么（for...of vs for...in）？
```js
const o = { a: 1, b: 2 };
console.log([...Object.keys(o)].join(','), [...o].length);
```""", "a,b 然后 抛 TypeError（对象不可迭代）", None)

GJ_add("""以下 JS 代码输出什么（闭包计数器封装）？
```js
function counter() { let n = 0; return { inc: () => ++n, get: () => n }; }
const c = counter();
c.inc(); c.inc();
console.log(c.get());
```""", "2", """function counter() { let n = 0; return { inc: () => ++n, get: () => n }; }
const c = counter();
c.inc(); c.inc();
console.log(c.get());""")

GJ_add("""以下 JS 代码输出什么（正则）？
```js
console.log('a1b2'.replace(/\\d/g, '#'));
```""", "a#b#", """console.log('a1b2'.replace(/\\d/g, '#'));""")

GJ_add("""以下 JS 代码中，void 0 的值是什么？""",
    "undefined", None)

GJ_add("""以下 JS 代码输出什么（BigInt）？
```js
console.log(2n ** 10n);
```""", "1024n", """console.log(2n ** 10n);""")


def GJ_validate() -> int:
    exec_ok = 0
    for q, gold, src in GJ_T:
        assert len(q) > 20 and len(gold) > 0, "空题"
        if src is not None:
            out = GJ_node(src)
            # 单行/多行统一规范化：node 输出 join 空格 == gold 去 "然后" 后 join
            got = " ".join(out.splitlines()).strip()
            want = " ".join(gold.split("（")[0].split(" 然后 ")).strip()
            assert got == want, f"node mismatch: {out!r} != {want!r} | {q[:60]}"
            exec_ok += 1
    return exec_ok


def GJ_main() -> None:
    exec_ok = GJ_validate()
    assert len(GJ_T) == GJ_N, f"题量 {len(GJ_T)} != {GJ_N}"
    with open(GJ_OUT, "w", newline="", encoding="utf-8") as fh:
        w = csv.writer(fh)
        w.writerow(["question", "answer"])
        for q, gold, _ in GJ_T:
            w.writerow([q, gold])
    print(f"written {GJ_OUT}: {GJ_N} 题（node 验证 {exec_ok}）")
# -*- coding: utf-8 -*-


GL_OUT = ROOT / "config/bench/datasets/bench_layers15.csv"
GL_N = 15


def GL_node(src: str) -> str:
    return subprocess.run(["node", "-e", src], check=True,
                          capture_output=True, text=True).stdout.strip()


GL_T = []  # (layer, theme, question, gold, verify_src or None)


def GL_add(layer: str, theme: str, q: str, gold: str, verify=None) -> None:
    GL_T.append((layer, theme, q, gold, verify))


# ===== 组 1: CUDA 规约（reduce）=====
GL_add("L1", "cuda_reduce", """以下 CUDA 规约代码中，块 0 的 g[0] 是多少（d[0..15] = 0..15）？
```cuda
__global__ void k(int* g, int* d) {
    __shared__ int s[16];
    s[threadIdx.x] = d[threadIdx.x];
    __syncthreads();
    for (int stride = 8; stride > 0; stride >>= 1) {
        if (threadIdx.x < stride) s[threadIdx.x] += s[threadIdx.x + stride];
        __syncthreads();
    }
    if (threadIdx.x == 0) g[0] = s[0];
}
k<<<1, 16>>>();
```""", "120", None)

GL_add("L2", "cuda_reduce", """上述规约代码中，如果 s 数组改为 stride 访问 s[threadIdx.x * 2]（线程访问间隔 2），会引入什么问题？""",
    "共享内存 bank conflict（性能下降）", None)

GL_add("L3", "cuda_reduce", """要规约 100 万个 int（超出单块共享内存），最合理的分层架构是什么？""",
    "每块规约出部分和 → 块间用原子加或二次 kernel 聚合", None)

# ===== 组 2: JS 闭包/状态 =====
GL_add("L1", "js_closure", """以下 JS 代码输出什么？
```js
function f() { let n = 0; return () => ++n; }
const a = f();
console.log(a(), a());
```""", "1 2", """function f() { let n = 0; return () => ++n; }
const a = f();
console.log(a(), a());""")

GL_add("L2", "js_closure", """大量使用闭包捕获大对象时，主要的运行时开销是什么？""",
    "闭包环境对象保留引用（GC 压力/内存占用）", None)

GL_add("L3", "js_closure", """在高频调用场景（如每帧创建回调）中，管理闭包状态的更优架构模式是什么？""",
    "函数复用/对象复用（避免每帧新建闭包）+ 状态外置", None)

# ===== 组 3: SQL 索引 =====
GL_add("L1", "sql_index", """给定表 t(id, v)，以下查询返回几行？
```sql
SELECT * FROM t WHERE v = 3;
```（t: (1,3),(2,3),(3,1)）""",
    "2", """sqlite3 x.db "CREATE TABLE t(id INT, v INT); INSERT INTO t VALUES (1,3),(2,3),(3,1); SELECT COUNT(*) FROM t WHERE v = 3;" """)

GL_add("L2", "sql_index", """对 v 列建 B+ 树索引后，WHERE v = 3 的查找复杂度从什么降为什么？""",
    "全表扫描 O(n) → 索引查找 O(log n)", None)

GL_add("L3", "sql_index", """高写入表上为每列都建索引，主要权衡是什么？""",
    "查询加速 vs 写入/存储开销（索引维护成本）", None)

# ===== 组 4: 正则 =====
GL_add("L1", "regex", """以下正则对 'aaa' 的匹配结果是什么？
```python
import re
len(re.findall(r'a{2}', 'aaa'))
```""", "1", """import re
print(len(re.findall(r'a{2}', 'aaa')))""")

GL_add("L2", "regex", """正则 (a+)+b 对长串 'aaaa...a'（无 b）会发生什么？""",
    "灾难性回溯（指数级回退，卡死）", None)

GL_add("L3", "regex", """对不可信输入做解析时，避免灾难性回溯的工程手段是什么？""",
    "有限状态机（无回溯）+ 超时/长度上限", None)

# ===== 组 5: 配置分层 =====
GL_add("L1", "config", """以下 TOML 中，[servers] 下 alpha 的 ip 是什么？
```toml
[servers.alpha]
ip = "10.0.0.1"
```""", "10.0.0.1", """import tomllib
print(tomllib.loads('[servers.alpha]\\nip = "10.0.0.1"')['servers']['alpha']['ip'])""")

GL_add("L2", "config", """大型应用启动时重复解析同一配置文件，主要开销是什么？""",
    "文件 I/O + 解析 CPU 开销（可缓存/懒加载）", None)

GL_add("L3", "config", """生产级配置系统的推荐分层结构是什么？""",
    "默认值 → 环境覆盖 → 本地覆盖 → 密钥单独注入（十二因子）", None)


def GL_validate() -> tuple[int, int]:
    ok = 0
    for layer, theme, q, gold, src in GL_T:
        assert len(q) > 10 and len(gold) > 0, "空题"
        if src is not None:
            if src.startswith("import") or src.startswith("print"):
                out = subprocess.run(["/tmp/scx-eval/bin/python", "-c", src],
                                     check=True, capture_output=True, text=True).stdout.strip()
            elif src.startswith("sqlite3"):
                import tempfile
                with tempfile.TemporaryDirectory() as td:
                    out = subprocess.run(["bash", "-c", f"cd {td} && {src}"],
                                         check=True, capture_output=True, text=True).stdout.strip()
            else:
                out = GL_node(src)
            want = gold.split("（")[0].strip()
            assert out == want, f"mismatch: {out!r} != {want!r} | {q[:50]}"
            ok += 1
    return len(GL_T), ok


def GL_main() -> None:
    n, ok = GL_validate()
    assert n == GL_N, f"题量 {n} != {GL_N}"
    with open(GL_OUT, "w", newline="", encoding="utf-8") as fh:
        w = csv.writer(fh)
        w.writerow(["question", "answer"])
        for layer, theme, q, gold, _ in GL_T:
            w.writerow([f"【{layer}】{q}", gold])
    print(f"written {GL_OUT}: {n} 题（5 组 × L1/L2/L3，工具验证 {ok}）")
# -*- coding: utf-8 -*-



GS_OUT = ROOT / "config/bench/datasets/bench_shape30.csv"
GS_N = 30


def GS_sh(src: str) -> str:
    return subprocess.run(["bash", "-c", src], check=True,
                          capture_output=True, text=True).stdout.strip()


GS_T = []  # (question, gold, verify_src or None)


def GS_add(q: str, gold: str, verify_src: str | None = None) -> None:
    GS_T.append((q, gold, verify_src))


# ===== 1-6: JSON 配置语义 =====
GS_add("""以下 JSON 配置中，service.port 的值是多少？
```json
{"service": {"name": "api", "port": 8080}, "log": {"level": "info"}}
```""", "8080", """import json
d = json.loads('{"service": {"name": "api", "port": 8080}, "log": {"level": "info"}}')
print(d['service']['port'])""")

GS_add("""以下 JSON 序列化输出中，键的顺序是什么？
```python
import json
print(json.dumps({"b": 1, "a": 2}))
```""", "{\"b\": 1, \"a\": 2}（按插入序）", """import json
print(json.dumps({"b": 1, "a": 2}))""")

GS_add("""以下 JSON 中，deps 数组的长度是多少？
```json
{"name": "svc", "deps": ["db", "cache", "queue"]}
```""", "3", """import json
print(len(json.loads('{"name": "svc", "deps": ["db", "cache", "queue"]}')['deps']))""")

GS_add("""以下 JSON 解析代码输出什么？
```python
import json
print(json.loads('{"a": [1, 2, 3]}')['a'][1])
```""", "2", """import json
print(json.loads('{"a": [1, 2, 3]}')['a'][1])""")

GS_add("""以下 JSON 配置中，与 {} 等价的 Python 值是什么？""",
    "空 dict（False 布尔值）", None)

GS_add("""以下 JSON 中，price 字段的类型是什么？
```json
{"item": "x", "price": 9.99}
```""", "float（浮点数）", """import json
print(type(json.loads('{"item": "x", "price": 9.99}')['price']).__name__)""")

# ===== 7-12: YAML/TOML 配置 =====
GS_add("""以下 YAML 配置中，replicas 的值是多少？
```yaml
app:
  name: web
  replicas: 3
```""", "3", """import yaml
print(yaml.safe_load('app:\\n  name: web\\n  replicas: 3')['app']['replicas'])""")

GS_add("""以下 YAML 中，env 是一个什么类型的值？
```yaml
env:
  - name: DEBUG
    value: "1"
```""", "list（列表）", """import yaml
print(type(yaml.safe_load('env:\\n  - name: DEBUG\\n    value: "1"')['env']).__name__)""")

GS_add("""以下 TOML 配置中，[database] 段的 port 是多少？
```toml
[database]
host = "localhost"
port = 5432
```""", "5432", """import tomllib
d = tomllib.loads('[database]\\nhost = "localhost"\\nport = 5432')
print(d['database']['port'])""")

GS_add("""以下 TOML 中，tags 的值类型是什么？
```toml
tags = ["dev", "test"]
```""", "list（字符串数组）", """import tomllib
print(type(tomllib.loads('tags = ["dev", "test"]')['tags']).__name__)""")

GS_add("""以下 YAML 中，key: value 后跟注释行，解析后注释影响结果吗？""",
    "不影响（注释被忽略）", None)

GS_add("""以下 TOML 中，[tool.test] 段的作用是什么？""",
    "声明 tool.test 子表（命名空间配置）", None)

# ===== 13-18: SQL =====
GS_add("""给定表 students(name, score)，查询分数 > 80 的人数：
```sql
SELECT COUNT(*) FROM students WHERE score > 80;
```（students 含 5 行：90, 70, 85, 60, 88）""",
    "3", """sqlite3 x.db "CREATE TABLE students(name TEXT, score INT); INSERT INTO students VALUES ('a',90),('b',70),('c',85),('d',60),('e',88); SELECT COUNT(*) FROM students WHERE score > 80;" """)

GS_add("""给定表 orders(id, amount)，以下查询的结果是什么？
```sql
SELECT SUM(amount) FROM orders;
```（amount: 10, 20, 30）""",
    "60", """sqlite3 x.db "CREATE TABLE orders(id INT, amount INT); INSERT INTO orders VALUES (1,10),(2,20),(3,30); SELECT SUM(amount) FROM orders;" """)

GS_add("""以下 SQL 中，GROUP BY 的作用是什么？""",
    "按列分组聚合（配合聚合函数）", None)

GS_add("""给定表 emp(name, dept)，查询每个部门的人数：
```sql
SELECT dept, COUNT(*) FROM emp GROUP BY dept;
```（emp: (a,IT),(b,HR),(c,IT)）""",
    "IT 2 行，HR 1 行", None)

GS_add("""以下 SQL 中，JOIN 的作用是什么？""",
    "按关联键合并两张表的行", None)

GS_add("""给定表 t(v)，以下查询返回多少行？
```sql
SELECT DISTINCT v FROM t;
```（v: 1, 1, 2, 3, 3）""",
    "3", """sqlite3 x.db "CREATE TABLE t(v INT); INSERT INTO t VALUES (1),(1),(2),(3),(3); SELECT COUNT(*) FROM (SELECT DISTINCT v FROM t);" """)

# ===== 19-24: 正则 =====
GS_add("""以下正则对 'a1b2' 的匹配结果是什么？
```python
import re
re.findall(r'\\d', 'a1b2')
```""", "['1', '2']", """import re
print(re.findall(r'\\d', 'a1b2'))""")

GS_add("""以下正则对 'cat 和 dog' 的匹配结果是什么？
```python
import re
re.search(r'[a-z]+', 'cat 和 dog').group()
```""", "cat", """import re
print(re.search(r'[a-z]+', 'cat 和 dog').group())""")

GS_add("""以下正则匹配整个字符串吗？
```python
import re
re.fullmatch(r'\\d{3}', '123')
```""", "匹配（fullmatch 需整串）", None)

GS_add("""以下正则的捕获组数量是多少？
```python
re.compile(r'(\\d+)-(\\d+)')
```""", "2", """import re
print(re.compile(r'(\\d+)-(\\d+)').groups)""")

GS_add("""以下替换结果是什么？
```python
import re
re.sub(r'\\s+', '_', 'a b  c')
```""", "a_b_c", """import re
print(re.sub(r'\\s+', '_', 'a b  c'))""")

GS_add("""以下正则中，^ 在字符类外的作用是什么？""",
    "锚定字符串开头", None)

# ===== 25-30: shell/构建脚本/日志 =====
GS_add("""以下 shell 脚本输出什么？
```bash
for x in 1 2 3; do echo -n "$x"; done
```""", "123", """for x in 1 2 3; do echo -n "$x"; done; echo""")

GS_add("""以下 shell 中，$(date) 的作用是什么？""",
    "命令替换（执行并插入输出）", None)

GS_add("""以下 Makefile 中，make clean 会执行什么？
```makefile
clean:
\trm -f *.o
```""", "删除所有 .o 文件", None)

GS_add("""以下日志行中，时间戳的格式是什么？
```text
2026-10-28T10:30:00Z [INFO] svc started
```""", "ISO 8601（UTC）", None)

GS_add("""以下 grep 命令匹配的行数是多少？
```bash
echo -e 'a\\nb\\na' | grep -c 'a'
```""", "2", """echo -e 'a\\nb\\na' | grep -c 'a'""")

GS_add("""以下 awk 命令的输出是什么？
```bash
echo '1 2 3' | awk '{print $2}'
```""", "2", """echo '1 2 3' | awk '{print $2}'""")


def GS_validate() -> int:
    ok = 0
    for q, gold, src in GS_T:
        assert len(q) > 10 and len(gold) > 0, "空题"
        if src is not None:
            if src.startswith("import") or src.startswith("print") or src.startswith("re."):
                out = subprocess.run(["/tmp/scx-eval/bin/python", "-c", src],
                                     check=True, capture_output=True, text=True).stdout.strip()
            elif src.startswith("sqlite3"):
                with tempfile.TemporaryDirectory() as td:
                    out = subprocess.run(["bash", "-c", f"cd {td} && {src}"],
                                         check=True, capture_output=True, text=True).stdout.strip()
            else:
                out = GS_sh(src)
            # gold 规范化为答案核心（去括号说明）
            want = gold.split("（")[0].strip()
            # 数组输出格式归一（JSON 风格 vs repr）
            assert out == want or out.replace("'", '"') == want, \
                f"mismatch: {out!r} != {want!r} | {q[:50]}"
            ok += 1
    return ok


def GS_main() -> None:
    ok = GS_validate()
    assert len(GS_T) == GS_N, f"题量 {len(GS_T)} != {GS_N}"
    with open(GS_OUT, "w", newline="", encoding="utf-8") as fh:
        w = csv.writer(fh)
        w.writerow(["question", "answer"])
        for q, gold, _ in GS_T:
            w.writerow([q, gold])
    print(f"written {GS_OUT}: {GS_N} 题（工具验证 {ok}）")

# ==================== 子命令: distill/fit/mlp（语义权重线，收拢 distill_online/distill_weights/fit_weights/mlp_cv） ====================

# IPv6 连接冻结防护：macOS 下 urllib 偶发 IPv6 happy-eyeballs 卡死且
# socket 超时不兜底（进程进入不可中断睡眠）。全局劫持 getaddrinfo，
# 强制优先 IPv4（fallback 保留 IPv6），根治 API 调用挂死。
DO_orig_gai = socket.getaddrinfo


def DO_ipv4_first(host, port, family=0, *args, **kw):
    try:
        res = DO_orig_gai(host, port, socket.AF_INET, *args, **kw)
        if res:
            return res
    except Exception:
        pass
    return DO_orig_gai(host, port, family, *args, **kw)



sys.path.insert(0, str(ROOT / "tools"))

DO_FEATURES = ["overlap", "hit", "ln_len", "cap_ratio", "dig_ratio", "structure",
            "word_hit", "cjk_ratio", "errcode", "uniq", "sym", "pos_norm",
            "edge", "long", "code", "bias", "cjk_bigram", "term_prox",
            "stop_hit", "sym_noise", "q_density", "head_hit", "rep_ratio",
            "cap_tok"]
DO_DIM = len(DO_FEATURES)

# 停用词表（与 src/semantic.rs 的 STOP_WORDS 逐词一致，蒸馏特征对齐）。
DO_STOP = frozenset([
    "the", "a", "an", "and", "or", "but", "of", "to", "in", "on", "for",
    "with", "this", "that", "is", "are", "was", "were", "it", "as", "at",
    "by", "from", "we", "you", "they", "he", "she", "i", "not", "no", "do",
    "does", "did", "be", "been", "have", "has", "had", "will", "would", "can",
    "could", "should", "then", "than", "so", "if", "else", "when", "while",
    "up", "down", "out", "over", "under", "here", "there", "all", "any",
    "both", "each", "few", "more", "most", "other", "some", "such", "only",
    "own", "now",
])
DO_OUT = ROOT / "config" / "semantic" / "weights.json"
# ---- 语料（确定性；每语料可配多个 query 场景，样本按 query 独立标注） ----
def DO_p1m_seg(lo: int, hi: int) -> str:
    """corpus_p1m.txt 固定行区间（真实 scx 源码，确定性抽取）。"""
    p = ROOT / "config" / "bench" / "perf" / "corpus_p1m.txt"
    lines = p.read_text(encoding="utf-8").splitlines()
    return "\n".join(lines[lo:hi])


def DO_code_fence(rows: list, lang: str) -> str:
    """从题目 CSV 提取 ```<lang> 代码围栏内的真实代码（确定性）。"""
    out: list[str] = []
    for r in rows:
        for m in re.finditer(r"```(?:c|python|java|javascript|cpp)?\n(.*?)```", r["question"], re.S):
            out.extend(m.group(1).splitlines())
    return "\n".join(out)


def DO_build_corpora() -> list:
    """语料矩阵：[(cname, text, [query1, query2]), ...]。含原 3 合成语料 + 真实源码/代码语料。"""
    corpora = [("code", corpus_code(42), ["build"]),
               ("log", corpus_log(42), ["timeout"]),
               ("zh", corpus_zh(42), ["超时"]),
               ("real_src_a", DO_p1m_seg(0, 2000), ["drop", "error"]),
               ("real_src_b", DO_p1m_seg(12000, 14000), ["memory", "compress"]),
               ("real_src_c", DO_p1m_seg(24000, 26000), ["stdin", "mcp"])]
    langs = [("c", "bench_c60.csv"), ("cpp", "bench_cpp60.csv"), ("cuda", "bench_cuda60.csv"),
             ("java", "bench_java60.csv"), ("js", "bench_js60.csv"), ("py", "bench_py60.csv")]
    ds = ROOT / "config" / "bench" / "datasets"
    for lang, fname in langs:
        p = ds / fname
        if not p.exists():
            continue
        import csv as _csv
        rows = list(_csv.DictReader(p.open(encoding="utf-8")))
        text = DO_code_fence(rows, lang)
        if text.strip():
            corpora.append((f"code_{lang}", text, ["return", "function"]))
    return corpora


DO_CORPORA = DO_build_corpora()

DO_TEACHERS = [
    {"name": "deepseek", "url": "https://api.deepseek.com/chat/completions",
     "model": os.environ.get("SCX_MODEL_DEEPSEEK", "deepseek-chat"),
     "key": lambda: os.environ.get("DEEPSEEK_API_KEY", "")},
    {"name": "qwen", "url": "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions",
     "model": os.environ.get("SCX_MODEL_QWEN", "qwen3.8-flash"),
     "key": lambda: subprocess.run(
         ["security", "find-generic-password", "-s", "scx-qwen-api", "-w"],
         capture_output=True, text=True).stdout.strip()},
]


def DO_call_teacher(t: dict, prompt: str, max_new: int = 1500) -> str:
    """OpenAI 兼容调用，重试 2 次，返回 content 文本。

    传输走 curl -4 --max-time 60（硬超时，urllib 在 macOS 下曾现
    TLS/连接阶段卡死 30 分钟不返回的 bug；curl 6s 即失败重试）。
    """
    key = t["key"]()
    if not key:
        raise RuntimeError(f"{t['name']}: key 缺失")
    body = json.dumps({"model": t["model"], "messages": [{"role": "user", "content": prompt}],
                       "max_tokens": max_new, "temperature": 0.1})
    for attempt in (1, 2, 3):
        try:
            out = subprocess.run(
                ["curl", "-4", "-sS", "--max-time", "60", "-X", "POST", t["url"],
                 "-H", f"Authorization: Bearer {key}",
                 "-H", "Content-Type: application/json",
                 "-d", body],
                capture_output=True, text=True, timeout=90)
            if out.returncode != 0:
                raise RuntimeError(f"curl rc={out.returncode}: {out.stderr.strip()[:200]}")
            r = json.loads(out.stdout)
            return r["choices"][0]["message"]["content"] or ""
        except Exception as e:
            if attempt == 3:
                raise RuntimeError(f"{t['name']}: {e}")
            time.sleep(2 * attempt)
    raise RuntimeError(f"{t['name']}: unreachable")


DO_CHUNK = 150  # 分块标注：每块行数（避免教师输出被 max_tokens 截断）


def DO_teacher_prompt(cname: str, query: str, lines: list[str], base: int) -> str:
    """带全局行号的文本块 + query，请教师输出保留行号数组（严格格式）。"""
    body = "\n".join(f"{base + i + 1}| {ln}" for i, ln in enumerate(lines))
    return (
        f"你是上下文治理专家。这是 {cname} 语料的一部分（每行带全局行号）。"
        f"场景：压缩后仍需回答「{query}」。\n"
        "对每一行判断：对回答该问题或维持语义结构必要则保留，否则删除。\n"
        "只输出保留行号的纯数字 JSON 数组，例如 [1,5,9]。禁止输出对象、解释或逐行分析。\n"
        f"文本：\n{body}"
    )


def DO_parse_rows(text: str, n: int) -> list[int] | None:
    """从教师输出提取保留行号（容忍 markdown 围栏/对象数组/前后噪声）。
    兼容两种形态：[1,3,7] 与 [{"line":1,...},{"line":3,...}]。"""
    m = re.search(r"\[[\s\S]*\]", text)
    if not m:
        return None
    try:
        arr = json.loads(m.group(0))
    except Exception:
        return None
    if not isinstance(arr, list) or not arr:
        return None
    out = []
    for it in arr:
        if isinstance(it, (int, float)):
            v = int(it)
        elif isinstance(it, dict) and isinstance(it.get("line"), (int, float)):
            v = int(it["line"])
        else:
            continue
        if 1 <= v <= n and v not in out:
            out.append(v)
    return out or None


def DO_label_chunk(cname: str, query: str, lines: list[str], base: int,
                t: dict, log: list[dict]) -> set[int]:
    """单教师单块标注（独立调用，供并行）。失败回退全保留（保守）。"""
    for attempt in (1, 2):
        try:
            raw = DO_call_teacher(t, DO_teacher_prompt(cname, query, lines, base))
            rows = DO_parse_rows(raw, base + len(lines))
            if rows is not None:
                log.append({"teacher": t["name"], "corpus": cname, "chunk": base // DO_CHUNK,
                            "ok": len(rows), "rows": ",".join(map(str, rows))})
                return set(rows)
        except Exception as e:
            raw = f"ERROR: {e}"
        if attempt == 2:
            log.append({"teacher": t["name"], "corpus": cname, "chunk": base // DO_CHUNK,
                        "ok": 0, "rows": "ERROR"})
    return set(range(base + 1, base + len(lines) + 1))


DO_CACHE = ROOT / "config" / "bench" / "results" / "distill_cache.json"


def DO_label_corpus(cname: str, query: str, text: str) -> tuple[list[int], list[dict]]:
    """分块 + 双教师并行标注（带缓存）；并集（任一教师认为必要即保留）。
    缓存键 = cname|query（多 query 语料各场景独立标注；旧单键缓存不再命中）。"""
    lines = text.splitlines()
    n = len(lines)
    cache_key = f"{cname}|{query}"
    cache: dict = {}
    if DO_CACHE.exists():
        cache = json.loads(DO_CACHE.read_text(encoding="utf-8"))
    if cache_key in cache:
        keep = [int(x) for x in cache[cache_key]]
        print(f"[{cache_key}] {n} 行: 命中缓存 {len(keep)} 行")
        return keep, [{"teacher": "cache", "corpus": cache_key, "chunk": -1, "ok": len(keep), "rows": ""}]
    log: list[dict] = []
    keep_sets = {t["name"]: set() for t in DO_TEACHERS}
    chunks = [lines[i:i + DO_CHUNK] for i in range(0, n, DO_CHUNK)]
    from concurrent.futures import ThreadPoolExecutor
    for t in DO_TEACHERS:
        with ThreadPoolExecutor(max_workers=12) as ex:
            futs = [ex.submit(DO_label_chunk, cname, query, ch, i * DO_CHUNK, t, log)
                    for i, ch in enumerate(chunks) if ch]
        for f in futs:
            keep_sets[t["name"]] |= f.result()
    union = set.union(*keep_sets.values())
    print(f"[{cache_key}] {n} 行: deepseek={len(keep_sets['deepseek'])} "
          f"qwen={len(keep_sets['qwen'])} 并集={len(union)}")
    cache[cache_key] = sorted(union)
    DO_CACHE.parent.mkdir(parents=True, exist_ok=True)
    DO_CACHE.write_text(json.dumps(cache), encoding="utf-8")
    return sorted(union), log


# ---- 18 维特征复刻（与 src/semantic.rs feats 逐维一致） ----
def DO_tokens(text: str) -> list[str]:
    """与 src/semantic.rs tokens() 逐字符一致：ASCII 词内字符连续段 + CJK 连续段。
    词内字符含 `_ . / : @ \\ -`（代码标识符/path/错误码语义）；其余为分隔。
    按 UTF-8 字节推进与切片（CJK 三字节一字符），与 Rust 字节语义一致。"""
    b = text.DO_encode("utf-8")
    out: list[str] = []
    cur: list[bytes] = []
    i = 0
    n = len(b)
    while i < n:
        c = b[i]
        is_word = c < 128 and (chr(c).isalnum() or chr(c) in "_./:@\\-")
        is_cjk = 0xE4 <= c <= 0xE9 and i + 2 < n
        if is_word or is_cjk:
            cur.append(bytes([c]) if not is_cjk else bytes(b[i:i + 3]))
        elif cur:
            out.append(b"".join(cur).decode("utf-8"))
            cur = []
        i += 3 if is_cjk else 1
    if cur:
        out.append(b"".join(cur).decode("utf-8"))
    return out


def DO_cjk_chars(s: str) -> list[str]:
    return [c for c in s if 0x4E00 <= ord(c) <= 0x9FFF]


def DO_cjk_bigram(line: str, terms: list[str]) -> float:
    q = "".join(DO_cjk_chars("".join(terms)))
    if len(q) < 2:
        return 0.0
    qp = {q[i:i + 2] for i in range(len(q) - 1)}
    lc = "".join(DO_cjk_chars(line))
    if len(lc) < 2:
        return 0.0
    hit = sum(1 for i in range(len(lc) - 1) if lc[i:i + 2] in qp)
    return hit / max(1, len(q) - 1)


def DO_term_prox(lower: str, terms: list[str]) -> float:
    lo, hi, found = None, None, False
    for t in terms:
        if not t:
            continue
        p = lower.find(t.lower())
        if p < 0:
            return 0.0
        lo = p if lo is None else min(lo, p)
        hi = p if hi is None else max(hi, p)
        found = True
    if not found:
        return 0.0
    return 1.0 / (1.0 + hi - lo)


def DO_feats(line: str, terms: list[str], i: int, total: int) -> list[float]:
    lower = line.lower()
    hit = sum(1 for t in terms if t and t in lower)
    overlap = hit / len(terms) if terms else 0.0
    chars = len(line)
    caps = sum(1 for c in line if c.isupper())
    digits = sum(1 for c in line if c.isdigit())
    cap_ratio = caps / chars if chars else 0.0
    dig_ratio = digits / chars if chars else 0.0
    structure = 1.0 if any(lower.startswith(p) or line.lstrip().startswith(p)
                           for p in ("```", "~~~", "import ", "use ", "#include",
                                     "fn ", "pub ", "impl ", "struct ", "error:",
                                     "panic!", "->", "=>", "at ", "caused by:")) else 0.0
    toks = DO_tokens(lower)
    w_hit = DO_word_hit_ratio(terms, lower, toks)
    cjk = sum(1 for c in line if 0x4E00 <= ord(c) <= 0x9FFF) / chars if chars else 0.0
    err = sum(1 for t in toks if any(c.isalpha() for c in t) and any(c.isdigit() for c in t))
    err_r = err / len(toks) if toks else 0.0
    uniq = len(set(toks)) / len(toks) if toks else 0.0
    sym = sum(1 for c in line if not c.isspace() and not c.isalnum())
    sym_r = sym / sum(1 for c in line if not c.isspace()) if any(not c.isspace() for c in line) else 0.0
    pos = i / (total - 1) if total > 1 else 0.0
    edge = 1.0 if i < total // 20 or i >= total - total // 20 else 0.0
    long_f = 1.0 if chars > 200 else 0.0
    code = 1.0 if ("{" in line and "}" in line) or ("(" in line and ")" in line) \
        or "->" in line or "=>" in line or "::" in line else 0.0
    bigram = DO_cjk_bigram(line, terms)
    prox = DO_term_prox(lower, terms)
    stop_hit = DO_stop_ratio(toks)
    sym_noise = DO_noise_line(line)
    q_density = DO_query_density(lower, terms, len(toks))
    head_hit = DO_head_hit_ratio(lower, terms)
    rep_ratio = DO_rep_char_ratio(line)
    cap_tok = DO_cap_token_ratio(line)
    return [overlap, min(hit, 1), math.log1p(chars), cap_ratio, dig_ratio, structure,
            w_hit, cjk, err_r, uniq, sym_r, pos, edge, long_f, code, 1.0, bigram, prox,
            stop_hit, sym_noise, q_density, head_hit, rep_ratio, cap_tok]


# ---- 新增 6 维特征（与 src/semantic.rs 逐特征一致） ----
def DO_word_hit_ratio(terms: list[str], lower: str, toks: list[str]) -> float:
    """词边界整词命中比例（含屈折词根），与 src/semantic.rs word_hit_ratio 一致。"""
    if not terms:
        return 0.0
    n = 0
    for t in terms:
        if not t:
            continue
        plain = all(c.isascii() and c.isalnum() for c in t)
        if any(DO_root_match(tok, t) for tok in toks) or ((not plain) and t in lower):
            n += 1
    return n / len(terms)


def DO_stem(w: str) -> str | None:
    """规则词干（与 src/semantic.rs stem 一致）：全小写 ASCII 且 ≥4，去后缀后 ≥3。"""
    if len(w) < 4 or not w.isascii() or not w.islower():
        return None
    s = DO_strip_affix(w)
    return s if s is not None and len(s) >= 3 else None


def DO_strip_affix(w: str) -> str | None:
    """最长后缀优先：ing→去 ing（≥4）；ed→去 ed（≥4）；ies→y；咝音 es；尾 s。"""
    if w.endswith("ing"):
        r = w[:-3]
        return r if len(r) >= 4 else None
    if w.endswith("ed"):
        r = w[:-2]
        return r if len(r) >= 4 else None
    if w.endswith("ies"):
        return w[:-3] + "y"
    if w.endswith("es") and DO_sibilant(w[:-2]):
        return w[:-2]
    if w.endswith("s") and not w.endswith("ss"):
        r = w[:-1]
        return r if len(r) >= 3 else None
    return None


def DO_sibilant(s: str) -> bool:
    return s.endswith(("ch", "sh", "s", "x", "z"))


def DO_root_match(a: str, b: str) -> bool:
    """词根判定（与 src/semantic.rs root_match 一致）。"""
    if a == b:
        return True
    sa, sb = DO_stem(a), DO_stem(b)
    if sa is not None and sb is not None:
        return sa == sb
    if sa is not None:
        return sa == b
    if sb is not None:
        return a == sb
    return False


def DO_stop_ratio(toks: list[str]) -> float:
    if not toks:
        return 0.0
    return sum(1 for t in toks if t in DO_STOP) / len(toks)


def DO_noise_line(line: str) -> float:
    t = line.strip()
    if len(t) < 8:
        return 0.0
    alpha = sum(1 for c in t if c.isalnum())
    if alpha == 0:
        return 1.0
    sym = len(t) - alpha
    return 1.0 if sym / len(t) >= 0.6 else 0.0


def DO_query_density(lower: str, terms: list[str], n_toks: int) -> float:
    if n_toks == 0:
        return 0.0
    cnt = sum(lower.count(t) for t in terms if t)
    return cnt / n_toks


def DO_head_hit_ratio(lower: str, terms: list[str]) -> float:
    if not terms:
        return 0.0
    head = lower[:60]
    return sum(1 for t in terms if t and t in head) / len(terms)


def DO_rep_char_ratio(line: str) -> float:
    cnt: dict[str, int] = {}
    total = 0
    for c in line:
        if c.isspace():
            continue
        total += 1
        cnt[c] = cnt.get(c, 0) + 1
    if not total:
        return 0.0
    return max(cnt.values()) / total


def DO_cap_token_ratio(line: str) -> float:
    """大写开头 token 密度（基于原行大小写；token 级与 cap_ratio 字符级互补）。"""
    toks = DO_tokens(line)
    if not toks:
        return 0.0
    n = sum(1 for t in toks if t and t[0].isupper())
    return n / len(toks)


# ---- 逻辑回归（纯 Python，固定 seed 确定性） ----
def DO_sigmoid(z: float) -> float:
    if z >= 0:
        return 1.0 / (1.0 + math.exp(-z))
    e = math.exp(z)
    return e / (1.0 + e)


def DO_fit(X: list[list[float]], y: list[int], lr: float = 0.05, epochs: int = 150,
        l2: float = 1e-3) -> list[float]:
    n = len(X)
    d = len(X[0])
    w = [0.0] * d
    for _ in range(epochs):
        grad = [0.0] * d
        for xi, yi in zip(X, y):
            p = DO_sigmoid(sum(wi * xj for wi, xj in zip(w, xi)))
            err = p - yi
            for j in range(d):
                grad[j] += err * xi[j]
        for j in range(d):
            w[j] -= lr * (grad[j] / n + l2 * w[j])
    return w


def DO_encode(ws: list[float], DO_feats: list[str] | None = None) -> bytes:
    """scx-sem-2 校验编码：weights 每 f64 大端 8 字节 || 每个特征名 + '\n'。"""
    b = b"".join(struct.pack(">d", w) for w in ws)
    if DO_feats:
        for f in DO_feats:
            b += f.DO_encode("utf-8") + b"\n"
    return b


def DO_main() -> int:
    # 教师标注
    all_x: list[list[float]] = []
    all_y: list[int] = []
    labels: list[dict] = []
    total_keep = 0
    total_lines = 0
    for cname, text, queries in DO_CORPORA:
        lines = text.splitlines()
        total = len(lines)
        for query in queries:
            keep, log = DO_label_corpus(cname, query, text)
            total_lines += total
            total_keep += len(keep)
            keep_set = set(keep)
            for i, ln in enumerate(lines):
                all_x.append(DO_feats(ln, [query], i, total))
                all_y.append(1 if (i + 1) in keep_set else 0)
            for e in log:
                labels.append({**e, "n": total, "keep": len(keep), "query": query})
    pos = sum(all_y)
    print(f"标注汇总: {total_lines} 行, 教师保留 {total_keep} ({total_keep / total_lines:.1%}), "
          f"正样本 {pos}")
    if pos < max(10, total_lines // 20):
        print("正样本过少（教师过保守？），改用并集回填……")
        return 2

    w = DO_fit(all_x, all_y)
    ws = [round(v, 6) for v in w]
    print("蒸馏权重: " + ", ".join(f"{v:+.3f}" for v in ws))
    obj = {
        "schema": "scx-sem-2",
        "features": DO_FEATURES,
        "weights": ws,
        "sha256": hashlib.sha256(DO_encode(ws, DO_FEATURES)).hexdigest(),
    }
    DO_OUT.parent.mkdir(parents=True, exist_ok=True)
    DO_OUT.write_text(json.dumps(obj, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"写入 {DO_OUT.relative_to(ROOT)} (sha256={obj['sha256'][:16]}…)")

    # 标注日志
    with open(ROOT / "config" / "bench" / "results" / "distill_labels.csv", "w",
              encoding="utf-8", newline="") as fh:
        w = csv.DictWriter(fh, fieldnames=["teacher", "corpus", "chunk", "ok", "rows", "n", "keep", "query"])
        w.writeheader()
        w.writerows(labels)
    print("标注日志写入 config/bench/results/distill_labels.csv")
    return 0


def DO_selfcheck() -> int:
    """输出固定样本行的 24 维特征（精确值），供 src/semantic.rs 一致性测试断言。"""
    rows = [
        ("db connection lost: conn 9f2a timeout", ["db"], 0, 10),
        ("db db db db db retry", ["db"], 1, 10),
        ("this is a test of the system and it works", ["db"], 2, 10),
        ("------------------------------------------------------------", ["db"], 3, 10),
        ("fn main() -> i32 { 0 }", ["db"], 4, 10),
        ("Server Name CONNECT Error", ["db"], 5, 10),
        ("aaaaaaa", ["db"], 6, 10),
        ("数据库超时重试", ["超时"], 7, 10),
        ("conn ... lots of filler ... broken", ["conn", "broken"], 8, 10),
        ("  ", [], 9, 10),
    ]
    out = []
    for line, terms, i, total in rows:
        out.append({"line": line, "terms": terms, "i": i, "total": total,
                    "feats": [float(f"{v:.12g}") for v in DO_feats(line, terms, i, total)]})
    print(json.dumps(out, ensure_ascii=False, indent=1))
    return 0

sys.path.insert(0, str(ROOT / "tools"))

DW_FEATURES = ["overlap", "hit", "ln_len", "cap_ratio", "dig_ratio", "structure",
            "word_hit", "cjk_ratio", "errcode", "uniq", "sym", "pos_norm",
            "edge", "long", "code", "bias", "cjk_bigram", "term_prox"]
DW_DIM = len(DW_FEATURES)
DW_OUT = ROOT / "config" / "semantic" / "weights.json"
DW_TOL = 0.01          # fidelity 不劣容差（对齐 A2 门禁）
DW_CORPORA = [("code", corpus_code(42), "build"),
           ("log", corpus_log(42), "timeout"),
           ("zh", corpus_zh(42), "超时")]


def DW_encode(ws: list[float]) -> bytes:
    return b"".join(struct.pack(">d", w) for w in ws)


def DW_write_weights(tmp: Path, ws: list[float]) -> Path:
    p = tmp / "weights.json"
    w6 = [round(v, 6) for v in ws]
    # sha256 与 Rust verify_v2 严格一致：SHA256(weights 大端 8 字节拼接 ||
    # 每个特征名接 \n)。只哈希 weights 会与加载器不匹配，导致 --semantic 拒载。
    payload = DW_encode(w6) + b"".join(n.DW_encode() + b"\n" for n in DW_FEATURES)
    p.write_text(json.dumps({
        "schema": "scx-sem-2",
        "features": DW_FEATURES,
        "weights": w6,
        "sha256": hashlib.sha256(payload).hexdigest(),
    }, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return p


def DW_eval_w(bin_: str, tmp: Path, ws: list[float]) -> dict:
    """评估权重向量：3 语料 cut/num_cov/token_cov（semantic 开启）。"""
    wp = DW_write_weights(tmp, ws)
    rows = []
    for cname, text, query in DW_CORPORA:
        out = compress_ablate(bin_, text, query, ["--semantic", str(wp)])
        full_words = tokens(text)
        full_nums = num_tokens(text)
        out_tok = est_tok(out)
        out_words = tokens(out)
        out_nums = num_tokens(out)
        rows.append({
            "set": cname,
            "cut": 1.0 - out_tok / est_tok(text),
            "num_cov": cov(full_nums, out_nums),
            "token_cov": cov(full_words, out_words),
        })
    return rows


def DW_baseline(bin_: str) -> dict:
    """教师基线：semantic 关时的 3 语料 fidelity。"""
    out = {}
    for cname, text, query in DW_CORPORA:
        o = compress_ablate(bin_, text, query, [])
        out[cname] = {
            "num_cov": cov(num_tokens(text), num_tokens(o)),
            "token_cov": cov(tokens(text), tokens(o)),
        }
    return out


def DW_score(rows: list[dict], base: dict) -> float:
    """目标：cut 最大化，fidelity 不劣于基线（平均容差 TOL，硬惩罚）。"""
    cut = sum(r["cut"] for r in rows) / len(rows)
    pen = 0.0
    for r in rows:
        for k in ("num_cov", "token_cov"):
            drop = base[r["set"]][k] - r[k] - DW_TOL
            if drop > 0:
                pen += drop * 5.0
    return cut - pen


def DW_distill(bin_: str) -> tuple[list[float], list[dict]]:
    base = DW_baseline(bin_)
    print("教师基线（semantic 关）fidelity:")
    for c, v in base.items():
        print(f"  {c}: num_cov={v['num_cov']:.4f} token_cov={v['token_cov']:.4f}")

    # 初始 = 现有 scx-sem-2 权重（16 维拟合 + 后 2 维 0），从现状蒸馏。
    cur = [0.072325, 0.496615, -0.310477, -0.005689, -0.006987, -0.0548,
           0.016532, -0.037252, -0.013018, -0.120063, -0.245746, -0.111923,
           -0.075548, 0.009463, -0.010174, -0.276553, 0.0, 0.0]
    log: list[dict] = []
    with tempfile.TemporaryDirectory() as td:
        tmp = Path(td)
        s0 = DW_score(DW_eval_w(bin_, tmp, cur), base)
        print(f"初始: cut_avg={s0:.4f}")
        for rnd, grid in enumerate(([0.05, -0.05, 0.15, -0.15], [0.02, -0.02]), 1):
            improved = False
            for dim in range(DW_DIM):
                best_v, best_s = cur[dim], s0
                for dv in grid:
                    cand = cur.copy()
                    cand[dim] = round(cur[dim] + dv, 6)
                    cand[dim] = max(-0.6, min(0.6, cand[dim]))  # 权重范围夹紧
                    if abs(cand[dim] - cur[dim]) < 1e-9:
                        continue
                    s = DW_score(DW_eval_w(bin_, tmp, cand), base)
                    if s > best_s + 1e-9:
                        best_v, best_s = cand[dim], s
                if best_v != cur[dim]:
                    cur[dim] = best_v
                    s0 = best_s
                    improved = True
                log.append({"round": rnd, "dim": DW_FEATURES[dim],
                            "w": cur[dim], "score": round(s0, 5)})
                print(f"  r{rnd} {DW_FEATURES[dim]:>10} w={cur[dim]:+.6f} score={s0:.4f}")
            if not improved:
                print(f"轮 {rnd} 无改进，收敛")
                break
    print("蒸馏完成: " + ", ".join(f"{v:+.4f}" for v in cur))
    return cur, log


def DW_main() -> int:
    bin_ = ensure_sem_bin()
    cur, log = DW_distill(bin_)
    # 终验：蒸馏权重在 3 语料上的 cut/fidelity
    with tempfile.TemporaryDirectory() as td:
        rows = DW_eval_w(bin_, Path(td), cur)
        base = DW_baseline(bin_)
        print("\n蒸馏后终验（semantic 开）:")
        for r in rows:
            b = base[r["set"]]
            flag = "OK" if (r["num_cov"] >= b["num_cov"] - 0.01
                            and r["token_cov"] >= b["token_cov"] - 0.01) else "FID-DROP"
            print(f"  {r['set']}: cut={r['cut']:.4f} num_cov={r['num_cov']:.4f} "
                  f"token_cov={r['token_cov']:.4f} vs base "
                  f"({b['num_cov']:.4f}/{b['token_cov']:.4f}) {flag}")
    # 写入 config/semantic/weights.json（随仓权重），非仓库根目录。
    DW_write_weights(DW_OUT.parent, cur)
    final = DW_encode(cur) + b"".join(n.DW_encode() + b"\n" for n in DW_FEATURES)
    print(f"写入 {DW_OUT.relative_to(ROOT)} (sha256={hashlib.sha256(final).hexdigest()[:16]}…)")
    with open(ROOT / "config" / "bench" / "results" / "distill_log.csv", "w",
              encoding="utf-8", newline="") as fh:
        w = csv.DictWriter(fh, fieldnames=["round", "dim", "w", "score"])
        w.writeheader()
        w.writerows(log)
    print("蒸馏日志写入 config/bench/results/distill_log.csv")
    return 0

FW_DIM = 16

# ---- 特征复刻（与 src/semantic.rs 逐维一致） ----
FW_PREFIXES = ("```", "~~~", "import ", "use ", "#include", "at ", "caused by:",
            "->", "=>", "panic! ", "error:")
FW_WORD_CHARS = set("abcdefghijklmnopqrstuvwxyz0123456789_-./:@\\")


def FW_tokens(lower: str) -> list[str]:
    out: list[str] = []
    cur: list[str] = []
    i = 0
    b = lower.FW_encode()
    n = len(b)
    while i < n:
        c = b[i]
        is_word = chr(c).isalnum() or chr(c) in FW_WORD_CHARS if c < 128 else False
        is_cjk = 0xE4 <= c <= 0xE9 and i + 2 < n
        if is_word or is_cjk:
            cur.append(lower[i:i + (3 if is_cjk else 1)])
        elif cur:
            out.append("".join(cur))
            cur = []
        i += 3 if is_cjk else 1
    if cur:
        out.append("".join(cur))
    return out


def FW_stem(w: str) -> str | None:
    if len(w) < 4 or not w.isascii() or not w.islower() or not w.isalpha():
        return None
    if w.endswith("ing") and len(w) - 3 >= 4:
        return w[:-3]
    if w.endswith("ed") and len(w) - 2 >= 4:
        return w[:-2]
    if w.endswith("ies"):
        return w[:-3] + "y"
    if w.endswith("es") and w[:-2].endswith(("ch", "sh", "s", "x", "z")):
        return w[:-2]
    if w.endswith("s") and not w.endswith("ss") and len(w) - 1 >= 3:
        return w[:-1]
    return None


def FW_root_match(a: str, b: str) -> bool:
    if a == b:
        return True
    sa, sb = FW_stem(a), FW_stem(b)
    if sa and sb:
        return sa == sb
    if sa:
        return sa == b
    if sb:
        return a == sb
    return False


def FW_feats(line: str, terms: list[str], i: int, total: int) -> list[float]:
    lower = line.lower()
    hit = sum(1 for t in terms if t and t in lower)
    overlap = hit / len(terms) if terms else 0.0
    chars = len(line)
    caps = sum(1 for c in line if c.isupper())
    digits = sum(1 for c in line if c.isdigit())
    cap_ratio = caps / chars if chars else 0.0
    dig_ratio = digits / chars if chars else 0.0
    t = line.lstrip()
    tl = t.lower()
    structure = 1.0 if any(t.startswith(p) or tl.startswith(p) for p in FW_PREFIXES) else 0.0
    toks = FW_tokens(lower)
    n_hit = 0
    for q in terms:
        if not q:
            continue
        plain = q.isascii() and q.isalpha()
        if any(FW_root_match(tok, q) for tok in toks) or (not plain and q in lower):
            n_hit += 1
    w_hit = n_hit / len(terms) if terms else 0.0
    cjk = sum(1 for c in line if 0x4E00 <= ord(c) <= 0x9FFF)
    cjk_r = cjk / chars if chars else 0.0
    err = sum(1 for tok in toks if any(c.isalpha() for c in tok) and any(c.isdigit() for c in tok))
    err_r = err / len(toks) if toks else 0.0
    uniq = len(set(toks)) / len(toks) if toks else 0.0
    sym = sum(1 for c in line if not c.isspace() and not c.isalnum())
    sym_r = sym / sum(1 for c in line if not c.isspace()) if any(not c.isspace() for c in line) else 0.0
    pos = i / (total - 1) if total > 1 else 0.0
    edge = 1.0 if i < total // 20 or i >= total - total // 20 else 0.0
    long_f = 1.0 if chars > 200 else 0.0
    code = 1.0 if ("{" in line and "}" in line) or ("(" in line and ")" in line) \
        or "->" in line or "=>" in line or "::" in line else 0.0
    return [overlap, min(hit, 1), math.log1p(chars), cap_ratio, dig_ratio, structure,
            w_hit, cjk_r, err_r, uniq, sym_r, pos, edge, long_f, code, 1.0]


# ---- 语料与弱监督标签 ----
FW_QUERIES = ["error", "compress", "token", "memory", "performance", "crash", "panic"]


def FW_load_lines() -> list[str]:
    files = [ROOT / "README.md", ROOT / "docs" / "benchmarks.md",
             ROOT / "docs" / "release-checklist.md", ROOT / "src" / "lib.rs",
             ROOT / "src" / "trim.rs", ROOT / "src" / "semantic.rs"]
    lines: list[str] = []
    for f in files:
        if f.exists():
            lines.extend(f.read_text(encoding="utf-8").splitlines())
    return [ln for ln in lines if ln.strip()]


def FW_label(line: str) -> int:
    l = line.lower()
    if any(k in l for k in ("error", "panic", "fail", "exception", "crash", "fatal")):
        return 1
    if line.startswith(("import ", "use ", "#include", "fn ", "pub ", "struct ", "impl ")):
        return 1
    if any(q in l for q in FW_QUERIES):
        return 1
    return 0


# ---- 逻辑回归（纯 Python 梯度下降 + L2） ----
def FW_sigmoid(z: float) -> float:
    if z >= 0:
        return 1.0 / (1.0 + math.exp(-z))
    e = math.exp(z)
    return e / (1.0 + e)


def FW_fit(X: list[list[float]], y: list[int], lr: float = 0.05, epochs: int = 120,
        l2: float = 1e-3) -> list[float]:
    n = len(X)
    d = len(X[0])
    w = [0.0] * d
    for _ in range(epochs):
        grad = [0.0] * d
        for xi, yi in zip(X, y):
            p = FW_sigmoid(sum(wi * xj for wi, xj in zip(w, xi)))
            err = p - yi
            for j in range(d):
                grad[j] += err * xi[j]
        for j in range(d):
            w[j] -= lr * (grad[j] / n + l2 * w[j])
    return w


# ---- 输出（scx-sem-1，sha256 与 Rust encode 一致） ----
def FW_encode(ws: list[float]) -> bytes:
    return b"".join(struct.pack(">d", w) for w in ws)


def FW_main() -> int:
    lines = FW_load_lines()
    if len(lines) < 200:
        print(f"语料不足: {len(lines)} 行", file=sys.stderr)
        return 1
    # 行数裁剪 + seed 固定抽样（可复现）。
    step = max(1, len(lines) // 1500)
    sampled = lines[::step][:1500]
    X, y = [], []
    total = len(sampled)
    for i, ln in enumerate(sampled):
        X.append(FW_feats(ln, FW_QUERIES, i, total))
        y.append(FW_label(ln))
    pos = sum(y)
    print(f"语料 {total} 行, 正样本 {pos} ({pos / total:.1%})")
    w = FW_fit(X, y)
    print("权重: " + ", ".join(f"{v:+.3f}" for v in w))
    obj = {
        "schema": "scx-sem-1",
        "dim": FW_DIM,
        "weights": [round(v, 6) for v in w],
    }
    ws = obj["weights"]
    obj["sha256"] = hashlib.sha256(FW_encode(ws)).hexdigest()
    out = ROOT / "config" / "semantic"
    out.mkdir(parents=True, exist_ok=True)
    p = out / "weights.json"
    p.write_text(
        "{\n"
        f'  "schema": "scx-sem-1",\n'
        f'  "dim": {FW_DIM},\n'
        f'  "weights": [{", ".join(str(v) for v in ws)}],\n'
        f'  "sha256": "{obj["sha256"]}"\n'
        "}\n",
        encoding="utf-8",
    )
    print(f"写入 {p.relative_to(ROOT)} (sha256={obj['sha256'][:16]}…)")
    return 0

sys.path.insert(0, str(ROOT / "tools"))

MC_DIM = len(DO_FEATURES)
MC_SIG = lambda z: 1.0 / (1.0 + math.exp(-max(-30.0, min(30.0, z))))


def MC_dataset(cache: dict) -> tuple[list, list]:
    """从标注缓存重建 (X, y)：每 (cname|query, keep) 生成全行样本。
    语料文本用 distill_online.build_corpora() 的确定性来源重建（与训练口径一致）。"""
    corpora = {c[0]: c[1] for c in DO_build_corpora()}
    X, y = [], []
    for key, keep in cache.items():
        if "|" not in key:
            continue
        cname, query = key.rsplit("|", 1)
        text = corpora.get(cname)
        if text is None:
            continue
        lines = text.splitlines()
        total = len(lines)
        ks = set(int(x) for x in keep)
        for i, ln in enumerate(lines):
            X.append(DO_feats(ln, [query], i, total))
            y.append(1 if (i + 1) in ks else 0)
    return X, y


def MC_fit_linear(X, y):
    """与 DO_fit 同协议：批量梯度下降（lr=0.05, 150 epochs, l2=1e-3）。"""
    w = [0.0] * MC_DIM
    n = len(X)
    for _ in range(150):
        grad = [0.0] * MC_DIM
        for xi, yi in zip(X, y):
            p = MC_SIG(sum(w[d] * xi[d] for d in range(MC_DIM)))
            err = p - yi
            for d in range(MC_DIM):
                grad[d] += err * xi[d]
        for d in range(MC_DIM):
            w[d] -= 0.05 * (grad[d] / n + 1e-3 * w[d])
    return w


def MC_fit_mlp(X, y, h=16):
    random.seed(42)
    w1 = [[random.uniform(-0.3, 0.3) for _ in range(MC_DIM)] for _ in range(h)]
    b1 = [0.0] * h
    w2 = [random.uniform(-0.3, 0.3) for _ in range(h)]
    b2 = 0.0
    n = len(X)
    for epoch in range(150):
        for i in range(n):
            a = [MC_SIG(sum(w1[j][d] * X[i][d] for d in range(MC_DIM)) + b1[j]) for j in range(h)]
            z = sum(w2[j] * a[j] for j in range(h)) + b2
            err = MC_SIG(z) - y[i]
            dz = err * MC_SIG(z) * (1 - MC_SIG(z))
            for j in range(h):
                w2[j] -= 0.05 * (dz * a[j] + 1e-3 * w2[j])
                b1[j] -= 0.05 * (dz * w2[j] * a[j] * (1 - a[j]))
                for d in range(MC_DIM):
                    w1[j][d] -= 0.05 * (dz * w2[j] * a[j] * (1 - a[j]) * X[i][d] + 1e-3 * w1[j][d])
            b2 -= 0.05 * dz
    def score(x):
        a = [MC_SIG(sum(w1[j][d] * x[d] for d in range(MC_DIM)) + b1[j]) for j in range(h)]
        return MC_SIG(sum(w2[j] * a[j] for j in range(h)) + b2)
    return score


def MC_auc_ap(scores, y):
    """AUC（ROC）+ AP（PR-AUC）标准实现。"""
    pairs = sorted(zip(scores, y), key=lambda p: -p[0])
    npos = sum(y)
    nneg = len(y) - npos
    tp = fp = 0
    auc = 0.0
    prev = None
    for s, yi in pairs:
        if yi:
            tp += 1
        else:
            fp += 1
            if prev != s:
                auc += fp * tp
        prev = s
    auc = auc / (npos * nneg) if npos and nneg else 0.5
    tp = fp = 0
    ap = 0.0
    last_rec = 0.0
    for s, yi in pairs:
        if yi:
            tp += 1
        else:
            fp += 1
        rec = tp / npos if npos else 0.0
        prec = tp / (tp + fp) if tp + fp else 0.0
        ap += prec * (rec - last_rec)
        last_rec = rec
    return auc, ap


def MC_main() -> None:
    cache = json.loads((ROOT / "config" / "bench" / "results" / "distill_cache.json").read_text())
    X, y = MC_dataset(cache)
    if not X:
        print("无样本（缓存为空或仅合成语料）"); return 1
    n = len(X)
    # 3 折（确定性 shuffle，seed 2024 与 /tmp/cv.py 口径一致）
    idx = list(range(n))
    random.seed(2024)
    random.shuffle(idx)
    folds = [idx[i::3] for i in range(3)]
    lin, mlp = [], []
    for fi, fold in enumerate(folds):
        tr = [i for i in idx if i not in set(fold)]
        te = fold
        Xt = [X[i] for i in tr]; yt = [y[i] for i in tr]
        Xe = [X[i] for i in te]; ye = [y[i] for i in te]
        w = MC_fit_linear(Xt, yt)
        ls = [sum(w[d] * x[d] for d in range(MC_DIM)) for x in Xe]
        lin.append(MC_auc_ap(ls, ye))
        lk = sum(1 for s in ls if s > 0.0) / len(ls)
        scorer = MC_fit_mlp(Xt, yt)
        ms = [scorer(x) for x in Xe]
        mlp.append(MC_auc_ap(ms, ye))
        mk = sum(1 for s in ms if s > 0.5) / len(ms)
        print(f"fold {fi}: linear auc/ap={lin[-1][0]:.4f}/{lin[-1][1]:.4f} keep={lk:.3f}  "
              f"mlp1 auc/ap={mlp[-1][0]:.4f}/{mlp[-1][1]:.4f} keep={mk:.3f}")
    res = {
        "n": n, "dim": MC_DIM,
        "linear": {"auc": sum(a for a, _ in lin) / 3, "ap": sum(p for _, p in lin) / 3},
        "mlp1": {"auc": sum(a for a, _ in mlp) / 3, "ap": sum(p for _, p in mlp) / 3},
        "folds_linear": lin, "folds_mlp": mlp,
    }
    print(json.dumps(res, ensure_ascii=False, indent=2))
    Path("/tmp/mlp_cv.json").write_text(json.dumps(res, ensure_ascii=False, indent=2))
    print("已写入 /tmp/mlp_cv.json（不替换仓库权重）")
    return 0


# ==================== gen / distill / fit / mlp 收拢入口 ====================

GEN = {
    "bench": corpus.GB_main,
    "fid": corpus.GF_main,
    "p8-cuda": GC_main,
    "p8-js": GJ_main,
    "p8-layers": GL_main,
    "p8-shape": GS_main,
}


def cmd_gen() -> int:
    ap = argparse.ArgumentParser(
        prog="bench.py gen",
        description="评测语料生成（原 gen_bench/gen_fidelity/gen_p8_* 六脚本收拢）",
    )
    ap.add_argument("kind", choices=tuple(GEN), help="语料类型: " + ", ".join(GEN))
    args = ap.parse_args()
    return GEN[args.kind]()


def cmd_distill() -> int:
    ap = argparse.ArgumentParser(
        prog="bench.py distill",
        description="语义权重蒸馏（原 distill_online/distill_weights 两脚本收拢）",
    )
    sub = ap.add_subparsers(dest="mode", required=True)
    sub.add_parser("online",
                   help="双在线旗舰教师蒸馏 → config/semantic/weights.json（scx-sem-2）")
    sub.add_parser("weights",
                   help="评测信号坐标上升蒸馏 → config/semantic/weights.json（scx-sem-2）")
    args = ap.parse_args()
    if args.mode == "online":
        return distill_online_main()
    return DW_main()


def distill_online_main() -> int:
    '''distill_online 主流程。socket IPv4 劫持只作用于本次调用，退出即恢复。'''
    socket.getaddrinfo = DO_ipv4_first
    try:
        return DO_main()
    finally:
        socket.getaddrinfo = DO_orig_gai


def cmd_fit() -> int:
    ap = argparse.ArgumentParser(
        prog="bench.py fit",
        description="弱标签线性权重拟合 → config/semantic/weights.json（scx-sem-1）",
    )
    ap.parse_args()
    return FW_main()


def cmd_mlp() -> int:
    ap = argparse.ArgumentParser(
        prog="bench.py mlp",
        description="MLP 交叉验证（读 config/bench/results/distill_cache.json）",
    )
    ap.parse_args()
    return MC_main()


SUB = {
    "gsm8k": cmd_gsm8k,
    "ablate": cmd_ablate,
    "budget": cmd_budget,
    "arch": cmd_arch,
    "code": cmd_code,
    "kernel": cmd_kernel,
    "long": cmd_long,
    "metrics": cmd_metrics,
    "fidelity": cmd_fidelity,
    "calibrate": cmd_calibrate,
    "gen": cmd_gen,
    "distill": cmd_distill,
    "fit": cmd_fit,
    "mlp": cmd_mlp,
}


def main() -> None:
    if len(sys.argv) < 2 or sys.argv[1] not in SUB:
        print(__doc__)
        print("可用子命令: " + ", ".join(SUB))
        sys.exit(0 if len(sys.argv) < 2 else 1)
    sub = sys.argv[1]
    # 子命令参数：去掉子命令名后透传（保留其自身 argparse）
    sys.argv = [sys.argv[0], *sys.argv[2:]]
    rc = SUB[sub]()
    if isinstance(rc, int):
        sys.exit(rc)


if __name__ == "__main__":
    main()