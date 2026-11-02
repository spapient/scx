#!/usr/bin/env python3
"""tools/ 统一运维入口：run/install/setup/doctor/check/perf 六合一，配置表驱动。

等价替代原 tools/scx（bash）与 tools/scx.cmd（Windows）：零第三方依赖，仅标准库 +
cargo + 评测 Python（SCX_EVAL_PY，缺省 /tmp/scx-eval/bin/python，缺则回退解释器）。
路径/命令/门禁全部由 TOOLCHAIN 常量声明，禁硬编码。
"""
import csv
import glob
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import time

# ---- 配置表（SCX_TARGET_DIR / SCX_BIN_DIR / SCX_EVAL_PY 可覆盖） ----
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TD = os.environ.get("SCX_TARGET_DIR", os.path.join(tempfile.gettempdir(), "scx-cargo-target"))
BIN = os.path.join(TD, "release", "scx")
BIN_DIR = os.environ.get("SCX_BIN_DIR", os.path.expanduser("~/.local/bin"))
PY = os.environ.get("SCX_EVAL_PY", "/tmp/scx-eval/bin/python")
if not os.path.isfile(PY) or not os.access(PY, os.X_OK):
    PY = sys.executable
MANIFEST, LOCK = os.path.join(ROOT, "Cargo.toml"), os.path.join(ROOT, "Cargo.lock")
RESULTS = os.path.join(ROOT, "config", "bench", "results")
EVAL_MANIFEST = os.path.join(ROOT, "config", "bench", "eval-manifest.json")
SCX_PY = os.path.join(ROOT, "tools", "scx.py")
VERSION = "0.11.0"


def xtask_binary():
    """运维命令已由 Rust xtask 承接；未构建时回退 Python。"""
    for profile in ("release", "debug"):
        for name in ("xtask.exe", "xtask"):
            path = os.path.join(ROOT, "target", profile, name)
            if os.path.isfile(path):
                return path
    return None


def proc(cmd, **kw):
    return subprocess.run(cmd, text=True, **kw)


def require_scx():
    """自举构建：二进制缺失或 src/Cargo.toml/Cargo.lock 更新即重建（产物只落 target-dir）。"""
    stale = not os.path.isfile(BIN) or any(
        os.path.getmtime(p) > os.path.getmtime(BIN)
        for p in [MANIFEST, LOCK] + glob.glob(os.path.join(ROOT, "src", "**", "*.rs"), recursive=True))
    if stale:
        print("[scx] release build (--locked) ...", file=sys.stderr)
        p = proc(["cargo", "build", "--release", "--quiet", "--locked",
                  "--manifest-path", MANIFEST, "--target-dir", TD])
        if p.returncode:
            raise SystemExit(p.returncode)


def cmd_run(args):
    require_scx()
    os.execv(BIN, [BIN] + args)


def cmd_install(_args):
    require_scx()
    os.makedirs(BIN_DIR, exist_ok=True)
    link = os.path.join(BIN_DIR, "scx")
    if os.path.lexists(link):
        os.unlink(link)
    os.symlink(BIN, link)
    print(proc([BIN, "--version"], capture_output=True).stdout.strip())
    print(f"installed: {link}\n提示：若 {BIN_DIR} 不在 PATH，可追加 export PATH=\"{BIN_DIR}:$PATH\"")


def cmd_setup(args):
    require_scx()
    os.execv(BIN, [BIN, "setup"] + args)


def cmd_doctor(_args):
    require_scx()
    for msg, want in (("initialize", '"serverInfo"'), ("tools/list", "compress_context")):
        req = json.dumps({"jsonrpc": "2.0", "id": 1, "method": msg, "params": {}})
        out = proc([BIN, "mcp"], input=req, capture_output=True).stdout
        if want not in out:
            print(f"fail: {msg} 握手失败", file=sys.stderr)
            raise SystemExit(1)
    print("ok: scx mcp ready (stdio)")


def check_manifest():
    """eval-manifest 冻结校验：frozen sha256+rows 全匹配、present 行数达标。"""
    m = json.load(open(EVAL_MANIFEST, encoding="utf-8"))
    ok = True
    for name, want in m.get("frozen", {}).items():
        p = os.path.join(RESULTS, name)
        if not os.path.exists(p):
            print(f"  frozen {name}: MISSING")
            ok = False
            continue
        data = open(p, "rb").read()
        got = {"sha256": hashlib.sha256(data).hexdigest(),
               "rows": data.count(b"\n") + (0 if data.endswith(b"\n") else 1)}
        for key, val in got.items():
            if str(want.get(key)) != str(val):
                print(f"  frozen {name}: {key} want={want.get(key)} got={val}")
                ok = False
    for item in m.get("present", []):
        p = os.path.join(RESULTS, item["file"])
        if not os.path.exists(p):
            print(f"  present {item['file']}: MISSING")
            ok = False
        elif sum(1 for _ in open(p, encoding="utf-8")) < item.get("min_rows", 2):
            print(f"  present {item['file']}: rows 低于 min_rows")
            ok = False
    print("  " + ("OK: frozen + present 全部匹配" if ok else "FAIL"))
    return ok


def cmd_check(_args):
    """评测闭环门禁（P7-E）：cargo test×3 + clippy/fmt + manifest 冻结 + A2 + 冒烟 + 审计。"""
    fail = []
    step = lambda s: print(f"\n== {s} ==")
    must = lambda label, cond: fail.append(label) if not cond else None
    def cargo_tests(label, cmd):
        """跑 cargo test：returncode==0 且尾部含 0 failed 为过；失败时打印完整现场。"""
        p = proc(cmd, capture_output=True)
        ok = p.returncode == 0 and "0 failed" in p.stdout[-400:]
        if not ok:
            print(f"  rc={p.returncode}")
            print(f"  stderr tail:\n{p.stderr[-1500:]}")
            print(f"  stdout tail:\n{p.stdout[-1500:]}")
        return ok
    for label, cmd in (("lib tests", ["cargo", "test", "--locked", "--lib"]),
                       ("semantic tests", ["cargo", "test", "--locked", "--features", "semantic", "--lib"]),
                       ("golden tests", ["cargo", "test", "--locked", "--test", "golden"])):
        step(f"cargo test {'/'.join(cmd[4:])}" if label == "lib tests" else "cargo test --locked --test golden")
        must(label, cargo_tests(label, cmd))
    step("clippy --all-targets")
    must("clippy", not any(l.startswith(("warning:", "error"))
                           for l in proc(["cargo", "clippy", "--locked", "--all-targets"],
                                         capture_output=True).stderr.splitlines()))
    step("fmt --check")
    must("fmt not clean", proc(["cargo", "fmt", "--check"]).returncode == 0)
    step("eval-manifest 冻结校验")
    must("manifest integrity", check_manifest())
    step("A2 保真断言（fidelity_thresholds 分档）")
    must("A2 fidelity", proc([PY, SCX_PY, "check", "a2"]).returncode == 0)
    step("新开关冒烟（budget/fold/auto-anchor/cluster：rc + stats + 双跑确定性）")
    require_scx()
    tmp = tempfile.mkdtemp()
    src = os.path.join(tmp, "in.txt")
    open(src, "w", encoding="utf-8").write(
        "task start\n  log: build err=42 retry timeout\n  log: build ok step 2\n"
        "  log: flush cache err=7\n  a=1 b=2\n  summary: done\n")
    must("smoke version", VERSION in proc([BIN, "--version"], capture_output=True).stdout)
    for flag in ["--budget 0.5", "--fold 1", "--auto-anchor 1", "--cluster 1", "--budget 0.35 --cluster 1"]:
        o1 = proc([BIN, src, "--query", "build timeout", *flag.split(), "--json"], capture_output=True)
        o2 = proc([BIN, src, "--query", "build timeout", *flag.split(), "--json"], capture_output=True)
        must(f"smoke {flag}",
             o1.returncode == 0 and "compressed: kept=" in o1.stdout and o1.stdout == o2.stdout)
    step("unwrap/panic 审计（生产裸 unwrap=0，消息非空）")
    must("audit", proc([PY, SCX_PY, "audit"]).returncode == 0)
    if fail:
        for f in fail:
            print(f"FAIL: {f}")
        print("\nscx check: 存在失败项")
        raise SystemExit(1)
    print("\nscx check: 全部通过 ✓")


def cmd_perf(_args):
    """压缩性能基线（P7-C）：src+docs+评测集语料，lossless/r0.3/r0.2 三档 ×3 次最优。"""
    require_scx()
    buf = []
    for f in sorted(glob.glob(os.path.join(ROOT, "src", "**", "*.rs"), recursive=True)):
        buf.append(open(f, encoding="utf-8").read())
    for p in (os.path.join(ROOT, "docs", "plan.md"), os.path.join(ROOT, "README.md")):
        if os.path.exists(p):
            buf.append(open(p, encoding="utf-8").read())
    for f in sorted(glob.glob(os.path.join(ROOT, "config", "bench", "datasets", "*.csv"))):
        for row in csv.reader(open(f, encoding="utf-8")):
            if len(row) >= 2 and row[0].strip():
                buf.append(row[0])
    tmp = tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False)
    tmp.write("\n".join(buf))
    tmp.close()
    chars = os.path.getsize(tmp.name)
    print("== scx perf baseline ==")
    print(f"input: {chars} chars / {sum(1 for _ in open(tmp.name, encoding='utf-8'))} lines")

    def one(ratio, label):
        best = None
        for _ in range(3):
            t0 = time.perf_counter()
            out = proc([BIN, "--query", "fn compress", "--ratio", str(ratio), "--trim", "1",
                        "--trim-ratio", str(ratio), "--trim-op-keep", "1", "--num-line-keep", "1",
                        tmp.name], capture_output=True)
            if out.returncode == 0 and (best is None or time.perf_counter() - t0 < best[0]):
                best = (time.perf_counter() - t0, len(out.stdout))
        print(f"{label:<12} best={best[0]:.2f}s  cut={(1 - best[1] / chars) * 100:.1f}%" if best
              else f"{label:<12} best=n/a")

    one(1.0, "lossless")
    one(0.3, "r0.3")
    one(0.2, "r0.2")
    os.unlink(tmp.name)


CMDS = {
    "run": (cmd_run, "自举构建后转发参数（默认；非子命令首参即 CLI 参数）"),
    "install": (cmd_install, "release 构建 + 软链到 ~/.local/bin/scx"),
    "setup": (cmd_setup, "注册 MCP 配置（Claude/Codex/Cursor，幂等+备份+自检）"),
    "doctor": (cmd_doctor, "scx mcp stdio 握手自检"),
    "check": (cmd_check, "评测闭环门禁（cargo test/clippy/fmt + manifest 冻结 + A2 + 冒烟 + 审计）"),
    "perf": (cmd_perf, "压缩性能基线（lossless/r0.3/r0.2 三档 ×3 次最优）"),
}


def usage(err=None):
    if err:
        print(f"scx: {err}", file=sys.stderr)
    print("usage: tools/ops.py [run] [ARGS...] | install | setup | doctor | check | perf", file=sys.stderr)
    for name, (_, help) in CMDS.items():
        print(f"  {name:<8} {help}", file=sys.stderr)
    raise SystemExit(2)


def main():
    argv = sys.argv[1:]
    xtask = xtask_binary()
    if xtask:
        os.execv(xtask, [xtask, "ops", *argv])
    name = argv[0] if argv else "run"
    if name in ("-h", "--help"):
        usage()
    if name in CMDS:
        return CMDS[name][0](argv[1:])
    cmd_run(argv)  # 非子命令/参数（--version/--query/路径等）一律转发给 scx 自身


if __name__ == "__main__":
    main()
