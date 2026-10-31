#!/usr/bin/env python3
"""P23 AMA-Bench · 官方 BM25 基线的零依赖复刻（对照通道，与 scx 同语料同 K）。

官方实现：repo `src/method/bm25.py` + `configs/method_configs/bm25_config.json`
（top_k=10；rank_bm25.BM25Okapi 默认 k1=1.5, b=0.75；tokenize = `doc.lower().split()`）。
本文件逐公式复刻 rank_bm25 BM25Okapi，避免新增第三方依赖；分词口径与官方一致。
"""
from __future__ import annotations

import math
from typing import Any


def tokenize(text: str) -> list[str]:
    return text.lower().split()


class BM25Okapi:
    """rank_bm25 BM25Okapi 默认参数复刻。"""

    def __init__(self, corpus: list[list[str]], k1: float = 1.5, b: float = 0.75,
                 epsilon: float = 0.25):
        self.corpus = corpus
        self.k1 = k1
        self.b = b
        self.epsilon = epsilon
        self.doc_len = [len(doc) for doc in corpus]
        self.avgdl = sum(self.doc_len) / len(self.doc_len) if corpus else 0.0
        self.nd = {}  # term → doc frequency
        self.tf = []  # per-doc term freq map
        for doc in corpus:
            freq: dict[str, int] = {}
            for term in doc:
                freq[term] = freq.get(term, 0) + 1
            self.tf.append(freq)
            for term in freq:
                self.nd[term] = self.nd.get(term, 0) + 1
        self.idf = self._idf()

    def _idf(self) -> dict[str, float]:
        N = len(self.corpus)
        idf: dict[str, float] = {}
        for term, n in self.nd.items():
            idf[term] = math.log(1 + (N - n + 0.5) / (n + 0.5))
        # rank_bm25：负 idf 整体替换为 epsilon × 平均 idf
        neg = [v for v in idf.values() if v < 0]
        if neg:
            avg_neg = sum(neg) / len(neg)
            floor = self.epsilon * avg_neg
            for term in idf:
                if idf[term] < 0:
                    idf[term] = floor
        return idf

    def get_scores(self, query: list[str]) -> list[float]:
        scores: list[float] = []
        for i, doc in enumerate(self.corpus):
            if not doc:
                scores.append(0.0)
                continue
            dl = self.doc_len[i]
            score = 0.0
            for term in query:
                if term not in self.tf[i]:
                    continue
                f = self.tf[i][term]
                idf = self.idf.get(term, 0.0)
                tf_norm = f * (self.k1 + 1) / (f + self.k1 * (1 - self.b + self.b * dl / self.avgdl))
                score += idf * tf_norm
            scores.append(score)
        return scores


class BM25Episode:
    """单 episode 的 turn 级 BM25 索引（对照通道）。"""

    def __init__(self, episode: dict[str, Any], k: int = 10):
        from p23_scx import turn_text, turn_query, turn_fix  # 同粒度，复用格式函数
        self.episode = episode
        self.k = k
        self.records: list[dict[str, Any]] = []
        corpus_tokens: list[list[str]] = []
        for i, turn in enumerate(episode["trajectory"], start=1):
            text = turn_text(turn, i)
            self.records.append({
                "turn_index": i,
                "text": text,
                "query": turn_query(episode, i, turn),
                "fix": turn_fix(episode, i, turn),
            })
            corpus_tokens.append(tokenize(text))
        self.index = BM25Okapi(corpus_tokens) if corpus_tokens else None

    def search(self, question: str) -> list[dict[str, Any]]:
        if self.index is None:
            return []
        scores = self.index.get_scores(tokenize(question))
        order = sorted(range(len(scores)), key=lambda i: scores[i], reverse=True)
        hits: list[dict[str, Any]] = []
        for rank, idx in enumerate(order[: self.k], start=1):
            if scores[idx] <= 0:
                continue
            rec = self.records[idx]
            hits.append({"rank": rank, "score": scores[idx], "sim": None,
                         "case_id": f"bm25-turn-{rec['turn_index']}",
                         "turn_index": rec["turn_index"], "query": rec["query"],
                         "fix": rec["fix"], "text": rec["text"],
                         "compressed": {}, "truncated": False})
        return hits

    @staticmethod
    def context_from_hits(hits: list[dict[str, Any]]) -> str:
        return "\n\n".join(h["text"].rstrip() for h in hits)
