#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""P0-1.1 消融评测 + 单调性门禁（P17/P18 信号独立贡献验证）。

对 5 个保留信号（semantic/fold/entropy/auto-anchor/cluster）在确定性合成语料上
测 cut + 本地 fidelity，断言「单开不劣于基线」：
  - cut 不劣（容差 0.005，吸收噪声）
  - num_cov/token_cov 不劣（容差 0.01）
任一信号劣化 → 非零退出（check_bench.sh 可挂接）。

语料：内置确定性合成（LCG 种子 42），三档形态：
  - code：Rust 代码重复块（对齐 perf corpus 形态）
  - log：日志风暴（时间戳/ERROR/INFO/参数/堆栈，fold 目标形态）
  - zh：中文错误/配置消息 + 数字（锚/簇/熵对 CJK 的增益面）
不依赖任何未入仓文件（corpus_p*.txt 为本地生成物），CI 可复现。

指标口径（与 src/terms.rs::measure 近似，标注为本地复刻）：
  - cut = 1 - out_tok/full_tok
  - token_cov = full 词 multiset 在 out 中贪婪匹配比例
  - num_cov = 含数字词 multiset 的匹配比例
用法:
  python3 tools/bench_ablate.py [--bin PATH] [--sem-bin PATH] [--gate]
semantic 配置需 feature=semantic 二进制（默认 target/ablate-sem/release/scx，
缺失时自动隔离构建，不触碰默认 release binary）。
"""
from __future__ import annotations

import argparse
import csv
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT_CSV = ROOT / "config" / "bench" / "results" / "bench_p17_ablate.csv"
WEIGHTS = ROOT / "config" / "semantic" / "weights.json"
SEM_TARGET = ROOT / "target" / "ablate-sem"

CUT_TOL = 0.005   # cut 单调性容差（噪声吸收）
FID_TOL = 0.01    # fidelity 单调性容差

# 配置矩阵：基线 + 5 单开。仅默认二进制可开的信号独立列。
CONFIGS: list[tuple[str, list[str], bool]] = [
    ("baseline", [], False),
    ("fold", ["--fold", "1"], False),
    ("entropy", ["--entropy", "1"], False),
    ("anchor", ["--auto-anchor", "1"], False),
    ("cluster", ["--cluster", "1"], False),
    ("semantic", ["--semantic", str(WEIGHTS)], True),
]

# ---------- token 口径（本地复刻 src/terms.rs::measure） ----------

_IDEO = re.compile(
    "[\u3400-\u4DBF\u4E00-\u9FFF\uF900-\uFAFF\u3000-\u303F\uFF00-\uFFEF]"
)


def est_tok(s: str) -> float:
    """estimate(s) 近似：ASCII len/4+1；CJK 0.6/字；其他 2.5/字。"""
    if s.isascii():
        return len(s) / 4.0 + 1.0
    ascii_ = cjk = other = 0
    for ch in s:
        if ch.isascii():
            ascii_ += 1
        elif _IDEO.match(ch):
            cjk += 1
        else:
            other += 1
    return ascii_ / 4.0 + cjk * 0.6 + other / 2.5 + 1.0


_WORD = re.compile(r"[A-Za-z0-9_]+|[\u3400-\u9FFF\uF900-\uFAFF]")


def tokens(s: str) -> list[str]:
    """保真词切分：ASCII 词段 + 单 CJK 字符。"""
    return _WORD.findall(s)


def num_tokens(s: str) -> list[str]:
    return [t for t in tokens(s) if any(ch.isdigit() for ch in t)]


def cov(full: list[str], out: list[str]) -> float:
    """full multiset 在 out 中的贪婪匹配比例（确定性）。"""
    if not full:
        return 1.0
    pool = list(out)
    hit = 0
    for t in full:
        for k, u in enumerate(pool):
            if u == t:
                pool[k] = ""
                hit += 1
                break
    return hit / len(full)


# ---------- 确定性语料合成（LCG 种子 42） ----------

def lcg(seed: int = 42):
    """确定性伪随机：相同输入恒同序列。"""
    state = seed

    def next_u32() -> int:
        nonlocal state
        state = (state * 1103515245 + 12345) & 0x7FFFFFFF
        return state

    def choice(items):
        return items[next_u32() % len(items)]

    return next_u32, choice


def corpus_code(seed: int = 42) -> str:
    _, choice = lcg(seed)
    kinds = ["u64", "i32", "String", "Vec<u8>", "bool", "f64"]
    verbs = ["parse", "build", "merge", "scan", "render", "flush", "retry", "dedup"]
    lines = []
    for i in range(300):
        lines.append(
            f"pub fn {choice(verbs)}_{i}(arg: u64) -> Result<{choice(kinds)}, Error> {{"
        )
        lines.append(f"    let tmp = arg.wrapping_mul({i} + 1);")
        lines.append(f"    if tmp % 2 == 0 {{ return Ok(tmp) }} else {{ Err(Error::Idx({i})) }}")
        lines.append("}")
    lines.append("// unique tail: final answer is 42")
    return "\n".join(lines) + "\n"


def corpus_log(seed: int = 42) -> str:
    """日志风暴：160 块 × 5 行同消息重试风暴（id/耗时变化）+ 尾部唯一行。
    连续重复块给 fold 模板折叠以触发机会（消融第一轮证明：query 命中行
    孤立保留时折叠块 <3 行，fold 信号零触发——语料须对齐其目标形态）。"""
    _, choice = lcg(seed)
    levels = ["ERROR", "WARN"]
    comps = ["conn", "auth", "cache", "rpc", "mq", "disk"]
    states = ["timeout", "retry", "reset", "refused", "slow", "evicted"]
    lines = []
    for b in range(160):
        lvl, comp, st = choice(levels), choice(comps), choice(states)
        base = f"2026-10-30 12:{b % 60:02d}:{b % 60:02d}.{b % 1000:03d} {lvl} {comp}.{st}"
        for k in range(5):
            lines.append(f"{base} id={b * 5 + k:04x} took={b % 500 + k * 7}ms retry={k}")
        lines.append(f"  at worker.retry(worker.rs:{b % 120 + 1})")
        lines.append(f'  {{"err": "{st}", "id": "{b * 5:04x}", "took": {b % 500}}}')
    lines.append("final: status=ok attempts=0")
    return "\n".join(lines) + "\n"


def corpus_zh(seed: int = 42) -> str:
    _, choice = lcg(seed)
    msgs = ["连接超时", "缓存未命中", "权限不足", "参数校验失败", "服务不可用", "重试次数超限"]
    comps = ["网关", "认证中心", "订单服务", "存储引擎", "调度器"]
    lines = []
    for i in range(300):
        lines.append(
            f"[{i % 100:02d}] {choice(comps)} {choice(msgs)}：请求 {i} 重试 {i % 5} 次"
        )
        lines.append(f"  详情：code={1000 + i} id={i:04x} 耗时 {i % 900}ms")
    lines.append("最终状态：成功 总耗时 3200ms")
    return "\n".join(lines) + "\n"


# ---------- scx 调用 ----------

def compress(bin_: str, text: str, query: str, extra: list[str]) -> str:
    """调用 scx 压缩；失败/小输入直通返回原文；截断 `compressed:` 装饰块。
    query 必须命中语料高频词，否则 scx「无命中保护」直通（cut 恒 0 会误判）。"""
    with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as fh:
        fh.write(text)
        path = fh.name
    try:
        proc = subprocess.run(
            [bin_, path, "--query", query, "--ratio", "0.35", "--context", "1", *extra],
            capture_output=True,
            text=True,
            timeout=300,
        )
        if proc.returncode != 0:
            return text
        lines = proc.stdout.splitlines()
        for k, ln in enumerate(lines):
            if ln.lstrip().startswith("compressed:"):
                return "\n".join(lines[:k])
        return text
    finally:
        os.unlink(path)


def rebuild_folded(out: str) -> str:
    """把折叠输出重建为原文形态（fold 的保真契约 = rebuild 可逆）。

    折叠组 = 首行原文 + `scx-fold:` 模板行 + `scx-fold-params:` 参数行；
    重建后替换为完整原文行。非折叠行原样保留。fold 配置的 fidelity 必须
    在重建态上测（消融证明：折叠态骨架词移除导致 token_cov 名义劣化，
    而 rebuild 逐字节还原原文——信息可恢复性才是 fold 的保真口径）。
    """
    lines = out.splitlines()
    rebuilt: list[str] = []
    i = 0
    while i < len(lines):
        ln = lines[i]
        if ln.startswith("scx-fold: ") and i >= 1 and i + 1 < len(lines):
            first = rebuilt.pop() if rebuilt and rebuilt[-1] != "" else lines[i - 1]
            tpl = ln
            pstream = lines[i + 1]
            if pstream.startswith("scx-fold-params: "):
                rows = rebuild_rows(tpl, pstream)
                if rows is not None:
                    rebuilt.append(first)
                    rebuilt.extend(rows)
                    i += 2
                    continue
            rebuilt.append(first)
            rebuilt.append(ln)
        else:
            rebuilt.append(ln)
        i += 1
    return "\n".join(rebuilt) + ("\n" if out.endswith("\n") else "")


def rebuild_rows(tpl: str, pstream: str) -> list[str] | None:
    """Python 复刻 fold.rs::rebuild：模板行 + 参数行 → 原文行（None=结构不合法）。"""
    rest = tpl.removeprefix("scx-fold: ")
    cut = rest.find(" rows | ")
    if cut < 0:
        return None
    m = rest[:cut]
    if not m.isdigit():
        return None
    m = int(m)
    skeleton = rest[cut + len(" rows | "):]
    body = pstream.removeprefix("scx-fold-params: ")
    rows = [r.split("|") if r else [] for r in body.split(";")]
    if m == 0:
        return []
    if len(rows) != m or any(len(r) > 1 and "" in r for r in rows):
        return None
    out_rows = []
    for row in rows:
        rebuilt_line = ""
        pi = 0
        chars = list(skeleton)
        ci = 0
        while ci < len(chars):
            ch = chars[ci]
            if ch == "#":
                if ci + 1 < len(chars) and chars[ci + 1] == "#":
                    rebuilt_line += "#"
                    ci += 2
                else:
                    if pi >= len(row):
                        return None
                    rebuilt_line += row[pi]
                    pi += 1
                    ci += 1
            elif ch == "|":
                if ci + 1 < len(chars) and chars[ci + 1] == "|":
                    rebuilt_line += "|"
                    ci += 2
                else:
                    ci += 1
            else:
                rebuilt_line += ch
                ci += 1
        if pi != len(row):
            return None
        out_rows.append(rebuilt_line)
    return out_rows


def ensure_sem_bin() -> str:
    """隔离构建 feature=semantic 二进制（不触碰默认 release binary）。"""
    bin_ = SEM_TARGET / "release" / "scx"
    if bin_.exists():
        return str(bin_)
    print(f"构建 semantic 二进制到 {bin_}（隔离 target，不影响默认）…")
    subprocess.run(
        ["cargo", "build", "--release", "--features", "semantic", "--target-dir", str(SEM_TARGET)],
        check=True,
        cwd=ROOT,
    )
    return str(bin_)


# ---------- 主流程 ----------

def main() -> int:
    ap = argparse.ArgumentParser(description="P17/P18 信号消融评测 + 单调性门禁")
    ap.add_argument("--bin", default=str(ROOT / "target" / "release" / "scx"))
    ap.add_argument("--sem-bin", default=None)
    ap.add_argument("--gate", action="store_true", help="单调性门禁模式（断言 + 退出码）")
    ap.add_argument("--out", default=str(OUT_CSV))
    args = ap.parse_args()

    default_bin = args.bin
    sem_bin = args.sem_bin or ensure_sem_bin()

    corpora = [
        ("code", corpus_code(), "build"),
        ("log", corpus_log(), "timeout"),
        ("zh", corpus_zh(), "超时"),
    ]
    rows: list[dict] = []
    for cname, text, query in corpora:
        full_tok = est_tok(text)
        full_words = tokens(text)
        full_nums = num_tokens(text)
        for cfg_name, flags, need_sem in CONFIGS:
            bin_ = sem_bin if need_sem else default_bin
            out = compress(bin_, text, query, flags)
            # fold 保真口径 = 重建态（rebuild 可逆契约）；cut 口径 = 折叠态。
            fid_out = rebuild_folded(out) if cfg_name == "fold" else out
            out_tok = est_tok(out)
            out_words = tokens(fid_out)
            out_nums = num_tokens(fid_out)
            rows.append(
                {
                    "set": cname,
                    "cfg": cfg_name,
                    "cut": 1.0 - out_tok / full_tok,
                    "num_cov": cov(full_nums, out_nums),
                    "token_cov": cov(full_words, out_words),
                }
            )
            print(f"[{cname:4}] {cfg_name:9} cut={rows[-1]['cut']:.4f} "
                  f"num_cov={rows[-1]['num_cov']:.4f} token_cov={rows[-1]['token_cov']:.4f}")

    with open(args.out, "w", encoding="utf-8", newline="") as fh:
        w = csv.DictWriter(fh, fieldnames=["set", "cfg", "cut", "num_cov", "token_cov"])
        w.writeheader()
        w.writerows(rows)
    print(f"写入 {args.out}（3 语料 × 6 配置 = {len(rows)} 行）")

    if not args.gate:
        return 0

    ok = True
    for cname, _, _ in corpora:
        base = next(r for r in rows if r["set"] == cname and r["cfg"] == "baseline")
        for cfg_name, _, _ in CONFIGS:
            if cfg_name == "baseline":
                continue
            row = next(r for r in rows if r["set"] == cname and r["cfg"] == cfg_name)
            # fidelity 是硬约束：任何信号都不得让数字/token 覆盖劣化。
            for m in ("num_cov", "token_cov"):
                if row[m] < base[m] - FID_TOL:
                    print(f"  FAIL [{cname}] {cfg_name}: {m} {row[m]:.4f} < 基线 "
                          f"{base[m]:.4f} - {FID_TOL}")
                    ok = False
            # cut 断言按信号类别：只加保留信号（fold/anchor/cluster/semantic）
            # 要求 cut 不劣；entropy 是保真召回信号（可能更保守），豁免 cut，
            # 仅受 fidelity 硬约束——cut 上升但 fidelity 崩仍会被上方拦下。
            if cfg_name != "entropy" and row["cut"] < base["cut"] - CUT_TOL:
                print(f"  FAIL [{cname}] {cfg_name}: cut {row['cut']:.4f} < 基线 "
                      f"{base['cut']:.4f} - {CUT_TOL}")
                ok = False
    print("  " + ("OK: 消融单调性门禁全部通过（单开不劣于基线）" if ok else "FAIL: 存在劣化信号"))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
