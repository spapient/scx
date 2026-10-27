#!/usr/bin/env python3
"""Extract real coding-agent session dumps from Claude Code transcripts.

Reads the JSONL transcripts under a project's ~/.claude/projects/<slug>/ dir
and extracts the high-redundancy content (tool_use inputs: commands/file
contents; tool_result text: command stdout, logs, diffs, stack traces). The
output mirrors the mixed tool-output/log/diff/stereotype of a SuperCompress-style
coding-agent session dump, for building a real-dump compression corpus.

Only the standard library is used. Deterministic: transcripts are sorted by
basename; blocks are emitted in transcript order.
"""
import argparse
import glob
import json
import os


def tool_blocks(path):
    """Yield (kind, text) for every tool_use / tool_result content block."""
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            try:
                obj = json.loads(line)
            except (json.JSONDecodeError, UnicodeDecodeError):
                continue
            # tool_use lives in assistant messages, tool_result in user messages;
            # skip api-request-blob (a duplicate copy of both) to avoid doubling.
            if obj.get("type") not in ("user", "assistant"):
                continue
            content = (obj.get("message") or {}).get("content")
            if not isinstance(content, list):
                continue
            for block in content:
                kind = block.get("type")
                if kind == "tool_use":
                    name = block.get("name", "")
                    inp = block.get("input")
                    if isinstance(inp, dict):
                        if isinstance(inp.get("command"), str):
                            yield ("use", "command: " + inp["command"])
                        elif isinstance(inp.get("content"), str):
                            yield ("use", inp["content"])
                        else:
                            yield ("use", json.dumps(inp, ensure_ascii=False))
                    elif isinstance(inp, str):
                        yield ("use", inp)
                elif kind == "tool_result":
                    inner = block.get("content")
                    if isinstance(inner, list):
                        parts = []
                        for sub in inner:
                            if isinstance(sub, dict) and sub.get("type") == "text":
                                parts.append(sub.get("text", ""))
                            elif isinstance(sub, str):
                                parts.append(sub)
                        yield ("result", "\n".join(parts))
                    elif isinstance(inner, str):
                        yield ("result", inner)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--dir",
        default=os.path.expanduser("~/.claude/projects/-Users-fei-code-scx"),
        help="transcript dir (JSONL files)",
    )
    ap.add_argument("--scan", action="store_true", help="report per-file tool content sizes")
    ap.add_argument(
        "--extract",
        metavar="N",
        type=int,
        help="extract the top-N transcripts by tool-result chars",
    )
    ap.add_argument("--out", metavar="DIR", default=os.environ.get("TMPDIR", "/tmp"))
    args = ap.parse_args()

    files = sorted(glob.glob(os.path.join(args.dir, "*.jsonl")))
    stats = []
    for path in files:
        use_n = res_n = use_c = res_c = 0
        for kind, text in tool_blocks(path):
            if kind == "use":
                use_n += 1
                use_c += len(text)
            else:
                res_n += 1
                res_c += len(text)
        stats.append((path, use_n, res_n, use_c, res_c))
    stats.sort(key=lambda row: -row[4])

    for path, use_n, res_n, use_c, res_c in stats:
        print(
            f"{os.path.basename(path)[:12]:14s} tool_use={use_n:4d} "
            f"tool_result={res_n:4d} use_chars={use_c:9d} result_chars={res_c:9d}"
        )

    if args.extract:
        os.makedirs(args.out, exist_ok=True)
        for path, _, _, _, _ in stats[: args.extract]:
            base = os.path.splitext(os.path.basename(path))[0]
            out = os.path.join(args.out, base + ".dump.txt")
            with open(out, "w", encoding="utf-8") as writer:
                for _, text in tool_blocks(path):
                    writer.write(text)
                    if text and not text.endswith("\n"):
                        writer.write("\n")
            n_lines = sum(1 for _ in open(out, encoding="utf-8"))
            print(f"  -> {out} ({n_lines} lines)")


if __name__ == "__main__":
    main()
