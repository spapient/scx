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

KINDS = ("build", "stack", "diff", "log", "test", "db", "network", "hardware")

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

DB_CASES = [
    ("postgres", "db.internal", 5432, "connection pool exhausted (pool_size=50)",
     "max_connections reached"),
    ("mysql", "db.internal", 3306, "replication stopped: sync_binlog error 1236",
     "slave_io_running=No"),
    ("postgres", "tx.internal", 5432, "deadlock detected: processes 42 and 77 wait forever",
     "rollback in progress"),
    ("mysql", "db.internal", 3306, "lock wait timeout exceeded; try restarting transaction",
     "innodb_lock_wait_timeout=50"),
]

NET_CASES = [
    ("dns", "api.internal", 53, "NXDOMAIN: api.internal not found",
     "unbound resolver, query timeout 5000ms"),
    ("tcp", "edge.internal", 443, "connection timed out after 30000ms",
     "traceroute stops at 10.20.0.1"),
    ("tls", "gw.internal", 443, "certificate has expired",
     "openssl verify return code 10"),
    ("route", "core.internal", 0, "Destination Net Unreachable",
     "no route to host via 172.16.0.1"),
]

HW_CASES = [
    ("disk", "sda", "I/O error, dev sda, sector 1126400 op 0x0:(READ)",
     "EXT4-fs error (device sda1)"),
    ("memory", "DIMM_A2", "Uncorrected memory error at DIMM_A2 rank 1",
     "EDAC MC0: UE row 7"),
    ("nvme", "nvme0n1", "nvme0: I/O Cmd 0x2a (READ) timed out",
     "nvme nvme0: controller is down"),
    ("link", "eth0", "eth0: link down (carrier lost)",
     "bond0: link status definitely down"),
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
    focus_code, focus_msg, _, _, _ = BUILD_CODES[idx % len(BUILD_CODES)]
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
            # only the query-focus error line is evidence for THIS slice's query
            if code == focus_code:
                ev.append(f"error[{code}]: {msg}")
    blocks.append("error: could not compile `app` due to previous errors")
    return {
        "kind": "build",
        "full_text": "\n".join(blocks) + "\n",
        "evidence": ev,
        "query": f"why did the build fail with {focus_code} {focus_msg.split('`')[1] if '`' in focus_msg else sym}",
        "note": f"synthetic multi-crate rust build failure (large, sieve path) focus {focus_code}",
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


# --- second failure domains: database / network / hardware (P2) ---


def db_large(idx: int) -> dict:
    svc, host, port, msg, extra = DB_CASES[idx % len(DB_CASES)]
    lines = [
        f"[2026-10-26T09:00:00Z] INFO  starting {svc} connection supervisor",
        f"[2026-10-26T09:00:01Z] DEBUG configured host={host} port={port}",
        "[2026-10-26T09:00:02Z] INFO  acquired 48/50 slots",
    ]
    for i in range(20):
        t = 3 + i * 4
        lines.append(f"[2026-10-26T09:{t // 60:02d}:{t % 60:02d}Z] ERROR {msg}")
        lines.append(f"[2026-10-26T09:{t // 60:02d}:{(t + 2) % 60:02d}Z] WARN  {extra}")
    lines.append(f"[2026-10-26T09:02:00Z] ERROR supervisor gave up after 20 alerts")
    lines.append("[2026-10-26T09:02:01Z] INFO  entering read-only fallback mode")
    ev = [f"[2026-10-26T09:00:03Z] ERROR {msg}"]
    return {
        "kind": "db",
        "full_text": "\n".join(lines) + "\n",
        "evidence": [ev[0]],
        "query": f"why did {svc} report {msg.split(':')[0].split('(')[0].strip()}",
        "note": f"synthetic database failure domain: {msg}",
    }


def network_large(idx: int) -> dict:
    tool, target, port, err, detail = NET_CASES[idx % len(NET_CASES)]
    blocks = []
    if tool == "dns":
        blocks += [f"dig @10.0.0.53 {target}",
                   "; <<>> DiG 9.18 <<>> @10.0.0.53 {target}",
                   f"; (1 server found)", f";; ->>HEADER<<- opcode: QUERY, status: {err.split(':')[0].strip()}",
                   f";; QUESTION SECTION:", f";{target}.\tIN\tA", f";; ANSWER SECTION:",
                   f";; AUTHORITY SECTION:", f"internal.\t3600\tIN\tSOA\tns1.internal.", f";; query time: 5023 msec"]
        blocks += [f";; {err}" for _ in range(20)]
    elif tool == "tcp":
        blocks += [f"traceroute to {target} ({target}), 30 hops max, 60 byte packets",
                   " 1  10.20.0.1  0.4 ms  0.3 ms  0.4 ms"]
        for h in range(2, 22):
            blocks.append(f" {h:2d}  172.16.{h}.1  15.2 ms  14.9 ms  15.4 ms")
        for h in range(22, 31):
            blocks.append(f" {h:2d}  * * *")
        blocks += [f"connect: {err}", f"; {detail}"]
    elif tool == "tls":
        blocks += [f"openssl s_client -connect {target}:{port} -servername {target}",
                   "CONNECTED(00000003)", "depth=0 CN = " + target,
                   "verify error:num=10:" + err,
                   "verify return:1", "verify return:0",
                   f"; {detail}"]
        blocks += [f"; {err}" for _ in range(30)]
    else:
        blocks += [f"ip route get {target}",
                   f"RTNETLINK answers: {err}",
                   f"; {detail}",
                   f"ping {target}",
                   "PING " + target + " (" + target + ") 56(84) bytes of data."]
        for i in range(5):
            blocks.append(f"{i * 10 + 1:5d} bytes from {target}: icmp_seq={i + 1} ttl=64 time=0.4 ms")
        blocks += ["--- " + target + " ping statistics ---", "5 packets transmitted, 5 received, 0% packet loss"]
        blocks += [f"; {err}" for _ in range(25)]
    while len(blocks) < 48:
        blocks.append(f"; {err}")
    return {
        "kind": "network",
        "full_text": "\n".join(blocks) + "\n",
        "evidence": [err],
        "query": f"what caused {err}",
        "note": f"synthetic network failure domain: {tool} {err}",
    }


def hardware_large(idx: int) -> dict:
    dev, slot, msg, extra = HW_CASES[idx % len(HW_CASES)]
    lines = ["[   12.441002] Linux version 6.6.10-amd64 (builder@kbuild) (gcc-12) #1 SMP"]
    for i in range(42):
        t = 12 + i
        lines.append(f"[{t:7d}.{i * 37 % 100:06d}] {msg}")
        if i % 3 == 0:
            lines.append(f"[{t + 1:7d}.{i * 53 % 100:06d}] {extra}")
    while len(lines) < 48:
        lines.append(f"[   58.00000{i}1] {extra}")
    lines.append(f"[   59.000001] systemd[1]: Failed to mount /dev/{dev}1 on /data")
    lines.append("[   58.000021] systemd[1]: Dependency failed for /data mount")
    ev = [msg, extra]
    return {
        "kind": "hardware",
        "full_text": "\n".join(lines) + "\n",
        "evidence": ev,
        "query": f"what {dev} failure caused {msg.split(':')[0].strip()}",
        "note": f"synthetic hardware failure domain: {dev} {slot}",
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


def db_small(idx: int) -> dict:
    svc, host, port, msg, extra = DB_CASES[idx % len(DB_CASES)]
    lines = [
        f"[2026-10-26T10:00:00Z] INFO  starting {svc} supervisor",
        f"[2026-10-26T10:00:01Z] ERROR {msg}",
        f"[2026-10-26T10:00:02Z] WARN  {extra}",
        "[2026-10-26T10:00:03Z] INFO  retry in 5s",
    ]
    return {"kind": "db", "full_text": "\n".join(lines) + "\n",
            "evidence": [lines[1]],
            "note": f"synthetic small database failure {msg}",
            "query": f"why did {svc} report {msg.split(':')[0].split('(')[0].strip()}"}


def network_small(idx: int) -> dict:
    tool, target, port, err, detail = NET_CASES[idx % len(NET_CASES)]
    lines = [
        f"; {err}",
        f"; {detail}",
        f"service health check: GET https://{target}/healthz -> connection failed",
        "alert: upstream unreachable, entering degraded mode",
    ]
    return {"kind": "network", "full_text": "\n".join(lines) + "\n",
            "evidence": [lines[0]],
            "note": f"synthetic small network failure {tool}",
            "query": f"what caused {err}"}


def hardware_small(idx: int) -> dict:
    dev, slot, msg, extra = HW_CASES[idx % len(HW_CASES)]
    lines = [
        f"[   12.441002] {msg}",
        f"[   12.441110] {extra}",
        f"[   12.442001] systemd[1]: Failed to mount /dev/{dev}1 on /data",
    ]
    return {"kind": "hardware", "full_text": "\n".join(lines) + "\n",
            "evidence": [lines[0]],
            "note": f"synthetic small hardware failure {dev}",
            "query": f"what {dev} failure caused {msg.split(':')[0].strip()}"}


LARGE = {"build": build_large, "stack": stack_large, "diff": diff_large,
         "log": log_large, "test": test_large, "db": db_large,
         "network": network_large, "hardware": hardware_large}
SMALL = {"build": build_small, "stack": stack_small, "diff": diff_small,
         "log": log_small, "test": test_small, "db": db_small,
         "network": network_small, "hardware": hardware_small}


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
