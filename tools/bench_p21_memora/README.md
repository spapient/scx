# P21 — Memora（arXiv:2604.20006, ACL 2026 Findings）scx 记忆体系接入与冻结基线

## 1. 基准与官方协议

- 论文：*From Recall to Forgetting: Benchmarking Long-Term Memory for Personalized Agents*
  （arXiv:2604.20006，ACL 2026 Findings）。
- 官方仓库：https://github.com/geniesinc/Memora ，冻结 commit
  `a6493188efc836d6511ed5e4163fe3ba87da30ff`（2026-06-03），数据随仓库发布，
  无需额外下载。
- 三任务（`evaluation_questions_*.json` 顶层 `questions` 键）：
  - `remembering`：记住已沉淀的事实（`memory_presence` 子问题）；
  - `reasoning`：跨多条记忆做数值/关系推理（仅 `memory_presence`，无遗忘子题）；
  - `recommending`：基于当前偏好推荐（`memory_presence` + `forgetting_absence`，
    后者要求回答**不**援引已失效/已被否定的旧偏好）。
- 三时间窗：`weekly`（10 persona × 15 题）、`monthly`（10 × 15）、
  `quarterly`（10 × 30）；共 600 题、6415 个 yes/no 子问题。
- 指标（对齐官方 `evals/agent_eval/memory_to_answer.py::fama_score()`，逐字实现于
  `run_p21.py::fama_score`）：
  - `MPA = correct memory_presence / total memory_presence`
  - `FAA = correct forgetting_absence / total forgetting_absence`
    （无遗忘子题时 `FAA = 1.0`，与官方一致）
  - `λ = N_forget / (N_presence + N_forget)`；reasoning 任务 `N_forget = 0`，`λ = 0`
  - `FAMA = max(0, MPA − λ·(1−FAA))`；桶内 FAMA = 桶内逐题 FAMA 均值 × 100
    （[0,100]，对齐官方 `by_task_type.fama` 口径）。
  - `penalty = λ·(1−FAA)`：过时/失效记忆被错误援引带来的惩罚项，治理对照的核心观测量。

## 2. 目录与落地物

| 路径 | 内容 |
|---|---|
| `tools/bench_p21_memora/run_p21.py` | 主编排：摄取、作答、判分、聚合、出报告 |
| `tools/bench_p21_memora/p21_scx.py` | scx CLI 封装 + 治理注册表（P2 仲裁 / P3 revise/retire） |
| `tools/bench_p21_memora/p21_provider.py` | DeepSeek 通道：作答/判分调用、重试退避、逐条审计落盘 |
| `tools/bench_p21_memora/README.md` | 本复现说明 |
| `/tmp/scx_bench_memora/Memora/` | 官方仓库克隆（只读引用） |
| `/tmp/scx_bench_memora/mem/<branch>/<period>__<persona>/` | 两分支独立 scx 记忆库 + `registry.json` 治理审计 |
| `/tmp/scx_bench_memora/results/<branch>/<period>__<persona>.json` | 逐题详细结果（作答、逐条裁判、MPA/FAA/FAMA） |
| `/tmp/scx_bench_memora/llm_calls.jsonl` | 全部 LLM 调用逐条审计（请求摘要、响应、usage、耗时、重试、错误） |
| `/Users/fei/code/scx/config/bench/results/bench_p21_memora.json` | 冻结自描述结果（schema/版本/commit/命令/seed/数字表/口径/双面结论/成本） |

## 3. 摄取 recipe（可复现、确定性）

按 `session_NNNN` 数字顺序逐会话摄取；每个 `period × persona` 独立记忆库
（对齐官方 `user_id = <persona>_<period>` 隔离口径）。

1. 会话内 `share_memory = true` 的用户消息 + `operation_details` 结构化事实拼成
   case 文本（日期 / session_type / operation / 用户消息 / 结构化事实 JSON）；
   `query` = 类型 + 操作 + 日期 + 实体；`fix` = 压缩事实摘要（偏好项显式写出
   `preference item (subcategory)`，旧偏好标注 `was …`）。
2. `operation = add`：普通 `--store 1` 入库，注册表记录稳定键 → case id。
3. `operation = update`：
   - 偏好切换（`old_item` 存在）：gov 分支先 `--retire` 旧偏好实体，再入库新偏好；
     nogov 分支旧实体保持 active，仅追加新 case。
   - 同一实体更新（`old_preference` 翻转或活动/日程内容更新）：gov 分支对命中的
     旧 case 执行 `--revise <full-id>`；未命中记 `revise_miss` 并新建；nogov 分支
     旧 case 保持 active，追加新 case。
4. `operation = delete`：gov 分支 `--retire` 命中旧 case（理由含会话号、日期、
   失效事实）；删除会话中的转折陈述另存为观察 case，不复活被删记忆；nogov 分支
   旧 case 不退役、保持 active 可召回。
5. 稳定注册表键：偏好 = `pref:<item>:<subcategory>`；其余实体 = 剔除
   `created_at/updated_at/date/session_history/…` 等易变字段后的规范 JSON，保证
   同一实体跨 update/delete 可匹配。
6. `no_memory` / 闲聊会话（无 `share_memory` 事实）不入库。

### 3.1 两分支治理口径（对照实验）

| 维度 | `nogov`（无治理基线） | `gov`（启用治理） |
|---|---|---|
| P2 冲突仲裁 | `meta.toml [params] conflict_sim = 1.0`，等价关闭近重复合并/仲裁 | 默认 `conflict_sim = 0.8`，store 时自动仲裁 |
| `update` | 不 revise，新旧版本并存 active | 命中即 `--revise`；偏好切换退役旧实体 |
| `delete` | 不 retire，过时记忆保持 active | 命中即 `--retire`，召回自动排除 |
| 目的 | 暴露“不治理 → 过时记忆污染作答 → FAA 下降、惩罚项上升” | 量化版本化 + 仲裁对 FAMA 惩罚项的实际削减 |

## 4. 评测协议

1. 对每题 `--recall-only --recall 20 --recall-detail` 召回；上下文使用入库压缩好的
   `fix` 事实摘要（每条 ≤500 字符，总量 ≤12000 字符），附 relevance 分数。
2. 作答：官方 Track 2 system/user prompt 逐字复用（`run_p21.py` 顶部常量）；
   模型 `deepseek-flash`，temperature 0，max_tokens 500，reasoning 关闭。
3. 判分：官方单裁判 system/user prompt 逐字复用；模型 `deepseek-chat`，
   temperature 0，max_tokens 400；JSON 解析失败按官方 `_evaluate_with_single_judge`
   文本兜底（yes/no/unclear）；裁判与作答两分支完全相同，保证对照可比。
4. 逐题统计 presence/absence 命中数 → MPA/FAA/λ/FAMA/penalty；桶内聚合见 §1。

## 5. 口径差异（与官方 Table 3 不可直接比，对照增益可比）

- 作答模型：官方 gpt-4o-mini（temperature 0.7）；本基准 deepseek-flash（temperature 0）。
- 裁判：官方三裁判多数票（gpt-4.1 / claude-haiku-4.5 / gemini-2.5-flash，OpenRouter）；
  本基准单裁判 deepseek-chat。
- 召回深度：官方 `limit=50`；本基准 K=20（上下文预算约束，`--recall-k` 可调）。
- 记忆记录形态：官方各 agent 自有抽取器；本基准 case = share_memory 消息 + 结构化事实，
  作答上下文用压缩 `fix` 而非原始多轮文本。
- 以上差异对两分支一致施加；`gov − nogov` 的 ΔFAMA / ΔFAA / Δpenalty 反映治理净效应。

## 6. 复现命令

```bash
# 0) 前置：scx 二进制已构建；DeepSeek key 在 ~/.zshrc（DEEPSEEK_API_KEY）
cd /Users/fei/code/scx

# 1) 官方仓库（已克隆到 /tmp/scx_bench_memora/Memora，commit a6493188）
git clone https://github.com/geniesinc/Memora /tmp/scx_bench_memora/Memora

# 2) 全量摄取两分支（本地 CLI，确定性；可 --periods/--personas 子集重跑，断点续跑）
/tmp/scx-eval/bin/python tools/bench_p21_memora/run_p21.py ingest

# 3) 全量评测（默认 sample-frac=1.0，seed=21，temperature 0）
/tmp/scx-eval/bin/python tools/bench_p21_memora/run_p21.py eval

# 4) 汇总生成冻结结果 JSON
/tmp/scx-eval/bin/python tools/bench_p21_memora/run_p21.py report

# 子集 / 抽样重放示例（固定 seed 分层抽样 50%，结果需另存，勿覆盖冻结 JSON）
/tmp/scx-eval/bin/python tools/bench_p21_memora/run_p21.py eval --sample-frac 0.5 --seed 21
```

子命令参数：`--periods weekly monthly quarterly`、`--personas <name…>`、
`--branches nogov gov`、`--recall-k 20`、`--model deepseek-flash`、
`--judge-model deepseek-chat`、`--resume-overwrite`（覆盖已存在的逐题结果）。

## 7. 门禁

仅新增 `tools/bench_p21_memora/` 下 Python 文件，未改 `src/**`、`tests/**`、
`Cargo.toml`、`config/bench/eval-manifest.json`、`tools/check_bench.sh`、`docs/**`、
`README.md` 与既有 `tools/*.py`。改动后必须跑：

```bash
cargo test --locked --features memory --lib   # 基线 397 passed
```
