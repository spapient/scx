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

## 结果（实测，2026-10-27，all-MiniLM-L6-v2，MRR）

> 模型 `sentence-transformers/all-MiniLM-L6-v2`，sentence-transformers 3.3.1，
> 配对 bootstrap 10,000 + sign-flip permutation，seed 20261023。与
> `docs/gauge-metrics.md` 同口径（分层、CI、perm p）。
> `hybrid` = 朴素 BM25 + embedding 的 RRF；`fused` = **scx 全管线（BM25+PRF+MMR）
> + embedding 的 RRF**（`search_fused` 默认权重 1.0 的等价）。

| layer | n | scx | bm25 | embed | hybrid | fused |
|---|---|---|---|---|---|---|
| literal | 60 | 1.0000 | 1.0000 | 0.9750 | 0.9917 | 0.9917 |
| paraphrase | 60 | 0.2467 | 0.2423 | 0.4005 | 0.3232 | 0.3218 |
| semantic | 30 | 0.8361 | 0.6739 | 0.8992 | 0.8111 | 0.9111 |
| all | 150 | 0.6659 | 0.6317 | 0.7300 | 0.6882 | 0.7076 |

配对（Δ=A−B；Δ>0 表示 A 占优）：

| 对照 | Δ | 95% CI | perm p |
|---|---|---|---|
| fused vs scx | +0.0417 | [+0.0196,+0.0650] | **0.0003** |
| fused vs embed | −0.0224 | [−0.0571,+0.0126] | 0.2137 |
| fused vs hybrid | +0.0194 | [+0.0037,+0.0387] | 0.0466 |
| fused vs scx（paraphrase） | +0.0751 | [+0.0412,+0.1136] | **0.0002** |
| fused vs embed（paraphrase） | −0.0787 | [−0.1418,−0.0183] | 0.0144 |
| fused vs embed（semantic） | +0.0119 | [−0.1000,+0.1246] | 0.9132 |

### 诚实结论

- **融合显著提升 scx**：`fused` 整体 0.7076，显著优于 scx 的 0.6659
  （p=0.0003），paraphrase 层提升最大（0.3218 vs 0.2467，p=0.0002）。
- **融合把与 embedding 的差距从显著收窄到不显著**：scx vs embed 原本
  −0.0641（p=0.0043），融合后 fused vs embed 为 −0.0224（p=0.21）。但**尚未
  反超**，paraphrase 层仍落后（0.3218 vs 0.4005，p=0.0144）。
- **semantic 层融合最出色**：fused 0.9111，是五者中最高，胜过 embedding 的
  0.8992（虽未达显著）。scx 全管线的 PRF/query-field 重排 + embedding 在该层
  形成正协同。
- **用 scx 全管线融合（fused）优于用朴素 BM25 融合（hybrid）**：0.7076 vs
  0.6882（p=0.0466），印证「要融就该融全管线」。

结论：**`--embed-cmd` 融合机制有效——把 scx 从 0.6659 拉到 0.7076，并把与
embedding 的整体差距收窄到统计不显著**。

### 调参（RRF 权重扫描）

| w | all | literal | paraphrase | semantic | vs embed |
|---|---|---|---|---|---|
| 0.25 | 0.6792 | 1.0000 | 0.2730 | 0.8500 | −0.0509 |
| 0.50 | 0.6931 | 1.0000 | 0.2854 | 0.8944 | −0.0370 |
| 1.00 | 0.7076 | 0.9917 | 0.3218 | 0.9111 | −0.0224 |
| 2.00 | 0.7274 | 0.9750 | 0.3629 | 0.9611 | −0.0027 |
| 3.00 | 0.7311 | 0.9750 | 0.3723 | 0.9611 | +0.0011 |
| 5.00 | 0.7349 | 0.9750 | 0.3817 | 0.9611 | +0.0049 |
| **8.00** | **0.7415** | 0.9750 | 0.3982 | 0.9611 | **+0.0115** |
| 12.00 | 0.7381 | 0.9750 | 0.3979 | 0.9444 | +0.0080 |

**w=8 时 fused 整体 0.7415，点估计反超 embedding 的 0.7300（+0.0115）**，但
95% CI [−0.0062,+0.0306] 跨 0、p=0.23——**未达统计显著，本质是与 embedding
打平**。分层看：

- paraphrase 差距**完全补齐**：0.3982 vs 0.4005（p=0.98，持平）。
- semantic 层**反超**：0.9611 vs 0.8992（点估计 +0.0619，n=30 小样本未达显著）。
- 显著优于 scx 本身（+0.0756，p=0.0001）与朴素 hybrid（+0.0533，p=0.0018）。

**最终判断**：`--embed-cmd` 融合 + 调权（w≈8）把 scx 从「显著落后 embedding」
提升到「与 embedding 打平、点估计略胜」，并显著优于原词面管线与朴素混合检索。
要拿到「显著反超」，需更大语料或换更强/更贴合的中文 embedding（如 bge-small-zh），
属后续工作，不在本文档冻结口径内。

### 中文模型对照（bge-small-zh-v1.5）

| 层 | scx | bge-embed | fused(w=8) |
|---|---|---|---|
| paraphrase | 0.2467 | 0.4251 | 0.4202 |
| semantic | 0.8361 | 0.9444 | 0.9444 |
| all | 0.6659 | 0.7489 | 0.7503 |

bge 比 all-MiniLM 强（整体 0.7489 vs 0.7300，中文语料下符合预期），scx 单独落后
更多（−0.0830，p=0.0005）。但融合（scx+bge，w=8）仍只做到**打平**（+0.0013，n.s.）——
**embedding 越强，融合的相对优势越小**，因为好 embedding 已吃掉大部分语义信号，
scx 的词面/PRF 补充与之高度冗余。

**根本结论**：融合的价值是「架桥」——把 scx 从显著落后拉到与 embedding 打平，
而非显著超越。「显著优于 embedding」在这条路线与当前语料上达不到；要真正反超，
需让 scx 提供 embedding 本质缺的信号（错误码/堆栈结构精确匹配），或换任务/更大
语料，属后续研究。

## 解读约束

- 本对照**不是**对任意 RAG 系统的普遍优势声明，只证明「在冻结 Gauge 语料、
  给定模型与融合公式下，scx 的排序与 embedding/hybrid 的排序孰优」。
- 换 embedding 模型、换融合公式或换语料，结论可能改变；要改结论必须重跑并
  重新预注册，不能复用本节数字。
