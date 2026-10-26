# scx vs embedding / 混合检索对照（方法学）

> 目的：在**同一份冻结 Gauge 语料 + 机械标签**上，把 scx 与 (a) 通用
> embedding、(b) BM25+embedding 混合检索做配对对照，并**按层**报告。
> 这是「优于 embedding/混合检索」唯一可信的出处——分层数字，不报笼统聚合数。

## 诚实的边界（先说结论）

Gauge 语料 query 分 `literal` / `paraphrase` / `semantic` 三层。预期是**分层**的：

- `literal` / `paraphrase`（关键词密集、有词面重叠）：scx 的 IDF 词面证据
  筛选通常占优；通用 embedding（all-MiniLM 训于通用 web 文本）在日志/事故
  这类专业词汇上常把大量无关行也判为相似。
- `semantic`（无共享词的释义查询）：embedding 大概率占优——这是 scx 当前
  「无词面命中即 fail-open」的已知盲区。

因此本文档强制按层输出，不把「某层领先」冒充「全面领先」。

## 方法学（冻结）

| 项 | 值 |
|---|---|
| 语料 | `config/gauge.corpus.json`（30 事故 / 150 case）+ 冻结机械标签 baseline JSON |
| 指标 | `mrr`（默认）或 `recall@k`；配对 bootstrap 95% CI + sign-flip permutation（10,000 次） |
| 随机源 | 固定 seed `20261023`（与 Gauge 预注册一致） |
| embedding 模型 | `sentence-transformers/all-MiniLM-L6-v2`（`tools/requirements-embed.txt` 钉版本） |
| embedding 排名 | query 向量与各 case `text` 向量的余弦相似度降序 |
| 混合检索 | **RRF**（k=60）：`RRF(d)=1/(60+rank_bm25)+1/(60+rank_embed)` |
| 词面分量 | scx `--eval-verbose` 的 **B 列（朴素 BM25）**排名——Rust 侧唯一实现，Python 不复刻排序 |
| 对比口径 | 每个 query 对全部候选 case 排名，`target_rank` 在 top-10 内找 target |

## 复现

```bash
# 1. 播种 + 机械标签 + scx A/B verbose（与 docs/eval.md 一致）
./tools/scx.py seed config/gauge.corpus.json \
  --mem-dir /tmp/scx-gauge --csv /tmp/scx-gauge.csv --force
./tools/scx.py baseline --csv /tmp/scx-gauge.csv \
  --out /tmp/scx-gauge-mechanical.json \
  --judge-by mechanical-target-gauge-v1 --judge-date 2026-10-23T00:00:00Z
./tools/scx --eval /tmp/scx-gauge-mechanical.json --eval-verbose \
  --mem-dir /tmp/scx-gauge > /tmp/scx-gauge-verbose.txt

# 2. embedding / 混合检索对照（dev-only，需 sentence-transformers）
pip install -r tools/requirements-embed.txt
python tools/embed_baseline.py \
  --baseline /tmp/scx-gauge-mechanical.json \
  --mem-dir /tmp/scx-gauge \
  --verbose /tmp/scx-gauge-verbose.txt \
  --metric mrr --out /tmp/embed-compare.json
```

## 结果（待实测）

> 本节在具备 sentence-transformers 与模型下载环境后填充；数字以本节为准，
> 与 `docs/gauge-metrics.md` 同口径（分层、CI、perm p）。

| layer | n | scx | bm25 | embed | hybrid |
|---|---|---|---|---|---|
| literal | — | — | — | — | — |
| paraphrase | — | — | — | — | — |
| semantic | — | — | — | — | — |
| all | — | — | — | — | — |

配对（scx 为 A）：`scx vs embed`、`scx vs hybrid` 的 Δ、95% CI、perm p，逐层列出。

## 解读约束

- 本对照**不是**对任意 RAG 系统的普遍优势声明，只证明「在冻结 Gauge 语料、
  给定模型与融合公式下，scx 的排序与 embedding/hybrid 的排序孰优」。
- 换 embedding 模型、换融合公式或换语料，结论可能改变；要改结论必须重跑并
  重新预注册，不能复用本节数字。
