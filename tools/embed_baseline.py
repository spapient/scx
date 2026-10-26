#!/usr/bin/env python3
"""scx 与 embedding / 混合检索的可复现对照（dev-only，需 sentence-transformers）。

本脚本**不属于** stdlib-only 评测器（tools/scx.py）；它显式依赖
sentence-transformers，独立运行。它不改写任何检索排序——BM25 词面排名
全部取自 scx `--eval-verbose` 的 A/B 输出（Rust 侧唯一实现），Python 侧只
计算 embedding 余弦相似度排名，并把两者做 RRF 融合。统计口径复用
tools/scx.py 的 `mrr` / `recall_at` / `target_rank` / `analyze`。

用法：
  python tools/embed_baseline.py \
      --baseline config/gauge-mechanical.json \
      --mem-dir /tmp/scx-gauge \
      --verbose /tmp/scx-gauge-verbose.txt \
      --metric mrr \
      --out /tmp/embed-compare.json
"""

import argparse
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "tools"))

import scx  # noqa: E402  复用 mrr / recall_at / target_rank / analyze / load_json / tail_of

RRF_K = 60.0
DEFAULT_MODEL = "sentence-transformers/all-MiniLM-L6-v2"


def load_case_texts(mem_dir, command):
    root = Path(mem_dir) / "cases"
    if not root.is_dir():
        sys.exit(f"{command}: missing memory cases dir: {root}")
    texts, order = {}, []
    for directory in sorted(root.iterdir()):
        path = directory / "case.json"
        if not path.is_file():
            continue
        case = scx.load_json(path)
        ident = str(case.get("id") or directory.name)
        body = str(case.get("text") or case.get("query") or "")
        if ident not in texts:
            order.append(ident)
        texts[ident] = body
    if not order:
        sys.exit(f"{command}: no cases found in {root}")
    return texts, order


def cosine(a, b):
    import numpy as np

    denom = float(np.linalg.norm(a) * np.linalg.norm(b))
    if denom == 0.0:
        return 0.0
    return float(np.dot(a, b) / denom)


def rank_map(ids):
    """rank-ordered id list -> {tail8: 1-based rank}（首个出现者胜）。"""
    m = {}
    for i, cid in enumerate(ids, 1):
        m.setdefault(str(cid)[-8:], i)
    return m


def embed_rank(query_emb, order, emb_by_id):
    scored = []
    for cid in order:
        scored.append((cosine(query_emb, emb_by_id[cid]), cid))
    scored.sort(key=lambda x: (-x[0], x[1]))
    return [cid for _, cid in scored]


def rrf_rank(rank_b, rank_embed, order):
    scored = []
    for cid in order:
        key = str(cid)[-8:]
        s = 0.0
        if key in rank_b:
            s += 1.0 / (RRF_K + rank_b[key])
        if key in rank_embed:
            s += 1.0 / (RRF_K + rank_embed[key])
        scored.append((s, cid))
    scored.sort(key=lambda x: (-x[0], x[1]))
    return [cid for _, cid in scored]


def metric_value(metric, ranked_ids, target):
    rank = scx.target_rank(ranked_ids, target)
    if metric == "mrr":
        return scx.mrr(rank)
    if metric.startswith("recall@"):
        return scx.recall_at(rank, int(metric.split("@")[1]))
    sys.exit(f"embed_baseline: unsupported metric {metric!r} (mrr|recall@k)")


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--baseline", required=True, help="机械目标 baseline JSON")
    ap.add_argument("--mem-dir", required=True, help="已播种记忆库目录")
    ap.add_argument("--verbose", required=True, help="scx --eval-verbose 输出")
    ap.add_argument("--metric", default="mrr", help="mrr 或 recall@k")
    ap.add_argument("--model", default=DEFAULT_MODEL, help="embedding 模型")
    ap.add_argument("--boot", type=int, default=10000)
    ap.add_argument("--perm", type=int, default=10000)
    ap.add_argument("--seed", type=int, default=20261023)
    ap.add_argument("--mde", type=float, default=0.05)
    ap.add_argument("--out", help="输出 JSON 路径（可选）")
    args = ap.parse_args()

    baseline = scx.load_json(args.baseline).get("cases") or []
    if not baseline:
        sys.exit("embed_baseline: baseline has no cases")

    texts, order = load_case_texts(args.mem_dir, "embed_baseline")

    # scx 自己的 A/B 排名（tail8 序）；A=完整管线，B=朴素 BM25。
    verbose = scx.parse_verbose(args.verbose, len(baseline))

    try:
        from sentence_transformers import SentenceTransformer
    except ImportError:
        sys.exit(
            "embed_baseline: 需要 sentence-transformers（见 tools/requirements-embed.txt）；"
            "本脚本独立于 stdlib 评测器，未安装即退出。"
        )
    model = SentenceTransformer(args.model)
    queries = [str(c.get("query") or "") for c in baseline]
    bodies = [texts[cid] for cid in order]

    q_embs = model.encode(queries, normalize_embeddings=True, show_progress_bar=False)
    c_embs = model.encode(bodies, normalize_embeddings=True, show_progress_bar=False)
    emb_by_id = {cid: c_embs[i] for i, cid in enumerate(order)}

    # 每 case 四个方法的指标值。
    rows = []
    for i, case in enumerate(baseline):
        target = str(case["expected"][0])
        layer = case.get("layer", "")
        a_ids = verbose[i]["A"]  # tail8 序
        b_ids = verbose[i]["B"]
        e_ids = embed_rank(q_embs[i], order, emb_by_id)
        h_ids = rrf_rank(rank_map(b_ids), rank_map(e_ids), order)
        rows.append(
            {
                "i": int(case.get("case_index", i)),
                "layer": layer,
                "A": metric_value(args.metric, a_ids, target),
                "B": metric_value(args.metric, b_ids, target),
                "embed": metric_value(args.metric, e_ids, target),
                "hybrid": metric_value(args.metric, h_ids, target),
            }
        )

    # 绝对值表：每层各方法均值。
    layers = ["literal", "paraphrase", "semantic", "all"]
    print(f"embed_baseline: metric={args.metric} model={args.model} "
          f"cases={len(rows)} rrf_k={RRF_K:.0f}")
    print("")
    print(f"{'layer':12s} {'n':>4s} {'scx':>8s} {'bm25':>8s} "
          f"{'embed':>8s} {'hybrid':>8s}")
    for layer in layers:
        group = rows if layer == "all" else [r for r in rows if r["layer"] == layer]
        if not group:
            continue
        n = len(group)
        mean = lambda k: sum(r[k] for r in group) / n  # noqa: E731
        print(f"{layer:12s} {n:4d} {mean('A'):8.4f} {mean('B'):8.4f} "
              f"{mean('embed'):8.4f} {mean('hybrid'):8.4f}")

    # 配对统计：scx(A) vs embed，scx(A) vs hybrid，按层。
    def paired(group, bkey):
        sub = [
            {"a": r["A"], "b": r[bkey]}
            for r in group
        ]
        return scx.analyze(
            "scx-vs-" + bkey, sub, args.boot, args.perm, args.seed, args.mde, args.metric
        )

    print("")
    print("配对（scx 为 A，另一方法为 B；Δ=A−B，Δ>0 表示 scx 占优）：")
    for layer in layers:
        group = rows if layer == "all" else [r for r in rows if r["layer"] == layer]
        if not group:
            continue
        for bkey in ("embed", "hybrid"):
            res = paired(group, bkey)
            if res:
                print(
                    f"{layer:12s} vs {bkey:6s} n={res['n']:3d} "
                    f"A={res[f'A_{args.metric}']:.4f} B={res[f'B_{args.metric}']:.4f} "
                    f"d={res['delta']:+.4f} CI=[{res['ci_lo']:+.4f},{res['ci_hi']:+.4f}] "
                    f"p={res['perm_p']:.4f}"
                )

    if args.out:
        payload = {
            "metric": args.metric,
            "model": args.model,
            "rrf_k": RRF_K,
            "seed": args.seed,
            "rows": rows,
        }
        with open(args.out, "w", encoding="utf-8") as fh:
            json.dump(payload, fh, ensure_ascii=False, indent=2)
        print(f"\nembed_baseline: wrote {args.out}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
