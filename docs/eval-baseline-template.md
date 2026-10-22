# eval-baseline.json 标注采集模板（人工标注指南）

版本：v1.0（2026-10-22）
用途：为 P2/P4 阶段门收集 30-50 组分层人工标注，喂给 `scx --eval <baseline.json>` 出正式 A/B 对拍表与 L1 命中率。

> 采集原则：只标真实库里的 case（expected 必须是库中实际存在的 case id），query 变体按真实使用场景设计，
> 宁可少标不编造。标注集一经确定即冻结（--eval 确定性重放），后续库变化不影响对拍结论的可重放性。

---

## 1. baseline.json 格式

顶层为对象，唯一必填键 `cases`（数组）；每条 case 四个字段：

```json
{
  "cases": [
    { "query": "db timeout conn_id", "layer": "literal", "expected": ["2026-10-22_0adada28"], "rel": {"0adada28": 2} },
    { "query": "数据库连接池超时 连接池", "layer": "paraphrase", "expected": ["0adada28"] },
    { "query": "存储空间不足 errno 磁盘", "layer": "semantic", "expected": ["2026-10-22_5f82b3ab"], "rel": {"5f82b3ab": 1} }
  ]
}
```

字段定义：

| 字段 | 类型 | 说明 |
|---|---|---|
| query | string | 评估时用的检索词（按真实用法写，可含字段/实体/中文混排） |
| layer | string | literal / paraphrase / semantic 三选一（分层定义见 §2） |
| expected | string[] | 相关 case id 列表（完整 id 或 hash16/hash8 后缀均可，后缀免日期漂移） |
| rel | object（可选） | case id → 相关度 0..2；缺省时 expected 内视为 1 |

## 2. 三层定义与造例规则

| 层 | 含义 | query 怎么造 | 示例（对同一 case） |
|---|---|---|---|
| literal | 词面命中 | 与 case.query / 证据块用词基本一致 | `db timeout conn_id` |
| paraphrase | 同义改写 | 换同义词/语序/中英混排，词面有偏移 | `数据库连接池超时 连接池`、`连接池 又 超时了` |
| semantic | 语义相关 | 说同一件事但几乎无共享词（最难的层） | 主题相同但换了 token：`存储空间不足 errno 磁盘` |

造例顺序：先从库选一个"真实发生过的问题"case，写它的 literal 原文 → 改写成 paraphrase → 再剥离成 semantic。每层至少 10 组（共 30-50 组）。

## 3. rel 分级语义（0..2）

| 值 | 语义 | 典型场景 |
|---|---|---|
| 2 | 强相关：该 case 直接就是答案 | 同 query 二次遇到、字段/实体精确对应 |
| 1 | 相关：参考价值，但不是直接答案 | 兄弟 case、相似根因 |
| 0 | 无关 | 干扰项（也可只列 expected 相关 case，0 可省） |

## 4. 怎么拿到真实 case id

```bash
# 对库中某 query 召回，看返回 id（--recall-detail 还带要点行，方便辨认是不是同一个问题）
scx --query "db timeout conn_id" --recall-only --recall 5 --recall-detail --mem-dir <库目录>
# 输出 mem: 段，每行一个 case：id score sim bucket [summary: ...]
```

取 `summary` 行的 error_code/file_loc 与你的问题核对，确认真是同一个根因再标 rel=2。

## 5. 推荐采集流程（照做即可）

1. 挑 30-50 个真实问题（日志/报错/diff 复盘记录），每个问题选 1 个代表 case。
2. 每个 case 写 1 条 literal + 1 条 paraphrase（同一问题可多条变体）；semantic 层按主题另造。
3. 全部 expected 用 `--recall-detail` 核对过 id 后填入。
4. 保存为 `baseline.json`（建议放 `docs/eval/` 下，与日期/库版本一起留档）。
5. 跑对拍：`scx --eval baseline.json --mem-dir <库目录>`，输出三层 A/B 指标 + `l1: hits/total/rate`。
6. 把报告行贴进 `docs/rag-implementation-plan.md` §7 P4.1 的验证记录（M4 阶段门证据）。

## 6. 约束与红线

- expected 必须是库中真实存在的 id——`--eval` 会按库实际召回计算，写错 id 等于污染结论。
- 同一 baseline 一旦用于结论，禁止再改（改了要重跑全流程并注明版本）。
- 分层标签要稳定：literal 只含词面命中例；paraphrase 必须与 literal 词面不同；semantic 几乎无共享词。
- 数量口径：30-50 组是下限建议，多多益善；每层独立统计，层内样本太少（<5）时报告会暴露 n，请勿拿单例下结论。
