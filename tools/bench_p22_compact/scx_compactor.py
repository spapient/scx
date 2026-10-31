"""P22 · scx Compactor 适配器（实现 compactbench 官方 Compactor 接口）。

把 scx（v0.11.0，本地二进制，逐行保留式机械压缩）作为压缩方法接入
CompactBench harness。本 compactor 自身**不调用任何 LLM**：压缩是本地
子进程；结构化状态由确定性规则从轮次标签与原文抽取，并跨 drift cycle
账本式累积（参考 hybrid-ledger 的 previous_artifact 合并口径）。

压缩口径（冻结，可复现）：
  <scx_bin> <tmpfile> --query <signal> --ratio 0.35 --context 1
      --fill-floor 2 --list-keep 1 --force <signal-turn>...
不启用 token 级 trim：命中行整行保留，保证动作短语/人名整串存活。
压缩文本 = stdout 中第一条 ``compressed:`` 行之前的全部行（spec §7 范式）。

注意（如实记录，不粉饰）：CompactBench 社区版对话 transcript 很短；
scx 的小输入透传保护（<40 行且 <400 token）在 starter / medium / hard
套件上会原样返回，压缩率 ≈1.0x；只有 elite 难度（~431-504 token）越过
保护阈值才进入真正的行保留筛选。
"""
from __future__ import annotations

import os
import re
import subprocess
import tempfile
from pathlib import Path
from typing import Any, ClassVar

from compactbench.compactors._utils import render_turns, uniq_preserve_order
from compactbench.compactors.base import Compactor
from compactbench.contracts import (
    CompactionArtifact,
    StructuredState,
    Transcript,
    Turn,
)

SCX_BIN_DEFAULT = "/Users/fei/code/scx/target/release/scx"
# 冻结档位（README 与结果 JSON 同步记录）。逐行保留路线：不做 token 级 trim，
# 命中行整行保留，保证动作短语/人名整串存活（机械提取口径，对齐论文参考系）。
SCX_FLAGS: tuple[str, ...] = (
    "--ratio", "0.35",
    "--context", "1",
    "--fill-floor", "2",
    "--list-keep", "1",
)

_NEVER_RE = re.compile(r"\bnever\s+(.*?)(?:[.!?,]|$)", re.I)
_NO_CIRCUM_RE = re.compile(
    r"under no circumstances\s+(?:should\s+)?"
    r"(?:anyone|anybody|everyone|nobody|no one|we|you|they|he|she|I)?\s*"
    r"(?:on this thread|in this thread)?\s*(.*?)(?:[.!?,]|$)", re.I)
_APPROVED_RES = [
    re.compile(r"instead[,:]?\s*(?:let'?s\s+)?(.*?)(?:[.!?,]|$)", re.I),
    re.compile(r"new plan:\s*(.*?)(?:[.!?,]|$)", re.I),
    re.compile(r"we'?re going to\s+(.*?)(?:[.!?,]|$)", re.I),
    re.compile(r"use\s+(.*?)\s+instead", re.I),
    re.compile(r"let'?s\s+(.*?)(?:[.!?,]|$)", re.I),
]
_PLAN_LABEL_RE = re.compile(
    r"(?:current plan|approach):\s*(.*?)(?:[.!?,]|$)", re.I)
_CAPITAL_WORD = re.compile(r"\b[A-Z][a-zA-Z]+\b")

CRITICAL_TAGS = {"critical_constraint"}
OVERRIDE_TAGS = {"decision_override", "first_override", "final_override",
                 "partial_override", "late_override", "meta_reset"}
CONFUSION_TAGS = {"confusion_risk"}
SIGNAL_TAGS = CRITICAL_TAGS | OVERRIDE_TAGS | CONFUSION_TAGS
# 句首大写词与常见普通大写词（非人名），实体启发式里剔除。
_STOP_NAMES = {
    "The", "This", "That", "These", "Those", "I", "OK", "Absolutely", "Quick",
    "Critical", "Given", "Sounds", "Under", "Rule", "Working", "Heads", "Side",
    "What", "By", "Do", "Oh", "Off", "Random", "Unrelated", "Alright", "Two",
    "Both", "HTML", "API", "Let", "Summarize", "Sure", "Happy", "Got", "Noted",
    "Fair", "Acknowledged", "Okay", "On", "For", "With",
}


def _scx_compress(bin_path: str, text: str, query: str, forces: list[str]) -> tuple[str, dict[str, Any]]:
    """调用 scx 二进制；返回 (压缩文本, 统计元数据)。fail-open：异常/无标记返回原文。"""
    fd, tmp = tempfile.mkstemp(suffix=".txt")
    meta: dict[str, Any] = {"rc": None, "mode": "unknown", "kept": None, "total": None,
                            "cut_pct": None, "passthrough": True}
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            fh.write(text)
        cmd: list[str] = [bin_path, tmp, "--query", query, *SCX_FLAGS]
        for f in forces:
            cmd += ["--force", f]
        proc = subprocess.run(
            cmd, capture_output=True, text=True, timeout=120,
        )
        meta["rc"] = proc.returncode
        if proc.returncode != 0 or not proc.stdout.strip():
            meta["mode"] = "failopen:subprocess"
            return text, meta
        lines = proc.stdout.splitlines()
        cut = 0
        stats_line = ""
        for k, ln in enumerate(lines):
            if ln.lstrip().startswith("compressed:"):
                cut = k
                stats_line = ln.strip()
                break
        else:
            meta["mode"] = "failopen:no-marker"
            return text, meta
        kept = "\n".join(lines[:cut])
        if not kept.strip():
            meta["mode"] = "failopen:empty"
            return text, meta
        meta["stats_line"] = stats_line
        m = re.search(r"kept=(\d+)/(\d+) lines chars=\d+->\d+ \(([\d.]+)% cut\)", stats_line)
        if m:
            meta["kept"], meta["total"], meta["cut_pct"] = int(m.group(1)), int(m.group(2)), float(m.group(3))
        m2 = re.search(r"mode=(\S+)", stats_line)
        if m2:
            meta["mode"] = m2.group(1)
        meta["passthrough"] = meta["mode"].startswith("passthrough")
        return kept, meta
    except Exception as exc:  # noqa: BLE001 —— fail-open 原样返回，不打断 harness
        meta["mode"] = f"failopen:{type(exc).__name__}"
        meta["error"] = str(exc)
        return text, meta
    finally:
        try:
            os.unlink(tmp)
        except OSError:
            pass


_FILLER_PREFIX_RE = re.compile(
    r"^(on this thread|in this thread|on the thread|anyone|anybody|everyone|nobody)\s+",
    re.I,
)


def _extract_forbidden(content: str) -> str | None:
    m = _NEVER_RE.search(content) or _NO_CIRCUM_RE.search(content)
    if not m:
        return None
    phrase = m.group(1).strip().rstrip(".!?,")
    while phrase:
        stripped = _FILLER_PREFIX_RE.sub("", phrase).strip()
        if stripped == phrase:
            break
        phrase = stripped
    return phrase or None


def _extract_approved(content: str) -> str | None:
    for rx in _APPROVED_RES:
        m = rx.search(content)
        if m:
            phrase = m.group(1).strip().rstrip(".!?,")
            if phrase:
                return phrase
    return None


def _extract_plan_label(content: str) -> str | None:
    m = _PLAN_LABEL_RE.search(content)
    return m.group(1).strip().rstrip(".!?,") if m else None


def _entity_names(turn: Turn | None) -> list[str]:
    """句中（非句首）大写词视为候选人名；句首词跳过。"""
    if turn is None:
        return []
    names: list[str] = []
    for sentence in re.split(r"(?<=[.!?])\s+", turn.content):
        stripped = sentence.strip()
        words = _CAPITAL_WORD.findall(sentence)
        for i, w in enumerate(words):
            if i == 0 and stripped.startswith(w):
                continue
            if w in _STOP_NAMES:
                continue
            if w not in names:
                names.append(w)
    return names


class ScxCompactor(Compactor):
    """scx 逐行保留压缩 + 规则结构化状态 + 跨 cycle 账本。"""

    name: ClassVar[str] = "scx-line-retention"
    version: ClassVar[str] = "0.11.0-p22"

    def __init__(self, provider: Any, model: str) -> None:
        super().__init__(provider, model)
        self.bin = os.environ.get("SCX_BIN", SCX_BIN_DEFAULT)

    async def compact(
        self,
        transcript: Transcript,
        config: dict[str, Any] | None = None,
        previous_artifact: CompactionArtifact | None = None,
    ) -> CompactionArtifact:
        turns = list(transcript.turns)
        rendered = render_turns(turns)

        tagged = [t for t in turns if t.tags and any(tag in SIGNAL_TAGS for tag in t.tags)]
        critical_turns = [t for t in tagged if any(x in CRITICAL_TAGS for x in t.tags)]
        override_turns = [t for t in tagged if any(x in OVERRIDE_TAGS for x in t.tags)]
        confusion_turns = [t for t in tagged if any(x in CONFUSION_TAGS for x in t.tags)]
        first_user = next((t for t in turns if t.role.value == "user"), None)
        signal_turns = tagged + ([first_user] if first_user is not None and first_user not in tagged else [])
        query = " ".join(t.content for t in signal_turns)[:1000]
        # 信号轮整行强制保留（scx --force 子串匹配，内容是渲染行的子串）。
        forces = [t.content for t in signal_turns]

        compressed, meta = _scx_compress(self.bin, rendered, query, forces)

        # ---- 规则化状态抽取（无 LLM；确定性；规则未命中时整行兜底进 facts）----
        forbidden: list[str] = []
        locked: list[str] = []
        facts: list[str] = []
        unresolved: list[str] = []
        entity_map: dict[str, str] = {}

        for t in critical_turns:
            phrase = _extract_forbidden(t.content)
            if phrase:
                forbidden.append(phrase)
            else:
                forbidden.append(t.content)  # 兜底：整行，动作短语是其子串
            facts.append(t.content)

        # override：按出现顺序记录候选；最后一个 = 当前锁定决策，之前的 = 已否决。
        approved_candidates: list[str] = []
        for t in override_turns:
            facts.append(t.content)
            approved = _extract_approved(t.content)
            if approved:
                approved_candidates.append(approved)
        if approved_candidates:
            locked.append(approved_candidates[-1])
            forbidden.extend(approved_candidates[:-1])
        # 首个 user 轮里的旧方案（v1_1/v1_4/v1_5 被否决项）。
        if override_turns and first_user is not None:
            rejected = _extract_plan_label(first_user.content)
            if rejected:
                forbidden.append(rejected)

        for t in confusion_turns:
            facts.append(t.content)

        if first_user is not None:
            facts.append(first_user.content)
            for name in _entity_names(first_user):
                entity_map[name] = "primary_subject"
        for t in confusion_turns:
            for name in _entity_names(t):
                entity_map.setdefault(name, "owner")

        last_user = next((t for t in reversed(turns) if t.role.value == "user"), None)
        if last_user and last_user.content.strip().endswith("?"):
            unresolved.append(last_user.content)

        # 跨 cycle 账本合并（locked / forbidden / deferred / unresolved 累积去重）。
        if previous_artifact is not None:
            prev = previous_artifact.structured_state
            locked = uniq_preserve_order(list(prev.locked_decisions) + locked)
            forbidden = uniq_preserve_order(list(prev.forbidden_behaviors) + forbidden)
            unresolved = uniq_preserve_order(list(prev.unresolved_items) + unresolved)
            deferred = list(prev.deferred_items)
            entity_map = {**prev.entity_map, **entity_map}
        else:
            deferred = []

        state = StructuredState(
            immutable_facts=uniq_preserve_order(facts)[:200],
            locked_decisions=locked[:200],
            deferred_items=deferred[:200],
            forbidden_behaviors=forbidden[:200],
            entity_map=entity_map,
            unresolved_items=unresolved[:200],
        )

        summary = compressed[:8000]
        warnings: list[str] = []
        if meta["passthrough"]:
            warnings.append(f"scx passthrough ({meta['mode']}): 小输入保护，原文未压缩")

        return CompactionArtifact(
            summaryText=summary,
            structured_state=state,
            selectedSourceTurnIds=[t.id for t in turns],
            warnings=warnings,
            methodMetadata={
                "method": self.name,
                "version": self.version,
                "scx_bin": self.bin,
                "scx_flags": list(SCX_FLAGS),
                "query": query,
                "force_turns": len(forces),
                "calls": 0,  # scx 本地压缩，无 LLM 调用
                "provider": self.provider.key,
                "model": self.model,
                "scx": meta,
            },
        )
