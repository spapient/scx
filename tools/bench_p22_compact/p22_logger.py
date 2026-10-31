"""P22 CompactBench 运行 · LLM 调用逐条审计日志（JSONL）。

规格（共享 spec §2）：每次 LLM 调用请求/响应落盘 llm_calls.jsonl，含时间、模型、
prompt 摘要、response、耗时；API key 绝不写进任何记录。线程/异步并发安全（锁）。
"""
from __future__ import annotations

import json
import os
import threading
import time
from pathlib import Path
from typing import Any


class CallLogger:
    """把每次 provider.complete 调用追加写为一行 JSON。"""

    def __init__(self, path: str | os.PathLike[str] | None = None) -> None:
        raw = path or os.environ.get("SCX_P22_LLM_LOG", "")
        self._path = Path(raw) if raw and raw != "." else None
        self._lock = threading.Lock()
        self._count = 0
        if self._path:
            self._path.parent.mkdir(parents=True, exist_ok=True)

    @property
    def enabled(self) -> bool:
        return bool(self._path)

    def record(
        self,
        *,
        model: str,
        role: str,
        system: str | None,
        prompt: str,
        response: str,
        usage: dict[str, Any] | None,
        latency_ms: int,
        retries: int,
        provider: str,
        error: str | None = None,
        extra: dict[str, Any] | None = None,
    ) -> None:
        self._count += 1
        record: dict[str, Any] = {
            "call_index": self._count,
            "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
            "provider": provider,
            "model": model,
            "role": role,
            "latency_ms": latency_ms,
            "retries": retries,
            "prompt_chars": len(prompt),
            "prompt_head": prompt[:800],
            "system": (system or "")[:400],
            "response": response,
            "response_chars": len(response),
            "usage": usage or {},
            "error": error,
        }
        if extra:
            record["extra"] = extra
        if not self._path:
            return
        with self._lock:
            with self._path.open("a", encoding="utf-8") as fp:
                fp.write(json.dumps(record, ensure_ascii=False) + "\n")


# 进程内单例：provider 在 harness 内被多次实例化，共享同一份日志句柄。
_LOGGER: CallLogger | None = None


def get_logger() -> CallLogger:
    global _LOGGER
    if _LOGGER is None:
        _LOGGER = CallLogger()
    return _LOGGER


def reset_logger(path: str | os.PathLike[str]) -> None:
    """驱动脚本在开跑前显式绑定日志路径。"""
    global _LOGGER
    _LOGGER = CallLogger(path)
