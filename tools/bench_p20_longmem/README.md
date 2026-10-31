# P20 · LongMemEval 接入适配器（scx memory 冻结基线）

官方基准：**LongMemEval**（arXiv 2410.10813，ICLR 2025），500 问、五能力
（information extraction / multi-session reasoning / temporal reasoning /
knowledge updates / abstention）。本包把 scx memory（`--store` 摄取 + `--recall-only`
召回）作为被测记忆系统接入官方 ingest → recall → answer → judge 流程，跑出冻结基线。

> 与 P18 的边界：P18 是 scx 自建检索维度对标（Recall@K / NDCG@K，自有 gauge/v2/real
> 语料）；本包是**官方 500 问五能力**全新接入，两套结果互不混用、互不替代。

## 1. 环境与资源（复现入口）

| 项 | 值 |
| --- | --- |
| scx 仓库 | `/Users/fei/code/scx`（HEAD `b507343b6372186a640f04cf433b8935e7b8135c`） |
| scx 二进制 | `/Users/fei/code/scx/target/release/scx`（v0.11.0，memory 默认开启） |
| Python | `/tmp/scx-eval/bin/python`（3.12.15，datasets / requests / openai 已装） |
| 官方仓库 clone | `/tmp/scx_bench_longmem/repo`（commit `9e0b455f4ef0e2ab8f2e582289761153549043fc`） |
| 官方数据 | HF `xiaowu0162/longmemeval-cleaned` → `/tmp/scx_bench_longmem/data/` |
| 工作/日志目录 | `/tmp/scx_bench_longmem/`（语料、mem-dir、`llm_calls.jsonl`、runs） |
| 固定 seed | `20261031`；LLM `temperature=0` |

在线判分通道（共享规格 §2）：endpoint `https://api.deepseek.com/chat/completions`，
reader 与 judge 均为 `deepseek-flash`（reasoning 关闭；`max_tokens` reader=8192、
judge=256。官方 reader 未限长、judge=10；flash 即使关闭 reasoning 仍会消耗
reasoning_tokens，2048/32 会被思考占满导致空响应，故放宽预算，口径在此记录）。

官方 judge 模型为 gpt-4o；本环境无 OpenAI key，**判分 prompt 模板逐字复用官方**，
仅替换模型，口径差异在结果 JSON `protocol.judge_model_note` 中显式标注。

## 2. 官方协议映射（出处可溯）

| 官方环节 | 官方实现 | 本适配器 |
| --- | --- | --- |
| 建库粒度 | 每问独立语料库（`run_retrieval.py` `batch_get_retrieved_context_and_eval`） | 每问独立 `mem-dir`，问后清理 |
| 会话索引语料 | session 粒度仅 user 发言进 BM25 语料（`run_retrieval.py:206`） | scx store query = 该会话 user 发言拼接 |
| reader prompt | `run_generation.py prepare_prompt`：`con=true`、`history_format=json`、`useronly=false` | `common.reader_prompt` 模板逐字；会话按日期升序渲染 `### Session n: / Session Date: / Session Content:` |
| 喂给 reader 前 | 剥离 `has_answer` 标注（`run_generation.py:182-189`） | `common._clean_turns` |
| 判分 prompt | `evaluate_qa.py get_anscheck_prompt` 六类 + 弃权模板 | `common.judge_prompt` 逐字；`label = 'yes' in response.lower()` |
| 聚合口径 | `print_qa_metrics.py`：六类 accuracy、task-averaged、overall、abstention 单列 | `report_p20.py` 同口径 |
| 检索诊断 | `eval_utils.py` `recall_any/recall_all/nDCG@K`，弃权题跳过 | `common.evaluate_retrieval` 逐公式移植 |

## 3. 摄取 recipe（逐命令可复现）

对每一道题（`entry`），在独立 mem-dir `<mem-root>/<arm>/<question_id>/` 内按时间序
处理 `haystack_sessions`（S 文件官方已按时间戳排序）：

```text
# 每个会话一条 case（子进程，stdlib subprocess，capture_output，timeout=120s）
scx <session-text> \
    --query "<该会话全部 user 发言拼接，截断 1800 字符>" \
    --store 1 \
    --fix "Session Date: <date> | official_session_id: <sid>" \
    --ratio 1.0 --context 1 --kind session-message \
    --mem-dir <mem-dir>
```

- 会话文本（stdin）：
  ```text
  ### Session: <sid>
  Session Date: <date>
  Session Content:
  user: ...
  assistant: ...
  ```
- 解析 stderr/stdout 中 `scx: stored case=<full-id> added=yes|dedup`；
  同时记录 `official sid ↔ scx case_id` 映射。
- 召回：`scx --recall-only --recall 10 --recall-detail --query <question> --mem-dir <mem-dir>`，
  解析 `mem: sim=.. score=.. case=<short-id>` 排名，短 id 由 `cases/*/case.json` 目录名
  反查 full id。
- `--ratio 1.0 --context 1`：可选内容预算拉满，会话正文整段保留；实测字符保留率
  见结果 JSON `baseline.ingest.char_retention`。

### 3.1 knowledge-update 版本化专项臂（P3）

对 78 道 `knowledge-update` 题另跑 `versioned` 臂：除旧证据会话外摄取流程相同；
证据会话按 `haystack_dates` 升序，摄取**较新**证据会话时调用：

```text
scx <new-session-text> --query ... --store 1 \
    --revise <旧证据会话 case_id> \
    --fix "..." --ratio 1.0 --context 1 --kind session-message --mem-dir <mem-dir>
```

P3 行为（已在冒烟测试核验）：新 case `version=旧+1 / supersedes=旧`；旧 case
`life=revised / superseded_by=新`；召回候选过滤 `life≠active`（`recall.rs:660-661`、
`787-788`），旧事实不再进入 top-K。`baseline` 臂同题对照，量化版本化对
knowledge-update 正确率的增益与陈旧事实泄漏率。

### 3.2 abstention 弃权专项

30 道 `*_abs` 题走同一 baseline 臂；官方弃权判分模板判定模型是否正确识别不可答。
另记录弃权表达命中率、top-1 sim、上下文长度等诊断；检索诊断按官方口径跳过弃权题。

## 4. 复现命令

```bash
cd /Users/fei/code/scx
export HF_HOME=/tmp/scx_bench_longmem/hf

# （数据已下载；需要重下时）
/tmp/scx-eval/bin/python - <<'PY'
from huggingface_hub import hf_hub_download
for f in ["longmemeval_oracle.json", "longmemeval_s_cleaned.json",
          "longmemeval_m_cleaned.json"]:
    hf_hub_download("xiaowu0162/longmemeval-cleaned", f, repo_type="dataset",
                    local_dir="/tmp/scx_bench_longmem/data")
PY

# ① 全量 baseline 500 问（scx memory 标准臂）
/tmp/scx-eval/bin/python tools/bench_p20_longmem/run_p20.py \
    --dataset S --arm baseline --workers 8 --recall-k 10 --out-tag full

# ② knowledge-update 版本化对照臂（78 问）
/tmp/scx-eval/bin/python tools/bench_p20_longmem/run_p20.py \
    --dataset S --arm versioned --workers 8 --recall-k 10 --out-tag full

# ③ 可选 oracle 上界臂（直接喂证据会话，非 scx 成绩，仅作 reader 天花板参考）
/tmp/scx-eval/bin/python tools/bench_p20_longmem/run_p20.py \
    --dataset S --arm oracle --workers 8 --out-tag full

# ④ 聚合生成冻结结果 JSON
/tmp/scx-eval/bin/python tools/bench_p20_longmem/report_p20.py \
    --baseline-tag full --versioned-tag full --oracle-tag full
```

断点续跑：`runs/p20_S_<arm>_<tag>.jsonl` 已落盘的 `question_id` 自动跳过；
每条 LLM 调用（reader / judge）请求摘要 + 响应 + usage + 延迟 + 重试次数逐条落盘
`/tmp/scx_bench_longmem/llm_calls.jsonl`，可审计、可统计成本。

## 5. 落地物

- 适配器目录：`tools/bench_p20_longmem/`（本 README + `common.py` + `llm.py` +
  `run_p20.py` + `report_p20.py`）
- 冻结结果：`config/bench/results/bench_p20_longmemeval.json`
- 完成记录：agent 工作目录 `P20_longmemeval_record.md`（体例照 docs/plan.md P18/P17）
- 中间产物：`/tmp/scx_bench_longmem/runs/*.jsonl`、`llm_calls.jsonl`、mem-dir

## 6. 口径与已知限制（诚实标注）

1. **判分模型替换**：官方 gpt-4o judge → deepseek-flash judge；prompt 逐字未改，
   但模型判分尺度存在差异，结果不宜与论文 gpt-4o 成绩做绝对值对齐比较。
2. **检索系统本体**：scx 为零依赖词面 + MinHash + RRF 融合检索（无稠密向量模型），
   与官方 BM25 / Contriever / Stella / GTE 基线不同族；本结果是 scx memory 的真实
   冻结基线，论文表格数字仅作参考系。
3. **LongMemEval-M**：~500 会话长库；若在本环境取得失败（未鉴权 xet CDN 403），
   在结果 JSON「受阻项」中如实记录，不冒充完成。
4. 官方 `con` 读取方式 = 先抽取相关信息再推理；reader prompt 已按官方 con 模板，
   未使用官方 `con-separate`（二次 LLM 抽取笔记）变体。
