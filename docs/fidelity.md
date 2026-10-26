# Fidelity（压缩保真评测，P1 + P2）

Gauge 演进路线 P1/P2 交付：在 contain5 的证据行保真骨架上，把评测从
「证据行逐字保留」升级为「压缩后是否仍然**可回答**」——零依赖、确定性、
无 LLM 的下游可回答性代理。P2 将语料扩展到第二故障域（数据库 / 网络 /
硬件），并加入 per-domain 零回退门禁。

## 口径

- **可回答性代理**：对每个 slice，两条同时成立判「可回答」：
  1. 全部证据行**逐字**保留在压缩输出中（contain 语义）；
  2. query 的实质 token（去停用词）在保留证据行中的**覆盖率 ≥ 0.5**。
  证据保留不足不判可回答（截断片段可能残留证据行但丢关键行），token
  覆盖不足也不判（保留的证据行可能与 query 无关）。
- **证据行语义**：evidence = 回答该 slice 的 query 所需的错误行（query
  聚焦行），不是文本里所有错误块。build 语料曾把非 focus 错误块也标为
  证据（导致 94.1% 假回退），P2 修正为只标 query 所需证据行。
- **token 信息保留**（**报告指标，不入门禁**）：原文实质 token 集合出现在
  压缩输出中的召回率。sieve 刻意去重去噪，输出 token 集合小，集合召回
  按构造输给 head-truncation（保留头部密集 token）与随机行采样（整行
  保留）——如实报告并注明口径局限；该指标不反映压缩质量排序。
- **基线**：与 scx 输出**同字节预算**的 head-truncation 与确定性随机行
  采样（seed `fidelity-rand-v1`）。

## 语料

`config/fidelity.corpus.json`：40 slice，**8 kinds**（build / stack / diff /
log / test 开发域 + db / network / hardware 第二故障域，各 5）。每 kind：
4 个**大 slice**（45–59 行，触发 sieve 压缩路径）+ 1 个**小 slice**
（3–14 行，走 passthrough:small 透传路径，对照组）。

- 确定性生成：`python3 tools/gen_fidelity.py`（seed=20261026，重跑逐字节
  复现）；每 slice 含 `query` / `full_text` / `evidence`（逐字在原文中）/
  `note`；provenance 与生成说明写入 corpus JSON。
- SHA256：`32194397471d91fff574123b44b857be8981eab68099793e606755773d09f997`。

## 复现

```bash
cargo build --release --locked
./tools/scx.py fidelity config/fidelity.corpus.json --out /tmp/fidelity-detail.json
./tools/scx.py fidelity config/fidelity.corpus.json --gate
```

`--gate` 冻结门禁（CI 强制执行）：

| 指标 | 冻结阈值 | 本轮实测（P2，40 slice） |
| --- | --- | --- |
| 证据保留（全局） | ≥ 0.95 且 **> trunc** | 65/65 = 100%（trunc 89.23% / rand 72.31%） |
| 可回答率（全局） | ≥ 0.90 且 **> trunc** | 40/40 = 100%（trunc 82.50% / rand 75.00%） |
| per-kind 证据保留 | 每域 ≥ 0.95（零域回退） | 8/8 kinds 均 100% |
| token recall | 报告（不入门禁） | 74.58%（trunc 81.83% / rand 80.99%） |
| mean char cut | 报告 | 27.44%（token cut 28.34%） |

per-kind 门禁只断言绝对阈值（零域回退），不与 trunc 逐域比较：5 slice
样本太小，trunc 头部可能恰好覆盖证据行（db/diff/hardware/log/stack 的
trunc=100%），"逐域严格优于 trunc" 过于脆弱。全局（40 slice）仍要求
严格优于 trunc。

## 本轮实测行为（2026-10-26，P2）

- 模式分布：sieve 32 / passthrough:small 8（两条压缩路径都被评测覆盖）。
- 第二故障域（db / network / hardware）全部可回答且证据全保留，跨域
  零回退：db（连接池/复制/死锁/锁超时）、network（DNS/TCP/TLS/路由）、
  hardware（磁盘/内存/NVMe/网卡）共 15 个新 slice 全绿。
- 证据语义修正（P2）：build 语料只把 query 聚焦错误行标为证据后，
  build 从 94.1% → 100%（此前把 query 无关错误块误标为证据）。
- 确定性：语料与基线全确定性（seed 固定）；scx 输出确定性，重跑数字一致。

## 架构（P2 优化）

- `validate_fidelity_corpus`：kinds 由 corpus `expected_kinds` **数据驱动**
  （不再硬编码 5 kind），20–60 slice、每 kind ≥3 例（per-kind 统计有意义）。
- `cmd_fidelity` 拆分为三个小函数：`_fidelity_aggregate`（折叠行 → 全局 +
  per-kind 聚合）、`_print_fidelity`（报告）、`_fidelity_gate_failures`
  （门禁判定，返回失败列表）——职责单一、可单测。
- 新增 per-kind 聚合输出与 per-kind 零域回退断言。

## 局限

- 自测性质：作者侧合成语料自测，非第三方独立评测（同 contain5）。
- 可回答性是代理（证据行 + query token 覆盖），未接入真实 LLM 下游问答。
- token recall 口径对去噪声压缩器不公平，只作信息报告。

## 与既有评测的关系

- `contain5`：证据行逐字保留 vs 同预算截断/随机（行级保真基座）。
- `fidelity`（本文件）：在 contain5 语义上加 query 相关性判定，
  把「行保留」提升为「可回答」；语料刻意覆盖 sieve 与透传两条路径，
  P2 起覆盖开发 + 数据库 + 网络 + 硬件八域。
- `Gauge`：记忆检索 A/B（MRR/显著检验）；fidelity 度量压缩本身的保真，
  两者互补。
