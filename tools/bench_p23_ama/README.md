# P23 · AMA-Bench（arXiv:2602.22769v4）scx 冻结基线适配器

> 基准：*AMA-Bench: Evaluating Long-Horizon Memory for Agentic Applications*
> 官网 <https://ama-bench.github.io/> ｜ 论文 arXiv:2602.22769v4 ｜ 代码 `AMA-Bench/AMA-Hub`
> 本目录只新增 Python 适配器；未改动 `src/**`、`tests/**`、`Cargo.toml`、既有 `tools/*.py`
> 与任何冻结清单。门禁① `cargo test --locked --features memory --lib` = **397 passed, 0 failed**。

## 1. 资源与版本（冻结口径，写入结果 JSON `official` 节）

| 项 | 值 |
|---|---|
| HF 数据集 | `AMA-bench/AMA-bench`，文件 `test/open_end_qa_set.jsonl` |
| 数据 sha256 | `45c36052e1520d87ad9de4114f71c9df42d4aac9cf158c0c353e800b653d65ff`（与 HF LFS oid 一致） |
| 规模（real-world） | 208 episodes × 12 QA = 2,496 QA；6 领域；QA 类型 A=Recall 839 / B=Causal Inference 596 / C=State Updating 647 / D=State Abstraction 414 |
| 官方代码仓库 | <https://github.com/AMA-Bench/AMA-Hub>，commit `ddfd319e0be33424288c13806f1eafc63e625b59` |
| Leaderboard | <https://huggingface.co/spaces/AMA-bench/AMA-bench-Leaderboard> |

**Synthetic 子集（BabyAI + TextWorld，1,200 QA，8K–128K 轨迹）公开渠道不可得**，详见
结果 JSON `synthetic_subset` 节：已尝试路径、受阻原因、为何不能本地复现；本适配器**未使用、
未自造**任何 synthetic 数据。公开数据中也不存在 MCQ 子集（论文 §6.1 明确全部为 open-ended
generation；`run.py` 内 `mcq_set.jsonl` 是遗留代码路径）。

## 2. 文件

| 文件 | 职责 |
|---|---|
| `p23_provider.py` | DeepSeek 通道：答案生成 + 官方二元 judge；官方 answer/judge prompt 逐字；官方 F1/答案抽取口径移植；逐条调用落盘 `llm_calls.jsonl`；curl 硬超时 + 5 次指数退避重试 |
| `p23_scx.py` | scx memory 适配器：turn → case 摄取、turn 级召回、per-episode 隔离 mem-dir、摄取审计与注册表 |
| `p23_bm25.py` | 官方 BM25 基线零依赖复刻（rank_bm25 默认参数，同 turn 文档粒度、top_k=10），作为同语料对照通道 |
| `run_p23.py` | 编排：摄取 → 召回 → 答案 → judge → 修复轮 → 聚合 → 自描述结果 JSON；断点续跑；固定种子分层抽样 |

## 3. 协议对齐（只读引用官方实现，未改写）

- **offline per-query**（论文 §6.1）：每个 episode 轨迹摄取一次；每个 QA 对固定记忆独立作答，
  QA 之间不更新记忆；两通道同一语料、同一 `K=10`。
- **答案 prompt**：`AMA-Hub src/memory_interface.py` L168–175（逐字）；答案抽取
  `utils/extract_final_answer.py`（`Answer[1]:` 正则 + `<think>` 剥离）。
- **judge prompt**：`utils/evaluation_metrics.py` L153–169（逐字）；决策取末个 `yes/no`
  （L143–151）；无法解析记 incorrect。
- **F1**：`utils/evaluation_metrics.py compute_f1_score` 逐字移植，离线机械计算，不经 LLM。

### 轨迹（非对话）→ scx case 映射规则（确定、可复现，摄取不调 LLM）

| scx case 字段 | 内容 |
|---|---|
| `text` | 官方 `_trajectory_to_text` 同款 turn 块：`Step n:\nAction: …\nObservation: …` |
| `query` | `{domain} {task_type} step {n} action {动作头 80 字符}` |
| `fix` | 规则摘要：`step n: action={动作头 120}; observation={观察头 160}` |
| 粒度 | 一个 turn 一个 case（与官方 BM25 文档粒度一致，保证两通道可比） |
| 隔离 | 每 episode 独立 mem-dir；空 action/observation 记 `(null)`/`(empty)`；超过 scx
  `MAX_CASE_TEXT=1MiB` 的 turn 头尾各留 480 KiB 并在审计中标记 `truncated` |

## 4. 口径差异（结果 JSON `models` / `judge_protocol` 节完整记录）

| 环节 | 官方 | 本适配器 |
|---|---|---|
| 答案模型 | Qwen3-32B（Table 5 backbone） | `deepseek-chat`，temperature 0，max_tokens 2048 |
| Judge | Qwen3-32B 二元 yes/no（Appendix C.1，300 例人工一致率 92.67%） | `deepseek-chat`，temperature 0，max_tokens 16 |
| 检索 | 官方各基线各自实现 | scx（生产 RRF：BM25 秩 + 字段/实体秩 + MinHash 秩）+ 复刻 BM25 |

> 因 backbone 与 judge 不同，**绝对值不可与论文 Table 4/5 直接比较**；结果 JSON
> `official_reference` 只保留论文数字用于量级定位。

## 5. 复现命令

环境：Python `/tmp/scx-eval/bin/python`（3.12.15，openai/requests/huggingface_hub 已装）；
`DEEPSEEK_API_KEY` 来自 `~/.zshrc`；scx 二进制 `target/release/scx`（v0.11.0，memory 默认开）。

```bash
# 0) 数据已校验（sha256 见上表）；如重新下载：
curl -4 -L "https://huggingface.co/datasets/AMA-bench/AMA-bench/resolve/main/test/open_end_qa_set.jsonl" \
  -o /tmp/scx_bench_ama/open_end_qa_set.jsonl

# 1) 全量基线（208 episodes / 2,496 QA / 两通道）
cd /Users/fei/code/scx
/tmp/scx-eval/bin/python tools/bench_p23_ama/run_p23.py \
  --work /tmp/scx_bench_ama \
  --results config/bench/results/bench_p23_amabench.json

# 2) 固定种子分层抽样（兜底口径；每 QA 类型 N 个 episode，seed=42）
/tmp/scx-eval/bin/python tools/bench_p23_ama/run_p23.py \
  --sample-per-type 200 --seed 42 --work /tmp/scx_bench_ama

# 3) 2 episode 冒烟（时延/口径校验）
/tmp/scx-eval/bin/python tools/bench_p23_ama/run_p23.py --pilot --work /tmp/scx_bench_ama
```

断点续跑：`answers.jsonl` 按 `(question_uuid, channel)` 去重；中断后重跑同一命令自动跳过已完成行；
失败行由**修复轮**重试（结果 JSON `repair` 节记录 `repaired` / `residual_failures`）。

## 6. 落地物与日志

| 路径 | 内容 |
|---|---|
| `config/bench/results/bench_p23_amabench.json` | 冻结结果：schema、日期、scx 版本、官方资源 commit+sha、命令、seed、数字表、口径、适用边界、双面结论、judge 口径与成本 |
| `/tmp/scx_bench_ama/answers.jsonl` | 逐 QA×通道行：命中 turn 序列、上下文长度、预测、judge 决策、F1、调用 id |
| `/tmp/scx_bench_ama/llm_calls.jsonl` | 逐条 LLM 调用：模型、prompt/usage、时延、重试、原始响应、错误 |
| `/tmp/scx_bench_ama/ingest_audit.jsonl` | 逐 episode 摄取审计：stored/dedup/truncated/failed 与错误明细 |
| `/tmp/scx_bench_ama/mem/ep_*` | 每 episode 独立 scx 记忆库 + `p23_registry.json`（case ↔ turn 映射） |

## 7. 已知边界（见结果 JSON `applicability_boundary`）

- scx 原生面向事故/复盘证据型 case（query + fix 结论）；本任务以 turn 为最小证据单元映射，
  未构建跨 turn 因果图，B/C/D 类跨步关系是已知短板。
- real-world 子集未标注证据 turn 编号，无法机械计算 recall@K；只报告答案级 accuracy/F1 与召回诊断。
- 超长工具输出受 1 MiB case 上限截断；截断 turn 细节不可恢复（审计可追溯）。
- synthetic 长轨迹尺度（8K–128K）本次未覆盖。
