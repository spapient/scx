#!/usr/bin/env python3
"""Deterministic generator for config/fidelity.corpus.json (P1 of Gauge roadmap).

Synthesises 25 slices (5 kinds x 5). Per kind: 4 large slices (>45 lines, so
scx takes the sieve path) + 1 small slice (<10 lines, so scx passes through) —
this makes the eval exercise BOTH compression paths. Every evidence line is
verbatim inside full_text and the query's substantive tokens are aligned to the
evidence lines, so the answerability proxy is meaningful. No randomness beyond
the fixed seed; rerunning reproduces the same corpus byte-for-byte.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

SEED = 20261026
ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "config" / "fidelity.corpus.json"

KINDS = ("build", "stack", "diff", "log", "test")

BUILD_CODES = [
    ("E0433", "use of undeclared crate or module `config`",
     "let cfg = config::load(\"/etc/app.toml\");", "crates/core/src/main.rs:17:5", "config"),
    ("E0308", "mismatched types: expected `u64`, found `&str`",
     "let ttl: u64 = env::var(\"CACHE_TTL\")?;", "crates/util/src/cache.rs:41:16", "ttl"),
    ("E0502", "cannot borrow `*buf` as mutable because it is also borrowed as immutable",
     "let head = &buf[..8]; buf.extend_from_slice(&head);", "crates/io/src/reader.rs:63:9", "buf"),
    ("E0382", "borrow of moved value: `conn`",
     "let pool = conn.pool(); drop(conn); pool.query(\"SELECT 1\");", "crates/db/src/pool.rs:88:5", "conn"),
    ("E0599", "no method named `retry` found for struct `Client`",
     "client.retry(3).connect(addr);", "crates/net/src/client.rs:22:9", "retry"),
]

STACK_SPOTS = [
    ("db.rs:42:9", "index out of bounds: the len is 5 but the index is 9",
     "app::db::connect", "pool"),
    ("cache.rs:107:5", "attempt to divide by zero", "app::cache::compute_ttl", "ttl"),
    ("net/tls.rs:29:17", "called `Result::unwrap()` on an `Err` value: \"handshake timeout\"",
     "app::net::handshake", "tls"),
    ("store/fs.rs:55:3", "No such file or directory (os error 2)", "app::store::open", "path"),
]

LOG_CASES = [
    ("db.internal", 5432, "postgres", "connection refused", "starting http server on 0.0.0.0:8080"),
    ("redis.internal", 6379, "redis", "connection reset by peer", "starting worker pool with 8 workers"),
    ("mq.internal", 5672, "rabbitmq", "authentication failed", "starting consumer daemon"),
    ("nfs.internal", 2049, "storage", "operation not permitted", "starting replication agent"),
]

DIFF_HUNKS = [
    ("src/cache.rs", "ttl", "let ttl = 60;", "let ttl = ttl_from_env()?;",
     "negative ttl", "if ttl < 0 { bail!(\"negative ttl\"); }"),
    ("src/io/reader.rs", "buf_size", "let buf = Vec::with_capacity(1024);",
     "let buf = Vec::with_capacity(buf_size);", "capacity overflow",
     "if buf_size > MAX_BUF { bail!(\"capacity overflow\"); }"),
    ("src/net/client.rs", "timeout", "let to = Duration::from_secs(5);",
     "let to = timeout_from_env()?;", "invalid timeout",
     "if to.is_zero() { bail!(\"invalid timeout\"); }"),
    ("src/store/fs.rs", "perm", "let mode = 0o644;",
     "let mode = mode_from_env()?;", "mode too loose",
     "if mode & 0o002 != 0 { bail!(\"mode too loose\"); }"),
]

TEST_CASES = [
    ("cache::tests::ttl_negative", "assertion failed: cache.ttl >= 0", "left: -5"),
    ("io::tests::buf_resize", "assertion failed: buf.len() == expected", "left: 1024, right: 2048"),
    ("net::tests::retry_exhausted", "assertion failed: attempts <= max_retries", "left: 5, right: 3"),
    ("store::tests::perm_restore", "assertion failed: mode & 0o002 == 0", "left: 0o666"),
]


def _token_est(lines):
    """Rough token estimate (words + CJK codepoints), mirrors contain_token_count."""
    import re
    text = "\n".join(lines)
    return len(re.findall(r"[A-Za-z0-9][A-Za-z0-9_.\-]*", text)) + len(re.findall(r"[\u4e00-\u9fff]", text))


# --- large slices (sieve path) ---


def build_large(idx: int) -> dict:
    # 4 crates; two error blocks each (indices wrap to keep 5 codes in play)
    crates = ["core", "util", "io", "db"]
    blocks = []
    ev = []
    start = idx * 2
    for ci, crate in enumerate(crates):
        blocks.append(f"   Compiling app-{crate} v0.4.{ci} (/workspace/crates/{crate})")
        for k in (start + ci * 2, start + ci * 2 + 1):
            code, msg, line, loc, sym = BUILD_CODES[k % len(BUILD_CODES)]
            fname = loc.split(":")[0]
            blocks.append(f"error[{code}]: {msg}")
            blocks.append(f"  --> {loc}")
            blocks.append("   |")
            blocks.append(f"{fname:>8} |     {line}")
            blocks.append(f"   |     {'^' * max(2, len(line.split(';')[0]))}")
            if len(ev) < 4 and code not in [e.split("]")[0] for e in ev]:
                ev.append(f"error[{code}]: {msg}")
    blocks.append("error: could not compile `app` due to previous errors")
    code, msg, line, loc, sym = BUILD_CODES[idx % len(BUILD_CODES)]
    return {
        "kind": "build",
        "full_text": "\n".join(blocks) + "\n",
        "evidence": ev,
        "query": f"why did the build fail with {code} {msg.split('`')[1] if '`' in msg else sym}",
        "note": f"synthetic multi-crate rust build failure (large, sieve path) focus {code}",
    }


def stack_large(idx: int) -> dict:
    fname, msg, func, tag = STACK_SPOTS[idx % len(STACK_SPOTS)]
    pool = [
        func, "app::main", "core::ops::function::FnOnce::call_once",
        "alloc::boxed::Box<F,A>::call_box", "std::sys::pal::common::thread::start_thread",
        "std::sys::pal::unix::thread::Thread::new", "app::net::poll",
        "app::store::flush", "app::evt::dispatch", "app::cache::refresh",
        "app::metrics::record", "app::config::reload", "app::log::rotate",
        "app::db::query", "app::http::handle", "app::worker::run",
        "app::service::tick", "app::service::poll_once", "app::runtime::block_on",
        "tokio::runtime::scheduler::multi_thread::worker",
    ]
    frames = [f"   {n}: {fn} (src/{fname})" for n, fn in enumerate(
        pool[n % len(pool)] for n in range(43))]
    full = [
        f"thread 'main' panicked at src/{fname}:",
        msg,
        "stack backtrace:",
        *frames,
        "note: run with `RUST_BACKTRACE=1` for a full backtrace",
    ]
    first = msg.split(":")[0]
    return {
        "kind": "stack",
        "full_text": "\n".join(full) + "\n",
        "evidence": [f"thread 'main' panicked at src/{fname}:", msg],
        "query": f"why did the program panic: {first}",
        "note": f"synthetic long panic backtrace ({func}, {tag})",
    }


def log_large(idx: int) -> dict:
    host, port, svc, err, start = LOG_CASES[idx % len(LOG_CASES)]
    lines = [f"[2026-10-26T08:00:00Z] INFO  {start}",
             "[2026-10-26T08:00:01Z] DEBUG loading config from /etc/app/config.toml",
             "[2026-10-26T08:00:02Z] INFO  cache warmup complete (1284 entries)"]
    for i in range(21):
        t = 8 + i * 5
        lines.append(f"[2026-10-26T08:{t // 60:02d}:{t % 60:02d}Z] ERROR failed to connect to {svc}: {err} (host={host} port={port})")
        lines.append(f"[2026-10-26T08:{t // 60:02d}:{(t + 3) % 60:02d}Z] WARN  retrying in 5s...")
    lines.append(f"[2026-10-26T08:10:00Z] ERROR giving up after 21 retries (host={host} port={port})")
    lines.append("[2026-10-26T08:09:01Z] INFO  entering degraded mode, local cache only")
    ev = [f"[2026-10-26T08:00:08Z] ERROR failed to connect to {svc}: {err} (host={host} port={port})"]
    return {
        "kind": "log",
        "full_text": "\n".join(lines) + "\n",
        "evidence": ev,
        "query": f"why did the service fail to connect to {svc} at {host}",
        "note": f"synthetic long service log: {svc} unreachable, 12 retries then degraded",
    }


def diff_large(idx: int) -> dict:
    path, var, old, new, guard_msg, guard = DIFF_HUNKS[idx % len(DIFF_HUNKS)]
    fname = path.split("/")[-1].replace(".rs", "").title()
    blocks = [f"diff --git a/{path} b/{path}", f"--- a/{path}", f"+++ b/{path}"]
    ev = []
    for i in range(7):
        blocks.append(f"@@ -{40 + i * 4},6 +{40 + i * 4},8 @@ impl {fname} {{")
        blocks.append(f"-    {old}")
        blocks.append(f"+    {new}")
        if i % 2 == 0:
            blocks.append(f"+    {guard}")
            if len(ev) < 3:
                ev.append(f"+    {guard}")
        blocks.append("     // untouched context line follows")
        blocks.append("     let _ = next();")
        blocks.append(" ")
    blocks.append("--")
    blocks.append("2.43.0")
    if len(ev) < 2:
        ev.append(f"+    {new}")
    return {
        "kind": "diff",
        "full_text": "\n".join(blocks) + "\n",
        "evidence": ev,
        "query": f"what happens with {guard_msg}",
        "note": f"synthetic large review diff: env-driven {var} guard {guard_msg}",
    }


def test_large(idx: int) -> dict:
    rows = []
    ev = []
    fail_name, fail_msg, fail_detail = TEST_CASES[idx % len(TEST_CASES)]
    fail_id = f"mod_{idx}::tests::case_05"
    total = 36
    rows.append(f"running {total} tests")
    for i in range(total):
        name = f"mod_{idx}::tests::case_{i:02d}"
        status = "ok" if i != 5 else "FAILED"
        rows.append(f"test {name} ... {status}")
    rows.append("")
    rows.append("failures:")
    rows.append("")
    rows.append(f"---- {fail_id} stdout ----")
    rows.append(fail_msg)
    rows.append(f"  {fail_detail}")
    rows.append("")
    rows.append("")
    rows.append("failures:")
    rows.append(f"    {fail_id}")
    rows.append("")
    rows.append(f"test result: FAILED. 1 failed; {total - 1} passed")
    ev = [f"test {fail_id} ... FAILED", fail_msg]
    return {
        "kind": "test",
        "full_text": "\n".join(rows) + "\n",
        "evidence": ev,
        "query": f"which test failed with {fail_msg.split(':')[0]}",
        "note": f"synthetic large unit-test run, {fail_name} fails ({fail_detail})",
    }


# --- small slices (passthrough path) ---


def build_small(idx: int) -> dict:
    code, msg, line, loc, sym = BUILD_CODES[idx % len(BUILD_CODES)]
    fname = loc.split(":")[0]
    full = (
        "   Compiling app v0.3.1 (/workspace/app)\n"
        f"error[{code}]: {msg}\n"
        f"  --> {loc}\n"
        "   |\n"
        f"{fname:>6} |     {line}\n"
        f"   |     {'^' * max(1, len(line.split(';')[0]))}\n"
        "error: could not compile `app` due to previous error\n"
    )
    return {"kind": "build", "full_text": full,
            "evidence": [f"error[{code}]: {msg}"],
            "note": f"synthetic small rust build failure {code}",
            "query": f"why did the build fail with {code} {msg.split('`')[1] if '`' in msg else sym}"}


def stack_small(idx: int) -> dict:
    fname, msg, func, tag = STACK_SPOTS[idx % len(STACK_SPOTS)]
    full = (
        f"thread 'main' panicked at src/{fname}:\n"
        f"{msg}\n"
        "stack backtrace:\n"
        f"   0: {func} (src/{fname})\n"
        "   1: app::main (src/main.rs:88)\n"
        "note: run with `RUST_BACKTRACE=1` for a full backtrace\n"
    )
    first = msg.split(":")[0]
    return {"kind": "stack", "full_text": full,
            "evidence": [f"thread 'main' panicked at src/{fname}:", msg],
            "note": f"synthetic small panic stack ({func})",
            "query": f"why did the program panic: {first}"}


def log_small(idx: int) -> dict:
    host, port, svc, err, start = LOG_CASES[idx % len(LOG_CASES)]
    lines = [
        f"[2026-10-26T08:01:0{idx}Z] INFO  {start}",
        f"[2026-10-26T08:01:0{idx + 1}Z] DEBUG loading config from /etc/app/config.toml",
        f"[2026-10-26T08:01:0{idx + 2}Z] ERROR failed to connect to {svc}: {err} (host={host} port={port})",
        f"[2026-10-26T08:01:0{idx + 3}Z] WARN  retrying in 5s...",
    ]
    return {"kind": "log", "full_text": "\n".join(lines) + "\n",
            "evidence": [lines[2]],
            "note": f"synthetic small service log: {svc} unreachable",
            "query": f"why did the service fail to connect to {svc} at {host}"}


def diff_small(idx: int) -> dict:
    path, var, old, new, guard_msg, guard = DIFF_HUNKS[idx % len(DIFF_HUNKS)]
    fname = path.split("/")[-1].replace(".rs", "").title()
    full = (
        f"diff --git a/{path} b/{path}\n"
        f"--- a/{path}\n"
        f"+++ b/{path}\n"
        f"@@ -41,6 +41,8 @@ impl {fname} {{\n"
        f"-    {old}\n"
        f"+    {new}\n"
        f"+    {guard}\n"
        " }\n"
    )
    return {"kind": "diff", "full_text": full,
            "evidence": [f"+    {new}", f"+    {guard}"],
            "note": f"synthetic small review diff: {var} guard",
            "query": f"what happens with {guard_msg}"}


def test_small(idx: int) -> dict:
    name, msg, detail = TEST_CASES[idx % len(TEST_CASES)]
    full = (
        "running 3 tests\n"
        f"test {name} ... FAILED\n"
        "\n"
        "failures:\n"
        "\n"
        f"---- {name} stdout ----\n"
        f"{msg}\n"
        f"  {detail}\n"
        "\n"
        "\n"
        "failures:\n"
        f"    {name}\n"
        "\n"
        "test result: FAILED. 1 failed; 0 passed\n"
    )
    return {"kind": "test", "full_text": full,
            "evidence": [f"test {name} ... FAILED", msg],
            "note": f"synthetic small unit test failure {name}",
            "query": f"which test failed with {msg.split(':')[0]}"}


LARGE = {"build": build_large, "stack": stack_large, "diff": diff_large,
         "log": log_large, "test": test_large}
SMALL = {"build": build_small, "stack": stack_small, "diff": diff_small,
         "log": log_small, "test": test_small}


def main() -> int:
    slices = []
    for kind in KINDS:
        for idx in range(4):
            slc = LARGE[kind](idx)
            slices.append({
                "id": f"{kind}-{idx + 1:02d}",
                "kind": kind,
                "query": slc["query"],
                "full_text": slc["full_text"],
                "evidence": slc["evidence"],
                "note": slc["note"],
            })
        slc = SMALL[kind](0)
        slices.append({
            "id": f"{kind}-05",
            "kind": kind,
            "query": slc["query"],
            "full_text": slc["full_text"],
            "evidence": slc["evidence"],
            "note": slc["note"],
        })
    # hard validation before writing
    seen = set()
    stats = {"large_sieve": 0, "small_passthrough": 0}
    for slc in slices:
        assert slc["id"] not in seen, slc["id"]
        seen.add(slc["id"])
        assert slc["kind"] in KINDS
        assert slc["query"] and slc["full_text"] and slc["evidence"] and slc["note"]
        lines = len(slc["full_text"].splitlines())
        toks = _token_est(slc["full_text"].splitlines())
        if slc["id"].endswith("-05"):
            assert lines <= 15 and toks < 600, (slc["id"], lines, toks)
            stats["small_passthrough"] += 1
        else:
            assert lines >= 45, (slc["id"], lines)
            stats["large_sieve"] += 1
        for ev in slc["evidence"]:
            assert ev in slc["full_text"], (slc["id"], ev)
    corpus = {
        "name": "scx-fidelity-v1",
        "description": "Synthetic incident slices for compression-fidelity eval (P1, Gauge roadmap)",
        "generated_note": (
            f"Deterministically generated by tools/gen_fidelity.py (seed={SEED}). "
            f"Per kind: 4 large slices (>45 lines, sieve path) + 1 small slice (<12 "
            f"lines, passthrough path). Evidence lines verbatim in full_text."
        ),
        "provenance": {"generator": "tools/gen_fidelity.py", "seed": SEED,
                       "python": sys.version.split()[0]},
        "expected_slices": len(slices),
        "expected_kinds": sorted({s["kind"] for s in slices}),
        "slices": slices,
    }
    OUT.write_text(json.dumps(corpus, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"fidelity corpus: wrote {OUT} slices={len(slices)} {stats}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
