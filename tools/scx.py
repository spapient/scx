#!/usr/bin/env python3
"""One tool for scx corpus replay, blind review, and paired statistics.

Only the Python standard library is used. The Rust binary remains the single
implementation of retrieval and gates; this tool only prepares inputs and
analyses the deterministic `--eval-verbose` output.

Commands:
  seed       corpus JSON -> memory cases + eval CSV
  baseline   eval CSV -> mechanical target baseline JSON
  stats      baseline + verbose (+ memory for strata) -> significance CSV
  blind      make/import/check/progress/review a frozen per-candidate packet
  verify     check a packet hash and its row count
  selftest   dependency-free assertions over the shared helpers
"""

import argparse
import csv
import hashlib
import json
import math
import os
import random
import re
import shutil
import subprocess
import sys
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CONFIG_DIR = ROOT / "config"
LAYERS = ("literal", "paraphrase", "semantic")
GRADE_HELP = "2=同事故/同根因或直接修复  1=同故障族/部分有用  0=无关"
PACKET_HEAD = ("row", "case_index", "layer", "query", "candidate", "hint", "grade")
# Local store writes are millisecond I/O; beyond 32 workers the cross-process
# lock and process startup dominate, while resource spikes grow unchecked.
MAX_JOBS = 32
STOP = {
    "the", "a", "an", "of", "to", "and", "in", "on", "for", "with", "by",
    "from", "after", "before", "was", "were", "is", "are", "be", "that",
    "this", "it", "as", "at", "into", "during", "caused", "cause", "causing",
}


# ---------------------------------------------------------------------------
# Shared I/O and identity helpers
# ---------------------------------------------------------------------------


def load_json(path):
    try:
        with open(path, encoding="utf-8") as handle:
            return json.load(handle)
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        sys.exit(f"{path}: {error}")


def dump_json(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    with open(tmp, "w", encoding="utf-8", newline="\n") as handle:
        json.dump(value, handle, ensure_ascii=False, indent=1)
        handle.write("\n")
    os.replace(tmp, path)


def write_csv(path, fields, rows):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    fields = list(fields)
    for row in rows:
        for key in row:
            if key not in fields:
                fields.append(key)
    tmp = path.with_suffix(path.suffix + ".tmp")
    with open(tmp, "w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(
            handle, fieldnames=fields, lineterminator="\n", restval=""
        )
        writer.writeheader()
        writer.writerows(rows)
    os.replace(tmp, path)


def read_csv(path):
    try:
        with open(path, encoding="utf-8-sig", newline="") as handle:
            return list(csv.DictReader(handle))
    except (OSError, UnicodeError, csv.Error) as error:
        sys.exit(f"{path}: {error}")


def is_line(value):
    return not any(char in value for char in "\r\n\t\0")


def require_line(label, value):
    if not is_line(value):
        sys.exit(f"{label}: must be one line without tabs or NUL")


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def sha256_text(text):
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def tail_of(key):
    return key.rsplit("_", 1)[-1]


def id_matches(exp, case_id):
    tail = tail_of(case_id)
    tail8 = tail[-8:]
    return (
        exp == case_id
        or exp == tail
        or exp == tail8
        or tail_of(exp) == tail
    )


def rel_of(case, case_id):
    rel = case.get("rel") or {}
    if case_id in rel:
        return float(rel[case_id])
    tail = tail_of(case_id)
    if tail in rel:
        return float(rel[tail])
    tail8 = tail[-8:]
    if tail8 in rel:
        return float(rel[tail8])
    for key in sorted(rel):
        if id_matches(key, case_id):
            return float(rel[key])
    return 0.0


def clean(text, limit=220):
    text = " ".join(str(text).replace("\t", " ").replace("\r", " ").split())
    return text[:limit]


# ---------------------------------------------------------------------------
# Corpus seeding and baseline construction
# ---------------------------------------------------------------------------


def default_binary():
    names = ("scx.exe", "scx")
    # Cargo 的 release 产物是本 checkout 唯一规范产物；源码树内不再拷贝二进制。
    for base in (ROOT / "target" / "release",):
        for name in names:
            path = base / name
            if path.is_file():
                return str(path)
    return ""


def _inside(path, parent):
    try:
        Path(path).resolve().relative_to(Path(parent).resolve())
        return True
    except ValueError:
        return False


def is_memory_dir(path):
    path = Path(path)
    return (path / "meta.toml").is_file() and (path / "cases").is_dir()


def cleanup_roots():
    roots = {ROOT.resolve(), Path(tempfile.gettempdir()).resolve()}
    # macOS resolves /tmp to /private/tmp while gettempdir() uses the user's
    # per-process DARWIN_USER_TEMP_DIR; both are standard scratch locations.
    if os.name != "nt":
        roots.add(Path("/tmp").resolve())
    return roots


def prepare_mem_dir(mem_dir, force):
    path = Path(mem_dir).resolve()
    if path.exists() and not path.is_dir():
        sys.exit(f"seed: memory path is not a directory: {path}")
    if path.exists() and any(path.iterdir()):
        if not force:
            sys.exit(f"seed: memory dir is not empty: {path} (use --force)")
        roots = cleanup_roots()
        if path in roots or any(_inside(root, path) for root in roots):
            sys.exit(f"seed: refusing to clear a workspace or temp root: {path}")
        if not is_memory_dir(path):
            sys.exit(
                f"seed: refusing to clear a non-memory directory: {path} "
                "(remove it manually)"
            )
        if not any(_inside(path, root) for root in roots):
            sys.exit(f"seed: refusing to clear outside workspace/temp roots: {path}")
        shutil.rmtree(path)
    path.mkdir(parents=True, exist_ok=True)
    return path


def validate_corpus(corpus):
    incidents = corpus.get("incidents") or []
    if len(incidents) != corpus.get("expected_incidents", len(incidents)):
        sys.exit(
            f"corpus: incidents={len(incidents)} "
            f"expected={corpus.get('expected_incidents')}"
        )
    seen_ids, seen_store, seen_eval = set(), set(), set()
    layers = {layer: 0 for layer in LAYERS}
    total = 0
    for incident in incidents:
        if not isinstance(incident, dict):
            sys.exit("corpus: every incident must be an object")
        ident = str(incident.get("id") or "").strip()
        source = str(incident.get("source") or "").strip()
        root_cause = str(incident.get("root_cause") or "").strip()
        fix = str(incident.get("fix") or "").strip()
        if not ident or ident in seen_ids:
            sys.exit(f"corpus: missing or duplicate incident id: {ident!r}")
        if not source.startswith(("http://", "https://")):
            sys.exit(f"corpus: incident {ident} has no public source URL")
        if not root_cause or not fix:
            sys.exit(f"corpus: incident {ident} has empty root_cause/fix")
        for label, value in (
            ("id", ident),
            ("source", source),
            ("root_cause", root_cause),
            ("fix", fix),
        ):
            require_line(f"corpus: incident {ident} {label}", value)
        seen_ids.add(ident)
        cases = incident.get("cases") or []
        if not cases:
            sys.exit(f"corpus: incident {ident} has no cases")
        for case in cases:
            if not isinstance(case, dict):
                sys.exit(f"corpus: incident {ident} case must be an object")
            layer = str(case.get("layer") or "")
            store_query = str(case.get("store_query") or "").strip()
            eval_query = str(case.get("eval_query") or "").strip()
            text_query = str(case.get("text_query") or store_query).strip()
            if layer not in layers:
                sys.exit(f"corpus: incident {ident} has bad layer: {layer!r}")
            if not store_query or not eval_query or not text_query:
                sys.exit(f"corpus: incident {ident} has an empty query")
            for label, value in (
                ("store_query", store_query),
                ("eval_query", eval_query),
                ("text_query", text_query),
            ):
                require_line(f"corpus: incident {ident} {label}", value)
            normalized = store_query.casefold()
            if normalized in seen_store:
                sys.exit(f"corpus: duplicate stored query: {store_query!r}")
            normalized_eval = eval_query.casefold()
            if normalized_eval in seen_eval:
                sys.exit(f"corpus: duplicate eval query: {eval_query!r}")
            seen_store.add(normalized)
            seen_eval.add(normalized_eval)
            layers[layer] += 1
            total += 1
    if total != corpus.get("expected_cases", total):
        sys.exit(f"corpus: cases={total} expected={corpus.get('expected_cases')}")
    return layers


def corpus_cases(corpus):
    for incident in corpus["incidents"]:
        for case in incident["cases"]:
            yield incident, case


def seed_case(binary, mem_dir, incident, case):
    store_query = str(case["store_query"]).strip()
    text_query = str(case.get("text_query") or store_query).strip()
    source = str(incident["source"]).strip()
    root_cause = str(incident["root_cause"]).strip()
    text = (
        f"{text_query}\n"
        f"公开复盘, 来源: {source}\n"
        f"根因: {root_cause}"
    )
    # Bytes on stdin avoid platform newline translation and per-case temp files.
    # Corpus identity therefore stays byte-stable on Linux, macOS, and Windows.
    proc = subprocess.run(
        [
            binary, "--query", store_query, "--store", "1",
            "--fix", str(incident["fix"]).strip(), "--mem-dir", str(mem_dir),
        ],
        input=text.encode("utf-8"),
        capture_output=True,
    )
    output = (proc.stdout + proc.stderr).decode("utf-8", "replace")
    match = re.search(r"stored case=(\S+) added=yes", output)
    if proc.returncode != 0 or not match:
        raise RuntimeError(f"seed: store failed for {case['store_query']!r}\n{output}")
    case_id = match.group(1)
    return {
        "query": case["eval_query"],
        "layer": case["layer"],
        "case_id": case_id,
        "note": f"target={tail_of(case_id)} source={incident['source']}",
    }


def seed_corpus(corpus, mem_dir, binary, force, jobs=1):
    layers = validate_corpus(corpus)
    binary = binary or default_binary()
    if not binary or not Path(binary).is_file():
        sys.exit("seed: missing release binary; run `./tools/scx --version` first")
    mem_dir = prepare_mem_dir(mem_dir, force)
    jobs = int(jobs)
    items = list(corpus_cases(corpus))
    if not items:
        sys.exit("seed: corpus has no cases")

    def run(item):
        incident, case = item
        return seed_case(binary, mem_dir, incident, case)

    try:
        with ThreadPoolExecutor(max_workers=min(jobs, len(items))) as pool:
            rows = list(pool.map(run, items))
    except (OSError, RuntimeError) as error:
        sys.exit(str(error))
    return rows, layers


def cmd_seed(args):
    if not 1 <= args.jobs <= MAX_JOBS:
        sys.exit(f"seed: --jobs must be in 1..={MAX_JOBS}")
    corpus = load_json(args.corpus)
    rows, layers = seed_corpus(
        corpus, args.mem_dir, args.binary or default_binary(), args.force, args.jobs
    )
    if args.csv:
        write_csv(args.csv, ("query", "layer", "case_id", "note"), rows)
    print(
        f"seed: corpus={corpus.get('name')} cases={len(rows)} "
        f"literal={layers['literal']} paraphrase={layers['paraphrase']} "
        f"semantic={layers['semantic']} jobs={args.jobs} mem={args.mem_dir}"
    )
    if args.csv:
        print(f"seed: wrote {args.csv}")


def cmd_baseline(args):
    rows = read_csv(args.csv)
    if not rows:
        sys.exit(f"baseline: no data rows: {args.csv}")
    cases, seen_ids = [], set()
    counts = {layer: 0 for layer in LAYERS}
    for number, row in enumerate(rows, 2):
        query = (row.get("query") or "").strip()
        layer = (row.get("layer") or "").strip().lower()
        case_id = (row.get("case_id") or "").strip()
        if not query or not case_id or layer not in counts:
            sys.exit(f"baseline: bad row {number}")
        require_line(f"baseline: row {number} query", query)
        if case_id in seen_ids:
            sys.exit(f"baseline: duplicate case_id: {case_id}")
        seen_ids.add(case_id)
        note = (row.get("note") or "").strip()
        cases.append(
            {
                "query": query,
                "layer": layer,
                "expected": [case_id],
                "rel": {case_id: 2},
                "judge": {"by": args.judge_by, "date": args.judge_date},
                "note": "target-only" + (f"; {note}" if note else ""),
            }
        )
        counts[layer] += 1
    dump_json(args.out, {"cases": cases})
    print(
        f"baseline: wrote {args.out} cases={len(cases)} "
        f"literal={counts['literal']} paraphrase={counts['paraphrase']} "
        f"semantic={counts['semantic']}"
    )
    if (
        len(cases) < 30
        or any(counts[layer] < 10 for layer in LAYERS)
    ):
        print("baseline: WARNING not gate-ready (need total>=30 and each layer>=10)")


# ---------------------------------------------------------------------------
# Verbose parsing, strata, and paired statistics
# ---------------------------------------------------------------------------


def parse_verbose(path, expected_cases):
    try:
        text = Path(path).read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeError) as error:
        sys.exit(f"{path}: {error}")
    rows = []
    current = None
    for line in text:
        match = re.match(r'case (\d+)/\d+ q="(.*)"', line)
        if match:
            current = {"A": None, "B": None}
            rows.append(current)
            continue
        match = re.match(r"  ([AB]): (.*)", line)
        if match and current is not None:
            if current[match.group(1)] is not None:
                sys.exit(f"stats: duplicate {match.group(1)} line in {path}")
            current[match.group(1)] = re.findall(r"\) (\S+) sim=", match.group(2))
            continue
        if line.startswith(("case ", "  A: ", "  B: ")):
            sys.exit(f"stats: malformed verbose line in {path}: {line!r}")
    if len(rows) != expected_cases:
        sys.exit(f"stats: verbose cases={len(rows)} baseline cases={expected_cases}")
    for index, row in enumerate(rows, 1):
        for side in ("A", "B"):
            if row[side] is None:
                sys.exit(f"stats: case {index} missing {side} line in {path}")
            if not row[side]:
                row[side] = []
    return rows


def target_rank(ids, target):
    tail = tail_of(target)
    for index, case_id in enumerate(ids[:10]):
        if case_id == target or case_id == tail or case_id[-8:] == tail[-8:]:
            return index + 1
    return 0


def mrr(rank):
    return 1.0 / rank if rank else 0.0


def dcg(rels):
    return sum(rel / math.log2(i + 2.0) for i, rel in enumerate(rels))


def ndcg(case, ids):
    observed = [rel_of(case, case_id) for case_id in ids[:10]]
    ideal = sorted(
        (float(v) for v in (case.get("rel") or {}).values() if float(v) > 0.0),
        reverse=True,
    )[:10]
    ideal_dcg = dcg(ideal)
    return dcg(observed) / ideal_dcg if ideal_dcg > 0.0 else 0.0


def tokens(text):
    out = set()
    for match in re.finditer(r"[a-z0-9][a-z0-9_\-.]*", text.lower()):
        token = match.group(0).strip(".-_")
        if len(token) >= 2 and token not in STOP:
            out.add(token)
    for match in re.finditer(r"[\u4e00-\u9fff]+", text):
        run = match.group(0)
        for index, char in enumerate(run):
            out.add(char)
            if index + 1 < len(run):
                out.add(run[index : index + 2])
    return out


def jaccard(left, right):
    a, b = tokens(left), tokens(right)
    if not a and not b:
        return 0.0
    union = len(a) + len(b) - len(a & b)
    return len(a & b) / union if union else 0.0


def tier_of(value):
    if value > 0.5:
        return "easy"
    if value >= 0.2:
        return "medium"
    return "hard"


def load_memory_cases(mem_dir, command):
    root = Path(mem_dir) / "cases"
    if not root.is_dir():
        sys.exit(f"{command}: missing memory cases dir: {root}")
    by_full, by_tail = {}, {}
    for directory in sorted(root.iterdir()):
        path = directory / "case.json"
        if not path.is_file():
            continue
        case = load_json(path)
        ident = str(case.get("id") or directory.name)
        entry = (
            ident,
            str(case.get("query") or ""),
            str(case.get("fix") or ""),
        )
        by_full[ident] = entry
        by_tail[tail_of(ident)] = entry
    return by_full, by_tail


def strata_for(cases, mem_dir, strata_csv=None):
    if strata_csv:
        tiers = {}
        for row in read_csv(strata_csv):
            try:
                index = int(row["case_index"])
            except (KeyError, ValueError):
                sys.exit(f"stats: bad strata case_index: {row.get('case_index')!r}")
            tier = (row.get("tier") or "").strip()
            if not 0 <= index < len(cases):
                sys.exit(f"stats: strata case_index out of range: {index}")
            if index in tiers:
                sys.exit(f"stats: duplicate strata case_index: {index}")
            if tier not in {"easy", "medium", "hard"}:
                sys.exit(f"stats: bad tier for case {index}: {tier!r}")
            tiers[index] = tier
        missing = sorted(set(range(len(cases))) - set(tiers))
        if missing:
            sys.exit(f"stats: strata missing case indexes: {missing[:8]}")
        return tiers
    if not mem_dir:
        sys.exit("stats: --mem-dir or --strata is required")
    by_full, by_tail = load_memory_cases(mem_dir, "stats")
    tiers = {}
    for index, case in enumerate(cases):
        target = str(case["expected"][0])
        hit = by_full.get(target) or by_tail.get(tail_of(target))
        if hit is None:
            sys.exit(f"stats: target {target} not found in memory dir {mem_dir}")
        query = hit[1]
        tiers[index] = tier_of(jaccard(str(case["query"]), query))
    return tiers


def ci_bounds(samples, boot):
    samples.sort()
    return samples[int(0.025 * (boot - 1))], samples[int(0.975 * (boot - 1))]


def result_row(name, rows, lo, hi, p_value, metric, mde, n_clusters=None):
    n = len(rows)
    observed = sum(row["a"] - row["b"] for row in rows) / n
    row = {
        "group": name,
        "n": n,
        f"A_{metric}": round(sum(row["a"] for row in rows) / n, 4),
        f"B_{metric}": round(sum(row["b"] for row in rows) / n, 4),
        "delta": round(observed, 4),
        "ci_lo": round(lo, 4),
        "ci_hi": round(hi, 4),
        "perm_p": round(p_value, 4),
        "mde": mde,
        "advantage": int(lo > 0 and observed >= mde and p_value < 0.05),
    }
    if n_clusters is not None:
        row["n_clusters"] = n_clusters
    return row


def analyze(name, rows, boot, perm, seed, mde, metric):
    if not rows:
        return None
    deltas = [row["a"] - row["b"] for row in rows]
    n = len(deltas)
    observed = sum(deltas) / n
    rng = random.Random(seed)
    means = [
        sum(deltas[rng.randrange(n)] for _ in range(n)) / n
        for _ in range(boot)
    ]
    lo, hi = ci_bounds(means, boot)
    hits = sum(
        1
        for _ in range(perm)
        if abs(sum(x if rng.random() >= 0.5 else -x for x in deltas) / n)
        >= abs(observed)
    )
    p_value = (hits + 1.0) / (perm + 1.0)
    return result_row(name, rows, lo, hi, p_value, metric, mde)


def analyze_cluster(rows, cluster_size, boot, perm, seed, mde, metric):
    groups = {}
    for row in rows:
        groups.setdefault(row["i"] // cluster_size, []).append(row)
    clusters = list(groups.values())
    if len(clusters) < 2:
        return None
    deltas = [row["a"] - row["b"] for row in rows]
    observed = sum(deltas) / len(deltas)
    rng = random.Random(seed)
    means = []
    for _ in range(boot):
        sample = [clusters[rng.randrange(len(clusters))] for _ in clusters]
        values = [row["a"] - row["b"] for cluster in sample for row in cluster]
        means.append(sum(values) / len(values))
    lo, hi = ci_bounds(means, boot)
    hits = 0
    for _ in range(perm):
        total = count = 0
        for cluster in clusters:
            sign = 1.0 if rng.random() >= 0.5 else -1.0
            for row in cluster:
                total += sign * (row["a"] - row["b"])
                count += 1
        if abs(total / count) >= abs(observed):
            hits += 1
    p_value = (hits + 1.0) / (perm + 1.0)
    return result_row(
        "tier:hard:cluster", rows, lo, hi, p_value, metric, mde, len(clusters)
    )


def cmd_stats(args):
    if args.boot < 1 or args.perm < 1:
        sys.exit("stats: --boot and --perm must be positive")
    if not math.isfinite(args.mde) or not 0.0 <= args.mde <= 1.0:
        sys.exit("stats: --mde must be a finite value in [0, 1]")
    if args.cluster_size < 0:
        sys.exit("stats: --cluster-size must be non-negative")
    baseline = load_json(args.baseline)
    cases = baseline.get("cases") or []
    if not cases:
        sys.exit("stats: baseline has no cases")
    for index, case in enumerate(cases):
        if not case.get("expected"):
            sys.exit(f"stats: case {index} has no expected target")
    verbose = parse_verbose(args.verbose, len(cases))
    tiers = strata_for(cases, args.mem_dir, args.strata)
    rows = []
    for index, (case, hits) in enumerate(zip(cases, verbose)):
        target = str(case["expected"][0])
        if args.metric == "mrr":
            left = mrr(target_rank(hits["A"], target))
            right = mrr(target_rank(hits["B"], target))
        else:
            left, right = ndcg(case, hits["A"]), ndcg(case, hits["B"])
        rows.append(
            {
                "i": int(case.get("case_index", index)),
                "layer": case["layer"],
                "tier": tiers.get(index, ""),
                "a": left,
                "b": right,
            }
        )
    def measure(name, group):
        return analyze(
            name, group, args.boot, args.perm, args.seed, args.mde, args.metric
        )

    results = []
    for layer in LAYERS:
        group = [row for row in rows if row["layer"] == layer]
        if group:
            results.append(measure(layer, group))
    for tier in sorted({row["tier"] for row in rows if row["tier"]}):
        group = [row for row in rows if row["tier"] == tier]
        results.append(measure(f"tier:{tier}", group))
    results.append(measure("all", rows))
    if args.cluster_size:
        hard = [row for row in rows if row["tier"] == "hard"]
        cluster = analyze_cluster(
            hard,
            args.cluster_size,
            args.boot,
            args.perm,
            args.seed,
            args.mde,
            args.metric,
        )
        if cluster:
            results.append(cluster)
    results = [row for row in results if row]
    fields = list(results[0].keys())
    write_csv(args.out, fields, results)
    for row in results:
        if "n_clusters" not in row:
            a_key, b_key = f"A_{args.metric}", f"B_{args.metric}"
            print(
                f"{row['group']:14s} n={row['n']:3d} A={row[a_key]:.4f} "
                f"B={row[b_key]:.4f} d={row['delta']:+.4f} "
                f"CI=[{row['ci_lo']:+.4f},{row['ci_hi']:+.4f}] p={row['perm_p']:.4f}"
            )
        else:
            print(
                f"{row['group']:14s} n={row['n']:3d} "
                f"clusters={row['n_clusters']:3d} d={row['delta']:+.4f} "
                f"CI=[{row['ci_lo']:+.4f},{row['ci_hi']:+.4f}] p={row['perm_p']:.4f}"
            )
    print(f"stats: wrote {args.out} groups={len(results)} metric={args.metric}")


# ---------------------------------------------------------------------------
# Blind review
# ---------------------------------------------------------------------------


def read_tsv(path):
    try:
        with open(path, encoding="utf-8") as handle:
            head = handle.readline().rstrip("\r\n").split("\t")
            if tuple(head) != PACKET_HEAD:
                sys.exit(f"blind: bad header: {head}")
            rows = [line.rstrip("\r\n").split("\t") for line in handle if line.strip()]
    except OSError as error:
        sys.exit(f"{path}: {error}")
    for number, row in enumerate(rows, 2):
        if len(row) != len(head):
            sys.exit(f"blind: row {number} has {len(row)} columns, expected {len(head)}")
    return head, rows


def tsv_text(head, rows):
    lines = ["\t".join(head)]
    lines.extend("\t".join(str(value) for value in row) for row in rows)
    return "\n".join(lines) + "\n"


def write_text(path, text):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    with open(tmp, "w", encoding="utf-8", newline="") as handle:
        handle.write(text)
    os.replace(tmp, path)


def read_packet(path):
    head, rows = read_tsv(path)
    return [dict(zip(head, row)) for row in rows]


def filled_packet(head, rows, cases):
    rel_by_case = {}
    for index, case in enumerate(cases):
        case_index = int(case.get("case_index", index))
        if case_index in rel_by_case:
            sys.exit(f"blind: duplicate label case_index: {case_index}")
        rel_by_case[case_index] = case.get("rel") or {}
    filled = []
    for number, row in enumerate(rows, 2):
        fields = dict(zip(head, row))
        try:
            grade = float(rel_by_case[int(fields["case_index"])][fields["candidate"]])
        except (KeyError, TypeError, ValueError):
            sys.exit(f"blind: packet row {number} cannot be materialized from labels")
        if grade not in (0.0, 1.0, 2.0):
            sys.exit(f"blind: packet row {number} grade {grade} outside 0..2")
        fields["grade"] = str(int(grade))
        filled.append([fields.get(key, "") for key in head])
    return tsv_text(head, filled)


def cmd_blind_make(args):
    baseline = load_json(args.baseline)
    cases = baseline["cases"]
    by_layer = {}
    for index, case in enumerate(cases):
        by_layer.setdefault(case["layer"], []).append(index)
    picks = []
    for layer, indexes in by_layer.items():
        want = args.per_layer.get(layer, 0)
        if want > len(indexes):
            sys.exit(f"blind: layer {layer} has {len(indexes)} cases < {want}")
        ranked = sorted(indexes, key=lambda i: sha256_text(f"{args.seed}:{layer}:{i}"))
        picks.extend(ranked[:want])
    picks.sort(key=lambda i: sha256_text(f"{args.seed}:case:{i}"))
    _, memory = load_memory_cases(args.mem_dir, "blind")
    rows, manifest_cases = [], []
    for case_index_value in picks:
        case = cases[case_index_value]
        keys = list((case.get("rel") or {}).keys())
        if not keys:
            sys.exit(f"blind: case {case_index_value} has empty rel pool")
        ordered = sorted(
            keys, key=lambda key: sha256_text(f"{args.seed}:{case_index_value}:{key}")
        )
        filled = []
        for key in ordered:
            hit = memory.get(tail_of(key))
            if hit is None:
                sys.exit(
                    f"blind: candidate {key} (case {case_index_value}) missing in memory"
                )
            full, query, fix = hit
            hint = query + (f" || fix: {fix}" if fix else "")
            filled.append((key, full, clean(hint)))
        manifest_cases.append(
            {
                "case_index": case_index_value,
                "layer": case["layer"],
                "candidates": len(filled),
            }
        )
        for key, _full, hint in filled:
            rows.append(
                (case_index_value, case["layer"], clean(case["query"]), key, hint)
            )
    write_text(
        args.out,
        tsv_text(
            PACKET_HEAD,
            [
                (number, ci, layer, query, key, hint, "")
                for number, (ci, layer, query, key, hint) in enumerate(rows, 1)
            ],
        ),
    )
    manifest = {
        "name": args.name,
        "seed": args.seed,
        "baseline": str(Path(args.baseline).resolve()),
        "baseline_sha256": sha256_file(args.baseline),
        "memory_dir": str(Path(args.mem_dir).resolve()),
        "packet": str(Path(args.out).resolve()),
        "packet_sha256": sha256_file(args.out),
        "protocol": GRADE_HELP,
        "cases": manifest_cases,
        "sample_cases": len(manifest_cases),
        "total_cases": len(cases),
        "judgments": len(rows),
    }
    dump_json(args.manifest, manifest)
    print(
        f"blind: cases={len(manifest_cases)}/{len(cases)} judgments={len(rows)} "
        f"packet={args.out}"
    )
    print(f"blind: packet_sha256={manifest['packet_sha256']}")
    print(f"blind: {GRADE_HELP}")


def parse_grades(rows, allow_partial):
    if not rows:
        sys.exit("blind: empty packet")
    grades = {}
    for row in rows:
        grade = (row.get("grade") or "").strip()
        if not grade and allow_partial:
            continue
        if grade not in {"0", "1", "2"}:
            if grade:
                sys.exit(f"blind: row {row['row']} grade={grade!r} must be 0, 1, or 2")
            sys.exit(
                f"blind: row {row['row']} has an empty grade "
                "(fill every row or pass --allow-partial)"
            )
        key = (int(row["case_index"]), row["candidate"])
        if key in grades:
            sys.exit(f"blind: duplicate row for case {key[0]} candidate {key[1]}")
        grades[key] = float(grade)
    if not grades:
        sys.exit("blind: no graded rows")
    return grades


def cmd_blind_import(args):
    head, table = read_tsv(args.packet)
    rows = [dict(zip(head, row)) for row in table]
    grades = parse_grades(rows, args.allow_partial)
    baseline = load_json(args.baseline)
    cases = baseline["cases"]
    # Canonicalize line endings so a Windows/Excel CRLF copy and the tracked
    # LF packet share one provenance hash, while verify still checks raw bytes.
    packet_sha = sha256_text(tsv_text(head, table))
    by_case = {}
    for (case_index_value, candidate), grade in grades.items():
        if not 0 <= case_index_value < len(cases):
            sys.exit(f"blind: case_index out of range: {case_index_value}")
        by_case.setdefault(case_index_value, {})[candidate] = grade
    output, violations = [], []
    for case_index_value in sorted(by_case):
        case = cases[case_index_value]
        if not case.get("expected"):
            sys.exit(f"blind: case {case_index_value} has no expected target")
        pool = set((case.get("rel") or {}).keys())
        got = set(by_case[case_index_value])
        if got != pool:
            sys.exit(
                f"blind: case {case_index_value} coverage mismatch "
                f"missing={sorted(pool - got)[:3]} extra={sorted(got - pool)[:3]}"
            )
        rel = {key: by_case[case_index_value][key] for key in case["rel"]}
        target = tail_of(case["expected"][0])
        target_grade = next(
            (value for key, value in rel.items() if tail_of(key) == target), None
        )
        if case["layer"] == "literal" and target_grade != 2.0:
            violations.append((case_index_value + 1, target[:8], target_grade))
        new_case = {
            "query": case["query"],
            "layer": case["layer"],
            "expected": case["expected"],
            "case_index": case_index_value,
            "rel": rel,
            "judge": {
                "by": args.judge,
                "date": args.date,
                "method": args.method,
                "packet_sha256": packet_sha,
                "packet": str(Path(args.packet).resolve()),
            },
            "judge_items": {key: {"by": args.judge, "date": args.date} for key in rel},
            "note": f"per-item blind review; packet={Path(args.packet).name}",
        }
        if args.note:
            new_case["note"] += f"; {args.note}"
        output.append(new_case)
    if violations:
        for case_number, target, grade in violations[:8]:
            print(
                f"blind: literal case {case_number} target {target} graded {grade} (must be 2)",
                file=sys.stderr,
            )
        sys.exit(
            f"blind: {len(violations)} literal self-match rows are not 2; nothing written"
        )
    dump_json(args.out, {"name": args.name, "provenance": "per-item-blind-v1", "cases": output})
    counts = {2: 0, 1: 0, 0: 0}
    for case in output:
        for value in case["rel"].values():
            if value in counts:
                counts[int(value)] += 1
    print(
        f"blind: wrote {args.out} cases={len(output)} "
        f"items={sum(counts.values())} rel2={counts[2]} rel1={counts[1]} rel0={counts[0]}"
    )


def cmd_blind_check(args):
    baseline = load_json(args.baseline)
    bad = items = 0
    for index, case in enumerate(baseline["cases"]):
        pool = set((case.get("rel") or {}).keys())
        got = set((case.get("judge_items") or {}).keys())
        items += len(got)
        if pool != got:
            bad += 1
            print(
                f"blind: case {index} uncovered={len(pool - got)} "
                f"extra={len(got - pool)}"
            )
    print(f"blind: cases={len(baseline['cases'])} items={items} incomplete={bad}")
    failed = bad > 0
    if args.out and not args.packet:
        sys.exit("blind: --out requires --packet")
    if args.packet:
        head, rows = read_tsv(args.packet)
        body = filled_packet(head, rows, baseline["cases"])
        actual = sha256_text(body)
        recorded = set()
        for case in baseline["cases"]:
            judge = case.get("judge") or {}
            if judge.get("packet_sha256"):
                recorded.add(str(judge["packet_sha256"]))
        if len(recorded) > 1:
            sys.exit("blind: labels disagree on packet_sha256")
        expected = next(iter(recorded), "")
        if expected and actual.lower() != expected.lower():
            print(
                f"blind: materialized packet drifted: {actual} != {expected}",
                file=sys.stderr,
            )
            failed = True
        if args.out:
            write_text(args.out, body)
        print(f"blind: materialized rows={len(rows)} sha256={actual}")
    return 1 if failed else 0


def cmd_blind_progress(args):
    rows = read_packet(args.packet)
    done = sum(1 for row in rows if (row.get("grade") or "").strip() in {"0", "1", "2"})
    print(f"blind: graded={done}/{len(rows)} remaining={len(rows) - done}")
    return 0 if done == len(rows) else 1


def agreement(cases_a, cases_b):
    if len(cases_a) != len(cases_b):
        sys.exit(f"blind: case counts differ: {len(cases_a)} vs {len(cases_b)}")
    matrix = [[0, 0, 0] for _ in range(3)]
    disagreements = []
    total = agree = 0
    for index, (left, right) in enumerate(zip(cases_a, cases_b), 1):
        rel_left = left.get("rel") or {}
        rel_right = right.get("rel") or {}
        for candidate, left_value in rel_left.items():
            if candidate not in rel_right:
                continue
            try:
                a = int(left_value)
                b = int(rel_right[candidate])
            except (TypeError, ValueError):
                sys.exit(f"blind: non-integer grade at case {index} candidate {candidate}")
            if a not in range(3) or b not in range(3):
                sys.exit(f"blind: grade outside 0..2 at case {index} candidate {candidate}")
            matrix[a][b] += 1
            total += 1
            if a == b:
                agree += 1
            else:
                disagreements.append((index, left.get("query", ""), candidate, a, b))
    if not total:
        return 0, 0.0, 0.0, matrix, disagreements
    raw = agree / total
    rows = [sum(row) for row in matrix]
    columns = [sum(matrix[a][b] for a in range(3)) for b in range(3)]
    expected = sum(rows[i] * columns[i] for i in range(3)) / (total * total)
    kappa = 1.0 if expected >= 1.0 else (raw - expected) / (1.0 - expected)
    return total, raw, kappa, matrix, disagreements


def cmd_blind_agree(args):
    left = load_json(args.a).get("cases") or []
    right = load_json(args.b).get("cases") or []
    total, raw, kappa, matrix, disagreements = agreement(left, right)
    if not total:
        print("blind: no common adjudicated candidates")
        return 0
    off_diag = sum(matrix[a][b] for a in range(3) for b in range(3) if a != b)
    print(f"blind: common={total} raw={raw:.3f} kappa={kappa:.3f}")
    print(
        f"  matrix(rows=A,cols=B) 2,2={matrix[2][2]} 1,1={matrix[1][1]} "
        f"0,0={matrix[0][0]} off-diag={off_diag}"
    )
    print("  disagreements:" if disagreements else "  disagreements: 0")
    for index, query, candidate, a, b in disagreements:
        print(f'    case={index} q="{query}" cand={candidate} A={a} B={b}')
    return 0


def cmd_blind_review(args):
    head, rows = read_tsv(args.packet)
    columns = {key: index for index, key in enumerate(head)}
    grade_index = columns["grade"]
    todo = [row for row in rows if not row[grade_index].strip()]
    print(f"blind: {len(rows) - len(todo)}/{len(rows)} already graded; {len(todo)} to go")
    print(f"blind: {GRADE_HELP}  (s=skip  q=save&quit)")
    dirty = False
    try:
        for row in todo:
            print(
                f"\n[row {row[columns['row']]}/{len(rows)}] "
                f"layer={row[columns['layer']]}\nquery: {row[columns['query']]}"
                f"\nhint:  {row[columns['hint']]}"
            )
            while True:
                answer = input("grade? [2/1/0/s/q] > ").strip().lower()
                if answer == "q":
                    raise KeyboardInterrupt
                if answer in {"2", "1", "0"}:
                    row[grade_index] = answer
                    dirty = True
                    break
                if answer == "s":
                    break
            if dirty:
                write_text(args.packet, tsv_text(head, rows))
                dirty = False
    except (KeyboardInterrupt, EOFError):
        pass
    graded = sum(1 for row in rows if row[grade_index].strip() in {"0", "1", "2"})
    print(f"\nblind: graded={graded}/{len(rows)} (saved)")
    return 0 if graded == len(rows) else 1


def cmd_blind(args):
    handlers = {
        "make": cmd_blind_make,
        "import": cmd_blind_import,
        "check": cmd_blind_check,
        "progress": cmd_blind_progress,
        "agree": cmd_blind_agree,
        "review": cmd_blind_review,
    }
    return handlers[args.blind_cmd](args)


# ---------------------------------------------------------------------------
# Evidence containment (contain5): query-aware compression vs head truncation
# ---------------------------------------------------------------------------


def contain_token_count(text):
    """Deterministic word-volume estimate (NOT a real LLM tokenizer).

    ASCII words/numbers delimited by non-word chars, plus one count per CJK
    codepoint. Used only for the token-side reduction estimate; no randomness.
    """
    count = len(re.findall(r"[A-Za-z0-9][A-Za-z0-9_.\-]*", text))
    count += len(re.findall(r"[\u4e00-\u9fff]", text))
    return count


def validate_contain_corpus(corpus):
    slices = corpus.get("slices") or []
    kinds = ("build", "stack", "diff", "log", "test")
    if not 20 <= len(slices) <= 30:
        sys.exit(f"contain: slices={len(slices)} must be 20..30")
    seen = set()
    kind_counts = {k: 0 for k in kinds}
    for index, slc in enumerate(slices):
        ident = str(slc.get("id") or "").strip()
        kind = str(slc.get("kind") or "").strip()
        query = str(slc.get("query") or "").strip()
        text = str(slc.get("full_text") or "")
        evidence = slc.get("evidence") or []
        note = str(slc.get("note") or "").strip()
        if not ident or ident in seen:
            sys.exit(f"contain: bad/duplicate slice id: {ident!r}")
        seen.add(ident)
        if kind not in kind_counts:
            sys.exit(f"contain: slice {ident} bad kind {kind!r}")
        if not query or not text or not evidence:
            sys.exit(f"contain: slice {ident} empty query/text/evidence")
        if not note:
            sys.exit(f"contain: slice {ident} missing note")
        kind_counts[kind] += 1
        for ev in evidence:
            if ev not in text:
                sys.exit(
                    f"contain: slice {ident} evidence line not verbatim in full_text:\n{ev!r}"
                )
    missing = [k for k, v in kind_counts.items() if v == 0]
    if missing:
        sys.exit(f"contain: corpus missing kinds: {missing}")
    return slices, kind_counts


def invoke_scx_once(binary, slc, ratio, context, work):
    """Run the release binary once; return (stats, body_text, wall_seconds)."""
    ident = slc["id"]
    path = work / f"{ident}.txt"
    write_text(path, slc["full_text"])
    start = time.perf_counter()
    proc = subprocess.run(
        [binary, "--query", slc["query"], "--ratio", str(ratio),
         "--context", str(context), "--json", str(path)],
        capture_output=True,
    )
    wall = time.perf_counter() - start
    out = proc.stdout.decode("utf-8", "replace")
    err = proc.stderr.decode("utf-8", "replace")
    if proc.returncode != 0:
        raise RuntimeError(f"contain: scx failed for {ident} rc={proc.returncode}\n{out}{err}")
    stats = None
    for line in err.splitlines():
        if line.startswith("scxjson: "):
            stats = json.loads(line[len("scxjson: "):])
            break
    if stats is None:
        raise RuntimeError(f"contain: no scxjson line for {ident}\n{err}")
    body_lines = [l for l in out.splitlines() if not l.startswith("compressed:")]
    body_text = "\n".join(body_lines)
    if body_text:
        body_text += "\n"
    return stats, body_text, wall


def trunc_kept_text(text, budget):
    """Head-truncation baseline: first `budget` raw bytes, byte-exact."""
    return text.encode("utf-8")[:budget].decode("utf-8", "replace")


def random_sample_kept_text(text, budget, seed, ident):
    """Random-line-sampling baseline at the SAME byte budget scx spent.

    Lines are shuffled with a per-slice deterministic RNG seeded from
    `seed:ident`, then whole lines are greedily kept in shuffle order until the
    budget is exhausted; the remaining slack is filled with a byte prefix of
    the next line. Selected lines are emitted in ORIGINAL order so evidence
    substring checks stay line-faithful. Same corpus + same seed => same bytes.
    """
    # keepends preserves each line's own terminator; no phantom trailing block.
    chunks = [line.encode("utf-8") for line in text.splitlines(keepends=True)]
    order = list(range(len(chunks)))
    random.Random(f"{seed}:{ident}").shuffle(order)
    kept = set()
    partial = {}
    used = 0
    for idx in order:
        size = len(chunks[idx])
        if used + size <= budget:
            kept.add(idx)
            used += size
        else:
            slack = budget - used
            if slack > 0:
                partial[idx] = chunks[idx][:slack]
            break
    parts = []
    for idx, chunk in enumerate(chunks):
        if idx in kept:
            parts.append(chunk)
        elif idx in partial:
            parts.append(partial[idx])
    return b"".join(parts).decode("utf-8", "replace")


def run_contain_slice(binary, slc, ratio, context, work, seed, rounds):
    ident = slc["id"]
    text = slc["full_text"]
    # scx output is deterministic; repeat only to get a stable wall-clock sample.
    walls_scx = []
    stats = body_text = None
    for _ in range(max(1, rounds)):
        stats, body_text, wall = invoke_scx_once(
            binary, slc, ratio, context, work
        )
        walls_scx.append(wall)
    # Both baselines spend the exact same byte budget scx spent.
    budget = len(body_text.encode("utf-8"))
    walls_trunc, trunc_text = [], ""
    for _ in range(max(1, rounds)):
        t0 = time.perf_counter()
        trunc_text = trunc_kept_text(text, budget)
        walls_trunc.append(time.perf_counter() - t0)
    walls_rand, rand_text = [], ""
    for _ in range(max(1, rounds)):
        t0 = time.perf_counter()
        rand_text = random_sample_kept_text(text, budget, seed, ident)
        walls_rand.append(time.perf_counter() - t0)
    ev_total = len(slc["evidence"])
    kept_ev = [ev for ev in slc["evidence"] if ev in body_text]
    missed = [ev for ev in slc["evidence"] if ev not in body_text]
    trunc_kept = sum(1 for ev in slc["evidence"] if ev in trunc_text)
    rand_kept = sum(1 for ev in slc["evidence"] if ev in rand_text)
    in_chars = len(text)
    out_chars = len(body_text)
    in_tokens = contain_token_count(text)
    out_tokens = contain_token_count(body_text)
    return {
        "id": ident,
        "kind": slc["kind"],
        "query": slc["query"],
        "mode": stats.get("mode", ""),
        "risk": stats.get("risk", ""),
        "kept": stats.get("kept"),
        "total": stats.get("total"),
        "seeds": stats.get("seeds"),
        "evidence_seeds": stats.get("evidence"),
        "chars_in": in_chars,
        "chars_out": out_chars,
        "chars_out_rand": len(rand_text),
        "token_in": in_tokens,
        "token_out": out_tokens,
        "char_cut": 1.0 - out_chars / in_chars if in_chars else 0.0,
        "char_cut_rand": 1.0 - len(rand_text) / in_chars if in_chars else 0.0,
        "token_cut": 1.0 - out_tokens / in_tokens if in_tokens else 0.0,
        "ev_total": ev_total,
        "ev_kept": len(kept_ev),
        "ev_missed": missed,
        "trunc_ev_kept": trunc_kept,
        "rand_ev_kept": rand_kept,
        "trunc_budget_chars": budget,
        "wall_scx_ms": [w * 1000.0 for w in walls_scx],
        "wall_trunc_ms": [w * 1000.0 for w in walls_trunc],
        "wall_rand_ms": [w * 1000.0 for w in walls_rand],
    }


def cmd_contain(args):
    corpus = load_json(args.corpus)
    slices, kind_counts = validate_contain_corpus(corpus)
    binary = args.binary or default_binary()
    if not binary or not Path(binary).is_file():
        sys.exit("contain: missing release binary; run `cargo build --release --locked` first")
    rows = []
    with tempfile.TemporaryDirectory(prefix="scx-contain-") as tmp:
        work = Path(tmp)
        for slc in slices:
            rows.append(run_contain_slice(
                binary, slc, args.ratio, args.context, work,
                args.seed, args.timing_rounds,
            ))
    # aggregate
    n = len(rows)
    ev_sum = sum(r["ev_total"] for r in rows)
    ev_kept = sum(r["ev_kept"] for r in rows)
    trunc_kept = sum(r["trunc_ev_kept"] for r in rows)
    rand_kept = sum(r["rand_ev_kept"] for r in rows)
    char_cut = sum(r["char_cut"] for r in rows) / n
    char_cut_rand = sum(r["char_cut_rand"] for r in rows) / n
    token_cut = sum(r["token_cut"] for r in rows) / n
    modes = {}
    for r in rows:
        modes[r["mode"]] = modes.get(r["mode"], 0) + 1
    # per-slice median wall, then mean across slices (ms).
    def med(values):
        values = sorted(values)
        mid = len(values) // 2
        return values[mid] if len(values) % 2 else (values[mid - 1] + values[mid]) / 2
    lat_scx = sum(med(r["wall_scx_ms"]) for r in rows) / n
    lat_trunc = sum(med(r["wall_trunc_ms"]) for r in rows) / n
    lat_rand = sum(med(r["wall_rand_ms"]) for r in rows) / n
    print(f"contain: slices={n} kinds={kind_counts} binary={binary}")
    print(f"contain: ratio={args.ratio} context={args.context} modes={modes}")
    print(f"contain: rand-seed={args.seed} timing-rounds={args.timing_rounds}")
    print()
    hdr = (f"{'id':10s} {'kind':6s} {'mode':18s} {'kept':>7s} "
           f"{'char_cut':>9s} {'tok_cut':>8s} {'scx_ev':>7s} "
           f"{'trunc_ev':>9s} {'rand_ev':>8s}")
    print(hdr)
    for r in rows:
        print(f"{r['id']:10s} {r['kind']:6s} {r['mode']:18s} "
              f"{str(r['kept'])+'/'+str(r['total']):>7s} "
              f"{r['char_cut']*100:8.1f}% {r['token_cut']*100:7.1f}% "
              f"{r['ev_kept']}/{r['ev_total']:>3d} {r['trunc_ev_kept']:>4d}/{r['ev_total']} "
              f"{r['rand_ev_kept']:>4d}/{r['ev_total']}")
    print()
    print(f"contain: scx   evidence retention = {ev_kept}/{ev_sum} = "
          f"{ev_kept/ev_sum*100:.2f}%")
    print(f"contain: trunc evidence retention = {trunc_kept}/{ev_sum} = "
          f"{trunc_kept/ev_sum*100:.2f}%")
    print(f"contain: rand  evidence retention = {rand_kept}/{ev_sum} = "
          f"{rand_kept/ev_sum*100:.2f}%")
    print(f"contain: mean char cut   = {char_cut*100:.2f}% (scx) / "
          f"{char_cut_rand*100:.2f}% (rand, same byte budget)")
    print(f"contain: mean token cut  = {token_cut*100:.2f}% (scx)")
    print(f"contain: mean per-slice wall = scx {lat_scx:.3f} ms / "
          f"trunc {lat_trunc*1000:.1f} us / rand {lat_rand*1000:.1f} us")
    misses = [(r["id"], m) for r in rows for m in r["ev_missed"]]
    if misses:
        print("contain: MISSED evidence lines:")
        for ident, line in misses:
            print(f"  {ident}: {line}")
    by_kind = {}
    for r in rows:
        k = by_kind.setdefault(r["kind"], [0, 0, 0.0, 0.0, 0])
        k[0] += r["ev_kept"]; k[1] += r["ev_total"]
        k[2] += r["char_cut"]; k[3] += r["token_cut"]
        k[4] += r["rand_ev_kept"]
    print("contain: per-kind (scx_ev / rand_ev / char_cut / token_cut):")
    for kind in sorted(by_kind):
        ke, tot, cc, tc, re = by_kind[kind]
        cnt = sum(1 for r in rows if r["kind"] == kind)
        print(f"  {kind:6s} ev={ke}/{tot} rand_ev={re}/{tot} "
              f"char={cc/cnt*100:5.1f}% tok={tc/cnt*100:5.1f}%")
    if args.out:
        dump_json(args.out, {
            "binary": binary, "ratio": args.ratio, "context": args.context,
            "rand_seed": args.seed, "timing_rounds": args.timing_rounds,
            "aggregate": {
                "slices": n, "kinds": kind_counts, "modes": modes,
                "scx_evidence_retention": ev_kept / ev_sum,
                "trunc_evidence_retention": trunc_kept / ev_sum,
                "rand_evidence_retention": rand_kept / ev_sum,
                "mean_char_cut": char_cut,
                "mean_char_cut_rand": char_cut_rand,
                "mean_token_cut": token_cut,
                "mean_wall_scx_ms": lat_scx,
                "mean_wall_trunc_ms": lat_trunc,
                "mean_wall_rand_ms": lat_rand,
            },
            "rows": rows,
        })
        print(f"contain: wrote {args.out}")
    return 0


# ---------------------------------------------------------------------------
# Verification and self-test
# ---------------------------------------------------------------------------


def cmd_verify(args):
    actual = sha256_file(args.packet)
    if args.sha256 and actual.lower() != args.sha256.lower():
        sys.exit(f"verify: packet drifted: {actual} != {args.sha256}")
    rows = read_packet(args.packet)
    print(f"verify: packet={args.packet} rows={len(rows)} sha256={actual}")


def selftest():
    assert not is_memory_dir(ROOT)
    with tempfile.TemporaryDirectory(prefix="scx-selftest-") as tmp:
        path = Path(tmp)
        assert not is_memory_dir(path)
        (path / "cases").mkdir()
        assert not is_memory_dir(path)
        (path / "meta.toml").write_text("schema = 2\n", encoding="utf-8")
        assert is_memory_dir(path)
        memory = path / "memory"
        (memory / "cases").mkdir(parents=True)
        (memory / "meta.toml").write_text("schema = 2\n", encoding="utf-8")
        (memory / "stale").write_text("remove\n", encoding="utf-8")
        assert prepare_mem_dir(memory, True) == memory.resolve()
        assert memory.is_dir() and not any(memory.iterdir())
        foreign = path / "foreign"
        foreign.mkdir()
        (foreign / "keep").write_text("keep\n", encoding="utf-8")
        try:
            prepare_mem_dir(foreign, True)
        except SystemExit as error:
            assert "non-memory" in str(error)
        else:
            raise AssertionError("--force must reject a non-memory directory")
        assert (foreign / "keep").read_text(encoding="utf-8") == "keep\n"
        verbose = path / "verbose.txt"
        verbose.write_text(
            'case 1/1 q="database timeout"\n'
            "  A: 1) abcdef12 sim=0.90 score=9.00 [hit]\n"
            "  B: (no hits)\n",
            encoding="utf-8",
        )
        assert parse_verbose(verbose, 1) == [{"A": ["abcdef12"], "B": []}]
        assert is_line("one query line")
        assert not is_line("two\nlines") and not is_line("tab\there")
    assert id_matches("2026-10-22_abcdef12", "abcdef12")
    assert id_matches("2026-10-22_abcdef12", "2026-10-25_abcdef12")
    assert not id_matches("2026-10-22_abcdef12", "deadbeef")
    case = {"rel": {"abcdef12": 2, "2026-10-22_beefcafe": 1}}
    assert rel_of(case, "abcdef12") == 2
    assert rel_of(case, "beefcafe") == 1
    assert rel_of(case, "00000000") == 0
    assert ndcg({"rel": {"a": 2, "b": 1}}, ["a", "b"]) == 1.0
    assert abs(mrr(2) - 0.5) < 1e-12
    rows = [
        {"a": 1.0, "b": 0.0},
        {"a": 0.0, "b": 0.0},
    ]
    first = analyze("all", rows, 200, 200, 7, 0.05, "mrr")
    second = analyze("all", rows, 200, 200, 7, 0.05, "mrr")
    assert first == second and first["delta"] == 0.5
    assert jaccard("database timeout", "database timeout") == 1.0
    cases = [
        {"query": "q1", "rel": {"a": 2, "b": 1}},
        {"query": "q2", "rel": {"c": 2, "d": 0}},
    ]
    total, raw, kappa, matrix, disagreements = agreement(cases, cases)
    assert total == 4 and raw == 1.0 and kappa == 1.0
    assert matrix[2][2] == 2 and not disagreements
    partial = parse_grades(
        [
            {"row": "1", "case_index": "0", "candidate": "a", "grade": "2"},
            {"row": "2", "case_index": "0", "candidate": "b", "grade": ""},
        ],
        True,
    )
    assert partial == {(0, "a"): 2.0}
    cluster = analyze_cluster(
        [{"i": 0, "a": 1.0, "b": 0.0}, {"i": 5, "a": 0.0, "b": 0.0}],
        5,
        20,
        20,
        7,
        0.05,
        "ndcg",
    )
    assert cluster["A_ndcg"] == 0.5 and "A_mrr" not in cluster
    for name, cases in (("confirm5", 150), ("v2", 150), ("real", 40)):
        corpus = load_json(CONFIG_DIR / f"{name}.corpus.json")
        layers = validate_corpus(corpus)
        assert sum(layers.values()) == cases
    sample = "alpha line\nbeta line\nseed line evidence\ngamma noise\n"
    big = random_sample_kept_text(sample, len(sample.encode()), "z", "s1")
    assert big == sample  # budget covers everything -> verbatim input
    small = random_sample_kept_text(sample, 10, "z", "s1")
    assert len(small.encode("utf-8")) == 10  # byte-exact fill of the budget
    assert small == random_sample_kept_text(sample, 10, "z", "s1")  # deterministic
    assert small != random_sample_kept_text(sample, 10, "z2", "s1")  # seed matters
    assert trunc_kept_text(sample, 10) == sample.encode("utf-8")[:10].decode("utf-8")
    for judge, expected in (
        ("ai", "6a5319c546fe68c20f3d4d7ffd38437b3736ea301e1319afb3c374eca9ac9b18"),
        ("human", "1f6cd83576dd3872635c2317131f8ce7858301e7afe88d490b8ce46e1b9240b5"),
    ):
        head, rows = read_tsv(CONFIG_DIR / "confirm5.blind40.packet.tsv")
        labels = load_json(CONFIG_DIR / f"confirm5.{judge}.labels.json")
        assert sha256_text(filled_packet(head, rows, labels["cases"])) == expected
    print("selftest: PASS")


def build_parser():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="cmd", required=True)

    seed = sub.add_parser("seed", help="corpus JSON -> memory cases + eval CSV")
    seed.add_argument("corpus")
    seed.add_argument("--mem-dir", required=True)
    seed.add_argument("--csv")
    seed.add_argument("--binary", default="")
    seed.add_argument(
        "--jobs", type=int, default=4,
        help="parallel store processes (default: 4)",
    )
    seed.add_argument("--force", action="store_true")
    seed.set_defaults(func=cmd_seed)

    baseline = sub.add_parser("baseline", help="eval CSV -> target baseline")
    baseline.add_argument("--csv", required=True)
    baseline.add_argument("--out", required=True)
    baseline.add_argument("--judge-by", default="mechanical-target-v1")
    baseline.add_argument("--judge-date", default="2026-10-23T00:00:00Z")
    baseline.set_defaults(func=cmd_baseline)

    stats = sub.add_parser("stats", help="verbose + baseline -> significance CSV")
    stats.add_argument("--baseline", required=True)
    stats.add_argument("--verbose", required=True)
    stats.add_argument("--out", required=True)
    stats.add_argument("--mem-dir")
    stats.add_argument("--strata")
    stats.add_argument("--metric", choices=("mrr", "ndcg"), default="mrr")
    stats.add_argument("--boot", type=int, default=10000)
    stats.add_argument("--perm", type=int, default=10000)
    stats.add_argument("--seed", type=int, default=20261023)
    stats.add_argument("--mde", type=float, default=0.05)
    stats.add_argument("--cluster-size", type=int, default=0)
    stats.set_defaults(func=cmd_stats)

    blind = sub.add_parser("blind", help="frozen per-candidate blind review")
    blind_sub = blind.add_subparsers(dest="blind_cmd", required=True)

    make = blind_sub.add_parser("make")
    make.add_argument("--baseline", required=True)
    make.add_argument("--mem-dir", required=True)
    make.add_argument("--out", required=True)
    make.add_argument("--manifest", required=True)
    make.add_argument("--name", default="confirm5-blind40")
    make.add_argument("--seed", default="confirm5-blind-v1")
    make.add_argument(
        "--per-layer", default="literal=14,paraphrase=13,semantic=13"
    )

    imp = blind_sub.add_parser("import")
    imp.add_argument("--baseline", required=True)
    imp.add_argument("--packet", required=True)
    imp.add_argument("--out", required=True)
    imp.add_argument("--judge", required=True)
    imp.add_argument("--date", required=True)
    imp.add_argument("--name", default="confirm5-blind40")
    imp.add_argument("--method", default="per-item-blind-v1")
    imp.add_argument("--note", default="")
    imp.add_argument("--allow-partial", action="store_true")

    check = blind_sub.add_parser("check")
    check.add_argument("--baseline", required=True)
    check.add_argument("--packet", help="reconstruct the filled packet")
    check.add_argument("--out", help="write the reconstructed filled packet")

    progress = blind_sub.add_parser("progress")
    progress.add_argument("--packet", required=True)

    agree = blind_sub.add_parser("agree")
    agree.add_argument("--a", required=True)
    agree.add_argument("--b", required=True)

    review = blind_sub.add_parser("review")
    review.add_argument("--packet", required=True)
    blind.set_defaults(func=cmd_blind)

    verify = sub.add_parser("verify", help="packet hash and row count")
    verify.add_argument("--packet", required=True)
    verify.add_argument("--sha256")
    verify.set_defaults(func=cmd_verify)

    contain = sub.add_parser(
        "contain", help="evidence containment vs head-truncation baseline"
    )
    contain.add_argument("corpus", help="contain*.corpus.json")
    contain.add_argument("--binary", default="")
    contain.add_argument("--ratio", type=float, default=0.35)
    contain.add_argument("--context", type=int, default=1)
    contain.add_argument("--out", help="write per-slice detail JSON")
    contain.add_argument(
        "--seed", default="contain-rand-v1",
        help="deterministic seed for the random-sampling baseline",
    )
    contain.add_argument(
        "--timing-rounds", type=int, default=5,
        help="repeat each method per slice, take median wall time",
    )
    contain.set_defaults(func=cmd_contain)

    test = sub.add_parser("selftest")
    test.set_defaults(func=lambda _args: selftest())
    return parser


def main():
    parser = build_parser()
    args = parser.parse_args()
    if args.cmd == "blind" and args.blind_cmd == "make":
        args.per_layer = {
            key: int(value)
            for key, value in (part.split("=", 1) for part in args.per_layer.split(","))
        }
    return args.func(args) or 0


if __name__ == "__main__":
    sys.exit(main())
