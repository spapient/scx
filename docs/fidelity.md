# Fidelity（压缩保真评测，P1）

Gauge 演进路线 P1 交付：在 contain5 的证据行保真骨架上，把评测从
「证据行逐字保留」升级为「压缩后是否仍然**可回答**」——零依赖、确定性、
无 LLM 的下游可回答性代理。

## 口径

- **可回答性代理**：对每个 slice，两条同时成立判「可回答」：
  1. 全部证据行**逐字**保留在压缩输出中（contain 语义）；
  2. query 的实质 token（去停用词）在保留证据行中的**覆盖率 ≥ 0.5**。
  证据保留不足不判可回答（截断片段可能残留证据行但丢关键行），token
  覆盖不足也不判（保留的证据行可能与 query 无关）。
- **token 信息保留**（**报告指标，不入门禁**）：原文实质 token 集合出现在
  压缩输出中的召回率。sieve 刻意去重去噪，输出 token 集合小，集合召回
  按构造输给 head-truncation（保留头部密集 token）与随机行采样（整行
  保留）——实测 66.99% vs trunc 73.03% / rand 72.12%，如实报告并注明
  口径局限；该指标不反映压缩质量排序。
- **基线**：与 scx 输出**同字节预算**的 head-truncation 与确定性随机行
  采样（seed `fidelity-rand-v1`）。

## 语料

`config/fidelity.corpus.json`：25 slice（build / stack / diff / log / test
各 5）。每 kind：4 个**大 slice**（45–51 行，触发 sieve 压缩路径）+ 1 个
**小 slice**（4–14 行，走 passthrough:small 透传路径，对照组）。

- 确定性生成：`python3 tools/gen_fidelity.py`（seed=20261026，重跑逐字节
  复现）；每 slice 含 `query` / `full_text` / `evidence`（逐字在原文中）/
  `note`；provenance 与生成说明写入 corpus JSON。
- SHA256：`e89c7f23124a986695c0111cd834fe04c1617e62ec0d06e7de600909ba093d63`。

## 复现

```bash
cargo build --release --locked
./tools/scx.py fidelity config/fidelity.corpus.json --out /tmp/fidelity-detail.json
./tools/scx.py fidelity config/fidelity.corpus.json --gate
```

`--gate` 冻结门禁（CI 强制执行）：

| 指标 | 冻结阈值 | 本轮实测 |
| --- | --- | --- |
| 证据保留（scx） | ≥ 0.95 且 **> trunc** | 55/56 = 0.9821（trunc 0.9107 / rand 0.5893） |
| 可回答率（scx） | ≥ 0.90 且 **> trunc** | 23/25 = 0.9200（trunc 0.7600 / rand 0.5600） |
| token recall | 报告（不入门禁） | 0.6699（trunc 0.7303 / rand 0.7212） |
| mean char cut | 报告 | 40.06%（token cut 40.82%） |

## 本轮实测行为（2026-10-26）

- 模式分布：sieve 20 / passthrough:small 5（两条压缩路径都被评测覆盖）。
- 未回答 2 例（build-02 / build-04，均 sieve）：语料每例含 4 条错误证据
  行、query 只聚焦其一；sieve 按 query 聚焦保留证据种子行，其余错误块
  被筛掉 → 证据行不完整。**这是评测价值所在**：如实揭示「证据种子机制
  是 query 聚焦的，不是全保真」；也说明门禁取 0.95（非 1.0）是正确余量。
- 确定性：语料与基线全确定性（seed 固定）；scx 输出确定性，重跑数字一致。

## 局限

- 自测性质：作者侧合成语料自测，非第三方独立评测（同 contain5）。
- 可回答性是代理（证据行 + query token 覆盖），未接入真实 LLM 下游问答。
- token recall 口径对去噪声压缩器不公平，只作信息报告。

## 与既有评测的关系

- `contain5`：证据行逐字保留 vs 同预算截断/随机（行级保真基座）。
- `fidelity`（本文件）：在 contain5 语义上加 query 相关性判定，
  把「行保留」提升为「可回答」；语料刻意覆盖 sieve 与透传两条路径。
- `Gauge`：记忆检索 A/B（MRR/显著检验）；fidelity 度量压缩本身的保真，
  两者互补。
