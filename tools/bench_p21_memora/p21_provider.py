#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""P21 Memora 接入：DeepSeek 在线判分/作答通道（规格 §2）。

口径（与官方 Track 2 差异显式记录）：
- 官方 Track 2：作答 gpt-4o-mini（temperature 0.7）；判分三裁判多数票
  （openai/gpt-4.1、anthropic/claude-haiku-4.5、google/gemini-2.5-flash，OpenRouter）。
- 本适配器：作答显式 deepseek-flash（=官方 DeepSeek-V4.1-Flash，默认 effort=high
  推理开启，max_tokens 2048 下推理挤占无碍）；判分 deepseek-chat 别名（同物理模型
  无推理模式，稳定输出 JSON；显式 flash 推理会吃空 judge 的 max_tokens）；
  单裁判；temperature=0。两分支（无治理 / 有治理）共用同一裁判，因此相对治理
  对照仍然可比；绝对分数与官方 Table 3 不直接可比。

审计：每次调用（请求/响应/耗时/重试/usage）逐条追加到 llm_calls.jsonl，
线程安全（锁内追加）。key 只在运行时从 ~/.zshrc 读取，不落盘、不写日志。
硬超时用 Popen + 进程组 kill（规避 macOS 上 curl 慢传输不被 --max-time 截断、
子进程持有管道导致 subprocess.run(timeout) 挂死的问题）。
"""
from __future__ import annotations

import json
import os
import signal
import subprocess
import tempfile
import threading
import time
from pathlib import Path
from typing import Any, Optional

ENDPOINT = "https://api.deepseek.com/chat/completions"
KEY_ENV = "DEEPSEEK_API_KEY"


class DeepSeekProvider:
    """curl -4 硬超时调用（规避 macOS urllib TLS 冻结，照抄 tools/eval_fidelity.py）。"""

    def __init__(self, jsonl_path: Path, model: str = "deepseek-flash",
                 judge_model: str = "deepseek-chat",
                 temperature: float = 0.0, max_tokens: int = 2048,
                 reasoning_off: bool = True, timeout_s: int = 60, retries: int = 3):
        self.jsonl_path = Path(jsonl_path)
        self.jsonl_path.parent.mkdir(parents=True, exist_ok=True)
        self.model = model
        self.judge_model = judge_model
        self.temperature = temperature
        self.max_tokens = max_tokens
        self.reasoning_off = reasoning_off
        self.timeout_s = timeout_s
        self.retries = retries
        self._lock = threading.Lock()
        self._key: Optional[str] = None
        self.stats = {"calls": 0, "failures": 0, "retries": 0,
                      "prompt_tokens": 0, "completion_tokens": 0,
                      "elapsed_s": 0.0}

    def _load_key(self) -> str:
        if self._key:
            return self._key
        key = os.environ.get(KEY_ENV, "")
        if not key:
            out = subprocess.run(
                ["/bin/zsh", "-lc", f"source ~/.zshrc && echo -n ${KEY_ENV}"],
                capture_output=True, text=True, timeout=30)
            key = out.stdout.strip()
        self._key = key
        return key

    def _post(self, body: dict[str, Any]) -> tuple[int, str, str]:
        key = self._load_key()
        # body 写临时文件、curl 用 -d @file 读入：长 prompt 不再撑爆 argv
        # （macOS ARG_MAX → 'Argument list too long'），调用语义与内联 -d 完全一致。
        # 响应写临时文件，避免管道被子进程/代理继承导致 communicate(timeout) 挂死；
        # --speed-time 15 在流停滞 15s 后主动放弃，--max-time 60 总时长硬上限。
        bodyf = tempfile.NamedTemporaryFile(delete=False, suffix=".body",
                                            mode="w", encoding="utf-8")
        json.dump(body, bodyf, ensure_ascii=False)
        bodyf.close()
        outf = tempfile.NamedTemporaryFile(delete=False, suffix=".out")
        errf = tempfile.NamedTemporaryFile(delete=False, suffix=".err")
        outf.close()
        errf.close()
        try:
            proc = subprocess.Popen(
                ["curl", "-4", "-sS", "--max-time", str(self.timeout_s),
                 "--speed-time", "15", "--speed-limit", "1",
                 "-X", "POST", ENDPOINT,
                 "-H", f"Authorization: Bearer {key}",
                 "-H", "Content-Type: application/json",
                 "-d", "@" + bodyf.name,
                 "-o", outf.name],
                stderr=open(errf.name, "w", encoding="utf-8"),
                start_new_session=True)
            rc = 0
            try:
                rc = proc.wait(timeout=self.timeout_s + 10)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(proc.pid, signal.SIGKILL)
                except Exception:  # noqa: BLE001
                    proc.kill()
                try:
                    proc.wait(timeout=5)
                except Exception:  # noqa: BLE001
                    pass
                rc = 124
        finally:
            try:
                os.unlink(bodyf.name)
            except OSError:
                pass
        out = Path(outf.name).read_text(encoding="utf-8", errors="replace")
        err = Path(errf.name).read_text(encoding="utf-8", errors="replace")
        for p in (outf.name, errf.name):
            try:
                os.unlink(p)
            except OSError:
                pass
        return rc, out, err

    def call(self, system: str, user: str, purpose: str,
             meta: Optional[dict[str, Any]] = None,
             max_tokens: Optional[int] = None,
             model: Optional[str] = None) -> dict[str, Any]:
        """一次 chat 调用；返回 {content, usage, elapsed_s, retries, ok}。

        失败重试 self.retries 次、指数退避；最终失败 content=""、ok=False（不抛断
        批次，失败 sub-question 按官方口径记 incorrect，并在结果中标注）。
        """
        use_model = model or self.model
        messages: list[dict[str, str]] = []
        if system:
            messages.append({"role": "system", "content": system})
        messages.append({"role": "user", "content": user})
        body: dict[str, Any] = {
            "model": use_model,
            "messages": messages,
            "max_tokens": max_tokens or self.max_tokens,
            "temperature": self.temperature,
        }
        if self.reasoning_off and "flash" in use_model:
            body["reasoning"] = {"enabled": False}

        retries = 0
        last_err = ""
        t0 = time.time()
        content = ""
        usage: dict[str, Any] = {}
        ok = False
        for attempt in range(self.retries + 1):
            try:
                rc, out, err = self._post(body)
                if rc != 0:
                    last_err = f"curl rc={rc}: {err[:300]}"
                    retries += 1
                    time.sleep(2.0 * (2 ** attempt))
                    continue
                resp = json.loads(out)
                choice = resp["choices"][0]["message"]
                content = (choice.get("content") or "").strip()
                usage = resp.get("usage", {}) or {}
                if not content:
                    last_err = "empty content"
                    retries += 1
                    time.sleep(2.0 * (2 ** attempt))
                    continue
                ok = True
                break
            except json.JSONDecodeError as exc:
                last_err = f"json decode: {exc}; raw={out[:200]}" if 'out' in dir() else str(exc)
                retries += 1
                time.sleep(2.0 * (2 ** attempt))
            except Exception as exc:  # noqa: BLE001 — 网络异常统一退避重试
                last_err = f"{type(exc).__name__}: {exc}"
                retries += 1
                time.sleep(2.0 * (2 ** attempt))

        elapsed = time.time() - t0
        with self._lock:
            self.stats["calls"] += 1
            self.stats["elapsed_s"] += elapsed
            self.stats["prompt_tokens"] += int(usage.get("prompt_tokens", 0) or 0)
            self.stats["completion_tokens"] += int(usage.get("completion_tokens", 0) or 0)
            if not ok:
                self.stats["failures"] += 1
            self.stats["retries"] += max(0, retries - (1 if ok else 0))
            record = {
                "ts": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
                "purpose": purpose,
                "model": use_model,
                "temperature": self.temperature,
                "max_tokens": max_tokens or self.max_tokens,
                "reasoning_enabled": not self.reasoning_off,
                "meta": meta or {},
                "system_prompt_chars": len(system),
                "user_prompt_chars": len(user),
                "user_prompt_preview": user[:400],
                "response": content[:4000],
                "usage": usage,
                "elapsed_s": round(elapsed, 3),
                "retries": retries,
                "ok": ok,
                "error": last_err if not ok else "",
            }
            with self.jsonl_path.open("a", encoding="utf-8") as fh:
                fh.write(json.dumps(record, ensure_ascii=False) + "\n")

        return {"content": content, "usage": usage, "elapsed_s": elapsed,
                "retries": retries, "ok": ok}
