#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""P20 LongMemEval 接入共享层（scx memory ↔ 官方 harness 协议）。

官方来源（commit 记录于结果 JSON）：
  仓库 : https://github.com/xiaowu0162/LongMemEval （clone 到 /tmp/scx_bench_longmem/repo）
  判分 : src/evaluation/evaluate_qa.py  get_anscheck_prompt() 逐字复用
  作答 : src/generation/run_generation.py prepare_prompt()（con / json / useronly=false）
  检索 : src/retrieval/eval_utils.py      dcg / ndcg / evaluate_retrieval 逐公式移植
  聚合 : src/evaluation/print_qa_metrics.py 六类 task-averaged + overall + abstention

口径声明：
  - reader / judge 模型官方为 gpt-4o；本环境无 OpenAI key，按共享规格 §2 替换为
    DeepSeek（reader=deepseek-flash，judge=deepseek-flash，temperature=0）。
    prompt 模板逐字保持官方；判分模型差异在结果 JSON 与完成记录中显式标注。
  - scx 记忆库 = 被测检索系统；每问独立 mem-dir（与官方 per-entry 建库一致，
    见 src/retrieval/run_retrieval.py batch_get_retrieved_context_and_eval）。
"""
from __future__ import annotations

import json
import os
import re
import subprocess
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

# ----------------------------------------------------------------------------
# 固定配置（复现性）
# ----------------------------------------------------------------------------
SEED = 20261031
SCX_BIN = "/Users/fei/code/scx/target/release/scx"
BENCH_ROOT = Path("/tmp/scx_bench_longmem")
DATA_DIR = BENCH_ROOT / "data"
WORK_DIR = BENCH_ROOT
MEM_ROOT = BENCH_ROOT / "mem"
LLM_LOG = BENCH_ROOT / "llm_calls.jsonl"

# 官方 reader 超参（run_generation.sh 默认 topk=50，reading_method=con）。
# scx 单库每问 ~48 会话；K=10 与官方 RAG 常用召回深度对齐，结果中同时报告 @1/3/5/10/30 检索诊断。
RECALL_K = 10
READER_MAX_TOKENS = 8192    # flash 即使关闭 reasoning 仍可能消耗 reasoning_tokens；
                            # 2048 会被思考占满致空 content，放宽到 8192（口径记录）
JUDGE_MAX_TOKENS = 256     # 官方 max_tokens=10（gpt-4o 无 reasoning）；flash 带 reasoning
                           # 会吃掉小预算导致空 content，放宽到 256 留思考余量（口径记录）
LLM_WORKERS = 8

# 官方六类非弃权题型 → 五能力分组（README §LongMemEval Overview 五能力；
# 题型名以数据集 question_type 为准，与 evaluate_qa.py 分支一致）。
ABILITY_MAP = {
    "single-session-user": "information_extraction",
    "single-session-assistant": "information_extraction",
    "single-session-preference": "information_extraction",
    "multi-session": "multi_session_reasoning",
    "temporal-reasoning": "temporal_reasoning",
    "knowledge-update": "knowledge_updates",
}
NON_ABSTENTION_TYPES = list(ABILITY_MAP)
ABILITY_ORDER = [
    "information_extraction",
    "multi_session_reasoning",
    "temporal_reasoning",
    "knowledge_updates",
    "abstention",
]

DATA_FILES = {
    "S": "longmemeval_s_cleaned.json",
    "M": "longmemeval_m_cleaned.json",
    "oracle": "longmemeval_oracle.json",
}


def is_abstention(qid: str) -> bool:
    return qid.endswith("_abs")


def ability_of(qtype: str, qid: str) -> str:
    if is_abstention(qid):
        return "abstention"
    return ABILITY_MAP[qtype]


# ----------------------------------------------------------------------------
# 数据加载
# ----------------------------------------------------------------------------
def load_dataset(which: str = "S") -> list[dict]:
    path = DATA_DIR / DATA_FILES[which]
    data = json.loads(path.read_text(encoding="utf-8"))
    # 官方数据集顺序确定性保持；不做随机重排。
    return data


# ----------------------------------------------------------------------------
# 官方 prompt 模板（逐字移植 evaluate_qa.py / run_generation.py）
# ----------------------------------------------------------------------------
# reader：con=true（官方推荐 reading_method=con），history_format=json，useronly=false。
READER_PROMPT_CON = (
    "I will give you several history chats between you and a user. Please answer "
    "the question based on the relevant chat history. Answer the question step by "
    "step: first extract all the relevant information, and then reason over the "
    "information to get the answer.\n\n\nHistory Chats:\n\n{history}\n\n"
    "Current Date: {date}\nQuestion: {question}\nAnswer (step by step):"
)

# 判分模板：evaluate_qa.py get_anscheck_prompt 逐字。
_JUDGE_TEMPLATES = {
    "single-session-user": (
        "I will give you a question, a correct answer, and a response from a model. "
        "Please answer yes if the response contains the correct answer. Otherwise, "
        "answer no. If the response is equivalent to the correct answer or contains "
        "all the intermediate steps to get the correct answer, you should also answer "
        "yes. If the response only contains a subset of the information required by "
        "the answer, answer no. \n\nQuestion: {}\n\nCorrect Answer: {}\n\nModel "
        "Response: {}\n\nIs the model response correct? Answer yes or no only."
    ),
    "single-session-assistant": (
        "I will give you a question, a correct answer, and a response from a model. "
        "Please answer yes if the response contains the correct answer. Otherwise, "
        "answer no. If the response is equivalent to the correct answer or contains "
        "all the intermediate steps to get the correct answer, you should also answer "
        "yes. If the response only contains a subset of the information required by "
        "the answer, answer no. \n\nQuestion: {}\n\nCorrect Answer: {}\n\nModel "
        "Response: {}\n\nIs the model response correct? Answer yes or no only."
    ),
    "multi-session": (
        "I will give you a question, a correct answer, and a response from a model. "
        "Please answer yes if the response contains the correct answer. Otherwise, "
        "answer no. If the response is equivalent to the correct answer or contains "
        "all the intermediate steps to get the correct answer, you should also answer "
        "yes. If the response only contains a subset of the information required by "
        "the answer, answer no. \n\nQuestion: {}\n\nCorrect Answer: {}\n\nModel "
        "Response: {}\n\nIs the model response correct? Answer yes or no only."
    ),
    "temporal-reasoning": (
        "I will give you a question, a correct answer, and a response from a model. "
        "Please answer yes if the response contains the correct answer. Otherwise, "
        "answer no. If the response is equivalent to the correct answer or contains "
        "all the intermediate steps to get the correct answer, you should also answer "
        "yes. If the response only contains a subset of the information required by "
        "the answer, answer no. In addition, do not penalize off-by-one errors for "
        "the number of days. If the question asks for the number of days/weeks/months, "
        "etc., and the model makes off-by-one errors (e.g., predicting 19 days when "
        "the answer is 18), the model's response is still correct. \n\nQuestion: {}\n\n"
        "Correct Answer: {}\n\nModel Response: {}\n\nIs the model response correct? "
        "Answer yes or no only."
    ),
    "knowledge-update": (
        "I will give you a question, a correct answer, and a response from a model. "
        "Please answer yes if the response contains the correct answer. Otherwise, "
        "answer no. If the response contains some previous information along with an "
        "updated answer, the response should be considered as correct as long as the "
        "updated answer is the required answer.\n\nQuestion: {}\n\nCorrect Answer: {}\n\n"
        "Model Response: {}\n\nIs the model response correct? Answer yes or no only."
    ),
    "single-session-preference": (
        "I will give you a question, a rubric for desired personalized response, and a "
        "response from a model. Please answer yes if the response satisfies the desired "
        "response. Otherwise, answer no. The model does not need to reflect all the "
        "points in the rubric. The response is correct as long as it recalls and "
        "utilizes the user's personal information correctly.\n\nQuestion: {}\n\n"
        "Rubric: {}\n\nModel Response: {}\n\nIs the model response correct? Answer yes "
        "or no only."
    ),
}

_JUDGE_ABSTENTION = (
    "I will give you an unanswerable question, an explanation, and a response from a "
    "model. Please answer yes if the model correctly identifies the question as "
    "unanswerable. The model could say that the information is incomplete, or some "
    "other information is given but the asked information is not.\n\nQuestion: {}\n\n"
    "Explanation: {}\n\nModel Response: {}\n\nDoes the model correctly identify the "
    "question as unanswerable? Answer yes or no only."
)


def judge_prompt(qtype: str, question: str, answer: str, response: str,
                 abstention: bool) -> str:
    """逐字复用官方 get_anscheck_prompt。"""
    if abstention:
        return _JUDGE_ABSTENTION.format(question, answer, response)
    return _JUDGE_TEMPLATES[qtype].format(question, answer, response)


# ----------------------------------------------------------------------------
# 会话渲染（与 run_generation.py prepare_prompt 的 json 格式一致）
# ----------------------------------------------------------------------------
def _clean_turns(turns: list[dict]) -> list[dict]:
    """官方：喂给 reader 前剥离 has_answer 标注（prepare_prompt L182-189）。"""
    out = []
    for t in turns:
        tt = {"role": t["role"], "content": t["content"]}
        out.append(tt)
    return out


def render_history(recalled: list[tuple[str, str, list[dict]]]) -> str:
    """recalled: [(session_id, date, turns)]，官方先按日期升序再逐会话渲染。"""
    recalled_sorted = sorted(recalled, key=lambda x: x[1])
    chunks = []
    for i, (_sid, date, turns) in enumerate(recalled_sorted, start=1):
        sess_string = "\n" + json.dumps(_clean_turns(turns), ensure_ascii=False)
        chunks.append(
            f"\n### Session {i}:\nSession Date: {date}\nSession Content:\n{sess_string}"
        )
    return "".join(chunks)


def reader_prompt(history: str, question_date: str, question: str) -> str:
    return READER_PROMPT_CON.format(
        history=history, date=question_date, question=question
    )


# ----------------------------------------------------------------------------
# scx 子进程包装（幂等、落日志、失败抛错）
# ----------------------------------------------------------------------------
_STORED_RE = re.compile(r"stored case=(\S+) added=(yes|dedup)")


def _scx(argv: list[str], stdin_text: str, timeout: int = 120) -> subprocess.CompletedProcess:
    proc = subprocess.run(
        [SCX_BIN, *argv],
        input=stdin_text.encode("utf-8"),
        capture_output=True,
        timeout=timeout,
    )
    proc.stdout = (proc.stdout or b"").decode("utf-8", "replace")
    proc.stderr = (proc.stderr or b"").decode("utf-8", "replace")
    return proc


def scx_store(mem_dir: str, query: str, text: str, fix: str,
              revise: str | None = None) -> dict:
    """摄取一个会话为 case；返回 case_id / added / 压缩统计 / 耗时。

    配方（README 复现节逐命令可查）：
      --store 1 --fix <date+官方session_id 标注> --ratio 1.0 --context 1
      --kind session-message --mem-dir <per-question dir> --seed 20261031
      --revise <old-case-id>   仅 P3 版本化臂使用
    """
    argv = [
        "--query", query,
        "--store", "1",
        "--fix", fix[:4000],
        "--ratio", "1.0",
        "--context", "1",
        "--kind", "session-message",
        "--mem-dir", mem_dir,
    ]
    if revise:
        argv += ["--revise", revise]
    t0 = time.monotonic()
    proc = _scx(argv, text)
    ms = (time.monotonic() - t0) * 1000
    combined = (proc.stdout or "") + (proc.stderr or "")
    m = _STORED_RE.search(combined)
    if proc.returncode != 0 or not m:
        raise RuntimeError(
            f"scx store failed rc={proc.returncode}\nSTDOUT:\n{proc.stdout}\n"
            f"STDERR:\n{proc.stderr}"
        )
    # compressed: kept=a/b lines chars=x->y ...
    cm = re.search(r"compressed: kept=(\d+)/(\d+) lines chars=(\d+)->(\d+)", combined)
    stats = {}
    if cm:
        stats = {"kept_lines": int(cm.group(1)), "total_lines": int(cm.group(2)),
                 "chars_in": int(cm.group(3)), "chars_out": int(cm.group(4))}
    return {"case_id": m.group(1), "added": m.group(2) == "yes",
            "stats": stats, "wall_ms": ms, "raw": combined}


_RECALL_RE = re.compile(r"^mem: sim=([\d.]+) score=([\d.]+) case=(\S+)")


def scx_recall(mem_dir: str, query: str, k: int) -> list[dict]:
    """--recall-only；返回 [{short_id, sim, score, full_id}]，顺序即官方召回排名。"""
    proc = _scx([
        "--recall-only", "--recall", str(k), "--recall-detail",
        "--query", query, "--mem-dir", mem_dir,
    ], "")
    if proc.returncode != 0:
        raise RuntimeError(f"scx recall failed rc={proc.returncode}\n{proc.stderr}")
    hits: list[dict] = []
    for line in proc.stdout.splitlines():
        mm = _RECALL_RE.match(line.strip())
        if mm:
            hits.append({"short_id": mm.group(3), "sim": float(mm.group(1)),
                         "score": float(mm.group(2))})
    # short id → full id：扫描 cases/<full-id>/case.json
    cases_dir = Path(mem_dir) / "cases"
    short2full: dict[str, str] = {}
    if cases_dir.exists():
        for cj in cases_dir.glob("*/case.json"):
            cid = cj.parent.name
            short2full[cid.rsplit("_", 1)[-1]] = cid
    for h in hits:
        h["full_id"] = short2full.get(h["short_id"], "")
    return hits


def load_case(mem_dir: str, full_id: str) -> dict:
    cj = Path(mem_dir) / "cases" / full_id / "case.json"
    return json.loads(cj.read_text(encoding="utf-8"))


# ----------------------------------------------------------------------------
# 官方检索指标（移植 src/retrieval/eval_utils.py，session 粒度）
# ----------------------------------------------------------------------------
def ndcg(rankings, correct_docs, corpus_ids, k=10):
    import math
    relevances = [1 if doc in correct_docs else 0 for doc in corpus_ids]
    sorted_relevances = [relevances[idx] for idx in rankings[:k]]
    ideal = sorted(relevances, reverse=True)
    ideal_dcg = ideal[0] + (sum(rel / math.log2(i + 2) for i, rel in enumerate(ideal[1:], start=1))
                            if len(ideal) > 1 else 0)
    actual_dcg = sorted_relevances[0] + (sum(rel / math.log2(i + 2)
                                             for i, rel in enumerate(sorted_relevances[1:], start=1))
                                         if len(sorted_relevances) > 1 else 0)
    if ideal_dcg == 0:
        return 0.0
    return actual_dcg / ideal_dcg


def evaluate_retrieval(rankings, correct_docs, corpus_ids, k=10):
    recalled = {corpus_ids[idx] for idx in rankings[:k]}
    recall_any = float(any(d in recalled for d in correct_docs))
    recall_all = float(all(d in recalled for d in correct_docs))
    return recall_any, recall_all, ndcg(rankings, correct_docs, corpus_ids, k)
