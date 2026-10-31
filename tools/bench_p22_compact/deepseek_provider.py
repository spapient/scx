"""P22 · DeepSeek OpenAI 兼容 provider（接入 compactbench 0.1.0 harness）。

背景：pip 0.1.0 内置 provider 只有 mock / groq / google-ai-studio / ollama，
没有 OpenAI 兼容端点；本机无 ollama。本类按官方 Provider ABC 实现 OpenAI
``/chat/completions`` 线协议，指向 DeepSeek（spec §2），由驱动脚本注册进
``compactbench.providers`` 注册表后供 harness 调用。

传输层：实测 httpx 在本机（macOS）会在长 transcript 上偶发连接冻结（无超时
触发，挂死 20+ 分钟）；与 tools/eval_fidelity.py 一致，改用 ``curl -4`` 子进程
+ 硬超时，在线程池里跑，对外仍是 async Provider 接口。

口径固定：temperature=0；max_tokens ≥2048（deepseek-flash 带 reasoning，
思考会吃额度，见 tools/bench_code.py L50-64 实测注记）；失败指数退避重试；
每次调用经 p22_logger 逐条落盘（key 不进日志）。
"""
from __future__ import annotations

import asyncio
import json
import os
import subprocess
import time
from typing import Any, ClassVar

from compactbench.providers._retry import retry_with_backoff
from compactbench.providers.base import CompletionRequest, CompletionResponse, Provider
from compactbench.providers.errors import ProviderError, ProviderResponseError

from p22_logger import get_logger


def _resolve_key() -> str:
    key = os.environ.get("DEEPSEEK_API_KEY", "").strip()
    if key:
        return key
    # 与 tools/eval_fidelity.py 同口径：从 ~/.zshrc 取，不写入任何文件。
    out = subprocess.run(
        ["/bin/zsh", "-lc", "source ~/.zshrc && echo -n $DEEPSEEK_API_KEY"],
        capture_output=True, text=True, timeout=30,
    )
    return out.stdout.strip()


class DeepSeekProvider(Provider):
    """异步 OpenAI 兼容客户端（DeepSeek chat completions，curl -4 传输）。"""

    key: ClassVar[str] = "deepseek"

    def __init__(
        self,
        *,
        api_key: str | None = None,
        base_url: str | None = None,
        max_retries: int = 5,
        base_backoff_seconds: float = 3.0,
    ) -> None:
        self._base_url = (
            base_url or os.environ.get("DEEPSEEK_BASE_URL")
            or "https://api.deepseek.com/chat/completions"
        )
        self._key = (api_key or "").strip() or _resolve_key()
        if not self._key:
            raise ProviderError("DEEPSEEK_API_KEY 未设置（env 与 ~/.zshrc 均未取到）")
        self._max_retries = max_retries
        self._base_backoff = base_backoff_seconds
        # 双传输轮替：直连 <-> 本机 ssh 转发代理（127.0.0.1:8080 -> 远端 8888）。
        # 两条路都间歇卡顿；轮替可绕过单边坏路。SCX_P22_HTTPS_PROXY 覆盖代理地址，
        # 置空串则只用直连。
        self._proxy = os.environ.get("SCX_P22_HTTPS_PROXY", "http://127.0.0.1:8080")

    async def close(self) -> None:
        return None

    # ---- 同步 curl 调用（在线程内执行）；transport=0 直连，1 走代理 ----
    def _curl(self, body: dict[str, Any], transport: int) -> dict[str, Any]:
        payload = json.dumps(body)
        cmd = [
            "curl", "-4", "-sS", "--connect-timeout", "8", "--max-time", "40",
            "-X", "POST", self._base_url,
            "-H", f"Authorization: Bearer {self._key}",
            "-H", "Content-Type: application/json",
            "-d", payload,
        ]
        if transport == 1 and self._proxy:
            cmd += ["-x", self._proxy]
        try:
            proc = subprocess.run(cmd, capture_output=True, text=True, timeout=50)
        except subprocess.TimeoutExpired as exc:
            raise ProviderError(f"subprocess-timeout transport={transport}") from exc
        if proc.returncode != 0:
            raise ProviderError(
                f"curl rc={proc.returncode} transport={transport}: "
                f"{proc.stderr.strip()[:180]}"
            )
        data = json.loads(proc.stdout)
        if "error" in data:
            raise ProviderResponseError(f"DeepSeek error: {data['error']}")
        return data

    async def complete(self, request: CompletionRequest) -> CompletionResponse:
        messages: list[dict[str, str]] = []
        if request.system:
            messages.append({"role": "system", "content": request.system})
        messages.append({"role": "user", "content": request.prompt})

        max_tokens = max(2048, request.max_tokens)
        body: dict[str, Any] = {
            "model": request.model,
            "messages": messages,
            "temperature": 0,  # spec §2：固定 temperature 0
            "max_tokens": max_tokens,
        }
        if request.response_format:
            body["response_format"] = request.response_format
        if "flash" in request.model:
            body["reasoning"] = {"enabled": False}

        started = time.perf_counter()
        retries = 0
        attempt_errors: list[str] = []

        def _is_retryable(exc: Exception) -> bool:
            msg = str(exc)
            return any(code in msg for code in (
                "429", "500", "502", "503", "504", "timeout", "Timeout",
                "rc=7", "rc=28", "rc=56", "Connection reset",
            ))

        last_transport = -1

        async def _call() -> dict[str, Any]:
            nonlocal retries, last_transport
            retries += 1
            # 交替：首次直连；仅当配置了代理时第 2、4、6 次走代理。
            transport = 0 if (retries % 2 == 1 or not self._proxy) else 1
            last_transport = transport
            try:
                data = await asyncio.to_thread(self._curl, body, transport)
            except Exception as exc:  # noqa: BLE001
                attempt_errors.append(
                    f"attempt {retries} transport={transport}: {str(exc)[:180]}"
                )
                raise
            choices = data.get("choices") or []
            if not choices:
                raise ProviderResponseError("DeepSeek 返回空 choices")
            return data

        data = await retry_with_backoff(
            _call,
            is_retryable=_is_retryable,
            max_retries=self._max_retries,
            base_delay=self._base_backoff,
        )
        choices = data["choices"]
        message = choices[0].get("message") or {}
        text = message.get("content") or ""
        usage = data.get("usage") or {}
        model_out = data.get("model", request.model)

        latency = int((time.perf_counter() - started) * 1000)
        get_logger().record(
            model=model_out,
            role="evaluation",
            system=request.system,
            prompt=request.prompt,
            response=text,
            usage={
                "prompt_tokens": usage.get("prompt_tokens", 0),
                "completion_tokens": usage.get("completion_tokens", 0),
                "total_tokens": usage.get("total_tokens", 0),
            },
            latency_ms=latency,
            retries=retries,
            provider=self.key,
            extra={"endpoint_kind": "custom", "max_tokens": max_tokens,
                   "transport": "direct" if last_transport == 0 else "proxy",
                   "attempt_errors": attempt_errors},
        )

        if not text.strip():
            raise ProviderResponseError("DeepSeek 空 content（reasoning 吃光 max_tokens？）")

        return CompletionResponse(
            text=text,
            prompt_tokens=usage.get("prompt_tokens", 0) or 0,
            completion_tokens=usage.get("completion_tokens", 0) or 0,
            model=model_out,
            raw={"provider": self.key, "finish_reason": choices[0].get("finish_reason")},
        )
