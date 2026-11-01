"""tools/ 公共封装（bench 脚本复用）。

chat:    OpenAI 兼容 chat/completions 调用（消除 bench_arch/bench_code/bench_kernel 重复）。
compress: scx 二进制压缩调用（消除 bench.py/bench_ablate.py 重复）。
"""
import json
import os
import subprocess
import tempfile
import time
import urllib.request


def chat(cfg, prompt, max_new, retries=3, sleep_base=2.0, timeout=240,
         reason_off=False, strict_content=False):
    """按 cfg{url,model,env} 调 chat/completions，返回 content 字符串。

    reason_off=True 且模型名含 'flash' 时下发 {"reasoning":{"enabled":False}}
    （deepseek-flash/qwen3.8-flash 带思考：关思考提速 20-30x）。
    strict_content=True 时空 content 视为失败重试，最终返回 ""（不抛）。
    """
    key = os.environ.get(cfg["env"], "")
    if not key:
        raise SystemExit(f"{cfg['env']} env missing")
    body = {
        "model": cfg["model"],
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": max_new,
        "temperature": 0.1,
    }
    if reason_off and "flash" in cfg["model"]:
        body["reasoning"] = {"enabled": False}
    last = None
    for attempt in range(retries):
        try:
            req = urllib.request.Request(
                cfg["url"],
                data=json.dumps(body).encode(),
                headers={"Authorization": f"Bearer {key}",
                         "Content-Type": "application/json"},
            )
            with urllib.request.urlopen(req, timeout=timeout) as r:
                content = json.loads(r.read())["choices"][0]["message"]["content"]
            if not strict_content or (content and content.strip()):
                return content
            last = RuntimeError("empty model content")
        except Exception as e:  # noqa: BLE001 网络/超时/限流均重试
            last = e
        time.sleep(sleep_base * (attempt + 1))
    if strict_content:
        return ""
    raise last


def compress(binary, text, query, extra=(), timeout=120):
    """调用 scx 压缩 text；失败/空输出直通返回原文；截断 `compressed:` 装饰块。"""
    with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as fh:
        fh.write(text)
        path = fh.name
    try:
        proc = subprocess.run(
            [binary, path, "--query", query, "--ratio", "0.35", "--context", "1", *extra],
            capture_output=True, text=True, timeout=timeout,
        )
        if proc.returncode != 0:
            return text
        lines = proc.stdout.splitlines()
        for k, ln in enumerate(lines):
            if ln.lstrip().startswith("compressed:"):
                out = "\n".join(lines[:k])
                return out if out.strip() else text
        return text
    finally:
        os.unlink(path)
