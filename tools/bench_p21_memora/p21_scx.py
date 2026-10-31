#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""P21 Memora 接入：scx 记忆库 CLI 封装 + 治理注册表（P2 仲裁 / P3 版本化）。

两分支口径：
- gov（启用治理）：P2 冲突仲裁保持默认（conflict_sim=0.8，store 时自动执行）；
  update 会话对命中的旧 case 走 --revise；delete 会话对命中旧 case 走 --retire。
- nogov（无治理基线）：写入前把 meta.toml 的 [params] conflict_sim 抬到 1.0
  （加权 Jaccard ≤1.0，等价关闭近重复仲裁/合并）；update/delete 不做任何
  revise/retire，旧 case 保持 active 可被召回。

注册表（registry.json）Python 侧维护：官方库只回显 case id 指纹段（hash16），
revise/retire 需要完整 id；同时保存 case 文本/fix 供作答阶段构造记忆上下文。
"""
from __future__ import annotations

import json
import re
import subprocess
import time
from pathlib import Path
from typing import Any, Optional

BIN = "/Users/fei/code/scx/target/release/scx"

STORED_RE = re.compile(r"scx: stored case=(\S+) added=(\w+)")
RECALL_RE = re.compile(r"^mem: sim=([0-9.]+) score=([0-9.]+) case=(\S+)")
RETIRE_RE = re.compile(r"retire: life=(\w+).*case=(\S+)")

# 注册表稳定键需要剔除的易变字段（时间戳、状态、流程元数据）：同一实体跨
# update/delete 的身份由稳定标识字段（item / description / event_name 等）决定。
VOLATILE = {"created_at", "updated_at", "date", "session_history", "session_count",
            "total_sessions_before_completion", "moved_to_completed",
            "completion_threshold", "pool_status", "generation_method",
            "conversion_reason", "deletion_method", "partial_content_generated",
            "update_type", "actual_operation", "operation_converted",
            "update_type", "category"}


def _stable(value: Any) -> Any:
    if isinstance(value, dict):
        return {k: _stable(v) for k, v in value.items() if k not in VOLATILE}
    if isinstance(value, list):
        return [_stable(x) for x in value]
    return value


def stable_key(od: dict[str, Any]) -> Optional[str]:
    """operation_details.item 的稳定注册表键。

    偏好项 = `pref:<item>:<subcategory>`；其余实体 = 剔除易变字段后的规范 JSON。
    """
    item = od.get("item")
    if item is None:
        return None
    if isinstance(item, str) and od.get("subcategory") is not None:
        return f"pref:{item}:{od.get('subcategory')}"
    return json.dumps(_stable(item), sort_keys=True, ensure_ascii=False, default=str)


def _old_pref_key(od: dict[str, Any]) -> Optional[str]:
    old = od.get("old_item")
    if old is None:
        return None
    return f"pref:{old}:{od.get('subcategory')}"


class ScxMem:
    """一个 persona-period 对应一个独立记忆库（官方 Track 2 同 user_id 隔离口径）。"""

    def __init__(self, root: Path, governance: bool, workdir: Path):
        self.root = Path(root)
        self.governance = governance  # True=gov, False=nogov
        self.workdir = Path(workdir)
        self.workdir.mkdir(parents=True, exist_ok=True)
        (self.root / "cases").mkdir(parents=True, exist_ok=True)
        (self.root / "index").mkdir(parents=True, exist_ok=True)
        self.registry_path = self.root / "registry.json"
        self.registry: dict[str, Any] = {"governance": "gov" if governance else "nogov",
                                        "active": {}, "by_id": {}, "events": []}
        self._seq = 0
        if self.registry_path.exists():
            self.registry = json.loads(self.registry_path.read_text(encoding="utf-8"))
            self._seq = len(self.registry["events"])
        else:
            self._initialize_meta()

    # ------------------------------------------------------------------ setup
    def _initialize_meta(self) -> None:
        """nogov 分支预置 meta.toml：conflict_sim=1.0 关闭 P2 仲裁；gov 留待 scx 默认。"""
        if self.governance:
            return
        meta = self.root / "meta.toml"
        if meta.exists():
            return
        meta.write_text(
            'schema = 2\nembed_engine = "stat-v1"\ncase_count = 0\ngen = 0\n\n'
            "[params]\nconflict_sim = 1.0\n", encoding="utf-8")

    def _save_registry(self) -> None:
        tmp = self.registry_path.with_suffix(".json.tmp")
        tmp.write_text(json.dumps(self.registry, ensure_ascii=False, indent=1),
                       encoding="utf-8")
        tmp.replace(self.registry_path)

    # ------------------------------------------------------------- CLI 调用
    def _run(self, argv: list[str], timeout: int = 120) -> tuple[int, str, str]:
        proc = subprocess.run([BIN, *argv], capture_output=True, text=True,
                              timeout=timeout)
        return proc.returncode, proc.stdout, proc.stderr

    def _write_input(self, text: str) -> Path:
        self.workdir.mkdir(parents=True, exist_ok=True)
        f = self.workdir / f"case_{self._seq:06d}.txt"
        f.write_text(text, encoding="utf-8")
        return f

    def store(self, text: str, query: str, fix: str,
              session_id: Optional[int], op: str, key: Optional[str],
              revise_id: Optional[str] = None) -> dict[str, Any]:
        """写入一个 case（gov 分支可带 --revise <旧完整 id>）。返回登记记录。"""
        inp = self._write_input(text)
        argv = [str(inp), "--query", query, "--store", "1",
                "--fix", fix[:4096], "--mem-dir", str(self.root)]
        if revise_id:
            argv += ["--revise", revise_id]
        rc, out, err = self._run(argv)
        # 成功行可能落在 stderr（与 spec §7 一致：stderr/stdout 均可能出现）。
        m = STORED_RE.search(out) or STORED_RE.search(err)
        if not m:
            raise RuntimeError(f"store failed rc={rc} session={session_id}\n"
                               f"stdout={out[-500:]}\nstderr={err[-500:]}")
        full_id = m.group(1)
        short = full_id.rsplit("_", 1)[-1]
        record = {"id": full_id, "short": short, "query": query, "fix": fix,
                  "text": text, "session_id": session_id, "op": op, "key": key,
                  "life": "active", "supersedes": revise_id or "",
                  "retire_reason": ""}
        self.registry["by_id"][full_id] = record
        if revise_id:
            old = self.registry["by_id"].get(revise_id)
            if old:
                old["life"] = "revised"
        if key:
            self.registry["active"][key] = full_id
        self._event(session_id, op, "revise" if revise_id else "store", key,
                    revise_id, full_id)
        self._save_registry()
        return record

    def retire(self, case_id: str, reason: str, session_id: Optional[int],
               op: str, key: Optional[str]) -> bool:
        rc, out, err = self._run(
            ["--retire", case_id, "--retire-reason", reason[:4096],
             "--mem-dir", str(self.root)])
        if RETIRE_RE.search(out) or RETIRE_RE.search(err) or \
                "retire: life=retired" in out or "retire: life=retired" in err:
            rec = self.registry["by_id"].get(case_id)
            if rec:
                rec["life"] = "retired"
                rec["retire_reason"] = reason
            if key and self.registry["active"].get(key) == case_id:
                del self.registry["active"][key]
            self._event(session_id, op, "retire", key, case_id, None)
            self._save_registry()
            return True
        self._event(session_id, op, "retire_failed", key, case_id, None,
                    note=err[-200:] or out[-200:])
        self._save_registry()
        return False

    # ----------------------------------------------------------- 治理动作
    def ingest_session(self, session: dict[str, Any]) -> dict[str, Any]:
        """按会话顺序摄取一个 session；返回动作审计记录。"""
        sid = session.get("session_id")
        op = session.get("operation") or "observation"
        od = session.get("operation_details") or {}
        key = stable_key(od)
        old_key = _old_pref_key(od)
        turns = [t["message"] for t in session.get("conversation", [])
                 if t.get("share_memory")]
        facts = json.dumps(od, ensure_ascii=False, default=str)
        text = self._build_text(session, turns, facts)
        query, fix = self._build_query_fix(session, od, turns)

        audit: dict[str, Any] = {"session_id": sid, "op": op, "key": key,
                                 "old_key": old_key, "action": "skipped",
                                 "target": None, "note": ""}

        # no_memory / 闲聊：无记忆事实，不入库（与官方“只沉淀记忆事实”口径一致）。
        if not (op == "observation" and not turns):
            if op == "update":
                self._ingest_update(session, sid, op, od, key, old_key, turns,
                                    text, query, fix, audit)
            elif op == "delete":
                self._ingest_delete(session, sid, op, od, key, turns, facts,
                                    text, query, fix, audit)
            else:
                # add / 其余：普通入库
                rec = self.store(text, query, fix, sid, op, key)
                audit.update(action="store", target=rec["id"])

        self.registry.setdefault("audits", []).append(audit)
        self._save_registry()
        return audit

    def _ingest_update(self, session, sid, op, od, key, old_key, turns,
                       text, query, fix, audit) -> None:
        old_id = self.registry["active"].get(key) if key else None
        old_entity_id = self.registry["active"].get(old_key) if old_key else None
        if self.governance:
            # 偏好切换（old_item）：旧实体退役，新偏好另存为新 case。
            if old_key and old_entity_id:
                reason = (f"Session {sid} ({session.get('date')}): preference "
                          f"switched away from '{od.get('old_item')}'.")
                self.retire(old_entity_id, reason, sid, op, old_key)
                audit["action"] = "gov_retire_old_switch"
                audit["target"] = old_entity_id
                if turns or key:
                    rec = self.store(text, query, fix, sid, op, key)
                    audit["note"] = f"new={rec['id']}"
                return
            if old_key and not old_entity_id:
                audit["note"] = f"old_item '{od.get('old_item')}' not active; "
            # 同一实体更新：revise 命中旧 case；未命中则新建并记 miss。
            if old_id:
                rec = self.store(text, query, fix, sid, op, key, revise_id=old_id)
                audit.update(action="gov_revise", target=old_id,
                             note=f"new={rec['id']}")
            else:
                rec = self.store(text, query, fix, sid, op, key)
                audit.update(action="revise_miss", target=None,
                             note=f"no active case for key; stored new {rec['id']}")
        else:
            # 无治理：旧 case 全部保留 active，新内容追加；旧偏好实体不退役。
            if old_key and old_entity_id:
                audit["note"] += f"obsolete old entity {old_entity_id} left active; "
            rec = self.store(text, query, fix, sid, op, key)
            audit.update(action="nogov_append", target=old_id,
                         note="old case left active; new appended")

    def _ingest_delete(self, session, sid, op, od, key, turns, facts,
                       text, query, fix, audit) -> None:
        old_id = self.registry["active"].get(key) if key else None
        if self.governance:
            if old_id:
                reason = (f"Session {sid} ({session.get('date')}): invalidated. "
                          f"{facts[:300]}")
                ok = self.retire(old_id, reason, sid, op, key)
                audit.update(action="gov_retire" if ok else "retire_failed",
                             target=old_id)
                # 治理分支不再把删除会话的转折陈述另存为 active 观察 case：
                # 否则旧实体名会经新 case 重新进入召回上下文，等同于未退役。
            else:
                audit.update(action="retire_miss", target=old_id,
                             note="no active case for key (already retired or "
                                  "duplicate delete); delete turn not stored")
        else:
            # 无治理：旧 case 不退役，保持 active；新转折追加，新旧并存。
            audit.update(action="nogov_keep_active", target=old_id,
                         note="obsolete case remains active")
            if turns:
                self.store(text, query, fix, sid, op, key)

    # --------------------------------------------------------- 文本构造
    @staticmethod
    def _build_text(session: dict[str, Any], turns: list[str],
                    facts: str) -> str:
        lines = [f"Date: {session.get('date', '')}",
                 f"Session type: {session.get('session_type', '')}",
                 f"Operation: {session.get('operation') or 'observation'}"]
        if turns:
            lines.append("User messages:")
            for msg in turns:
                lines.append(f"- {msg}")
        if facts and facts != "{}":
            lines.append(f"Structured facts: {facts}")
        return "\n".join(lines)

    @staticmethod
    def _build_query_fix(session: dict[str, Any], od: dict[str, Any],
                         turns: list[str]) -> tuple[str, str]:
        stype = session.get("session_type", "")
        op = session.get("operation") or "observation"
        date = session.get("date", "")
        item = od.get("item")
        item_str = item if isinstance(item, str) else (
            json.dumps(item, ensure_ascii=False, default=str)[:120] if item else "")
        query = f"{stype} {op} {date} {item_str}".strip()
        # 偏好类：fix 只写当前结构化偏好，不带旧偏好/旧实体名，避免过时实体经
        # 活跃 case 的摘要重新进入作答上下文（旧实体由 retire 墓碑记录）。
        if od.get("preference") is not None:
            fix = (f"User {od.get('preference')} {item} "
                   f"({od.get('subcategory', '')})")
            return query, fix
        fix_parts: list[str] = []
        if turns:
            fix_parts.append(" ".join(turns)[:600])
        elif item_str:
            fix_parts.append(str(item_str)[:600])
        fix = " | ".join(p for p in fix_parts if p) or query
        return query, fix

    # ----------------------------------------------------------- 召回/统计
    def recall(self, question: str, k: int = 50) -> list[dict[str, Any]]:
        rc, out, err = self._run(
            ["--recall-only", "--recall", str(k), "--recall-detail",
             "--mem-dir", str(self.root), "--query", question])
        hits: list[dict[str, Any]] = []
        current: Optional[dict[str, Any]] = None
        for line in out.splitlines():
            m = RECALL_RE.match(line)
            if m:
                if current:
                    hits.append(current)
                current = {"sim": float(m.group(1)), "score": float(m.group(2)),
                           "short": m.group(3), "summary": "", "fix": ""}
            elif current and line.startswith("  summary:"):
                current["summary"] = line.split("summary:", 1)[1].strip()
            elif current and line.startswith("  fix:"):
                current["fix"] = line.split("fix:", 1)[1].strip()
        if current:
            hits.append(current)
        # 用注册表补全完整文本（作答上下文用入库原文，口径更接近官方记忆记录）。
        for h in hits:
            rec = next((r for r in self.registry["by_id"].values()
                        if r["short"] == h["short"]), None)
            if rec:
                h["record"] = rec
                h["text"] = rec["text"]
                h["full_id"] = rec["id"]
            else:
                h["record"] = None
                h["text"] = h["summary"]
                h["full_id"] = h["short"]
        return hits

    def stats(self) -> dict[str, Any]:
        rc, out, err = self._run(["--stats", "--mem-dir", str(self.root)])
        info: dict[str, Any] = {"raw": out.strip()}
        for line in out.splitlines():
            if line.startswith("stats:"):
                m = re.search(r"cases=(\d+) gen=(\d+)", line)
                if m:
                    info["cases"] = int(m.group(1))
            for tag in ("life", "kind", "conflicts", "tier"):
                if line.strip().startswith(tag + ":"):
                    info[tag] = line.strip()
        info["events"] = len(self.registry["events"])
        info["active_keys"] = len(self.registry["active"])
        return info

    def _event(self, sid, op, action, key, old, new, note="") -> None:
        self._seq += 1
        self.registry["events"].append(
            {"seq": self._seq, "session_id": sid, "op": op, "action": action,
             "key": key, "old": old, "new": new, "note": note,
             "ts": time.strftime("%Y-%m-%dT%H:%M:%S%z")})
