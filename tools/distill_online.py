#!/usr/bin/env python3
"""semantic 权重在线蒸馏（双旗舰教师 → 紧凑学生权重）。

教师 = 双在线旗舰（DeepSeek flash + Qwen 3.8-flash，temperature 0.1，
交叉标注：两模型一致保留才算保留）——在文档上下文中对每行做
"LLM 问答场景保留/删除"判断，输出行号 JSON。
学生 = 18 维特征线性权重（scx-sem-2）——纯 stdlib 逻辑回归拟合，
产物仍是一个几 KB 的 weights.json（体积不膨胀、零模型、零依赖）。

蒸馏语义：把旗舰模型的保留/删除知识蒸馏进紧凑权重；18 维全启用
（cjk_bigram/term_prox 不再锁 0）。默认路径不受影响（semantic 默认关）。

用法: tools/distill_online.py [--out config/semantic/weights.json]
key 来源：DEEPSEEK_API_KEY env（~/.zshrc）；QWEN key 走 keychain
条目 scx-qwen-api（不进 git / 不进脚本）。
"""
from __future__ import annotations
import csv
import hashlib
import json
import math
import os
import re
import struct
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "tools"))
import bench_ablate as ba  # 确定性语料 / compress / fidelity 口径

FEATURES = ["overlap", "hit", "ln_len", "cap_ratio", "dig_ratio", "structure",
            "word_hit", "cjk_ratio", "errcode", "uniq", "sym", "pos_norm",
            "edge", "long", "code", "bias", "cjk_bigram", "term_prox"]
DIM = len(FEATURES)
OUT = ROOT / "config" / "semantic" / "weights.json"
CORPORA = [("code", ba.corpus_code(42), "build"),
           ("log", ba.corpus_log(42), "timeout"),
           ("zh", ba.corpus_zh(42), "超时")]

TEACHERS = [
    {"name": "deepseek", "url": "https://api.deepseek.com/chat/completions",
     "model": os.environ.get("SCX_MODEL_DEEPSEEK", "deepseek-chat"),
     "key": lambda: os.environ.get("DEEPSEEK_API_KEY", "")},
    {"name": "qwen", "url": "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions",
     "model": os.environ.get("SCX_MODEL_QWEN", "qwen3.8-flash"),
     "key": lambda: subprocess.run(
         ["security", "find-generic-password", "-s", "scx-qwen-api", "-w"],
         capture_output=True, text=True).stdout.strip()},
]


def call_teacher(t: dict, prompt: str, max_new: int = 1500) -> str:
    """OpenAI 兼容调用，重试 2 次，返回 content 文本。"""
    key = t["key"]()
    if not key:
        raise RuntimeError(f"{t['name']}: key 缺失")
    body = {"model": t["model"], "messages": [{"role": "user", "content": prompt}],
            "max_tokens": max_new, "temperature": 0.1}
    for attempt in (1, 2, 3):
        try:
            req = urllib.request.Request(
                t["url"],
                data=json.dumps(body).encode(),
                headers={"Authorization": f"Bearer {key}", "Content-Type": "application/json"})
            r = json.load(urllib.request.urlopen(req, timeout=120))
            return r["choices"][0]["message"]["content"] or ""
        except Exception as e:
            if attempt == 3:
                raise RuntimeError(f"{t['name']}: {e}")
            time.sleep(2 * attempt)
    raise RuntimeError(f"{t['name']}: unreachable")


CHUNK = 150  # 分块标注：每块行数（避免教师输出被 max_tokens 截断）


def teacher_prompt(cname: str, query: str, lines: list[str], base: int) -> str:
    """带全局行号的文本块 + query，请教师输出保留行号数组（严格格式）。"""
    body = "\n".join(f"{base + i + 1}| {ln}" for i, ln in enumerate(lines))
    return (
        f"你是上下文治理专家。这是 {cname} 语料的一部分（每行带全局行号）。"
        f"场景：压缩后仍需回答「{query}」。\n"
        "对每一行判断：对回答该问题或维持语义结构必要则保留，否则删除。\n"
        "只输出保留行号的纯数字 JSON 数组，例如 [1,5,9]。禁止输出对象、解释或逐行分析。\n"
        f"文本：\n{body}"
    )


def parse_rows(text: str, n: int) -> list[int] | None:
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


def label_chunk(cname: str, query: str, lines: list[str], base: int,
                t: dict, log: list[dict]) -> set[int]:
    """单教师单块标注（独立调用，供并行）。失败回退全保留（保守）。"""
    for attempt in (1, 2):
        try:
            raw = call_teacher(t, teacher_prompt(cname, query, lines, base))
            rows = parse_rows(raw, base + len(lines))
            if rows is not None:
                log.append({"teacher": t["name"], "corpus": cname, "chunk": base // CHUNK,
                            "ok": len(rows), "rows": ",".join(map(str, rows))})
                return set(rows)
        except Exception as e:
            raw = f"ERROR: {e}"
        if attempt == 2:
            log.append({"teacher": t["name"], "corpus": cname, "chunk": base // CHUNK,
                        "ok": 0, "rows": "ERROR"})
    return set(range(base + 1, base + len(lines) + 1))


CACHE = ROOT / "config" / "bench" / "results" / "distill_cache.json"


def label_corpus(cname: str, query: str, text: str) -> tuple[list[int], list[dict]]:
    """分块 + 双教师并行标注（带缓存）；并集（任一教师认为必要即保留）。"""
    lines = text.splitlines()
    n = len(lines)
    cache: dict = {}
    if CACHE.exists():
        cache = json.loads(CACHE.read_text(encoding="utf-8"))
    if cname in cache:
        keep = [int(x) for x in cache[cname]]
        print(f"[{cname}] {n} 行: 命中缓存 {len(keep)} 行")
        return keep, [{"teacher": "cache", "corpus": cname, "chunk": -1, "ok": len(keep), "rows": ""}]
    log: list[dict] = []
    keep_sets = {t["name"]: set() for t in TEACHERS}
    chunks = [lines[i:i + CHUNK] for i in range(0, n, CHUNK)]
    from concurrent.futures import ThreadPoolExecutor
    for t in TEACHERS:
        with ThreadPoolExecutor(max_workers=8) as ex:
            futs = [ex.submit(label_chunk, cname, query, ch, i * CHUNK, t, log)
                    for i, ch in enumerate(chunks) if ch]
        for f in futs:
            keep_sets[t["name"]] |= f.result()
    union = set.union(*keep_sets.values())
    print(f"[{cname}] {n} 行: deepseek={len(keep_sets['deepseek'])} "
          f"qwen={len(keep_sets['qwen'])} 并集={len(union)}")
    cache[cname] = sorted(union)
    CACHE.parent.mkdir(parents=True, exist_ok=True)
    CACHE.write_text(json.dumps(cache), encoding="utf-8")
    return sorted(union), log


# ---- 18 维特征复刻（与 src/semantic.rs feats 逐维一致） ----
def tokens(lower: str) -> list[str]:
    out: list[str] = []
    cur: list[str] = []
    i = 0
    b = lower.encode()
    n = len(b)
    while i < n:
        c = b[i]
        is_word = chr(c).isalnum() if c < 128 else False
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


def cjk_chars(s: str) -> list[str]:
    return [c for c in s if 0x4E00 <= ord(c) <= 0x9FFF]


def cjk_bigram(line: str, terms: list[str]) -> float:
    q = "".join(cjk_chars("".join(terms)))
    if len(q) < 2:
        return 0.0
    qp = {q[i:i + 2] for i in range(len(q) - 1)}
    lc = "".join(cjk_chars(line))
    if len(lc) < 2:
        return 0.0
    hit = sum(1 for i in range(len(lc) - 1) if lc[i:i + 2] in qp)
    return hit / max(1, len(q) - 1)


def term_prox(lower: str, terms: list[str]) -> float:
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


def feats(line: str, terms: list[str], i: int, total: int) -> list[float]:
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
    toks = tokens(lower)
    w_hit = hit / len(terms) if terms else 0.0
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
    bigram = cjk_bigram(line, terms)
    prox = term_prox(lower, terms)
    return [overlap, min(hit, 1), math.log1p(chars), cap_ratio, dig_ratio, structure,
            w_hit, cjk, err_r, uniq, sym_r, pos, edge, long_f, code, 1.0, bigram, prox]


# ---- 逻辑回归（纯 Python，固定 seed 确定性） ----
def sigmoid(z: float) -> float:
    if z >= 0:
        return 1.0 / (1.0 + math.exp(-z))
    e = math.exp(z)
    return e / (1.0 + e)


def fit(X: list[list[float]], y: list[int], lr: float = 0.05, epochs: int = 150,
        l2: float = 1e-3) -> list[float]:
    n = len(X)
    d = len(X[0])
    w = [0.0] * d
    for _ in range(epochs):
        grad = [0.0] * d
        for xi, yi in zip(X, y):
            p = sigmoid(sum(wi * xj for wi, xj in zip(w, xi)))
            err = p - yi
            for j in range(d):
                grad[j] += err * xi[j]
        for j in range(d):
            w[j] -= lr * (grad[j] / n + l2 * w[j])
    return w


def encode(ws: list[float], feats: list[str] | None = None) -> bytes:
    """scx-sem-2 校验编码：weights 每 f64 大端 8 字节 || 每个特征名 + '\n'。"""
    b = b"".join(struct.pack(">d", w) for w in ws)
    if feats:
        for f in feats:
            b += f.encode("utf-8") + b"\n"
    return b


def main() -> int:
    # 教师标注
    all_x: list[list[float]] = []
    all_y: list[int] = []
    labels: list[dict] = []
    total_keep = 0
    total_lines = 0
    for cname, text, query in CORPORA:
        keep, log = label_corpus(cname, query, text)
        lines = text.splitlines()
        total = len(lines)
        total_lines += total
        total_keep += len(keep)
        keep_set = set(keep)
        for i, ln in enumerate(lines):
            all_x.append(feats(ln, [query], i, total))
            all_y.append(1 if (i + 1) in keep_set else 0)
        for e in log:
            labels.append({**e, "n": total, "keep": len(keep)})
    pos = sum(all_y)
    print(f"标注汇总: {total_lines} 行, 教师保留 {total_keep} ({total_keep / total_lines:.1%}), "
          f"正样本 {pos}")
    if pos < max(10, total_lines // 20):
        print("正样本过少（教师过保守？），改用并集回填……")
        return 2

    w = fit(all_x, all_y)
    ws = [round(v, 6) for v in w]
    print("蒸馏权重: " + ", ".join(f"{v:+.3f}" for v in ws))
    obj = {
        "schema": "scx-sem-2",
        "features": FEATURES,
        "weights": ws,
        "sha256": hashlib.sha256(encode(ws, FEATURES)).hexdigest(),
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(obj, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"写入 {OUT.relative_to(ROOT)} (sha256={obj['sha256'][:16]}…)")

    # 标注日志
    with open(ROOT / "config" / "bench" / "results" / "distill_labels.csv", "w",
              encoding="utf-8", newline="") as fh:
        w = csv.DictWriter(fh, fieldnames=["teacher", "corpus", "chunk", "ok", "rows", "n", "keep"])
        w.writeheader()
        w.writerows(labels)
    print("标注日志写入 config/bench/results/distill_labels.csv")
    return 0


if __name__ == "__main__":
    sys.exit(main())
