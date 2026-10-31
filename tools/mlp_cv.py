#!/usr/bin/env python3
"""#67 MLP 对照实验（非默认档位）：单隐层 MLP 蒸馏 vs 线性权重。

口径：与 tools/distill_online.py 相同的标注缓存（config/bench/results/distill_cache.json，
键 cname|query）、相同 24 维特征、相同 3 折 CV 协议，仅学生模型不同：
- linear：纯 stdlib 逻辑回归（基线，对应线上默认 weights.json）
- mlp1：单隐层 24→16→1，sigmoid 输出，同一批梯度下降（epochs=150/lr=0.05）

输出：3 折 CV AUC/AP（linear vs mlp1），写入 /tmp/mlp_cv.json；不替换仓库权重。
"""
from __future__ import annotations
import json, math, random, sys
from pathlib import Path

ROOT = Path("/Users/fei/code/scx")
sys.path.insert(0, str(ROOT / "tools"))
from distill_online import feats, FEATURES, STOP, tokens, build_corpora  # noqa: E402

DIM = len(FEATURES)
SIG = lambda z: 1.0 / (1.0 + math.exp(-max(-30.0, min(30.0, z))))


def dataset(cache: dict) -> tuple[list, list]:
    """从标注缓存重建 (X, y)：每 (cname|query, keep) 生成全行样本。
    语料文本用 distill_online.build_corpora() 的确定性来源重建（与训练口径一致）。"""
    corpora = {c[0]: c[1] for c in build_corpora()}
    X, y = [], []
    for key, keep in cache.items():
        if "|" not in key:
            continue
        cname, query = key.rsplit("|", 1)
        text = corpora.get(cname)
        if text is None:
            continue
        lines = text.splitlines()
        total = len(lines)
        ks = set(int(x) for x in keep)
        for i, ln in enumerate(lines):
            X.append(feats(ln, [query], i, total))
            y.append(1 if (i + 1) in ks else 0)
    return X, y


def fit_linear(X, y):
    """与 distill_online.fit 同协议：批量梯度下降（lr=0.05, 150 epochs, l2=1e-3）。"""
    w = [0.0] * DIM
    n = len(X)
    for _ in range(150):
        grad = [0.0] * DIM
        for xi, yi in zip(X, y):
            p = SIG(sum(w[d] * xi[d] for d in range(DIM)))
            err = p - yi
            for d in range(DIM):
                grad[d] += err * xi[d]
        for d in range(DIM):
            w[d] -= 0.05 * (grad[d] / n + 1e-3 * w[d])
    return w


def fit_mlp(X, y, h=16):
    random.seed(42)
    w1 = [[random.uniform(-0.3, 0.3) for _ in range(DIM)] for _ in range(h)]
    b1 = [0.0] * h
    w2 = [random.uniform(-0.3, 0.3) for _ in range(h)]
    b2 = 0.0
    n = len(X)
    for epoch in range(150):
        for i in range(n):
            a = [SIG(sum(w1[j][d] * X[i][d] for d in range(DIM)) + b1[j]) for j in range(h)]
            z = sum(w2[j] * a[j] for j in range(h)) + b2
            err = SIG(z) - y[i]
            dz = err * SIG(z) * (1 - SIG(z))
            for j in range(h):
                w2[j] -= 0.05 * (dz * a[j] + 1e-3 * w2[j])
                b1[j] -= 0.05 * (dz * w2[j] * a[j] * (1 - a[j]))
                for d in range(DIM):
                    w1[j][d] -= 0.05 * (dz * w2[j] * a[j] * (1 - a[j]) * X[i][d] + 1e-3 * w1[j][d])
            b2 -= 0.05 * dz
    def score(x):
        a = [SIG(sum(w1[j][d] * x[d] for d in range(DIM)) + b1[j]) for j in range(h)]
        return SIG(sum(w2[j] * a[j] for j in range(h)) + b2)
    return score


def auc_ap(scores, y):
    """AUC（ROC）+ AP（PR-AUC）标准实现。"""
    pairs = sorted(zip(scores, y), key=lambda p: -p[0])
    npos = sum(y)
    nneg = len(y) - npos
    tp = fp = 0
    auc = 0.0
    prev = None
    for s, yi in pairs:
        if yi:
            tp += 1
        else:
            fp += 1
            if prev != s:
                auc += fp * tp
        prev = s
    auc = auc / (npos * nneg) if npos and nneg else 0.5
    tp = fp = 0
    ap = 0.0
    last_rec = 0.0
    for s, yi in pairs:
        if yi:
            tp += 1
        else:
            fp += 1
        rec = tp / npos if npos else 0.0
        prec = tp / (tp + fp) if tp + fp else 0.0
        ap += prec * (rec - last_rec)
        last_rec = rec
    return auc, ap


def main() -> None:
    cache = json.loads((ROOT / "config" / "bench" / "results" / "distill_cache.json").read_text())
    X, y = dataset(cache)
    if not X:
        print("无样本（缓存为空或仅合成语料）"); return 1
    n = len(X)
    # 3 折（确定性 shuffle，seed 2024 与 /tmp/cv.py 口径一致）
    idx = list(range(n))
    random.seed(2024)
    random.shuffle(idx)
    folds = [idx[i::3] for i in range(3)]
    lin, mlp = [], []
    for fi, fold in enumerate(folds):
        tr = [i for i in idx if i not in set(fold)]
        te = fold
        Xt = [X[i] for i in tr]; yt = [y[i] for i in tr]
        Xe = [X[i] for i in te]; ye = [y[i] for i in te]
        w = fit_linear(Xt, yt)
        ls = [sum(w[d] * x[d] for d in range(DIM)) for x in Xe]
        lin.append(auc_ap(ls, ye))
        lk = sum(1 for s in ls if s > 0.0) / len(ls)
        scorer = fit_mlp(Xt, yt)
        ms = [scorer(x) for x in Xe]
        mlp.append(auc_ap(ms, ye))
        mk = sum(1 for s in ms if s > 0.5) / len(ms)
        print(f"fold {fi}: linear auc/ap={lin[-1][0]:.4f}/{lin[-1][1]:.4f} keep={lk:.3f}  "
              f"mlp1 auc/ap={mlp[-1][0]:.4f}/{mlp[-1][1]:.4f} keep={mk:.3f}")
    res = {
        "n": n, "dim": DIM,
        "linear": {"auc": sum(a for a, _ in lin) / 3, "ap": sum(p for _, p in lin) / 3},
        "mlp1": {"auc": sum(a for a, _ in mlp) / 3, "ap": sum(p for _, p in mlp) / 3},
        "folds_linear": lin, "folds_mlp": mlp,
    }
    print(json.dumps(res, ensure_ascii=False, indent=2))
    Path("/tmp/mlp_cv.json").write_text(json.dumps(res, ensure_ascii=False, indent=2))
    print("已写入 /tmp/mlp_cv.json（不替换仓库权重）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
