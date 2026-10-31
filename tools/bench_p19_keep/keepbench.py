#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""P19 · KEEP-Bench（证据保留诊断基准）scx 冻结基线适配器。

官方基准：KEEP-Bench: Evaluating Evidence Preservation in Context Compression
  - 仓库: https://github.com/PlusLabNLP/KEEP-Bench  (commit 119bdaad2a417df24c1a3f1ba02676b1126425de)
  - 数据: HuggingFace uclanlp/KEEP-Bench (snapshot 91c37d5470f81ab7655b4893e7ba10536d5c952f), 4,102 cases
  - 论文: https://arxiv.org/abs/2610.26827

协议复用（只读导入官方源码，不改动官方仓库）：
  - 判分 prompt / 路由 / 解析：直接 import 官方
    src/evaluation/run_entailment_check.py 的 PROPOSITION_PROMPT / DISAMBIGUATION_BLOCK /
    KEY_PROMPT / PERSPECTIVE_PROMPT / JURISDICTION_PROMPT / messages / disambiguation_block /
    judge_for / parse_single / parse_dual / try_parse_json / shortcut。
  - 指标口径：镜像官方 src/evaluation/score.py 的 case 级 All-Rec / Para-Rec / Cond-Rec 聚合；
    同时把压缩/判分产物写成官方目录布局，可用官方 score.py 独立复核。

与官方 LLM 压缩器的口径差异（如实记录，不冒充同口径）：
  - 官方压缩器是 ICL 抽象摘要（1x 预算 = K 句：two_hop 4 / three_hop 6 / four_hop 8 /
    science-3p 7 / science-4p 9 / policy 9）；scx 是行级抽取式压缩，无句子预算概念。
    本适配器固定 scx 配置：--context 0（针尖模式：仅证据种子行 + 守卫行，不做可选邻域填充）、
    --ratio 0.35、不做行内 trim；query 由问句经确定性清洗 + 当前上下文文档频率过滤得到。
    压缩长度（字符/行）单独统计并随结果上报，All-Rec 与官方在「压缩后证据是否仍可判定」
    这一判分协议上可比，压缩率口径不可直接与官方句子预算对齐。

判分通道：DeepSeek chat-completions（OpenAI 兼容），temperature=0，max_tokens=2048；
  每条 LLM 调用逐条落盘 llm_calls.jsonl（时间/模型/prompt 摘要/response/usage/耗时/重试）。

用法（全量）：
  /tmp/scx-eval/bin/python tools/bench_p19_keep/keepbench.py \
      --results-json config/bench/results/bench_p19_keepbench.json

分层抽样（固定 seed=42，每子集 N 例）：
  /tmp/scx-eval/bin/python tools/bench_p19_keep/keepbench.py --limit-per-subset 60 \
      --results-json config/bench/results/bench_p19_keepbench.json
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import os
import random
import re
import subprocess
import sys
import tempfile
import threading
import time

import httpx
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

_AUDIT_LOCK = threading.Lock()

# 共享连接池（httpx 线程安全）；硬超时防止半开连接长期占住 worker。
_HTTP_CLIENT = None


def _http():
    global _HTTP_CLIENT
    if _HTTP_CLIENT is None:
        import httpx
        _HTTP_CLIENT = httpx.Client(
            timeout=httpx.Timeout(connect=8.0, read=10.0, write=10.0, pool=10.0),
            limits=httpx.Limits(max_connections=JUDGE_WORKERS,
                                max_keepalive_connections=2,
                                keepalive_expiry=5.0),
            headers={"Content-Type": "application/json"},
        )
    return _HTTP_CLIENT


def audit_record(rec: dict) -> None:
    """每条 LLM 调用即时追加落盘（线程安全），供事后审计/成本统计。"""
    with _AUDIT_LOCK:
        with LLM_CALLS.open("a", encoding="utf-8") as fh:
            fh.write(json.dumps(rec, ensure_ascii=False) + "\n")
            fh.flush()

# --------------------------------------------------------------------------- #
# 路径与官方资源定位
# --------------------------------------------------------------------------- #
REPO = Path("/Users/fei/code/scx")
DEFAULT_SCX_BIN = REPO / "target/release/scx"
OFFICIAL_REPO = Path(os.environ.get("KEEP_REPO", "/tmp/scx_bench_keep"))
DATA_ROOT = OFFICIAL_REPO / "data"
METHOD_DIR = "scx/without_thinking/1x"
OUT_ROOT = OFFICIAL_REPO / "outputs" / METHOD_DIR
IN_ROOT = OFFICIAL_REPO / "entailment_check_input" / METHOD_DIR
JUDGE_ROOT = OFFICIAL_REPO / "entailment_check_output" / METHOD_DIR
LLM_CALLS = OFFICIAL_REPO / "llm_calls.jsonl"

# stem -> (setting, 官方数据文件相对 DATA_ROOT, 官方 score.py 分组)
# 与官方 prepare_data.py / score.py SUBSETS 一一对应。
SUBSETS = [
    {"stem": "parallel_setting_two_hop", "setting": "parallel",
     "data": "parallel/two_hop.jsonl", "group": "parallel"},
    {"stem": "parallel_setting_three_hop", "setting": "parallel",
     "data": "parallel/three_hop.jsonl", "group": "parallel"},
    {"stem": "parallel_setting_four_hop", "setting": "parallel",
     "data": "parallel/four_hop.jsonl", "group": "parallel"},
    {"stem": "conditional_setting_two_hop", "setting": "conditional",
     "data": "conditional/two_hop.jsonl", "group": "conditional"},
    {"stem": "conditional_setting_three_hop", "setting": "conditional",
     "data": "conditional/three_hop.jsonl", "group": "conditional"},
    {"stem": "conditional_setting_four_hop", "setting": "conditional",
     "data": "conditional/four_hop.jsonl", "group": "conditional"},
    {"stem": "science_domain_three_perspectives", "setting": "imbalanced",
     "data": "science/three_perspectives.jsonl", "group": "science"},
    {"stem": "science_domain_four_perspectives", "setting": "imbalanced",
     "data": "science/four_perspectives.jsonl", "group": "science"},
    {"stem": "policy_domain", "setting": "imbalanced",
     "data": "policy/four_perspectives.jsonl", "group": "policy"},
]

SEED = 42
JUDGE_MODEL = os.environ.get("SCX_P19_JUDGE", "deepseek-flash")
JUDGE_URL = "https://api.deepseek.com/chat/completions"
JUDGE_MAX_TOKENS = 2048
JUDGE_TEMPERATURE = 0
JUDGE_WORKERS = 12
JUDGE_MAX_RETRIES = 5
# DeepSeek 公开报价（2026-10 文档，USD / 1M tokens；flash 缓存未命中输入 $0.15、输出 $0.60 峰值，
# 取保守峰值估算；实际以 usage 累计为准，见结果 JSON cost_estimate）。
PRICING_USD_PER_M = {"deepseek-flash": {"input": 0.15, "output": 0.60},
                     "deepseek-chat": {"input": 0.27, "output": 1.10}}


# --------------------------------------------------------------------------- #
# 官方 judge 模块只读复用
# --------------------------------------------------------------------------- #
def load_official_judge():
    spec = importlib.util.spec_from_file_location(
        "keepbench_official_judge",
        OFFICIAL_REPO / "src" / "evaluation" / "run_entailment_check.py")
    mod = importlib.util.module_from_spec(spec)
    sys.modules["keepbench_official_judge"] = mod
    spec.loader.exec_module(mod)
    return mod


# --------------------------------------------------------------------------- #
# 确定性 query 清洗（适配器侧，非官方协议；规则固定、可复现）
# --------------------------------------------------------------------------- #
STOP = {
    "what", "which", "who", "whom", "whose", "where", "when", "why", "how",
    "did", "does", "do", "is", "are", "was", "were", "be", "been", "being",
    "can", "could", "should", "would", "will", "may", "might", "must", "shall",
    "have", "has", "had", "having",
    "i", "you", "he", "she", "it", "we", "they", "me", "him", "her", "us", "them",
    "my", "your", "his", "its", "our", "their", "this", "that", "these", "those",
    "there", "here", "a", "an", "the", "and", "or", "but", "if", "then", "than",
    "so", "such", "same", "into", "onto", "upon", "within", "among", "between",
    "during", "after", "before", "while", "about", "above", "below", "under",
    "over", "against", "by", "for", "from", "to", "of", "in", "on", "at",
    "with", "as", "per",
    "responsible", "existence", "formation", "roles", "role", "sources", "source",
    "processes", "process", "mechanisms", "mechanism", "way", "ways", "many",
    "much", "most", "some", "any", "other", "another", "performer", "performed",
    "perform", "inspiration", "inspired", "inspire", "period", "year", "years",
    "show", "shows", "shown", "part", "parts", "type", "types", "kind", "kinds",
    "form", "forms", "group", "groups", "name", "named", "called", "based",
    "using", "use", "used", "find", "found", "make", "made", "come", "came",
    "give", "gave", "take", "took", "located", "location", "current", "country",
    "state", "city", "town", "world", "known", "first", "main", "subject",
    "question", "answer", "single", "place", "set", "sets", "band", "album",
    "song", "artist", "general", "fort", "headquarters", "military", "branch",
    "allies", "alliance", "relations", "continent", "boundary", "latitude",
    "population", "territorial", "entity", "birthplace", "capture", "failed",
    "conditions", "condition", "layers", "layer", "surface", "observed",
}

# 看似大写实为通用名词，不作为专名保留。
GENERIC_CAP = {
    "grand", "title", "federation", "wrestling", "cup", "league", "season",
    "county", "district", "region", "list", "game", "match", "championship",
    "champions", "award", "awards", "record", "records", "album", "song", "band",
    "artist", "actor", "actress", "film", "series", "episode", "novel", "book",
    "story", "history", "war", "battle", "island", "river", "mountain",
    "highlands", "city", "state", "country", "town", "village", "territory",
    "university", "school", "station", "edition", "version", "original", "regular",
    "flagship", "finals", "group", "team", "club", "party", "general", "fort",
}

QUOTED = re.compile(r'"([^"]{2,80})"')
WORD = re.compile(r"[A-Za-z0-9'’\-]+")


def _doc_freq(tok: str, docs: list[str]) -> int:
    low = tok.lower()
    pat = re.escape(low) if "-" in low else r"\b" + re.escape(low) + r"\b"
    rx = re.compile(pat, re.I)
    return sum(1 for d in docs if rx.search(d))


def build_query(question: str, docs: list[str], df_max_frac: float = 0.25,
                floor_terms: int = 4) -> str:
    """确定性 query 构造：引号原文保留；专名（大写，剔除通用大写词）保留；
    小写主题词仅当文档频率 ≤ df_max_frac 时保留；候选为空时保底取最稀有 floor_terms 个。"""
    n = max(1, len(docs))
    quoted = QUOTED.findall(question)
    rest = QUOTED.sub(" ", question)
    cands: list[tuple[str, int, bool]] = []
    for q in quoted:
        cands.append((q, _doc_freq(q, docs), True))
    for tok in WORD.findall(rest):
        low = tok.lower()
        if low in STOP or low in GENERIC_CAP or len(low) < 3:
            continue
        cands.append((tok, _doc_freq(tok, docs), False))
    seen: dict[str, tuple[str, int, bool]] = {}
    for tok, df, q in cands:
        k = tok.lower()
        if k not in seen:
            seen[k] = (tok, df, q)
    kept: list[str] = []
    fallback: list[tuple[int, str]] = []
    for tok, df, q in seen.values():
        if q:
            kept.append(tok)
        elif df / n <= df_max_frac:
            kept.append(tok)
        else:
            fallback.append((df, tok))
    if not kept:
        fallback.sort()
        kept = [t for _, t in fallback[:floor_terms]]
    # 去重保序
    out, s = [], set()
    for t in kept:
        k = t.lower()
        if k not in s:
            s.add(k); out.append(t)
    return " ".join(out)


# --------------------------------------------------------------------------- #
# scx 压缩（spec §7 范式：临时文件 → subprocess → compressed: 行之前内容）
# --------------------------------------------------------------------------- #
def scx_compress(scx_bin: str, context: str, query: str, extra: list[str]) -> tuple[str, dict]:
    """返回 (compressed_text, meta)。失败/零信号 fail-open 返回原文，meta 记录原因。"""
    fd, path = tempfile.mkstemp(suffix=".txt")
    meta = {"rc": None, "kept_lines": None, "orig_chars": len(context),
            "comp_chars": None, "compressed": False, "stat": "", "query": query}
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            fh.write(context)
        proc = subprocess.run(
            [scx_bin, path, "--query", query, "--ratio", "0.35", "--context", "0", *extra],
            capture_output=True, text=True, timeout=120)
        meta["rc"] = proc.returncode
        if proc.returncode != 0 or not proc.stdout.strip():
            meta["stat"] = f"scx rc={proc.returncode}; fail-open original"
            return context, meta
        lines = proc.stdout.splitlines()
        cut = next((k for k, l in enumerate(lines) if l.lstrip().startswith("compressed:")),
                   len(lines))
        stat = next((l for l in lines[cut:] if l.lstrip().startswith("compressed:")), "")
        meta["stat"] = stat.strip()
        m = re.search(r"kept=(\d+)/(\d+) lines", stat)
        if m:
            meta["kept_lines"] = int(m.group(1))
        out = "\n".join(lines[:cut])
        if not out.strip():
            return context, meta
        meta["comp_chars"] = len(out)
        meta["compressed"] = out != context
        return out, meta
    except Exception as exc:  # noqa: BLE001
        meta["stat"] = f"exception {type(exc).__name__}: {exc}; fail-open original"
        return context, meta
    finally:
        try:
            os.unlink(path)
        except OSError:
            pass


# --------------------------------------------------------------------------- #
# 数据加载与分层抽样
# --------------------------------------------------------------------------- #
def load_subset(rel: Path, limit: int | None, seed: int) -> list[dict]:
    rows = [json.loads(l) for l in (DATA_ROOT / rel).open(encoding="utf-8") if l.strip()]
    if limit is not None and limit < len(rows):
        rows = random.Random(seed).sample(rows, limit)
        rows.sort(key=lambda r: r["id"])  # 保持确定性顺序，便于复现
    return rows


# --------------------------------------------------------------------------- #
# 判分调用（DeepSeek，逐条落盘，退避重试）
# --------------------------------------------------------------------------- #
def _deepseek_key() -> str:
    k = os.environ.get("DEEPSEEK_API_KEY", "")
    if not k:
        out = subprocess.run(["/bin/zsh", "-lc",
                              "source ~/.zshrc && echo -n $DEEPSEEK_API_KEY"],
                             capture_output=True, text=True, timeout=30)
        k = out.stdout.strip()
    return k


def judge_call(official, judge, entry: dict, extra: dict | None, label: str) -> dict:
    """一次官方协议判分；返回 verdict dict；错误行返回 {'error': ...}（不计 yes/no）。
    每次尝试（成功/重试/失败）都即时追加到 llm_calls.jsonl。"""
    request = judge.build({f: entry[f] for f in judge.fields}, extra)
    body = {"model": JUDGE_MODEL, "messages": request,
            "max_tokens": JUDGE_MAX_TOKENS, "temperature": JUDGE_TEMPERATURE,
            "reasoning": {"enabled": False}}
    key = _deepseek_key()
    last = None
    for attempt in range(JUDGE_MAX_RETRIES + 1):
        t0 = time.time()
        record: dict = {"ts": time.strftime("%Y-%m-%dT%H:%M:%S%z"), "label": label,
                        "model": JUDGE_MODEL, "attempt": attempt, "judge": judge.name,
                        "system": request[0]["content"][:200],
                        "prompt": request[1]["content"][:1500],
                        "messages_chars": sum(len(m["content"]) for m in request),
                        "response": None, "usage": None, "latency_s": None,
                        "http_status": None, "error": None}
        try:
            resp = _http().post(
                JUDGE_URL, json=body,
                headers={"Authorization": f"Bearer {key}"})
            record["http_status"] = resp.status_code
            if resp.status_code in (429, 500, 502, 503, 504):
                raise httpx.HTTPStatusError(f"HTTP {resp.status_code}", request=resp.request,
                                             response=resp)
            resp.raise_for_status()
            data = resp.json()
            latency = time.time() - t0
            msg = data["choices"][0]["message"]
            raw = (msg.get("content") or "").strip()
            usage = data.get("usage") or {}
            record["response"] = raw
            record["usage"] = usage
            record["latency_s"] = round(latency, 3)
            verdict = judge.parse(official.try_parse_json(raw))
            if verdict is not None:
                audit_record(record)
                return verdict
            last = f"unparseable reply: {raw[:200]}"
            record["error"] = last
        except Exception as exc:  # noqa: BLE001
            latency = time.time() - t0
            last = f"{type(exc).__name__}: {exc}"
            record["error"] = last
            record["latency_s"] = round(latency, 3)
        audit_record(record)
        if attempt < JUDGE_MAX_RETRIES:
            time.sleep(min(2 ** attempt, 8) + random.random())
    return {"error": last}


# --------------------------------------------------------------------------- #
# 展开：镜像官方 build_entailment_check_input.py
# --------------------------------------------------------------------------- #
def expand_parallel(src: dict, context: str):
    rows, index, extras = [], [], []
    for chain_key in ("logical_chain_1", "logical_chain_2"):
        for pos, prop in enumerate(src[chain_key]):
            rows.append({"context": context, "proposition": prop})
            index.append({"id": src["id"], "source": chain_key, "position": pos})
            extras.append({"ambiguous_entities": src.get("ambiguous_entities"),
                           "unambiguous_entities": src.get("unambiguous_entities")})
    return {"": (rows, index, extras)}


def _key_position(src: dict) -> int:
    key = src["key"].lower()
    hits = [i for i, prop in enumerate(src["logical_chain"]) if key in prop.lower()]
    if len(hits) != 1:
        raise ValueError(f"{src['id']}: key {src['key']!r} matches {len(hits)} chain elements")
    return hits[0]


def expand_conditional(src: dict, context: str):
    plain, plain_idx, plain_extra = [], [], []
    keyed, keyed_idx, keyed_extra = [], [], []
    k = _key_position(src)
    for pos, prop in enumerate(src["logical_chain"]):
        ref = {"id": src["id"], "source": "logical_chain", "position": pos}
        if pos == k:
            keyed.append({"context": context, "key": src["key"],
                          "disambiguation_proposition": src["disambiguation_proposition"],
                          "proposition": prop})
            keyed_idx.append(ref); keyed_extra.append(None)
        else:
            plain.append({"context": context, "proposition": prop})
            plain_idx.append(ref); plain_extra.append(None)
    return {"_proposition_only": (plain, plain_idx, plain_extra),
            "_key_proposition": (keyed, keyed_idx, keyed_extra)}


def expand_imbalanced(src: dict, context: str):
    rows, index = [], []
    for pos, perspective in enumerate(src["perspectives"]):
        rows.append({"question": src["question"], "compressed_context": context,
                     "perspective": perspective})
        index.append({"id": src["id"], "source": "perspectives", "position": pos})
    return {"": (rows, index, [None] * len(rows))}


EXPAND = {"parallel": expand_parallel, "conditional": expand_conditional,
          "imbalanced": expand_imbalanced}


def write_jsonl(path: Path, rows: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    with tmp.open("w", encoding="utf-8") as fh:
        for row in rows:
            fh.write(json.dumps(row, ensure_ascii=False) + "\n")
    tmp.replace(path)


def read_jsonl(path: Path) -> list[dict]:
    with path.open(encoding="utf-8") as fh:
        return [json.loads(l) for l in fh if l.strip()]


# --------------------------------------------------------------------------- #
# 主流程
# --------------------------------------------------------------------------- #
def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--scx-bin", default=str(DEFAULT_SCX_BIN))
    ap.add_argument("--workers", type=int, default=JUDGE_WORKERS)
    ap.add_argument("--limit-per-subset", type=int, default=None,
                    help="每子集固定 seed 抽样 N 例；缺省=全量 4,102")
    ap.add_argument("--seed", type=int, default=SEED)
    ap.add_argument("--df-max-frac", type=float, default=0.25)
    ap.add_argument("--model", default="deepseek-flash")
    ap.add_argument("--compress-only", action="store_true", help="只跑压缩，不调 judge")
    ap.add_argument("--judge-only", action="store_true", help="跳过压缩，复用已落盘压缩产物")
    ap.add_argument("--results-json",
                    default=str(REPO / "config/bench/results/bench_p19_keepbench.json"))
    ap.add_argument("--scx-extra", default="", help="透传给 scx 的附加参数（空格分隔）")
    args = ap.parse_args()

    global JUDGE_MODEL
    JUDGE_MODEL = args.model
    scx_extra = args.scx_extra.split() if args.scx_extra else []
    official = load_official_judge()

    # ---- 阶段 1：压缩（逐子集落盘，行对齐官方数据）----
    print(f"== P19 KEEP-Bench scx 基线 | limit/subset={args.limit_per_subset} seed={args.seed} "
          f"judge={JUDGE_MODEL} ==", flush=True)
    subset_rows: dict[str, list[dict]] = {}
    for spec in SUBSETS:
        rows = load_subset(Path(spec["data"]), args.limit_per_subset, args.seed)
        subset_rows[spec["stem"]] = rows
        out_path = OUT_ROOT / f"{spec['stem']}.jsonl"
        if args.judge_only and out_path.exists():
            existing = read_jsonl(out_path)
            if len(existing) == len(rows):
                subset_rows[spec["stem"]] = existing
                print(f"  reuse compressed {spec['stem']}: {len(rows)} rows", flush=True)
                continue
        print(f"  compress {spec['stem']}: {len(rows)} cases", flush=True)

        def compress_one(idx: int, row: dict) -> tuple[int, dict]:
            docs = row["context"].split("\n")
            qc = build_query(row["question"], docs, args.df_max_frac)
            comp, _meta = scx_compress(args.scx_bin, row["context"], qc, scx_extra)
            out = dict(row)
            out["compressed_context"] = comp
            return idx, out

        with ThreadPoolExecutor(max_workers=args.workers) as ex:
            futs = [ex.submit(compress_one, i, r) for i, r in enumerate(rows)]
            done_rows: list[dict] = [None] * len(rows)  # type: ignore[list-item]
            for f in as_completed(futs):
                i, out = f.result()
                done_rows[i] = out
        write_jsonl(out_path, done_rows)
        subset_rows[spec["stem"]] = done_rows

    # ---- 阶段 2：展开 judge 输入（官方布局 + index）----
    all_entries: list[tuple[str, str, dict, dict | None]] = []  # stem, suffix, entry, extra
    index_rows: dict[str, list[dict]] = {}
    groups: dict[str, list[tuple[int, dict, dict | None]]] = {}
    for spec in SUBSETS:
        stem = spec["stem"]
        rows = subset_rows[stem]
        agg: dict[str, tuple[list[dict], list[dict], list]] = {}
        for row in rows:
            context = row["compressed_context"]
            for suffix, (ent, idx, extra) in EXPAND[spec["setting"]](row, context).items():
                a = agg.setdefault(suffix, ([], [], []))
                a[0].extend(ent); a[1].extend(idx); a[2].extend(extra)
        for suffix, (ent, idx, extra) in agg.items():
            group = f"{stem}{suffix}"
            write_jsonl(IN_ROOT / f"{group}.jsonl", ent)
            index_rows[group] = idx
            write_jsonl(IN_ROOT / "_index" / f"{group}.jsonl", idx)
            for j, e in enumerate(ent):
                all_entries.append((stem, suffix, e, extra[j]))
                groups.setdefault(group, []).append((j, e, extra[j]))

    print(f"  judge entries total: {len(all_entries)}", flush=True)

    # ---- 阶段 3：LLM judge（可续跑；每条调用即时落盘 llm_calls.jsonl）----
    verdicts: dict[str, list[dict]] = {}
    if not args.compress_only:
        for group, items in groups.items():
            out_path = JUDGE_ROOT / f"{group}.jsonl"
            results: list[dict | None] = [None] * len(items)
            if out_path.exists():
                prev = read_jsonl(out_path)
                if len(prev) == len(items):
                    def _judge_ok(r: dict) -> bool:
                        if not isinstance(r, dict) or "error" in r:
                            return False
                        if r.get("judgement") in ("yes", "no"):
                            return True
                        return (r.get("condition_judgement") in ("yes", "no")
                                and r.get("overall_judgement") in ("yes", "no"))

                    bad = [i for i, r in enumerate(prev) if not _judge_ok(r)]
                    if not bad:
                        verdicts[group] = prev
                        print(f"  reuse judge {group}: {len(prev)} rows", flush=True)
                        continue
                    print(f"  re-judge {group}: {len(bad)}/{len(prev)} invalid rows",
                          flush=True)
                    results = list(prev)

                    def judge_one(pos: int, entry: dict, extra: dict | None,
                                  label: str, group: str = group):
                        judge = official.judge_for(group)
                        sc = official.shortcut(judge, entry)
                        if sc is not None:
                            return pos, sc
                        return pos, judge_call(official, judge, entry, extra, label)

                    with ThreadPoolExecutor(max_workers=args.workers) as ex:
                        futs = [ex.submit(judge_one, pos, items[pos][1],
                                          items[pos][2], f"{group}:{pos + 1}")
                                for pos in bad]
                        for f in as_completed(futs):
                            pos, verdict = f.result()
                            results[pos] = verdict
                    write_jsonl(out_path, results)
                    verdicts[group] = results
                    err = sum(1 for v in results
                              if isinstance(v, dict) and "error" in v)
                    print(f"  judged {group}: {len(results)} rows (errors={err})",
                          flush=True)
                    continue

            def judge_one(pos: int, entry: dict, extra: dict | None, label: str,
                          group: str = group):
                judge = official.judge_for(group)
                sc = official.shortcut(judge, entry)
                if sc is not None:
                    return pos, sc
                return pos, judge_call(official, judge, entry, extra, label)

            with ThreadPoolExecutor(max_workers=args.workers) as ex:
                futs = [ex.submit(judge_one, pos, entry, extra, f"{group}:{pos + 1}")
                        for pos, entry, extra in items]
                for f in as_completed(futs):
                    pos, verdict = f.result()
                    results[pos] = verdict
            write_jsonl(out_path, results)
            verdicts[group] = results
            err = sum(1 for v in results if isinstance(v, dict) and "error" in v)
            print(f"  judged {group}: {len(results)} rows (errors={err})", flush=True)

    # ---- 阶段 4：聚合（镜像官方 score.py）----
    metrics = aggregate(official, subset_rows, index_rows, verdicts) if verdicts else {}
    compression_stats = summarize_compression(subset_rows)
    cost = estimate_cost()
    write_results(args, metrics, compression_stats, cost, subset_rows, all_entries)
    print(json.dumps(metrics, ensure_ascii=False, indent=1))


# --------------------------------------------------------------------------- #
# 聚合：镜像官方 score.py
# --------------------------------------------------------------------------- #
def aggregate(official, subset_rows, index_rows, verdicts) -> dict:
    def label(row: dict, field: str = "judgement") -> bool:
        v = row.get(field)
        if v not in ("yes", "no"):
            raise ValueError(f"missing/invalid {field}: {row}")
        return v == "yes"

    by_setting: dict[str, dict[str, dict]] = {"parallel": {}, "conditional": {},
                                              "science": {}, "policy": {}}
    per_subset: dict[str, dict] = {}

    for spec in SUBSETS:
        stem = spec["stem"]
        rows = subset_rows[stem]
        if spec["setting"] == "parallel":
            flat = {r["id"]: r["flattened_labels"] for r in rows}
            chain_len = {r["id"]: len(r["logical_chain_1"]) for r in rows}
            cases: dict[str, dict[int, bool]] = {}
            for ref, row in zip(index_rows[stem], verdicts[stem]):
                offset = 0 if ref["source"] == "logical_chain_1" else chain_len[ref["id"]]
                cases.setdefault(ref["id"], {})[offset + ref["position"]] = label(row)
            res = {}
            for cid, z in cases.items():
                labels = flat[cid]
                res[cid] = {"all_rec": all(z.values()),
                            "para_rec": all(z[i] for i, f in enumerate(labels) if f == "yes")}
            per_subset[stem] = res
            by_setting["parallel"].update(res)
        elif spec["setting"] == "conditional":
            cases: dict[str, list[bool]] = {}
            cond: dict[str, bool] = {}
            plain = verdicts.get(f"{stem}_proposition_only", [])
            keyed = verdicts.get(f"{stem}_key_proposition", [])
            for ref, row in zip(index_rows[f"{stem}_proposition_only"], plain):
                cases.setdefault(ref["id"], []).append(label(row))
            for ref, row in zip(index_rows[f"{stem}_key_proposition"], keyed):
                condition = label(row, "condition_judgement")
                cond[ref["id"]] = condition
                cases.setdefault(ref["id"], []).extend(
                    [condition, label(row, "overall_judgement")])
            res = {cid: {"all_rec": all(z), "cond_rec": cond[cid]} for cid, z in cases.items()}
            per_subset[stem] = res
            by_setting["conditional"].update(res)
        else:
            cases: dict[str, list[bool]] = {}
            for ref, row in zip(index_rows[stem], verdicts[stem]):
                cases.setdefault(ref["id"], []).append(label(row))
            res = {cid: {"all_rec": all(z)} for cid, z in cases.items()}
            per_subset[stem] = res
            by_setting[spec["group"]].update(res)

    def mean_bool(vals: list[bool]) -> float:
        return 100.0 * sum(vals) / len(vals) if vals else float("nan")

    table: dict[str, dict] = {}
    for group, cases in by_setting.items():
        if group == "parallel":
            table[group] = {"n": len(cases),
                            "para_rec": mean_bool([m["para_rec"] for m in cases.values()]),
                            "all_rec": mean_bool([m["all_rec"] for m in cases.values()])}
        elif group == "conditional":
            table[group] = {"n": len(cases),
                            "cond_rec": mean_bool([m["cond_rec"] for m in cases.values()]),
                            "all_rec": mean_bool([m["all_rec"] for m in cases.values()])}
        else:
            table[group] = {"n": len(cases),
                            "all_rec": mean_bool([m["all_rec"] for m in cases.values()])}

    overall_vals = [m["all_rec"] for g in by_setting.values() for m in g.values()]
    table["overall"] = {"n": len(overall_vals),
                        "all_rec": mean_bool(overall_vals)}

    subset_table: dict[str, dict] = {}
    for stem, res in per_subset.items():
        subset_table[stem] = {"n": len(res),
                              "all_rec": mean_bool([m["all_rec"] for m in res.values()])}
    return {"setting": table, "subset": subset_table}


# 官方 1x 参考系（docs/index.html 内嵌表；judge=GPT-5.2 high，不可与本基线直接比较）。
REFERENCE_OFFICIAL_1X = [
    {"method": "EXIT", "type": "extractive", "pr": 41.9, "pa": 3.0, "cr": 25.1,
     "ca": 4.5, "sci": 10.5, "pol": 7.6, "all": 6.3},
    {"method": "RECOMP (extractive)", "type": "extractive", "pr": 12.9, "pa": 1.4,
     "cr": 19.4, "ca": 2.5, "sci": 11.4, "pol": 4.0, "all": 4.8},
    {"method": "BRIEF-Pro", "type": "extractive", "pr": 0.3, "pa": 0.0, "cr": 61.6,
     "ca": 19.9, "sci": 0.4, "pol": 0.0, "all": 6.7},
    {"method": "RECOMP (abstractive)", "type": "abstractive", "pr": 0.0, "pa": 0.0,
     "cr": 5.6, "ca": 1.4, "sci": 0.2, "pol": 0.0, "all": 0.5},
    {"method": "Qwen3-32B", "type": "open-llm", "pr": 58.9, "pa": 42.1, "cr": 58.6,
     "ca": 44.6, "sci": 31.3, "pol": 40.5, "all": 39.9},
    {"method": "Gemma-4-31B-it", "type": "open-llm (strongest open)", "pr": 71.2,
     "pa": 57.6, "cr": 85.2, "ca": 71.2, "sci": 69.9, "pol": 65.8, "all": 66.8},
    {"method": "Claude-Haiku-4.5", "type": "closed-llm (strongest closed)", "pr": 76.1,
     "pa": 63.8, "cr": 83.5, "ca": 70.0, "sci": 45.5, "pol": 65.0, "all": 61.6},
]


def summarize_compression(rows: dict[str, list[dict]]) -> dict:
    """从压缩产物直接测算（orig=context, comp=compressed_context），judge-only 亦可复现。"""
    out: dict[str, dict] = {}
    tot_orig = tot_comp = 0
    n_compressed = 0
    for stem, data in rows.items():
        orig = sum(len(r["context"]) for r in data)
        comp = sum(len(r["compressed_context"]) for r in data)
        tot_orig += orig; tot_comp += comp
        n_compressed += sum(1 for r in data
                            if r["compressed_context"] != r["context"])
        kept_lines = [r["compressed_context"].count("\n") + 1 for r in data]
        out[stem] = {"n": len(data), "orig_chars": orig, "comp_chars": comp,
                     "char_ratio": comp / orig if orig else float("nan"),
                     "kept_lines_mean": sum(kept_lines) / len(kept_lines),
                     "compressed_cases": sum(1 for r in data
                                             if r["compressed_context"] != r["context"])}
    return {"per_subset": out,
            "total": {"orig_chars": tot_orig, "comp_chars": tot_comp,
                      "char_ratio": tot_comp / tot_orig if tot_orig else float("nan"),
                      "compressed_cases": n_compressed}}


def verdict_audit() -> dict:
    """扫描官方布局输出：shortcut(空上下文)/解析错误/有效判定计数。"""
    total = shortcut = errors = valid = 0
    per_group: dict[str, dict] = {}
    if JUDGE_ROOT.exists():
        for f in sorted(JUDGE_ROOT.glob("*.jsonl")):
            rows = read_jsonl(f)
            s = e = v = 0
            for row in rows:
                total += 1
                if isinstance(row, dict) and "error" in row:
                    e += 1; errors += 1
                elif isinstance(row, dict) and row.get("note"):
                    s += 1; shortcut += 1
                else:
                    v += 1; valid += 1
            per_group[f.stem] = {"n": len(rows), "valid": v, "shortcut": s, "error": e}
    return {"verdicts": total, "valid": valid, "shortcut_empty_context": shortcut,
            "error_rows": errors, "per_group": per_group}


def estimate_cost() -> dict:
    """从 llm_calls.jsonl 全量汇总 token 与成本（含重试尝试次数）。"""
    in_tok = out_tok = reasoning_tok = 0
    calls = attempts = 0
    if LLM_CALLS.exists():
        for line in LLM_CALLS.open(encoding="utf-8"):
            if not line.strip():
                continue
            rec = json.loads(line)
            attempts += 1
            u = rec.get("usage") or {}
            if u:
                calls += 1
                in_tok += int(u.get("prompt_tokens", 0) or 0)
                out_tok += int(u.get("completion_tokens", 0) or 0)
                cpd = u.get("completion_tokens_details") or {}
                reasoning_tok += int(cpd.get("reasoning_tokens", 0) or 0)
    price = PRICING_USD_PER_M.get(JUDGE_MODEL, {"input": 0, "output": 0})
    cost = in_tok * price["input"] / 1e6 + out_tok * price["output"] / 1e6
    return {"calls_with_usage": calls, "audit_records": attempts,
            "input_tokens": in_tok, "output_tokens": out_tok,
            "reasoning_tokens": reasoning_tok,
            "price_usd_per_m": price, "estimated_cost_usd": round(cost, 4)}


def _findings(metrics: dict, compression: dict) -> dict:
    """数据驱动的双面结论；结构性短板由适配器口径固定说明。"""
    setting = metrics.get("setting", {})
    overall = setting.get("overall", {}).get("all_rec")
    weak = sorted(
        ((g, v["all_rec"]) for g, v in setting.items() if g != "overall" and "all_rec" in v),
        key=lambda kv: kv[1])
    strong = list(reversed(weak))
    return {
        "overall_all_rec": overall,
        "strongest_setting": strong[0] if strong else None,
        "weakest_setting": weak[0] if weak else None,
        "strengths": [
            "scx 行级抽取在问句实体直接出现的证据文档上可命中（parallel/conditional 首跳锚点命题）。",
            "针尖模式 context=0 主动剔除多数干扰文档，压缩字符比见 compression.total。",
        ],
        "weaknesses": [
            "多跳证据链短板：二/三/四跳下游实体（机构、地名、后继事实）不出现在问句中，"
            "scx 词面种子无法桥接，对应命题文档被裁掉，All-Rec 随 hop 数下降。",
            "scx 是抽取式整行保留，无官方 ICL 摘要的句子级融合；条件限定词/少数派视角若所在行被裁即丢失。",
            "本基线 judge 为 deepseek-flash（非官方 gpt-5.2 high），判定严格度与论文不可直接对齐。",
        ],
        "epr": "n/a —— 官方 EPR 依赖压缩器推理 FLOPs；scx 为本地抽取式、无 LLM 压缩推理，不计算 EPR。",
    }


def write_results(args, metrics, compression_stats, cost, subset_rows, all_entries) -> None:
    import subprocess as sp
    scx_version = "0.11.0"
    try:
        v = sp.run([args.scx_bin, "--version"], capture_output=True, text=True, timeout=30).stdout.strip()
        scx_version = v or scx_version
    except Exception:
        pass
    try:
        head = sp.run(["git", "-C", str(REPO), "rev-parse", "HEAD"],
                      capture_output=True, text=True, timeout=30).stdout.strip()
    except Exception:
        head = "unavailable"
    official_commit = sp.run(
        ["git", "-C", str(OFFICIAL_REPO), "rev-parse", "HEAD"],
        capture_output=True, text=True, timeout=30).stdout.strip()
    verdict = verdict_audit()
    result = {
        "schema": "scx-bench-result/v1",
        "benchmark": "KEEP-Bench (arXiv:2610.26827)",
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "scx": {"binary": args.scx_bin, "version": scx_version, "repo_head": head,
                "cli": "scx <tmpfile> --query <cleaned-question> --ratio 0.35 --context 0",
                "scx_extra": args.scx_extra.split()},
        "official": {"repo": "https://github.com/PlusLabNLP/KEEP-Bench",
                     "commit": official_commit,
                     "hf_dataset": "uclanlp/KEEP-Bench",
                     "hf_snapshot": "91c37d5470f81ab7655b4893e7ba10536d5c952f",
                     "cases": 4102,
                     "judge_paper": "gpt-5.2 reasoning=high (官方；本适配器不可得，见 judge 口径)"},
        "run": {"seed": args.seed, "limit_per_subset": args.limit_per_subset,
                "sampling": "full" if args.limit_per_subset is None else "stratified fixed-seed sample",
                "workers": args.workers, "df_max_frac": args.df_max_frac,
                "compression_mode": "scx extractive line-level, context=0 needle mode, no line trim"},
        "judge": {"model": JUDGE_MODEL, "temperature": JUDGE_TEMPERATURE,
                  "max_tokens": JUDGE_MAX_TOKENS, "concurrency": args.workers,
                  "max_retries": JUDGE_MAX_RETRIES,
                  "protocol": "official prompts/routing/parsing reused from run_entailment_check.py; "
                              "transport swapped to DeepSeek chat-completions",
                  "note": "官方 judge=gpt-5.2 high reasoning；本结果由 deepseek-flash 判定，"
                          "与论文分数不可直接横向比较，仅作 scx 自身证据保留基线。"},
        "metrics": metrics,
        "reference_official_1x": REFERENCE_OFFICIAL_1X,
        "compression": compression_stats,
        "verdict_audit": verdict,
        "findings": _findings(metrics, compression_stats),
        "cost": cost,
        "cases_evaluated": {stem: len(rows) for stem, rows in subset_rows.items()},
        "judge_entries": len(all_entries),
        "reproduce": {
            "python": "/tmp/scx-eval/bin/python",
            "command": ("/tmp/scx-eval/bin/python tools/bench_p19_keep/keepbench.py "
                        "--results-json config/bench/results/bench_p19_keepbench.json"),
            "full_command": ("/tmp/scx-eval/bin/python tools/bench_p19_keep/keepbench.py "
                             "--results-json config/bench/results/bench_p19_keepbench.json"),
        },
    }
    out = Path(args.results_json)
    out.parent.mkdir(parents=True, exist_ok=True)
    tmp = out.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    tmp.replace(out)
    print(f"\nresults written: {out}", flush=True)


if __name__ == "__main__":
    main()
