#!/usr/bin/env python3
"""P23 AMA-Bench · DeepSeek 通道 provider（答案生成 + 官方二元 judge + 官方 F1 口径）。

口径来源（只读引用，不改官方仓库）：
- 答案 prompt: AMA-Hub `src/memory_interface.py` L168-175（open-ended 问答）。
- judge prompt: AMA-Hub `utils/evaluation_metrics.py` L153-169（二元 yes/no，末个 yes/no 为准）。
- 答案抽取: AMA-Hub `utils/extract_final_answer.py`（`Answer[1]:` 正则 + think 标签剥离）。
- F1: AMA-Hub `utils/evaluation_metrics.py` `categorize_answers` / `normalize` /
  `tokenize` / `compute_f1_score` 逐字移植（官方第二指标，离线机械计算，不经过 LLM）。

与官方口径差异（结果 JSON 必须记录）：
- 官方答案模型 = Qwen3-32B（Table 5 同 backbone）；本适配器答案模型 = deepseek-flash
  （reasoning 关闭，与 P21 冻结基线同口径）。
- 官方 judge = Qwen3-32B；本适配器 judge = deepseek-chat 别名（物理模型 =
  DeepSeek-V4.1-Flash，无推理模式，二元 yes/no，稳定输出）。
- 温度固定 0；所有 LLM 调用逐条落盘 `llm_calls.jsonl`；curl 硬超时 + 指数退避重试。
"""
from __future__ import annotations

import json
import os
import re
import signal
import subprocess
import tempfile
import threading
import time
from pathlib import Path
from typing import Any, Optional

# ---------------------------------------------------------------------------
# 官方 prompt 常量（逐字，含空行；修改即偏离官方协议，禁止"顺手优化"）
# ---------------------------------------------------------------------------

# memory_interface.py L168-175
ANSWER_PROMPT_TEMPLATE = (
    "{retrieved_context}\n\n"
    "## Questions\n"
    "Question 1: {question}\n\n"
    "## Instructions\n"
    "Provide a direct and concise answer.\n\n"
    "Answer[1]: [your answer here]"
)

# evaluation_metrics.py L153-169（judge triplet 由 caller 拼入）
JUDGE_PROMPT_TEMPLATE = (
    "Task Type: {task_type}\n"
    "Episode ID: {episode_id}\n"
    "Task Context: {task}\n\n"
    "Question: {question}\n"
    "Reference Answer: {golden_answer}\n"
    "Predicted Answer: {predicted_answer}\n\n"
    "Is the predicted answer correct? Respond with ONLY yes or no. "
    "Do not include any thinking process, explanation, or additional text. "
    "Answer:<think></think>"
)

ANSWER_PATTERN = re.compile(r"Answer\s*\[\s*1\s*\]\s*:\s*(.*)", re.IGNORECASE)
THINK_PATTERN = re.compile(r"<think\b[^>]*>.*?</think\s*>", re.IGNORECASE | re.DOTALL)


def extract_final_answer(text: str) -> str:
    """移植 extract_final_answer.py：优先 Answer[1]: 抽取，失败回退整段；剥离 think。"""
    if not text:
        return ""
    cleaned = THINK_PATTERN.sub("", text).strip()
    match = ANSWER_PATTERN.search(cleaned)
    if match:
        return match.group(1).strip()
    return cleaned


# ---------------------------------------------------------------------------
# 官方 F1 口径（evaluation_metrics.py 逐字移植；离线机械计算）
# ---------------------------------------------------------------------------

CATEGORIES = ["object", "location", "number", "person", "date", "other"]


def categorize_answers(answer: str) -> dict[str, list[str]]:
    """移植 evaluation_metrics.py L17-44：按类别切分答案 token。"""
    answer = answer.strip().lower()
    categorized: dict[str, list[str]] = {c: [] for c in CATEGORIES}
    for token in answer.split():
        token = token.strip(".,!?;:'\"()[]{}")
        if not token:
            continue
        if token.isdigit():
            categorized["number"].append(token)
        elif any(c.isdigit() for c in token) and re.search(r"\d", token):
            categorized["number"].append(token)
        elif token in {"he", "she", "they", "i", "you", "we", "him", "her", "them"}:
            categorized["person"].append(token)
        elif token in {"here", "there", "where", "near", "inside", "outside", "above",
                       "below", "left", "right", "north", "south", "east", "west"}:
            categorized["location"].append(token)
        elif token in {"today", "yesterday", "tomorrow", "now", "then", "later",
                       "earlier", "morning", "afternoon", "evening", "night"}:
            categorized["date"].append(token)
        else:
            categorized["other"].append(token)
    return categorized


def normalize(text: str) -> str:
    """移植 evaluation_metrics.py L46-53。"""
    text = text.lower()
    text = re.sub(r"[^\w\s]", "", text)
    text = re.sub(r"\s+", " ", text).strip()
    return text


def tokenize(text: str) -> list[str]:
    return normalize(text).split()


def compute_f1_score(reference: str, prediction: str) -> float:
    """移植 evaluation_metrics.py `compute_f1_score`：分类别加权 F1。"""
    ref_cats = categorize_answers(reference)
    pred_cats = categorize_answers(prediction)
    category_f1: dict[str, float] = {}
    for category in CATEGORIES:
        ref_tokens = set(ref_cats[category])
        pred_tokens = set(pred_cats[category])
        if not ref_tokens and not pred_tokens:
            category_f1[category] = 1.0
            continue
        if not ref_tokens or not pred_tokens:
            category_f1[category] = 0.0
            continue
        common = ref_tokens & pred_tokens
        if not common:
            category_f1[category] = 0.0
            continue
        precision = len(common) / len(pred_tokens)
        recall = len(common) / len(ref_tokens)
        if precision + recall == 0:
            category_f1[category] = 0.0
        else:
            category_f1[category] = 2 * precision * recall / (precision + recall)
    return sum(category_f1.values()) / len(category_f1)


# ---------------------------------------------------------------------------
# DeepSeek 通道（curl 硬超时；答案 flash 关 reasoning；judge chat）
# ---------------------------------------------------------------------------

KEY_ENV = "DEEPSEEK_API_KEY"
ENDPOINT = "https://api.deepseek.com/chat/completions"


class DeepSeekProvider:
    """线程安全；每次调用追加一行到 llm_calls.jsonl。"""

    # 答案与 judge 均用 deepseek-chat 别名：API 实测该别名路由到 deepseek-flash
    # （=官方 DeepSeek-V4.1-Flash）的【无推理模式】，响应无 reasoning_content、
    # 时延稳定（probe 2s 级）。勿改显式 deepseek-flash：显式名触发推理
    # （effort=high，reasoning.enabled=false 实测无效），推理 token 吃 max_tokens
    # 导致空 content（pilot 实测 5/13 空响应；judge max_tokens=16 必空）。
    # 官方答案模型 = Qwen3-32B，差异在结果 JSON 口径节记录。
    ANSWER_MODEL = "deepseek-chat"  # 别名 → DeepSeek-V4.1-Flash（无推理模式）
    JUDGE_MODEL = "deepseek-chat"
    ANSWER_MAX_TOKENS = 2048
    JUDGE_MAX_TOKENS = 16
    ANSWER_TIMEOUT_S = 60
    JUDGE_TIMEOUT_S = 20
    TEMPERATURE = 0.0
    MAX_RETRIES = 5

    def __init__(self, log_path: Path, answer_model: str = ANSWER_MODEL,
                 judge_model: str = JUDGE_MODEL):
        self.log_path = Path(log_path)
        self.log_path.parent.mkdir(parents=True, exist_ok=True)
        self.answer_model = answer_model
        self.judge_model = judge_model
        self._lock = threading.Lock()
        self._call_seq = 0
        self._key: str = ""
        self.totals: dict[str, dict[str, Any]] = {
            "answer": {"calls": 0, "prompt_tokens": 0, "completion_tokens": 0,
                       "reasoning_tokens": 0, "failures": 0, "retries": 0,
                       "elapsed_s": 0.0},
            "judge": {"calls": 0, "prompt_tokens": 0, "completion_tokens": 0,
                      "reasoning_tokens": 0, "failures": 0, "retries": 0,
                      "elapsed_s": 0.0},
        }

    def _load_key(self) -> str:
        if self._key:
            return self._key
        key = os.environ.get(KEY_ENV, "").strip()
        if not key:
            out = subprocess.run(
                ["/bin/zsh", "-lc", f"source ~/.zshrc && echo -n ${KEY_ENV}"],
                capture_output=True, text=True, timeout=30)
            key = out.stdout.strip()
        if not key:
            raise RuntimeError("DEEPSEEK_API_KEY missing (env and ~/.zshrc both empty)")
        self._key = key
        return key

    def _post(self, body: dict[str, Any], timeout_s: int) -> tuple[int, str, str]:
        key = self._load_key()
        # body 写临时文件、curl 用 -d @file 读入：长 prompt 不再撑爆 argv
        # （macOS ARG_MAX → 'Argument list too long'），调用语义与内联 -d 完全一致。
        bodyf = tempfile.NamedTemporaryFile(delete=False, suffix=".body",
                                            mode="w", encoding="utf-8")
        json.dump(body, bodyf, ensure_ascii=False)
        bodyf.close()
        try:
            proc = subprocess.Popen(
                ["curl", "-4", "-sS", "--connect-timeout", "15", "--max-time", str(timeout_s),
                 "-X", "POST", ENDPOINT,
                 "-H", f"Authorization: Bearer {key}",
                 "-H", "Content-Type: application/json",
                 "-d", "@" + bodyf.name],
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                start_new_session=True)
            try:
                out, err = proc.communicate(timeout=timeout_s + 15)
                return proc.returncode, out, err
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(proc.pid, signal.SIGKILL)
                except Exception:  # noqa: BLE001
                    proc.kill()
                proc.wait()
                return 124, "", f"timeout after {timeout_s}s"
        finally:
            try:
                os.unlink(bodyf.name)
            except OSError:
                pass

    def _chat(self, role: str, model: str, user_prompt: str, max_tokens: int,
              timeout_s: int, episode: dict[str, Any], question_uuid: Optional[str],
              channel: Optional[str], meta: dict[str, Any]) -> dict[str, Any]:
        self._call_seq += 1
        call_id = f"p23-{self._call_seq:06d}"
        body: dict[str, Any] = {
            "model": model,
            "messages": [{"role": "user", "content": user_prompt}],
            "max_tokens": max_tokens,
            "temperature": self.TEMPERATURE,
        }
        if "flash" in model:
            body["reasoning"] = {"enabled": False}

        usage: dict[str, Any] = {}
        content = ""
        ok = False
        error = ""
        retries = 0
        t0 = time.time()
        for attempt in range(1, self.MAX_RETRIES + 1):
            retries = attempt - 1
            try:
                rc, out, err = self._post(body, timeout_s)
                if rc != 0:
                    error = f"curl rc={rc}: {err[:300]}"
                    time.sleep(min(2 ** attempt, 15))
                    continue
                resp = json.loads(out)
                if "error" in resp:
                    error = str(resp["error"])[:300]
                    time.sleep(min(2 ** attempt, 15))
                    continue
                choice = resp["choices"][0]["message"]
                content = (choice.get("content") or "").strip()
                usage = resp.get("usage", {}) or {}
                if not content:
                    error = "empty content"
                    time.sleep(min(2 ** attempt, 15))
                    continue
                ok = True
                break
            except json.JSONDecodeError as exc:
                error = f"json decode: {exc}; raw={out[:200]}" if "out" in dir() else str(exc)
                time.sleep(min(2 ** attempt, 15))
            except Exception as exc:  # noqa: BLE001
                error = f"{type(exc).__name__}: {exc}"
                time.sleep(min(2 ** attempt, 15))
        elapsed = time.time() - t0

        pt = int(usage.get("prompt_tokens", 0) or 0)
        ct = int(usage.get("completion_tokens", 0) or 0)
        details = usage.get("completion_tokens_details") or {}
        rt = int(details.get("reasoning_tokens", 0) or 0)

        with self._lock:
            t = self.totals[role]
            t["calls"] += 1
            t["prompt_tokens"] += pt
            t["completion_tokens"] += ct
            t["reasoning_tokens"] += rt
            t["retries"] += retries
            t["elapsed_s"] += elapsed
            if not ok:
                t["failures"] += 1
            record = {
                "call_id": call_id, "role": role, "model": model,
                "episode_id": episode.get("episode_id"),
                "question_uuid": question_uuid, "channel": channel,
                "ts": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
                "latency_s": round(elapsed, 3), "retries": retries,
                "ok": ok, "error": error,
                "prompt_tokens": pt, "completion_tokens": ct, "reasoning_tokens": rt,
                "usage": usage, "raw": content[:4000], "meta": meta,
                "messages": body["messages"],
            }
            with self.log_path.open("a", encoding="utf-8") as fh:
                fh.write(json.dumps(record, ensure_ascii=False) + "\n")

        return {"content": content, "usage": usage, "elapsed_s": elapsed,
                "retries": retries, "ok": ok, "error": error, "call_id": call_id}

    def answer(self, question: str, context: str, episode: dict[str, Any],
               channel: str, question_uuid: str) -> tuple[str, dict[str, Any]]:
        prompt = ANSWER_PROMPT_TEMPLATE.format(
            retrieved_context=context.strip(), question=question)
        res = self._chat("answer", self.answer_model, prompt,
                         self.ANSWER_MAX_TOKENS, self.ANSWER_TIMEOUT_S,
                         episode, question_uuid, channel,
                         {"task_type": episode.get("task_type"),
                          "domain": episode.get("domain")})
        res["parsed"] = extract_final_answer(res["content"]) if res["ok"] else ""
        return res["parsed"], res

    def judge(self, question: str, gold: str, predicted: str, episode: dict[str, Any],
              channel: str, question_uuid: str) -> tuple[bool, str, dict[str, Any]]:
        prompt = JUDGE_PROMPT_TEMPLATE.format(
            task_type=episode.get("task_type", ""),
            episode_id=episode.get("episode_id", ""),
            task=episode.get("task", ""),
            question=question, golden_answer=gold, predicted_answer=predicted)
        res = self._chat("judge", self.judge_model, prompt,
                         self.JUDGE_MAX_TOKENS, self.JUDGE_TIMEOUT_S,
                         episode, question_uuid, channel,
                         {"task_type": episode.get("task_type"),
                          "domain": episode.get("domain")})
        decision, cleaned = parse_judge_decision(res["content"]) if res["ok"] else (False, "")
        res["parsed"] = cleaned
        return decision, cleaned, res


def parse_judge_decision(text: str) -> tuple[bool, str]:
    """移植 evaluation_metrics.py L143-151：末个 yes/no 为准；无法解析 → False。"""
    if not text:
        return False, ""
    cleaned = THINK_PATTERN.sub("", text).strip().lower()
    tokens = re.findall(r"\b(yes|no)\b", cleaned)
    if tokens:
        return tokens[-1] == "yes", cleaned
    return False, cleaned
