# P22 · scx 接入 CompactBench 官方 harness（社区版）适配器

本目录把 **scx v0.11.0**（本地逐行保留式机械压缩）实现为 CompactBench
官方 `Compactor` 接口的一个压缩方法，注册进官方 harness，在官方事件流 / 官方
机械指标 + 官方目标模型作答判分通道下跑出 scx 基线。**没有自写评分脚本**：
`run_experiment` / `execute_cycle` / `score_cycle` / `aggregate_run_metrics`
全部来自安装的 `compactbench` 包；本目录只提供适配器、provider 与驱动。

## 1. 环境与版本（冻结口径）

| 项 | 值 |
|---|---|
| scx 二进制 | `/Users/fei/code/scx/target/release/scx`，版本 `0.11.0` |
| harness（pip） | `compactbench==0.1.0`，Python 3.12.15（`/tmp/scx-eval/bin/python`） |
| harness 源码参考仓库 | `https://github.com/compactbench/compactbench`，clone 于 `/tmp/scx_bench_compact/repo`，commit `b2bb0e2254010e7842801b9101e56fd9a6542615`（master 0.2.0；**本地跑的是 pip 0.1.0**，两者接口 `Compactor` ABC 一致，scorer 版本 `1.0.0`） |
| 目标模型（作答/续写） | DeepSeek `deepseek-flash`，endpoint `https://api.deepseek.com/chat/completions`，`temperature=0`，`max_tokens≥2048`，flash 关闭 reasoning |
| 网络 | 本机直连 api.deepseek.com 间歇超时/重置；走本机既有 ssh 本地 HTTP 代理 `127.0.0.1:8080`（`SCX_P22_HTTPS_PROXY` 可覆盖，置空串强制直连） |
| 套件 | `/tmp/scx_bench_compact/repo/benchmarks/public`（`starter` 4 模板；`elite_practice` 20 模板） |
| seed | `seed_group=default`；case slot 由 harness `derive_case_seed` 确定性派生；scx 压缩路径无 RNG（确定性）；LLM `temperature=0` |

## 2. 文件

| 文件 | 职责 |
|---|---|
| `scx_compactor.py` | `ScxCompactor`：实现官方 `Compactor.compact()`；本地子进程调 scx；规则化结构化状态；跨 cycle 账本累积；无 LLM 调用 |
| `deepseek_provider.py` | `DeepSeekProvider`：实现官方 `Provider` ABC，OpenAI 兼容线协议，`curl -4` 传输 + 指数退避重试；调用逐条落盘 |
| `p22_logger.py` | LLM 调用 JSONL 审计日志（时间/模型/prompt 摘要/response/usage/耗时/重试；key 不落盘） |
| `run_p22.py` | 驱动：注册 provider → 调官方 `run_experiment` → 汇总官方 `RunResult` 指标 → scx 机械保留率离线审计 |
| `run_smoke2.sh` | 启动包装（透传参数） |

## 3. scx 压缩口径（冻结）

```
<scx_bin> <tmpfile> --query <signal> --ratio 0.35 --context 1 \
    --fill-floor 2 --list-keep 1 --force <signal-turn 1> [--force <signal-turn 2> ...]
```

- **signal 轮**：所有带 `critical_constraint / decision_override / first_override /
  final_override / partial_override / late_override / meta_reset / confusion_risk`
  标签的轮次 + 首个 user 轮；其内容拼成 `--query`，并逐轮 `--force` 整行保留。
- **压缩文本**：stdout 第一条 `compressed:` 行之前的全部行（共享规格 §7 范式）；
  失败 fail-open 返回原文，`methodMetadata.scx.mode` 记录 `failopen:*`。
- **不做 token 级 trim**：命中行整行保留，保证动作短语/人名整串存活（机械提取口径）。
- **结构化状态**（纯确定性规则，无 LLM）：
  - `critical_constraint` 轮 → 否定短语进 `forbidden_behaviors`（规则未命中整行兜底）；
  - override 轮按顺序抽取候选，最后一个进 `locked_decisions`，之前的进 `forbidden_behaviors`；
  - 首 user 轮 + confusion 轮原文进 `immutable_facts`；句中大写词进 `entity_map`；
  - 末 user 问句进 `unresolved_items`；跨 cycle 账本合并 `locked/forbidden/unresolved/entity_map`。

## 4. 安装与运行

```bash
# 依赖（Organizer 已装；失败重试一次）
/tmp/scx-eval/bin/pip install compactbench==0.1.0

# 官方仓库（套件模板 + 文档）
git clone --depth 1 https://github.com/compactbench/compactbench /tmp/scx_bench_compact/repo

# 主基线：elite_practice / elite / scx，3 case/模板，2 drift cycle
cd /Users/fei/code/scx/tools/bench_p22_compact
/tmp/scx-eval/bin/python run_p22.py \
  --suite elite_practice --difficulty elite \
  --case-count 3 --drift-cycles 2 --seed-group default \
  --model deepseek-flash --methods scx \
  --benchmarks-dir /tmp/scx_bench_compact/repo/benchmarks/public \
  --out-dir /tmp/scx_bench_compact/results \
  --llm-log /tmp/scx_bench_compact/llm_calls.jsonl \
  --summary-json /Users/fei/code/scx/config/bench/results/bench_p22_compactbench.json

# 同目标模型基线对比：starter / medium，scx + 四个内置 compactor
/tmp/scx-eval/bin/python run_p22.py \
  --suite starter --difficulty medium \
  --case-count 3 --drift-cycles 2 --seed-group default \
  --model deepseek-flash \
  --methods scx,naive-summary,structured-state,hybrid-ledger,hierarchical-summary \
  --benchmarks-dir /tmp/scx_bench_compact/repo/benchmarks/public \
  --out-dir /tmp/scx_bench_compact/results_starter \
  --llm-log /tmp/scx_bench_compact/llm_calls_starter.jsonl \
  --summary-json /tmp/scx_bench_compact/work/starter_summary.json
```

方法标识：`scx` = 本适配器；`built-in:naive-summary` / `structured-state` /
`hybrid-ledger` / `hierarchical-summary` 为 harness 内置（`--methods` 名见 `run_p22.py` `METHODS`）。

## 5. 指标口径（pip 0.1.0，scorer 1.0.0）

- `overall_score`：各 case `case_score`（跨 cycle 加权 cycle 分均值）的均值。
- `drift_resistance`：case 内 cycle 得分相对 cycle 0 的保持度（`scoring.drift`）。
- `constraint_retention`：`locked_decision_retention` + `forbidden_behavior_retention`
  两类 item 得分均值（权重 3.0）。
- `contradiction_rate`：模型回答与 ground truth 冲突的比例（`scoring.contradictions`）。
- `compression_ratio`：`transcript_tokens / artifact_tokens`（cl100k_base，>1 为压缩）。
- 机械保留率审计（本目录离线重放，非 harness 指标）：harness 实际判分探针
  （`contains_normalized/exact/set_match` 目标值 + entity_map 人名）在压缩产物中的
  整串存活比例。

## 6. 已知差距与限制（如实记录）

1. **scx 面向长日志/工具输出，不是短对话压缩器**：harness 社区版 transcript 很短，
   scx 小输入透传保护（`<40` 行且 `<400` token）在 starter / medium / hard 上
   原样返回，`compression_ratio≈1.0`；只有 elite 难度（~431–504 token）越过保护
   阈值进入行保留筛选。
2. **论文版未跑**：论文版（35 SWE-bench repairs / 146 agentic sessions / 106 chains）
   需要完整 coding agent 闭环，scx 非 agent，不在本次范围；社区版 = versioned
   templates + hidden ranked set，本地仅有 `starter` / `elite_practice` 公开模板，
   **hidden ranked set 不可得**，无法出官方排行榜排名。
3. **scorer 版本差异**：pip 0.1.0 为 scorer 1.0.0（旧 drift 公式）；repo master
   0.2.0 与官方已发布基线（qwen2.5-1.5b，scorer 2.0.0）口径不同，跨版本数字
   不可直接并列排名，仅作参考行引用。
4. **内置基线同模型对比**仅在 starter 套件完成；elite_practice 上内置 LLM compactor
   成本高，本次未全量复跑（命令已在 §4 给出可复现）。
