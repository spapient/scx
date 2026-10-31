#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""DeepSeek 在线调用层（reader + judge），逐条落盘 llm_calls.jsonl 供审计。

范式照抄 tools/eval_fidelity.py `_call` 与 tools/bench_code.py 判定配置：
  - curl -4 规避 macOS urllib TLS 冻结；429/5xx 指数退避重试；
  - key 仅从 ~/.zshrc 子进程取，不写日志/不落盘；
  - temperature=0（规格 §2 固定）；deepseek-flash 关闭 reasoning（bench_code.py
    实测：开启会吃光 max_tokens 致空响应且每题 30-40s）。
"""
from __future__ import annotations

import json
import os
import subprocess
import threading
import time
from datetime import datetime, timezone
from pathlib import Path

from common import LLM_LOG

URL = "https://api.deepseek.com/chat/completions"
READER_MODEL = "deepseek-flash"
JUDGE_MODEL = "deepseek-flash"

_lock = threading.Lock()
_key: str | None = None
_call_counter = 0


def _get_key() -> str:
    global _key
    if _key:
        return _key
    k = os.environ.get("DEEPSEEK_API_KEY", "")
    if not k:
        out = subprocess.run(
            ["/bin/zsh", "-lc", "source ~/.zshrc && echo -n $DEEPSEEK_API_KEY"],
            capture_output=True, text=True, timeout=30,
        )
        k = out.stdout.strip()
    if not k:
        raise RuntimeError("DEEPSEEK_API_KEY missing")
    _key = k
    return k


def _append(record: dict) -> None:
    LLM_LOG.parent.mkdir(parents=True, exist_ok=True)
    with _lock:
        with LLM_LOG.open("a", encoding="utf-8") as f:
            f.write(json.dumps(record, ensure_ascii=False) + "\n")


def _post(model: str, prompt: str, max_tokens: int, temperature: float,
          role: str, qid: str, arm: str) -> tuple[str, dict, int, int]:
    """返回 (content, usage, latency_ms, retries)。失败重试 3 次指数退避。"""
    global _call_counter
    body = {
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": max_tokens,
        "temperature": temperature,
        "reasoning": {"enabled": False},
    }
    key = _get_key()
    last_err = ""
    retries = 0
    for attempt in range(4):
        t0 = time.monotonic()
        try:
            out = subprocess.run(
                ["curl", "-4", "-sS", "--max-time", "240", "-X", "POST", URL,
                 "-H", f"Authorization: Bearer {key}",
                 "-H", "Content-Type: application/json",
                 "-d", json.dumps(body, ensure_ascii=False)],
                capture_output=True, text=True, timeout=300,
            )
            if out.returncode != 0:
                last_err = f"curl rc={out.returncode} {out.stderr[:200]}"
                retries += 1
                time.sleep(2 ** attempt)
                continue
            r = json.loads(out.stdout)
            if "choices" not in r:
                last_err = f"no choices: {out.stdout[:300]}"
                retries += 1
                time.sleep(2 ** attempt)
                continue
            msg = r["choices"][0]["message"]
            content = (msg.get("content") or "").strip()
            usage = r.get("usage", {}) or {}
            latency_ms = (time.monotonic() - t0) * 1000
            with _lock:
                _call_counter += 1
                cid = _call_counter
            _append({
                "call_id": cid,
                "ts": datetime.now(timezone.utc).astimezone().isoformat(),
                "role": role, "qid": qid, "arm": arm, "model": model,
                "temperature": temperature, "max_tokens": max_tokens,
                "prompt_chars": len(prompt),
                "prompt_excerpt": prompt[:800],
                "response": content,
                "usage": usage,
                "latency_ms": round(latency_ms, 1),
                "retries": retries,
                "error": None,
            })
            if not content:
                last_err = "empty content"
                retries += 1
                time.sleep(2 ** attempt)
                continue
            return content, usage, round(latency_ms, 1), retries
        except Exception as e:  # noqa: BLE001
            last_err = repr(e)
            retries += 1
            time.sleep(2 ** attempt)
    # 最终失败：落一条错误审计记录后抛出。
    with _lock:
        _call_counter += 1
        cid = _call_counter
    _append({
        "call_id": cid, "ts": datetime.now(timezone.utc).astimezone().isoformat(),
        "role": role, "qid": qid, "arm": arm, "model": model,
        "temperature": temperature, "max_tokens": max_tokens,
        "prompt_chars": len(prompt), "prompt_excerpt": prompt[:800],
        "response": "", "usage": {}, "latency_ms": None,
        "retries": retries, "error": last_err,
    })
    raise RuntimeError(f"LLM call failed after {retries} retries: {last_err}")


def reader_call(prompt: str, qid: str, arm: str, max_tokens: int) -> tuple[str, dict, float, int]:
    return _post(READER_MODEL, prompt, max_tokens, 0.0, "reader", qid, arm)


def judge_call(prompt: str, qid: str, arm: str, max_tokens: int = 32) -> tuple[str, dict, float, int]:
    return _post(JUDGE_MODEL, prompt, max_tokens, 0.0, "judge", qid, arm)
