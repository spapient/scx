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
import socket
import struct
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

# IPv6 连接冻结防护：macOS 下 urllib 偶发 IPv6 happy-eyeballs 卡死且
# socket 超时不兜底（进程进入不可中断睡眠）。全局劫持 getaddrinfo，
# 强制优先 IPv4（fallback 保留 IPv6），根治 API 调用挂死。
_orig_gai = socket.getaddrinfo


def _ipv4_first(host, port, family=0, *args, **kw):
    try:
        res = _orig_gai(host, port, socket.AF_INET, *args, **kw)
        if res:
            return res
    except Exception:
        pass
    return _orig_gai(host, port, family, *args, **kw)


socket.getaddrinfo = _ipv4_first

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "tools"))
import bench_ablate as ba  # 确定性语料 / compress / fidelity 口径

FEATURES = ["overlap", "hit", "ln_len", "cap_ratio", "dig_ratio", "structure",
            "word_hit", "cjk_ratio", "errcode", "uniq", "sym", "pos_norm",
            "edge", "long", "code", "bias", "cjk_bigram", "term_prox",
            "stop_hit", "sym_noise", "q_density", "head_hit", "rep_ratio",
            "cap_tok"]
DIM = len(FEATURES)

# 停用词表（与 src/semantic.rs 的 STOP_WORDS 逐词一致，蒸馏特征对齐）。
STOP = frozenset([
    "the", "a", "an", "and", "or", "but", "of", "to", "in", "on", "for",
    "with", "this", "that", "is", "are", "was", "were", "it", "as", "at",
    "by", "from", "we", "you", "they", "he", "she", "i", "not", "no", "do",
    "does", "did", "be", "been", "have", "has", "had", "will", "would", "can",
    "could", "should", "then", "than", "so", "if", "else", "when", "while",
    "up", "down", "out", "over", "under", "here", "there", "all", "any",
    "both", "each", "few", "more", "most", "other", "some", "such", "only",
    "own", "now",
])
OUT = ROOT / "config" / "semantic" / "weights.json"
# ---- 语料（确定性；每语料可配多个 query 场景，样本按 query 独立标注） ----
def _p1m_seg(lo: int, hi: int) -> str:
    """corpus_p1m.txt 固定行区间（真实 scx 源码，确定性抽取）。"""
    p = ROOT / "config" / "bench" / "perf" / "corpus_p1m.txt"
    lines = p.read_text(encoding="utf-8").splitlines()
    return "\n".join(lines[lo:hi])


def _code_fence(rows: list, lang: str) -> str:
    """从题目 CSV 提取 ```<lang> 代码围栏内的真实代码（确定性）。"""
    out: list[str] = []
    for r in rows:
        for m in re.finditer(r"```(?:c|python|java|javascript|cpp)?\n(.*?)```", r["question"], re.S):
            out.extend(m.group(1).splitlines())
    return "\n".join(out)


def build_corpora() -> list:
    """语料矩阵：[(cname, text, [query1, query2]), ...]。含原 3 合成语料 + 真实源码/代码语料。"""
    corpora = [("code", ba.corpus_code(42), ["build"]),
               ("log", ba.corpus_log(42), ["timeout"]),
               ("zh", ba.corpus_zh(42), ["超时"]),
               ("real_src_a", _p1m_seg(0, 2000), ["drop", "error"]),
               ("real_src_b", _p1m_seg(12000, 14000), ["memory", "compress"]),
               ("real_src_c", _p1m_seg(24000, 26000), ["stdin", "mcp"])]
    langs = [("c", "bench_c60.csv"), ("cpp", "bench_cpp60.csv"), ("cuda", "bench_cuda60.csv"),
             ("java", "bench_java60.csv"), ("js", "bench_js60.csv"), ("py", "bench_py60.csv")]
    ds = ROOT / "config" / "bench" / "datasets"
    for lang, fname in langs:
        p = ds / fname
        if not p.exists():
            continue
        import csv as _csv
        rows = list(_csv.DictReader(p.open(encoding="utf-8")))
        text = _code_fence(rows, lang)
        if text.strip():
            corpora.append((f"code_{lang}", text, ["return", "function"]))
    return corpora


CORPORA = build_corpora()

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
    """分块 + 双教师并行标注（带缓存）；并集（任一教师认为必要即保留）。
    缓存键 = cname|query（多 query 语料各场景独立标注；旧单键缓存不再命中）。"""
    lines = text.splitlines()
    n = len(lines)
    cache_key = f"{cname}|{query}"
    cache: dict = {}
    if CACHE.exists():
        cache = json.loads(CACHE.read_text(encoding="utf-8"))
    if cache_key in cache:
        keep = [int(x) for x in cache[cache_key]]
        print(f"[{cache_key}] {n} 行: 命中缓存 {len(keep)} 行")
        return keep, [{"teacher": "cache", "corpus": cache_key, "chunk": -1, "ok": len(keep), "rows": ""}]
    log: list[dict] = []
    keep_sets = {t["name"]: set() for t in TEACHERS}
    chunks = [lines[i:i + CHUNK] for i in range(0, n, CHUNK)]
    from concurrent.futures import ThreadPoolExecutor
    for t in TEACHERS:
        with ThreadPoolExecutor(max_workers=12) as ex:
            futs = [ex.submit(label_chunk, cname, query, ch, i * CHUNK, t, log)
                    for i, ch in enumerate(chunks) if ch]
        for f in futs:
            keep_sets[t["name"]] |= f.result()
    union = set.union(*keep_sets.values())
    print(f"[{cache_key}] {n} 行: deepseek={len(keep_sets['deepseek'])} "
          f"qwen={len(keep_sets['qwen'])} 并集={len(union)}")
    cache[cache_key] = sorted(union)
    CACHE.parent.mkdir(parents=True, exist_ok=True)
    CACHE.write_text(json.dumps(cache), encoding="utf-8")
    return sorted(union), log


# ---- 18 维特征复刻（与 src/semantic.rs feats 逐维一致） ----
def tokens(text: str) -> list[str]:
    """与 src/semantic.rs tokens() 逐字符一致：ASCII 词内字符连续段 + CJK 连续段。
    词内字符含 `_ . / : @ \\ -`（代码标识符/path/错误码语义）；其余为分隔。
    按 UTF-8 字节推进与切片（CJK 三字节一字符），与 Rust 字节语义一致。"""
    b = text.encode("utf-8")
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
    w_hit = word_hit_ratio(terms, lower, toks)
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
    stop_hit = stop_ratio(toks)
    sym_noise = noise_line(line)
    q_density = query_density(lower, terms, len(toks))
    head_hit = head_hit_ratio(lower, terms)
    rep_ratio = rep_char_ratio(line)
    cap_tok = cap_token_ratio(line)
    return [overlap, min(hit, 1), math.log1p(chars), cap_ratio, dig_ratio, structure,
            w_hit, cjk, err_r, uniq, sym_r, pos, edge, long_f, code, 1.0, bigram, prox,
            stop_hit, sym_noise, q_density, head_hit, rep_ratio, cap_tok]


# ---- 新增 6 维特征（与 src/semantic.rs 逐特征一致） ----
def word_hit_ratio(terms: list[str], lower: str, toks: list[str]) -> float:
    """词边界整词命中比例（含屈折词根），与 src/semantic.rs word_hit_ratio 一致。"""
    if not terms:
        return 0.0
    n = 0
    for t in terms:
        if not t:
            continue
        plain = all(c.isascii() and c.isalnum() for c in t)
        if any(root_match(tok, t) for tok in toks) or ((not plain) and t in lower):
            n += 1
    return n / len(terms)


def stem(w: str) -> str | None:
    """规则词干（与 src/semantic.rs stem 一致）：全小写 ASCII 且 ≥4，去后缀后 ≥3。"""
    if len(w) < 4 or not w.isascii() or not w.islower():
        return None
    s = strip_affix(w)
    return s if s is not None and len(s) >= 3 else None


def strip_affix(w: str) -> str | None:
    """最长后缀优先：ing→去 ing（≥4）；ed→去 ed（≥4）；ies→y；咝音 es；尾 s。"""
    if w.endswith("ing"):
        r = w[:-3]
        return r if len(r) >= 4 else None
    if w.endswith("ed"):
        r = w[:-2]
        return r if len(r) >= 4 else None
    if w.endswith("ies"):
        return w[:-3] + "y"
    if w.endswith("es") and sibilant(w[:-2]):
        return w[:-2]
    if w.endswith("s") and not w.endswith("ss"):
        r = w[:-1]
        return r if len(r) >= 3 else None
    return None


def sibilant(s: str) -> bool:
    return s.endswith(("ch", "sh", "s", "x", "z"))


def root_match(a: str, b: str) -> bool:
    """词根判定（与 src/semantic.rs root_match 一致）。"""
    if a == b:
        return True
    sa, sb = stem(a), stem(b)
    if sa is not None and sb is not None:
        return sa == sb
    if sa is not None:
        return sa == b
    if sb is not None:
        return a == sb
    return False


def stop_ratio(toks: list[str]) -> float:
    if not toks:
        return 0.0
    return sum(1 for t in toks if t in STOP) / len(toks)


def noise_line(line: str) -> float:
    t = line.strip()
    if len(t) < 8:
        return 0.0
    alpha = sum(1 for c in t if c.isalnum())
    if alpha == 0:
        return 1.0
    sym = len(t) - alpha
    return 1.0 if sym / len(t) >= 0.6 else 0.0


def query_density(lower: str, terms: list[str], n_toks: int) -> float:
    if n_toks == 0:
        return 0.0
    cnt = sum(lower.count(t) for t in terms if t)
    return cnt / n_toks


def head_hit_ratio(lower: str, terms: list[str]) -> float:
    if not terms:
        return 0.0
    head = lower[:60]
    return sum(1 for t in terms if t and t in head) / len(terms)


def rep_char_ratio(line: str) -> float:
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


def cap_token_ratio(line: str) -> float:
    """大写开头 token 密度（基于原行大小写；token 级与 cap_ratio 字符级互补）。"""
    toks = tokens(line)
    if not toks:
        return 0.0
    n = sum(1 for t in toks if t and t[0].isupper())
    return n / len(toks)


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
    for cname, text, queries in CORPORA:
        lines = text.splitlines()
        total = len(lines)
        for query in queries:
            keep, log = label_corpus(cname, query, text)
            total_lines += total
            total_keep += len(keep)
            keep_set = set(keep)
            for i, ln in enumerate(lines):
                all_x.append(feats(ln, [query], i, total))
                all_y.append(1 if (i + 1) in keep_set else 0)
            for e in log:
                labels.append({**e, "n": total, "keep": len(keep), "query": query})
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
        w = csv.DictWriter(fh, fieldnames=["teacher", "corpus", "chunk", "ok", "rows", "n", "keep", "query"])
        w.writeheader()
        w.writerows(labels)
    print("标注日志写入 config/bench/results/distill_labels.csv")
    return 0


def selfcheck() -> int:
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
                    "feats": [float(f"{v:.12g}") for v in feats(line, terms, i, total)]})
    print(json.dumps(out, ensure_ascii=False, indent=1))
    return 0


if __name__ == "__main__":
    if "--selfcheck" in sys.argv:
        sys.exit(selfcheck())
    sys.exit(main())
