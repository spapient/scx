# scx 评测工具

可重建的评测语料、冻结标签、盲审包与语义权重位于 `config/`（纯数据目录）；评测
入口为仓库根的 `tools/scx.py`。记忆库目录、verbose 日志、配对统计 CSV 等均为
**生成物**，不进入版本库。

在仓库根目录运行：

```bash
./tools/scx.py --help          # macOS / Linux
python tools/scx.py --help     # Windows
```

评测器只使用 Python 标准库，负责语料播种、基线构造、盲审编排与统计；检索和
证据门仍有且只有一份 Rust 实现，位于 `src/`。评测脚本不能在 Python 侧复刻排序。

## 目录

| 路径 | 用途 |
|---|---|
| `../tools/scx.py` | 语料播种、基线构造、盲审、配对统计、包校验 |
| `config/gauge.corpus.json` | Gauge：30 起公开事故，150 条存储 case |
| `config/v2.corpus.json` | v2 开发集：30 起公开事故，150 条存储 case |
| `config/real.corpus.json` | 真实事故集：11 起公开复盘，40 条 case |
| `config/gauge.policy.labels.json` | 规则物化控制集，**不是人工标签** |
| `config/gauge.ai.labels.json` | 384/384 AI 逐候选标签（`per-item-ai-v1`） |
| `config/gauge.human.labels.json` | 384/384 人工逐候选标签（`per-item-human-v1`） |
| `config/v2.labels.json` | v2 案例级标签，AI 预填后由两位审核者采纳 |
| `config/real.labels.json` | real 案例级标签（`judge.by=fei+hao`） |
| `config/gauge.blind40.packet.tsv` | 冻结盲审包：40 case / 384 候选 |

AI/人工 reviewer 填写后的 TSV 是 `packet + labels` 的派生视图，不再单独入库；
`blind check --packet ... --out ...` 可无损重建并校验历史 SHA256。

## 语料格式

语料 schema 与具体检索器解耦：

```json
{
  "name": "example",
  "expected_incidents": 1,
  "expected_cases": 1,
  "incidents": [{
    "id": "incident-id",
    "source": "https://example.invalid/postmortem",
    "root_cause": "one-line public root cause",
    "fix": "one-line public remediation",
    "cases": [{
      "store_query": "query used to store the case",
      "text_query": "optional first line of the stored text",
      "eval_query": "query used by --eval",
      "layer": "literal"
    }]
  }]
}
```

`text_query` 缺省时使用 `store_query`。播种后的文本形状固定为：

```text
<text_query>
公开复盘, 来源: <source>
根因: <root_cause>
```

2026-10-25 复验：三个归一化语料分别生成 Gauge 150/150、v2 150/150、
real 40/40，case hash 后缀与冻结基线一致；日期前缀变化不影响标签匹配。

## 标注契约

评估集应包含 30–50 个真实 case，`literal`、`paraphrase`、`semantic` 至少各 10 个。
每个 case 条目支持：

| 字段 | 含义 |
|---|---|
| `query` | 传给 `--eval` 的检索 query |
| `layer` | `literal` / `paraphrase` / `semantic` |
| `expected` | 一个或多个目标 case id（完整 id 或 hash 后缀） |
| `rel` | 候选 id → `2` 直接答案、`1` 相关、`0` 无关；未列出的 expected 默认 `1` |
| `judge` | `{by,date,method}`；`null` 表示尚未裁决 |
| `note` | 盲审提示；不得泄露 A/B 分数 |
| `judge_items` | 每个候选的 provenance，`--gate-human` 强制要求 |

三种门禁的证据强度不同：

- `--gate`：机械资格门，检查样本规模、层平衡、judge 元数据和基础指标。
- `--gate-items`：允许 AI 或非人工逐项裁决，同时明确标注证据等级。
- `--gate-human`：要求完整逐候选真人 provenance、弱相关/负例控制，且 NDCG A ≥ B。

自动标注、policy 物化、AI 预填、案例级签名都不能冒充独立人工证据。
逐候选盲审使用 `blind`；表格转 CSV 路径只用于机械目标标签。

## 复现 Gauge

```bash
./tools/scx.py seed config/gauge.corpus.json \
  --mem-dir /tmp/scx-gauge --csv /tmp/scx-gauge.csv --force
./tools/scx.py baseline --csv /tmp/scx-gauge.csv \
  --out /tmp/scx-gauge-mechanical.json \
  --judge-by mechanical-target-gauge-v1 \
  --judge-date 2026-10-23T00:00:00Z
./tools/scx --eval /tmp/scx-gauge-mechanical.json --gate --eval-verbose \
  --mem-dir /tmp/scx-gauge
```

配对统计与预注册断言（与 CI `Enforce pre-registered advantage` 一致）：

```bash
./tools/scx --eval /tmp/scx-gauge-mechanical.json --gate --eval-verbose \
  --mem-dir /tmp/scx-gauge > /tmp/scx-gauge-verbose.txt
./tools/scx.py stats --baseline /tmp/scx-gauge-mechanical.json \
  --verbose /tmp/scx-gauge-verbose.txt --mem-dir /tmp/scx-gauge \
  --out /tmp/scx-gauge-stats.csv
```

主指标为 `tier:hard` 的配对 MRR 差值：必须满足 Δ≥0.05、bootstrap 95% CI
下界 >0、permutation p<0.05（10,000 次，seed 20261023）；`--metric ndcg`
输出同构的 nDCG 报告。stats CSV 由 CI 直接消费，阈值冻结在
`.github/workflows/ci.yml`。

`--metric recall@k`（k∈{1,3,5,10}）输出同构的 Recall@k 报告：target 在
top-k 内计 1，否则 0。实测口径：recall@1 二进制指标翻转样本少，检验
功率不足（观测为正但未达显著，2026-10-26 实测 perm p=0.1236）；CI 以
**recall@3** 作 Recall@k 门禁（Δ≥0.05 且 CI 下界>0）；recall@1/5/10
供报告。MRR / recall@3 / LOO / fidelity 的实测数字统一以
`docs/gauge-metrics.md`（自动生成）为准。

**留一事故泛化（--loo --gate）**：按 `case_index//5` 把 150 查询归为 30
个 incident（每组 5 例），逐一剔除任一 incident 后在其余 hard 查询上
重估配对 MRR（bootstrap CI + permutation 同全量口径）；门禁要求每个
剔除后的 Δ≥0.05 且 CI 下界>0。实测结论：hard 优势**不依赖任一单一
事故**（数字见 `docs/gauge-metrics.md`）。

**标注一致性 κ（blind agree，核对性质）**：`blind agree --a
config/gauge.ai.labels.json --b config/gauge.human.labels.json` 实测
common=384、raw=0.344、**κ=0.017**——AI 逐项盲审与人工盲审一致性极低。
如实记录并解读：AI 盲审标签不能直接充当人工 gold；Gauge 机械 MRR 主
指标（tier:hard Δ 与显著检验）完全基于机械 target 与 relevance 定义，
不依赖任何盲审标签，故不受 κ 影响。提升 AI-人工一致性属校准工作，
不在 Gauge 预注册范围内。


`seed` 默认使用 4 个并发 store 进程，`--jobs` 取 `1..=32`；超过 32 只会加剧
进程启动与跨进程锁竞争。case 文本以 UTF-8 字节经 stdin 写入，不生成逐 case
临时文件，也不受 Windows 换行转换影响。输出 CSV 始终按语料顺序排列，case id
的 hash 后缀与串行播种逐字节一致；CI 会在归一化日期前缀后对比
`--jobs 4` 与 `--jobs 1` 的整个 CSV，避免跨午夜准备产生无意义差异。
2026-10-25 Apple M5 Pro 五轮交替样本：并行中位 0.452 s（0.426–0.485），
串行中位 0.606 s（0.593–0.610），中位加速 1.34×；绝对耗时随机器负载变化，
不作为跨机器承诺。

`seed` 在写入前验证语料：query 必须是单行、无制表符/NUL（否则
`--eval-verbose` 无法无歧义解析），store/eval query 不得重复，事故 ID 唯一，
来源必须是 HTTP(S) URL，层名、case 数和每个事故的 case 列表都必须完整。
`--force` 只会清理已确认包含 `meta.toml` 与 `cases/` 的 scx 记忆库，拒绝工作区
根、系统/用户临时目录根和任意非记忆库目录；macOS 的 `/tmp` 与
`DARWIN_USER_TEMP_DIR` 都按标准临时根处理。

人工、AI 与 policy 门禁复用同一记忆库：

```bash
./tools/scx --eval config/gauge.human.labels.json --gate-human --eval-verbose \
  --mem-dir /tmp/scx-gauge
./tools/scx --eval config/gauge.ai.labels.json --gate-items --mem-dir /tmp/scx-gauge
./tools/scx --eval config/gauge.policy.labels.json --gate-human --mem-dir /tmp/scx-gauge
```

预期：human `PASS / exit 0`；AI 只通过 `--gate-items`，`--gate-human` 必须
`FAIL / exit 2`；policy 同样 `FAIL / exit 2`。

## 配对统计

`stats` 读取确定性的 `--eval-verbose` 输出，计算配对 MRR/NDCG 差值、bootstrap
置信区间和 sign-flip permutation p 值。提供 `--mem-dir` 时由 case query 计算难度层；
也可传入 `case_index,tier` CSV；该 CSV 必须覆盖全部 case，重复、越界或未知 tier
都会被拒绝，避免静默漏算 hard 层。

```bash
./tools/scx --eval config/gauge.human.labels.json --gate-human --eval-verbose \
  --mem-dir /tmp/scx-gauge > /tmp/scx-gauge-hao.txt
./tools/scx.py stats \
  --baseline config/gauge.human.labels.json \
  --verbose /tmp/scx-gauge-hao.txt \
  --mem-dir /tmp/scx-gauge \
  --cluster-size 5 \
  --out /tmp/scx-gauge-hao-stats.csv
```

`--cluster-size 5` 生成事故聚类敏感性行，不能替代 case 级预注册主分析。

## 盲审

```bash
./tools/scx.py blind make \
  --baseline config/gauge.policy.labels.json --mem-dir /tmp/scx-gauge \
  --out /tmp/packet.tsv --manifest /tmp/manifest.json
./tools/scx.py blind progress --packet /tmp/packet.tsv
./tools/scx.py blind import \
  --baseline config/gauge.policy.labels.json --packet /tmp/packet.tsv \
  --out /tmp/human.json --judge <name> --date 2026-10-25T00:00:00Z
./tools/scx.py blind check --baseline /tmp/human.json
./tools/scx.py blind check --baseline /tmp/human.json \
  --packet /tmp/packet.tsv --out /tmp/human-filled.tsv
./tools/scx.py blind agree \
  --a /tmp/first-reviewer.json --b /tmp/second-reviewer.json
```

`blind make` 是确定性的。用 `gauge.policy.labels.json` 和新播种记忆库重建
冻结包，应得到 SHA256
`b8eebf2fcb78d0fb745c9760b55dd34d832a9bb976b377f3dfb8c7cea264093f`。
`blind agree` 报告共同裁决项、原始一致率、Cohen's kappa 与分歧列表。
历史标签中的包路径字符串可能过时，应以 `judge.packet_sha256` 定位包。
`blind import --allow-partial` 只允许留空的 grade；任何非空且不属于 `0/1/2`
的值都会拒绝整次导入，且不写输出文件。`blind check --packet` 会用冻结包和
labels 重建填写版；从 labels 读取的 `packet_sha256` 必须一致，否则失败。
填写包以 LF 规范化后的内容计算 provenance 哈希，因此 Windows/Excel 保存的
CRLF 文件与 LF 文件等价；`verify` 仍校验冻结文件的原始字节。
已删除的两个派生填写版可重建为 SHA256
`6a5319c5...ac9b18`（AI）和 `1f6cd835...240b5`（人工）。

## v2 与 real

```bash
./tools/scx.py seed config/v2.corpus.json \
  --mem-dir /tmp/scx-v2 --csv /tmp/scx-v2.csv --force
./tools/scx --eval config/v2.labels.json --gate --mem-dir /tmp/scx-v2

./tools/scx.py seed config/real.corpus.json \
  --mem-dir /tmp/scx-real --csv /tmp/scx-real.csv --force
./tools/scx --eval config/real.labels.json --gate-human --mem-dir /tmp/scx-real
```

v2 参与调参，且标签由 AI 预填后人工采纳，因此不是独立 holdout。`real` 只有案例级
签名；`--gate-human` 因缺少逐候选 provenance 而失败，这是预期证据边界，不是指标回退。

## 产物策略

- 保留语料、冻结标签、盲审包和预注册文档。
- 不提交记忆库、verbose 日志、gate 日志、rank-diff/strata/显著性 CSV 或草稿包。
- 引用结果时必须同时给出语料、标签集、二进制/配置、门禁模式、样本量和统计命令。
- CI 会在冻结语料上重建记忆库，并分别执行机械门、人工门、AI 逐项门、policy
  负向门和预注册优势断言；移动或重命名评测输入时，必须同步工作流路径。
- 旧 `eval-real/` 中间产物已删除，可从 git commit `5ada90d` 恢复。

## Evidence containment（contain5）

`contain5` 是独立的合成噪音语料评测，对标 SuperCompress 的 evidence containment：
压缩输出必须逐字保留"必需证据行"，并同场对比同等字节预算的头部截断基线。

```bash
cargo build --release --locked
./tools/scx.py contain config/contain5.corpus.json --out /tmp/contain5-detail.json
```

- 语料：`contain5.corpus.json`（25 切片，build/stack/diff/log/test 各 5，全 synthetic）。
- 入口：`tools/scx.py contain`（新增 subcommand，标准库、无随机源）。
- 口径、复现命令、实测数字与 SHA256 见 [`containment.md`](containment.md)。
- **自测性质**：作者侧合成语料自测，非第三方独立评测。
