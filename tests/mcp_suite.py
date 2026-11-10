#!/usr/bin/env python3
"""scx MCP 全场景自动化门禁（真实进程、真实 stdio、无 mock）。

覆盖六个面：
  1. 协议层：initialize / notification / tools/list / 未知 method / 未知 tool /
     非法 JSON / 字符串 id / 超长帧
  2. compress_context 参数面：压缩、空输入、单行、ratio 单调、max_tokens、
     kind、evidence 行号与子串、中文、CRLF、store / mem_dir
  3. 账本与统计：usage_summary、ledger_summary、context_diff、跨进程累积、
     坏账本容错、只读目录静默降级
  4. 向量后端：内置编码器（无模型可用）、余弦排序、外部 provider 协议、
     --embed-cache 命中、provider 失败不崩
  5. 压力与并发：100KB 长文、同进程 10 连压、两进程同账本并发追加
  6. 确定性：同输入两次输出逐字节一致

用法：
    python3 tests/mcp_suite.py [--bin PATH] [-v]

退出码：0 全部通过；1 有失败。任何 FAIL 都视为门禁不通过。
"""
from __future__ import annotations

import argparse
import json
import os
import signal
import stat
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FRAME_MAX = 1_000_000  # 本套件里最长的单帧（~120KB 文本）远小于服务端上限


def default_bin() -> str:
    """优先用本机已安装的 release 二进制，其次仓库构建产物。"""
    for p in (
        Path.home() / ".local/bin/scx",
        Path.home() / ".local/share/scx/release/scx",
        ROOT / "target/release/scx",
    ):
        if p.exists():
            return str(p)
    return "scx"


class Timeout(Exception):
    pass


class Client:
    """一个 `scx mcp` 子进程；请求与响应严格 1:1（notification 除外）。"""

    def __init__(self, binpath: str, ledger: Path, mem: Path | None = None, extra: dict | None = None):
        env = dict(os.environ)
        env["SCX_LEDGER_FILE"] = str(ledger)
        if mem is not None:
            env["SCX_MEM_DIR"] = str(mem)
        env.pop("SCX_EMBED_VEC", None)  # 避免宿主机环境影响默认后端判定
        env.update(extra or {})
        self.p = subprocess.Popen(
            [binpath, "mcp"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
            env=env,
        )
        self.n = 0
        self.code: int | None = None
        self.err = ""

    # --- I/O ---
    def raw(self, line: str) -> None:
        self.p.stdin.write(line + "\n")
        self.p.stdin.flush()

    def read(self, timeout: float = 30.0) -> dict:
        def _alarm(_s, _f):
            raise Timeout("no response within %.1fs" % timeout)

        old = signal.signal(signal.SIGALRM, _alarm)
        signal.setitimer(signal.ITIMER_REAL, timeout)
        try:
            line = self.p.stdout.readline()
        finally:
            signal.setitimer(signal.ITIMER_REAL, 0)
            signal.signal(signal.SIGALRM, old)
        if not line:
            raise AssertionError("server closed stdout")
        return json.loads(line)

    def call(self, method: str, params: dict | None = None, notify: bool = False):
        self.n += 1
        msg = {"jsonrpc": "2.0", "method": method}
        if not notify:
            msg["id"] = self.n
        if params is not None:
            msg["params"] = params
        self.raw(json.dumps(msg, ensure_ascii=False))
        return None if notify else self.read()

    def tool(self, name: str, args: dict | None = None):
        return self.call("tools/call", {"name": name, "arguments": args or {}})

    def close(self) -> tuple[int, str]:
        try:
            self.p.stdin.close()
        except Exception:
            pass
        try:
            self.code = self.p.wait(timeout=20)
        except Exception:
            self.p.kill()
            self.code = -9
        self.err = self.p.stderr.read()
        self.p.stderr.close()
        return self.code, self.err


def payload(r: dict) -> dict:
    """工具返回的单块文本：效果卡在上、机器 JSON 在最后一行。"""
    if "error" in r:
        raise AssertionError("unexpected error: %s" % json.dumps(r["error"], ensure_ascii=False))
    text = r["result"]["content"][0]["text"]
    return json.loads(text.splitlines()[-1])


def text_of(r: dict) -> str:
    return r["result"]["content"][0]["text"]


def expect(cond, msg: str) -> None:
    if not cond:
        raise AssertionError(msg)


class Gate:
    def __init__(self, verbose: bool):
        self.n = 0
        self.fail: list[tuple[str, str]] = []
        self.verbose = verbose

    def section(self, title: str) -> None:
        print("\n[%s]" % title)

    def check(self, name: str, fn) -> None:
        self.n += 1
        try:
            detail = fn()
        except Exception as e:  # noqa: BLE001 - 门禁需要把任何异常记为 FAIL
            self.fail.append((name, str(e)))
            print("  FAIL %-46s %s" % (name, e))
        else:
            print("  ok   %-46s%s" % (name, ("  " + str(detail)) if detail and self.verbose else ""))


def code_lines(n: int = 80) -> str:
    rows = []
    for i in range(n):
        rows.append(
            "fn item_%d(buf: &[u8], cap: usize) -> usize { if cap == 0 { return buf.len(); } "
            "let k = cap * %d; k %% (buf.len() + 1) }" % (i, i + 1)
        )
    return "\n".join(rows)


CN_TEXT = "\n".join(
    "第%d条：压缩器在长上下文里保留与问题相关的行，删掉重复和无关的样板文字，"
    "并保持行号锚点与数值不变。" % (i + 1)
    for i in range(40)
)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", default=default_bin())
    ap.add_argument("-v", "--verbose", action="store_true")
    a = ap.parse_args()
    binpath = a.bin
    if not Path(binpath).exists() and "/" in binpath:
        print("binary not found: %s" % binpath)
        return 1

    gate = Gate(a.verbose)
    tmp = Path(tempfile.mkdtemp(prefix="scx-mcp-suite-"))
    print("binary : %s" % binpath)
    print("sandbox: %s" % tmp)

    # ---------------------------------------------------------------- 协议层
    gate.section("1. 协议层")

    c = Client(binpath, tmp / "p1" / "ledger.jsonl", tmp / "p1" / "mem")
    (tmp / "p1").mkdir(parents=True, exist_ok=True)

    def init_ok():
        r = c.call("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                  "clientInfo": {"name": "suite", "version": "1"}})
        expect(r["result"]["serverInfo"]["name"].startswith("scx"),
               "serverInfo.name 不以 scx 开头: %s" % r["result"]["serverInfo"]["name"])
        expect("protocolVersion" in r["result"], "missing protocolVersion")
        return r["result"]["serverInfo"].get("version", "")

    gate.check("initialize 返回 serverInfo", init_ok)

    def notification_silent():
        c.call("notifications/initialized", notify=True)
        r = c.call("tools/list")
        expect(r["id"] == c.n, "notification 产生了额外响应：%s" % r)
        return "no stray reply"

    gate.check("notification 不回复", notification_silent)

    def tools_shape():
        r = c.call("tools/list")
        tools = r["result"]["tools"]
        expect(len(tools) == 8, "tools 数量 %d != 8" % len(tools))
        names = {t["name"] for t in tools}
        need = {"compress_context", "context_snapshot", "context_diff", "ledger_summary",
                "connect_account", "usage_summary", "vec", "vec_sim"}
        expect(names == need, "工具集不符：%s" % (names ^ need))
        for t in tools:
            expect(isinstance(t.get("inputSchema"), dict), "%s 缺 inputSchema" % t["name"])
            expect(t["inputSchema"].get("type") == "object", "%s schema 非 object" % t["name"])
            props = t["inputSchema"].get("properties", {})
            for req in t["inputSchema"].get("required", []):
                expect(req in props, "%s 的 required %r 不在 properties" % (t["name"], req))
        return "%d tools" % len(tools)

    gate.check("tools/list schema 完整", tools_shape)

    def new_params_advertised():
        r = c.call("tools/list")
        by = {t["name"]: t for t in r["result"]["tools"]}
        for key in ("store", "mem_dir"):
            expect(key in by["compress_context"]["inputSchema"]["properties"],
                   "compress_context 缺 %s" % key)
        for t in ("vec", "vec_sim"):
            expect("provider" in by[t]["inputSchema"]["properties"], "%s 缺 provider" % t)
        return "store/mem_dir/provider"

    gate.check("新增参数写入 schema", new_params_advertised)

    def unknown_method():
        r = c.call("nope/method")
        expect(r.get("error", {}).get("code") == -32601, "未知 method 未回 -32601: %s" % r)
        return "-32601"

    gate.check("未知 method → -32601", unknown_method)

    def unknown_tool():
        r = c.tool("no_such_tool")
        expect(r.get("error", {}).get("code") == -32602, "未知 tool 未回 -32602: %s" % r)
        return "-32602"

    gate.check("未知 tool → -32602", unknown_tool)

    def bad_json():
        c.raw('{"jsonrpc":"2.0","id":999,')
        r = c.read()
        expect("error" in r, "非法 JSON 无 error: %s" % r)
        r2 = c.call("tools/list")
        expect("result" in r2, "非法 JSON 后服务不再响应")
        return "recovered"

    gate.check("非法 JSON 报错且可恢复", bad_json)

    def string_id():
        c.n += 1
        c.raw(json.dumps({"jsonrpc": "2.0", "id": "abc", "method": "tools/list"}))
        r = c.read()
        expect(r["id"] == "abc", "字符串 id 未回显: %s" % r)
        return "echo"

    gate.check("字符串 id 回显", string_id)

    def oversize_frame():
        big = '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"context_snapshot","arguments":{"text":"%s"}}}' % ("x" * (FRAME_MAX + 200))
        c.raw(big)
        r = c.read()
        expect("error" in r or "result" in r, "超长帧无任何响应")
        r2 = c.call("tools/list")
        expect("result" in r2, "超长帧后服务不再同步")
        return "sync"

    gate.check("超长帧后仍按行同步", oversize_frame)

    # ------------------------------------------------------- compress_context
    gate.section("2. compress_context")

    c2 = Client(binpath, tmp / "p2" / "ledger.jsonl", tmp / "p2" / "mem")
    (tmp / "p2").mkdir(parents=True, exist_ok=True)
    code = code_lines()

    def basic():
        r = c2.tool("compress_context", {"query": "item_42 的 cap 计算", "text": code})
        p = payload(r)
        expect(p["kept"] < p["total"], "没有压缩：kept=%s total=%s" % (p["kept"], p["total"]))
        expect(p["token_cut_pct"] > 0, "token_cut_pct=0")
        expect(len(p["text"]) > 0, "压缩正文为空")
        expect(text_of(r).lstrip().startswith("✦ SCX"), "首行不是效果卡: %r" % text_of(r)[:40])
        return "-%.1f%%" % p["token_cut_pct"]

    gate.check("基本压缩 + 效果卡", basic)

    def empty_text():
        r = c2.tool("compress_context", {"query": "q", "text": ""})
        p = payload(r)
        expect(p["total"] == 0, "空输入 total=%s" % p["total"])
        return "total=0"

    gate.check("空 text 不崩", empty_text)

    def one_line():
        r = c2.tool("compress_context", {"query": "q", "text": "a single short line"})
        p = payload(r)
        expect(p["total"] == 1, "单行 total=%s" % p["total"])
        expect(p["text"] == "a single short line", "单行被改写：%r" % p["text"])
        return "passthrough"

    gate.check("单行原样返回", one_line)

    def ratio_monotone():
        lo = payload(c2.tool("compress_context", {"query": "item_42", "text": code, "ratio": 0.0}))["kept"]
        hi = payload(c2.tool("compress_context", {"query": "item_42", "text": code, "ratio": 1.0}))["kept"]
        expect(lo <= hi, "ratio 单调性破坏：%.0f vs %.0f" % (lo, hi))
        return "%.0f<=%.0f" % (lo, hi)

    gate.check("ratio 0..1 单调", ratio_monotone)

    def max_tokens():
        p = payload(c2.tool("compress_context", {"query": "item_42", "text": code, "max_tokens": 200}))
        expect(p["tokens_out"] <= 200 * 1.15, "max_tokens 未生效：%.1f" % p["tokens_out"])
        return "%.0f tokens" % p["tokens_out"]

    gate.check("max_tokens 上限", max_tokens)

    def kinds():
        for k in ("static-contract", "system-definition", "session-message", "tool-output"):
            p = payload(c2.tool("compress_context", {"query": "item_42", "text": code, "kind": k}))
            expect(p.get("kind") == k, "kind=%s 未生效：%s" % (k, p.get("kind")))
        p = payload(c2.tool("compress_context", {"query": "item_42", "text": code, "kind": "auto"}))
        expect(p.get("kind"), "auto 未推断出 kind")
        return p.get("kind")

    gate.check("kind 五种取值", kinds)

    def evidence_line():
        p = payload(c2.tool("compress_context", {"query": "无关问题", "text": code, "evidence": [30]}))
        expect("item_29" in p["text"], "evidence 行号 30 的内容被删掉了")
        return "line 30 kept"

    gate.check("evidence 行号强制保留", evidence_line)

    def evidence_substr():
        p = payload(c2.tool("compress_context", {"query": "无关问题", "text": code, "evidence": ["item_63"]}))
        expect("item_63" in p["text"], "evidence 子串被删掉了")
        return "substr kept"

    gate.check("evidence 子串强制保留", evidence_substr)

    def chinese():
        p = payload(c2.tool("compress_context", {"query": "第17条 提到什么", "text": CN_TEXT}))
        expect(p["total"] == 40, "中文行数 %s" % p["total"])
        expect(p["kept"] < 40, "中文未压缩：%s" % p["kept"])
        expect("第17条" in p["text"], "中文证据行丢失")
        return "kept=%s" % p["kept"]

    gate.check("中文压缩保证据", chinese)

    def crlf():
        crlf = code.replace("\n", "\r\n")
        p = payload(c2.tool("compress_context", {"query": "q", "text": crlf, "evidence": [3]}))
        expect("item_2(" in p["text"], "CRLF 下 evidence 行号错位")
        expect("\r\r" not in p["text"], "出现重复 CR，行边界被破坏")
        return "CRLF preserved"

    gate.check("CRLF 行号一致", crlf)

    def store_true():
        memroot = tmp / "p2" / "memstore"
        r = c2.tool("compress_context", {"query": "item_42 的 cap", "text": code, "store": True,
                                        "mem_dir": str(memroot)})
        p = payload(r)
        expect(p["stored"]["ok"] is True, "store 失败：%s" % p["stored"])
        cases = list((memroot / "cases").glob("*")) if (memroot / "cases").exists() else []
        expect(len(cases) == 1, "落库目录数 %d != 1" % len(cases))
        return p["stored"]["case"]

    gate.check("store=true 落库", store_true)

    def store_false():
        p = payload(c2.tool("compress_context", {"query": "q", "text": code}))
        expect(p["stored"] is None, "未传 store 却落库：%s" % p["stored"])
        return "null"

    gate.check("store 缺省不落库", store_false)

    # ------------------------------------------------------------ 账本与统计
    gate.section("3. 账本与统计")

    led = tmp / "p3" / "ledger.jsonl"
    c3 = Client(binpath, led, tmp / "p3" / "mem")
    (tmp / "p3").mkdir(parents=True, exist_ok=True)

    def usage_counts():
        c3.tool("compress_context", {"query": "item_42", "text": code})
        c3.tool("compress_context", {"query": "item_7", "text": CN_TEXT})
        u = json.loads(text_of(c3.tool("usage_summary")))
        expect(u["compressed"] == 2, "compressed=%s" % u["compressed"])
        expect(u["chars_out"] < u["chars_in"], "chars_out 不小于 chars_in")
        return "%s→%s" % (u["chars_in"], u["chars_out"])

    gate.check("usage_summary 计数", usage_counts)

    def ledger_agg():
        l = payload(c3.tool("ledger_summary"))
        expect(l["total"]["calls"] == 2, "calls=%s" % l["total"]["calls"])
        expect(l["total"]["tokens_saved"] > 0, "tokens_saved<=0")
        expect(0 < l["total"]["ratio"] < 1, "ratio 越界：%s" % l["total"]["ratio"])
        return "saved=%.0f ratio=%.3f" % (l["total"]["tokens_saved"], l["total"]["ratio"])

    gate.check("ledger_summary 聚合口径", ledger_agg)

    def snapshot_diff():
        c3.tool("context_snapshot", {"text": code, "kind": "tool-output"})
        c3.tool("context_snapshot", {"text": code[: len(code) // 2], "kind": "tool-output"})
        d = payload(c3.tool("context_diff"))
        expect("delta" in json.dumps(d), "diff 无 delta 字段: %s" % d)
        return "delta ok"

    gate.check("snapshot + diff", snapshot_diff)

    def ledger_persisted():
        expect(led.exists(), "账本文件未落盘：%s" % led)
        n = sum(1 for _ in led.open("rb"))
        expect(n >= 4, "账本行数 %d < 4" % n)
        return "%d lines" % n

    gate.check("账本落盘行数", ledger_persisted)

    def cross_process():
        c3b = Client(binpath, led, tmp / "p3" / "mem")
        l = payload(c3b.tool("ledger_summary"))
        expect(l["total"]["calls"] == 4, "新进程读回 calls=%s != 4" % l["total"]["calls"])
        c3b.close()
        return "restart ok"

    gate.check("跨进程读回账本", cross_process)

    def corrupt_ledger():
        bad = tmp / "p3" / "bad.jsonl"
        bad.write_text('{"round":1,"kind":"tool-output"}\nnot json at all\n\n', encoding="utf-8")
        cb = Client(binpath, bad, tmp / "p3" / "mem")
        r = cb.tool("ledger_summary")
        expect("result" in r, "坏账本导致报错：%s" % r)
        cb.close()
        return "tolerant"

    gate.check("坏账本容错", corrupt_ledger)

    def readonly_ledger():
        ro = tmp / "p3" / "ro"
        ro.mkdir(exist_ok=True)
        os.chmod(ro, 0o500)
        try:
            cr = Client(binpath, ro / "ledger.jsonl", tmp / "p3" / "mem")
            r = cr.tool("compress_context", {"query": "item_42", "text": code})
            expect("result" in r, "只读目录导致调用失败：%s" % r)
            cr.close()
        finally:
            os.chmod(ro, 0o700)
        return "silent degrade"

    gate.check("账本不可写时静默降级", readonly_ledger)

    # --------------------------------------------------------------- 向量
    gate.section("4. 向量后端")

    c4 = Client(binpath, tmp / "p4" / "ledger.jsonl", tmp / "p4" / "mem")
    (tmp / "p4").mkdir(parents=True, exist_ok=True)

    def vec_builtin():
        v = payload(c4.tool("vec", {"texts": ["Rust 内存安全", "今天天气很好"]}))
        expect(v["backend"] == "builtin", "默认后端不是 builtin：%s" % v["backend"])
        rows = v["vectors"]
        expect(len(rows) == 2, "向量行数 %d" % len(rows))
        expect(len(rows[0]) == 256, "维度 %d != 256" % len(rows[0]))
        norm = sum(x * x for x in rows[0]) ** 0.5
        expect(abs(norm - 1.0) < 1e-3, "未归一化：norm=%.4f" % norm)
        expect(rows[0] != rows[1], "不同文本得到相同向量")
        return "dim=256 norm=%.3f" % norm

    gate.check("vec 零配置内置编码", vec_builtin)

    def vec_deterministic():
        a = payload(c4.tool("vec", {"texts": ["同一条文本"]}))["vectors"][0]
        b = payload(c4.tool("vec", {"texts": ["同一条文本"]}))["vectors"][0]
        expect(a == b, "同文本两次编码不一致")
        return "stable"

    gate.check("vec 确定性", vec_deterministic)

    def vec_sim_rank():
        s = payload(c4.tool("vec_sim", {
            "query": "Rust 内存安全",
            "texts": ["Rust 内存安全", "今天天气很好", "C++ 手动管理内存"],
        }))["sim"]
        expect(abs(s[0] - 1.0) < 1e-2, "自身相似度 %.3f" % s[0])
        expect(s[0] > s[1], "无关文本得分过高：%.3f vs %.3f" % (s[0], s[1]))
        expect(s[0] >= s[2], "相关文本排名错误：%.3f vs %.3f" % (s[0], s[2]))
        return "%.2f/%.2f/%.2f" % tuple(s)

    gate.check("vec_sim 余弦与排序", vec_sim_rank)

    def vec_empty():
        r = c4.tool("vec", {"texts": []})
        expect(r.get("error", {}).get("code") == -32602, "空数组未报参数错误：%s" % r)
        return "-32602"

    gate.check("vec 空数组报参数错误", vec_empty)

    def vec_provider_cmd():
        stub = str(ROOT / "tests" / "vec_stub.py")
        logf = tmp / "p4" / "stub.log"
        v = payload(c4.tool("vec", {"texts": ["a", "bb"], "provider": "python3 %s" % stub}))
        expect(v["backend"] == "cmd", "显式 provider 未切换后端：%s" % v["backend"])
        expect(len(v["vectors"][0]) == 4, "provider 维度未透传")
        return "dim=4"

    gate.check("vec 外部 provider 协议", vec_provider_cmd)

    def vec_cache():
        stub = str(ROOT / "tests" / "vec_stub.py")
        cache = tmp / "p4" / "vec-cache"
        logf = tmp / "p4" / "cache.log"
        logf.write_text("", encoding="utf-8")
        env_cmd = "python3 %s" % stub
        args = {"query": "q1", "texts": ["t1", "t2", "t3"], "cache": str(cache), "provider": env_cmd}
        c4e = Client(binpath, tmp / "p4" / "ledger.jsonl", tmp / "p4" / "mem",
                     extra={"STUB_LOG": str(logf)})
        payload(c4e.tool("vec_sim", args))
        first = len(logf.read_text(encoding="utf-8").splitlines())
        payload(c4e.tool("vec_sim", args))
        second = len(logf.read_text(encoding="utf-8").splitlines())
        c4e.close()
        expect(first == 4, "首次编码输入行数 %d != 4 (Q+3)" % first)
        expect(second - first == 1, "缓存未命中候选：第二次多编码 %d 行" % (second - first))
        expect(len(list(cache.glob("*.vec")) + list(cache.glob("*.tmp"))) >= 3, "缓存文件不足 3")
        return "Q+3 → Q"

    gate.check("vec_sim 缓存命中", vec_cache)

    def vec_bad_provider():
        r = c4.tool("vec", {"texts": ["a"], "provider": "false"})
        expect("error" in r, "失败 provider 未报错：%s" % r)
        r2 = c4.tool("ledger_summary")
        expect("result" in r2, "provider 失败后进程不可用")
        return "clean error"

    gate.check("provider 失败不崩", vec_bad_provider)

    # --------------------------------------------------------- 压力与确定性
    gate.section("5. 压力、并发、确定性")

    big = code_lines(2000)
    c5 = Client(binpath, tmp / "p5" / "ledger.jsonl", tmp / "p5" / "mem")
    (tmp / "p5").mkdir(parents=True, exist_ok=True)

    def long_text():
        t0 = time.time()
        p = payload(c5.tool("compress_context", {"query": "item_1999 的 cap 计算", "text": big}))
        dt = time.time() - t0
        expect(p["total"] > 1500, "分段数异常：%s" % p["total"])
        expect(p["kept"] < p["total"], "长文未压缩")
        expect(dt < 3.0, "2000 行耗时 %.2fs" % dt)
        return "%d 行 %.0fms -%.1f%%" % (p["total"], dt * 1000, p["token_cut_pct"])

    gate.check("100KB 长文压缩", long_text)

    def pathological():
        # 24 万行单字符：极端形态必须 fail-open 返回原文，且不许虚报节省。
        bad = "\n".join("x" for _ in range(200000))
        p = payload(c5.tool("compress_context", {"query": "q", "text": bad}))
        expect(p["mode"].startswith(("failopen", "passthrough")), "极端输入未 fail-open：%s" % p["mode"])
        expect(p["tokens_out"] <= p["tokens_in"] + 1e-6,
               "fail-open 虚报节省：in=%.2f out=%.2f" % (p["tokens_in"], p["tokens_out"]))
        expect(len(p["text"]) == len(bad), "fail-open 篡改了原文")
        return p["mode"]

    gate.check("极端输入 fail-open 且不虚报", pathological)

    def ten_calls():
        base = json.loads(text_of(c5.tool("usage_summary")))["compressed"]
        base_lines = sum(1 for _ in (tmp / "p5" / "ledger.jsonl").open("rb"))
        for i in range(10):
            c5.tool("compress_context", {"query": "item_%d" % i, "text": code})
        u = json.loads(text_of(c5.tool("usage_summary")))
        expect(u["compressed"] == base + 10, "compressed=%s != %s" % (u["compressed"], base + 10))
        lines = sum(1 for _ in (tmp / "p5" / "ledger.jsonl").open("rb"))
        expect(lines == base_lines + 10, "账本行 %d != %d" % (lines, base_lines + 10))
        return "%d calls" % lines

    gate.check("同进程 10 连压累积", ten_calls)

    def concurrent_append():
        cl = tmp / "p5" / "conc.jsonl"
        if cl.exists():
            cl.unlink()
        ca = Client(binpath, cl, tmp / "p5" / "mem")
        cb = Client(binpath, cl, tmp / "p5" / "mem")
        for _ in range(3):
            ca.tool("compress_context", {"query": "item_1", "text": code})
            cb.tool("compress_context", {"query": "item_2", "text": code})
        ca.close()
        cb.close()
        cc = Client(binpath, cl, tmp / "p5" / "mem")
        l = payload(cc.tool("ledger_summary"))
        cc.close()
        expect(l["total"]["calls"] == 6, "并发后 calls=%s != 6" % l["total"]["calls"])
        lines = sum(1 for _ in cl.open("rb"))
        expect(lines == 6, "并发账本行 %d != 6" % lines)
        return "6 rows"

    gate.check("两进程并发追加账本", concurrent_append)

    def deterministic():
        a = payload(c5.tool("compress_context", {"query": "item_42 的 cap", "text": code}))["text"]
        b = payload(c5.tool("compress_context", {"query": "item_42 的 cap", "text": code}))["text"]
        expect(a == b, "同输入两次输出不一致")
        return "byte-identical"

    gate.check("输出确定性", deterministic)

    # ---------------------------------------------------------------- 收尾
    codes = []
    for cl in (c, c2, c3, c4, c5):
        codes.append(cl.close())
    gate.section("6. 进程退出")
    gate.check("所有进程正常退出且 stderr 干净", lambda: _clean(codes))

    print("\n%d checks, %d failed" % (gate.n, len(gate.fail)))
    if gate.fail:
        for name, err in gate.fail:
            print("  - %s: %s" % (name, err))
        return 1
    print("ALL PASS")
    return 0


def _clean(codes):
    for code, err in codes:
        expect(code == 0, "进程退出码 %s" % code)
        expect(err.strip() == "", "stderr 非空：%s" % err[:200])
    return "%d procs" % len(codes)


if __name__ == "__main__":
    sys.exit(main())
