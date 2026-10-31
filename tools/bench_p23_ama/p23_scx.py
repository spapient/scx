#!/usr/bin/env python3
"""P23 AMA-Bench · scx memory 适配器（轨迹 turn → scx case；turn 级召回）。

映射说明（轨迹非对话形态 → scx 证据型 case，规则确定、无 LLM 参与摄取）：
- 官方 BM25 基线以「一个 turn = 一篇文档」建索引（repo `src/method/bm25.py`，
  top_k=10，`configs/method_configs/bm25_config.json`）。scx 通道保持同一粒度：
  一个 turn 一个 case，保证两通道在同一语料、同一 K 下可比。
- case.text  = 官方 `_trajectory_to_text` 同款 turn 块（Step/Action/Observation）。
- case.query = 领域 + 任务类型 + 步号 + 动作头词（驱动 scx 词面/字段/MinHash/RRF 召回）。
- case.fix   = 确定性规则摘要（动作头 + 观察头 160 字符）；摄取不调用 LLM，可复现。
- 记忆隔离：每个 episode 独立 mem-dir；QA 之间记忆不更新（官方 offline per-query 协议）。
"""
from __future__ import annotations

import json
import re
import subprocess
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Optional

DEFAULT_BINARY = "/Users/fei/code/scx/target/release/scx"
MAX_CASE_TEXT_BYTES = 1024 * 1024  # 与 src/memory/ingest.rs MAX_CASE_TEXT 对齐
TRUNCATE_HEAD = 480 * 1024
TRUNCATE_TAIL = 480 * 1024

STORED_RE = re.compile(r"stored case=(\S+)\s+added=(yes|no)")
COMPRESSED_RE = re.compile(
    r"kept=(\d+)/(\d+) lines chars=(\d+)->(\d+) \(([\d.]+)% cut\)")
MEM_RE = re.compile(r"mem: sim=([\d.]+) score=([\d.]+) case=([0-9a-f]+)")


def _null(v: Any) -> str:
    if v is None:
        return "(null)"
    s = str(v)
    return s if s.strip() else "(empty)"


def turn_text(turn: dict[str, Any], idx: int) -> str:
    """官方 memory_interface.py `_trajectory_to_text` 同款格式（1-based 步号）。"""
    return (
        f"Step {idx}:\n"
        f"Action: {_null(turn.get('action'))}\n"
        f"Observation: {_null(turn.get('observation'))}\n"
    )


def _head(text: str, n: int) -> str:
    text = text.strip()
    return text if len(text) <= n else text[:n]


def turn_query(episode: dict[str, Any], idx: int, turn: dict[str, Any]) -> str:
    action_head = _head(str(turn.get("action") or ""), 80)
    return (f"{episode.get('domain','')} {episode.get('task_type','')} "
            f"step {idx} action {action_head}")


def turn_fix(episode: dict[str, Any], idx: int, turn: dict[str, Any]) -> str:
    action_head = _head(str(turn.get("action") or ""), 120)
    obs_head = _head(str(turn.get("observation") or ""), 160)
    return f"step {idx}: action={action_head}; observation={obs_head}"


@dataclass
class TurnRecord:
    turn_index: int
    case_id: str
    short: str
    query: str
    fix: str
    text: str
    compressed: dict[str, Any] = field(default_factory=dict)
    truncated: bool = False


class SCXMemory:
    """单 episode 的 scx 记忆库封装。"""

    def __init__(self, binary: str = DEFAULT_BINARY, mem_root: str | Path = "/tmp/scx_bench_ama/mem"):
        self.binary = binary
        self.mem_root = Path(mem_root)

    def _mem_dir(self, episode_id: Any) -> Path:
        return self.mem_root / f"ep_{episode_id}"

    def _registry_path(self, episode_id: Any) -> Path:
        return self._mem_dir(episode_id) / "p23_registry.json"

    def ingest_episode(self, episode: dict[str, Any], force: bool = False) -> dict[str, Any]:
        """摄取一个 episode 的全部 turn；返回摄取审计摘要。"""
        eid = episode["episode_id"]
        mem_dir = self._mem_dir(eid)
        registry_path = self._registry_path(eid)
        if registry_path.exists() and not force:
            reg = json.loads(registry_path.read_text(encoding="utf-8"))
            return reg["audit"]

        mem_dir.mkdir(parents=True, exist_ok=True)
        records: list[TurnRecord] = []
        audit: dict[str, Any] = {
            "episode_id": eid, "domain": episode.get("domain"),
            "task_type": episode.get("task_type"), "turns": len(episode["trajectory"]),
            "stored": 0, "dedup": 0, "truncated": 0, "failed": 0,
            "kept_chars": 0, "source_chars": 0, "errors": [],
        }
        for i, turn in enumerate(episode["trajectory"], start=1):
            text = turn_text(turn, i)
            query = turn_query(episode, i, turn)
            fix = turn_fix(episode, i, turn)
            audit["source_chars"] += len(text)
            record = self._store_turn(mem_dir, text, query, fix, episode, i, turn, audit)
            if record is not None:
                records.append(record)
                audit["kept_chars"] += len(record.text)
            else:
                audit["failed"] += 1
        registry = {
            "episode_id": eid,
            "records": [r.__dict__ for r in records],
            "audit": audit,
        }
        registry_path.write_text(json.dumps(registry, ensure_ascii=False, indent=1),
                                 encoding="utf-8")
        return audit

    def _store_turn(self, mem_dir: Path, text: str, query: str, fix: str,
                    episode: dict[str, Any], idx: int, turn: dict[str, Any],
                    audit: dict[str, Any]) -> Optional[TurnRecord]:
        # 官方数据个别字段含 \x00，subprocess args 会报 embedded null byte
        text = text.replace("\x00", "")
        query = query.replace("\x00", "")
        fix = fix.replace("\x00", "")
        payload = text.encode("utf-8")
        truncated = False
        if len(payload) > MAX_CASE_TEXT_BYTES:
            truncated = True
            head = text[:TRUNCATE_HEAD]
            tail = text[-TRUNCATE_TAIL:]
            text = head + "\n[... truncated by P23 adapter: scx MAX_CASE_TEXT=1MiB ...]\n" + tail
            payload = text.encode("utf-8")
        args = [self.binary, "--query", query, "--store", "1", "--fix", fix,
                "--ratio", "1.0", "--context", "1", "--mem-dir", str(mem_dir)]
        try:
            proc = subprocess.run(args, input=payload, capture_output=True,
                                  timeout=180)
        except subprocess.TimeoutExpired:
            audit["errors"].append(f"ep {episode.get('episode_id')} turn {idx}: timeout")
            return None
        out = (proc.stdout + proc.stderr).decode("utf-8", "replace")
        if proc.returncode != 0:
            audit["errors"].append(
                f"ep {episode.get('episode_id')} turn {idx}: rc={proc.returncode} "
                f"{out[-300:]}")
            return None
        m = STORED_RE.search(out)
        if not m:
            audit["errors"].append(
                f"ep {episode.get('episode_id')} turn {idx}: no stored match {out[-300:]}")
            return None
        case_id = m.group(1)
        added = m.group(2) == "yes"
        cm = COMPRESSED_RE.search(out)
        compressed: dict[str, Any] = {}
        if cm:
            compressed = {"kept": int(cm.group(1)), "total": int(cm.group(2)),
                          "chars_in": int(cm.group(3)), "chars_out": int(cm.group(4)),
                          "cut_pct": float(cm.group(5))}
        audit["stored" if added else "dedup"] += 1
        if truncated:
            audit["truncated"] += 1
        return TurnRecord(
            turn_index=idx, case_id=case_id, short=case_id.split("_")[-1],
            query=query, fix=fix, text=text, compressed=compressed,
            truncated=truncated,
        )

    def recall(self, episode: dict[str, Any], question: str, k: int = 10) -> list[dict[str, Any]]:
        eid = episode["episode_id"]
        registry_path = self._registry_path(eid)
        if not registry_path.exists():
            return []
        registry = json.loads(registry_path.read_text(encoding="utf-8"))
        by_short = {r["short"]: r for r in registry["records"]}
        args = [self.binary, "--recall-only", "--recall", str(k),
                "--recall-detail", "--mem-dir", str(self._mem_dir(eid)),
                "--query", question]
        try:
            proc = subprocess.run(args, capture_output=True, timeout=120)
        except subprocess.TimeoutExpired:
            return []
        out = (proc.stdout + proc.stderr).decode("utf-8", "replace")
        hits: list[dict[str, Any]] = []
        for rank, m in enumerate(MEM_RE.finditer(out), start=1):
            short = m.group(3)
            rec = by_short.get(short)
            if rec is None:
                continue
            hits.append({
                "rank": rank, "sim": float(m.group(1)), "score": float(m.group(2)),
                "case_id": rec["case_id"], "turn_index": rec["turn_index"],
                "query": rec["query"], "fix": rec["fix"], "text": rec["text"],
                "compressed": rec.get("compressed", {}),
                "truncated": rec.get("truncated", False),
            })
        return hits

    @staticmethod
    def context_from_hits(hits: list[dict[str, Any]]) -> str:
        """官方 `get_retrieved_context` 同款：命中 turn 块按召回序以空行连接。"""
        return "\n\n".join(h["text"].rstrip() for h in hits)

    def episode_stats(self, episode_id: Any) -> dict[str, Any]:
        proc = subprocess.run([self.binary, "--stats", "--mem-dir",
                               str(self._mem_dir(episode_id))],
                              capture_output=True, timeout=60)
        out = (proc.stdout + proc.stderr).decode("utf-8", "replace")
        stats: dict[str, Any] = {"raw": out.strip()}
        for line in out.splitlines():
            if line.startswith("stats:"):
                m = re.search(r"cases=(\d+)", line)
                if m:
                    stats["cases"] = int(m.group(1))
        return stats
