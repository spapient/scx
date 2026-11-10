# 评测实测指标（自动生成）

> 由 `scx-tooling report` 生成；CI 断言与本文件逐字节一致，文档数字一律以本文件为准。

## Gauge（150 查询，seed 20261023，tier:hard）

| 指标 | n | A | B | Δ | 95% CI | perm p |
| --- | --- | --- | --- | --- | --- | --- |
| MRR | 69 | 0.3650 | 0.2783 | +0.0866 | [+0.0387,+0.1416] | 0.0002 |
| recall@3 | 69 | 0.4203 | 0.3043 | +0.1159 | [+0.0290,+0.2029] | 0.0217 |

## 留一事故泛化（30 incidents 逐一剔除）

- min Δ **+0.0739** / mean +0.0866 / max +0.0906；min CI 下界 +0.0287
- 门禁：PASS

## Fidelity（40 slice / 8 kinds）

- 证据保留 65/65 = 100.00%（trunc 89.23%）
- 可回答率 40/40 = 100.00%（trunc 82.50%）
- token recall 74.58%（报告-only）
- per-kind 证据保留：8/8 域 ≥95%
- 跨域优势（cluster=kind，单侧 cluster-bootstrap）：mean Δ +11.07pp，p=0.0231（3/8 域正差）

## 多重比较（Holm-Bonferroni，Gauge 族 m=2，α=0.05）

| 检验 | perm p | Holm 阈值 | 校正后 |
| --- | --- | --- | --- |
| MRR hard | 0.0002 | 0.0250 | 显著 |
| recall@3 hard | 0.0217 | 0.0500 | 显著 |

fidelity 的 ev/ans 是阈值门禁（非显著性检验），无 p 需校正。

## 汇总门禁：PASS
