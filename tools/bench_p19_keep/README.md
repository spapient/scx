# P19 · KEEP-Bench（证据保留诊断基准）scx 基线适配器

## 1. 基准与官方资源

- 基准：**KEEP-Bench: Evaluating Evidence Preservation in Context Compression**，arXiv:2610.26827。
- 官方仓库：<https://github.com/PlusLabNLP/KEEP-Bench>（本地克隆 `/tmp/scx_bench_keep`，commit 见结果 JSON `official.commit`）。
- 官方数据：HuggingFace `uclanlp/KEEP-Bench`（snapshot `91c37d5470f81ab7655b4893e7ba10536d5c952f`），**4,102 cases / 15,953 条证据命题判分请求**，覆盖三种证据结构：
  - **parallel evidence**（两条并行合法解释，two/three/four hop：400 / 300 / 200）；
  - **conditional evidence**（仅在限定条件下成立，two/three/four hop：604 / 493 / 267）；
  - **imbalanced evidence**（主流 + 少数派视角，science 3/4 perspectives：154 / 856，policy 4 perspectives：828）。
- 官方判分：**GPT-5.2，reasoning=high**，每个证据命题一次请求；指标为 case 级 All-Rec（该 case 全部命题被保留）、parallel Para-Rec、conditional Cond-Rec，Overall All-Rec 为全 4,102 case 加权平均。
  > 官方 README 明确：**换用其他 judge 得到的分数不可与论文分数直接横向比较**。本适配器 judge 通道为 DeepSeek `deepseek-flash`（temperature 0），所有结果仅作为 scx 自身的证据保留基线，口径差异见 §5。

## 2. 目录与产物

| 路径 | 内容 |
| --- | --- |
| `tools/bench_p19_keep/keepbench.py` | 唯一适配器：压缩 → 官方协议展开 → LLM judge → 官方口径聚合 |
| `/tmp/scx_bench_keep/data/` | 官方 9 子集 jsonl（`scripts/prepare_data.py --local <HF snapshot> --out data` 生成，规模已逐子集校验） |
| `/tmp/scx_bench_keep/outputs/scx/without_thinking/1x/<stem>.jsonl` | 压缩产物（输入行 + `compressed_context`，行对齐官方数据） |
| `/tmp/scx_bench_keep/entailment_check_input/scx/without_thinking/1x/` | 展开后的 judge 输入 + `_index/` 对齐文件（官方布局，可供官方 `score.py` 复核） |
| `/tmp/scx_bench_keep/entailment_check_output/scx/without_thinking/1x/` | judge 判定输出（官方布局，可供官方 `score.py` 复核） |
| `/tmp/scx_bench_keep/llm_calls.jsonl` | **逐条** LLM 调用审计：时间戳 / 模型 / 尝试序号 / judge 类型 / system / prompt 摘要 / response / usage / 耗时 / 错误 |
| `config/bench/results/bench_p19_keepbench.json` | 冻结结果（自描述 schema、版本、commit、seed、数字表、口径、成本） |

## 3. 依赖与环境

- Python：`/tmp/scx-eval/bin/python`（3.12，已装 `datasets / huggingface_hub / requests / openai / tqdm`）。
- scx 二进制：`/Users/fei/code/scx/target/release/scx`（v0.11.0，memory 默认开）。
- 判分密钥：`DEEPSEEK_API_KEY`（`source ~/.zshrc`）；endpoint `https://api.deepseek.com/chat/completions`；模型 `deepseek-flash`（默认，可用 `--model deepseek-chat` 切换）；`temperature=0`、`max_tokens=2048`、并发 8、失败指数退避重试 5 次。
- 官方 judge 模块**只读导入**（不改动官方仓库）：`src/evaluation/run_entailment_check.py` 中的四类 prompt、judge 路由、消息拼装、JSON 解析、空上下文 shortcut 全部原样复用；仅把传输层从 OpenAI Responses API 换成 DeepSeek chat-completions。聚合口径镜像 `src/evaluation/score.py`，并以官方 `score.py` 独立跑一遍交叉核对（数字一致才算通过）。

## 4. 复现命令

### 4.1 一次性准备（数据）

```bash
export HF_HOME=/tmp/scx_bench_keep/hf
git clone https://github.com/PlusLabNLP/KEEP-Bench /tmp/scx_bench_keep   # 已完成则跳过
cd /tmp/scx_bench_keep
# 下载四个 test.jsonl 到 HF 缓存后切分 9 子集（规模自动校验）
/tmp/scx-eval/bin/python scripts/prepare_data.py \
  --local "$(ls -d $HF_HOME/hub/datasets--uclanlp--KEEP-Bench/snapshots/*)" \
  --out data
```

### 4.2 全量基线（4,102 cases / 15,953 请求）

```bash
cd /Users/fei/code/scx
/tmp/scx-eval/bin/python tools/bench_p19_keep/keepbench.py \
    --results-json config/bench/results/bench_p19_keepbench.json
```

- 阶段可续跑：压缩产物与 judge 输出按子集落盘，中断后重跑同一命令自动复用已完成分组；`llm_calls.jsonl` 为追加模式。
- 只跑压缩：`--compress-only`；跳过压缩复用产物：`--judge-only`。

### 4.3 分层抽样（固定 seed=42，每子集 N 例，覆盖三种结构）

```bash
/tmp/scx-eval/bin/python tools/bench_p19_keep/keepbench.py \
    --limit-per-subset 60 --seed 42 \
    --results-json config/bench/results/bench_p19_keepbench.json
```

### 4.4 官方 score.py 独立复核

```bash
cd /tmp/scx_bench_keep
/tmp/scx-eval/bin/python src/evaluation/score.py \
    --judgements entailment_check_output \
    --index entailment_check_input/scx/without_thinking/1x/_index \
    --data-root data
```

## 5. scx 压缩配置与口径差异（如实说明）

官方压缩器是 ICL 抽象摘要，1x 预算为 K 句（h-hop：K=2h；n 视角：K=2n+1）。**scx 是行级抽取式压缩，没有句子预算概念**，本适配器固定如下确定性配置（CLI 无随机项，`--seed 42` 仅用于抽样）：

```
scx <tmpfile> --query <cleaned-question> --ratio 0.35 --context 0
```

- `--context 0`：针尖模式，仅保留证据种子行（命中查询词且满足 scx 种子判定）+ 守卫行，不做可选邻域填充；不做行内 `--trim`。
- 查询词由问句经**确定性**规则构造：引号原文保留；大写专名保留（剔除通用大写名词表）；小写主题词仅当在当前上下文文档频率 ≤25% 时保留；候选为空时保底取最稀有 4 词。规则与停用词表见 `keepbench.py` 顶部 `STOP` / `GENERIC_CAP` / `build_query`。
- scx 失败或零信号 fail-open 返回原文（与 scx 既有适配器一致），结果 JSON 记录每 case 是否实际压缩、原/压缩字符数、保留行数。
- 因此：**All-Rec 与官方在“压缩后命题是否仍可被判定”的判分协议上可比；压缩长度口径不可与官方句子预算对齐**，压缩字符比在结果 JSON `compression` 中单独上报。
- 已做过的 knob 对比（12 例/子集，命题专名覆盖率代理指标，非 judge 口径）：`--context 0` 字符 27–37%、覆盖 87–88%；`--context 1` 字符 64–77%、覆盖 93%。基线冻结 `--context 0`；`--context 1` 作为可选对照，用 `--scx-extra "--context 1"` 复跑。

## 6. 参考系（官方 1x，官网 `docs/index.html` 内嵌表；judge=GPT-5.2 high，**不可与本基线直接比较**）

列序：pr=Para-Rec，pa=Parallel All-Rec，cr=Cond-Rec，ca=Conditional All-Rec，sci=Science All-Rec，pol=Policy All-Rec，all=Overall All-Rec。

| 方法 | 类型 | pr | pa | cr | ca | sci | pol | all |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| EXIT | 抽取式 | 41.9 | 3.0 | 25.1 | 4.5 | 10.5 | 7.6 | 6.3 |
| RECOMP (extractive) | 抽取式 | 12.9 | 1.4 | 19.4 | 2.5 | 11.4 | 4.0 | 4.8 |
| BRIEF-Pro | 抽取式 | 0.3 | 0.0 | 61.6 | 19.9 | 0.4 | 0.0 | 6.7 |
| RECOMP (abstractive) | 抽象式 | 0.0 | 0.0 | 5.6 | 1.4 | 0.2 | 0.0 | 0.5 |
| Qwen3-32B | 开源 LLM | 58.9 | 42.1 | 58.6 | 44.6 | 31.3 | 40.5 | 39.9 |
| Gemma-4-31B-it（最强开源） | 开源 LLM | 71.2 | 57.6 | 85.2 | 71.2 | 69.9 | 65.8 | **66.8** |
| Claude-Haiku-4.5（最强闭源） | 闭源 LLM | 76.1 | 63.8 | 83.5 | 70.0 | 45.5 | 65.0 | **61.6** |
| **scx（本基线，deepseek-flash judge）** | 抽取式 | 见结果 JSON | | | | | | |

## 7. 耗时与成本（全量实测值见结果 JSON `cost`）

- 判分并发 8；Pilot 76 请求实测均值 ~2.1s/请求；全量 15,953 请求预计 **60–90 分钟**。
- DeepSeek flash 报价（2026-10 官方文档，USD/1M，峰值保守取值：输入 $0.15、输出 $0.60）；全量预计 **$3–6**（token 总量以 `llm_calls.jsonl` 汇总为准）。
- 压缩阶段本地执行，不产生外部 API 成本。
