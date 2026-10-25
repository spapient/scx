# scx Evidence Containment 评测（contain5）

> **性质声明：这是 scx 作者侧的自测（self-hosted），不是第三方独立评测。**
> 语料全部为合成噪音（`note: synthetic`），不含真实事故、真实公司、真实凭据。
> 结果在本机一次 release 构建上测得，跨机器绝对值会漂移。

## 复现命令

```bash
# 0. 构建 release 二进制（零依赖，--locked）
cargo build --release --locked

# 1. 跑 containment 评测（stdout 表格 + 聚合；--out 写逐片 detail JSON）
./bin/scx-eval contain eval/contain5.corpus.json --out /tmp/contain5-detail.json

# 可选参数（默认即本次实测口径）：
#   --ratio 0.35 --context 1 --binary target/release/scx
```

评测器只用 Python 标准库，无随机源；对同一语料连跑两次输出逐字节一致。
Python 侧不复刻 scx 排序——压缩由 `target/release/scx`（Rust 唯一实现）完成，
`scx-eval contain` 只负责写临时输入、调用二进制、解析 `scxjson:` 统计行并核对证据。

## 口径定义

- **evidence containment（证据保留率）**：切片的 `evidence` 列表是该 query 下
  "必需证据行"的原文逐字拷贝。对每条证据行，检查它是否逐字出现在 scx 压缩
  正文里（正文 = stdout 去掉末尾 `compressed:` 统计行）。某切片全部证据行存活
  记 1.0；聚合口径 = 存活证据行数 / 证据行总数。miss 逐条打印。
- **压减率（字符口径）** = `1 − len(压缩正文字符) / len(原文字符)`。
- **压减率（token 估算口径）** = `1 − token_count(压缩正文) / token_count(原文)`。
  `token_count` 是确定性正则（ASCII 词/数字 + 每个 CJK 字计 1），
  **不是**真实 LLM tokenizer，只作同向量级参考。
- **truncation 基线**：把原文按 scx 实际输出的字节预算（`chars_out`）从头部截断，
  在**同等 kept 预算**下重算同一组证据行的保留率。朴素头部截断，不感知 query。

## 与 SuperCompress evidence containment 口径的差异

- SuperCompress B5 曾声称 64.14% 平均压减 + 24/24 证据保留（官方已挂起复核）；
  其压减率 = `1 − kept/total`，证据 = 人工标注的"必需证据行原文存活"。
- scx 的 `evidence` 是 **query 词面筛出的 seed 行**（机制上 seed 恒等、永不丢弃），
  本评测把这些 seed 行连同紧邻诊断行标为必需证据，逐字核对其在压缩输出里存活。
- 本评测额外给**字符 + token 估算双口径**压减率，并同场给出同等预算的
  truncation 基线对照，而不是只报单一压减数字。
- 语料为 25 条合成噪音切片，与 SuperCompress 的真实事故语料不可直接横向比大小。

## 语料

| 项 | 值 |
|---|---|
| 文件 | `eval/contain5.corpus.json` |
| 切片数 | 25（build 5 / stack 5 / diff 5 / log 5 / test 5） |
| 总行数 | 1787 |
| 必需证据行 | 70 |
| 总字符 | 92298 |
| SHA256 | `a077c35eedcdf1faa766b0ded9cc25e3039b2728fab926a9d994baa2c369b681` |

每个切片字段：`id / kind / query / full_text / evidence / note(synthetic)`。

## 实测结果（ratio=0.35, context=1, target/release/scx）

| id | kind | mode | kept/total | char压减 | token压减 | scx证据 | trunc证据 |
|---|---|---|---|---|---|---|---|
| build-01 | build | sieve | 30/70 | 56.8% | 67.1% | 4/4 | 0/4 |
| build-02 | build | sieve | 8/81 | 87.1% | 84.9% | 4/4 | 0/4 |
| build-03 | build | sieve | 7/69 | 83.1% | 80.3% | 3/3 | 0/3 |
| build-04 | build | sieve | 6/80 | 89.8% | 87.9% | 3/3 | 0/3 |
| build-05 | build | sieve | 7/69 | 86.3% | 84.4% | 3/3 | 0/3 |
| stack-01 | stack | sieve | 20/75 | 74.2% | 64.1% | 3/3 | 0/3 |
| stack-02 | stack | sieve | 20/75 | 74.7% | 64.7% | 3/3 | 0/3 |
| stack-03 | stack | sieve | 30/75 | 62.4% | 54.4% | 3/3 | 0/3 |
| stack-04 | stack | sieve | 21/75 | 73.2% | 63.4% | 3/3 | 0/3 |
| stack-05 | stack | sieve | 20/75 | 74.0% | 63.6% | 3/3 | 0/3 |
| diff-01 | diff | sieve | 35/93 | 48.7% | 45.9% | 2/2 | 0/2 |
| diff-02 | diff | sieve | 5/93 | 93.2% | 92.5% | 2/2 | 0/2 |
| diff-03 | diff | sieve | 6/93 | 91.3% | 89.5% | 2/2 | 0/2 |
| diff-04 | diff | sieve | 7/94 | 91.4% | 90.3% | 3/3 | 0/3 |
| diff-05 | diff | sieve | 6/93 | 90.5% | 89.5% | 2/2 | 0/2 |
| log-01 | log | sieve | 5/42 | 86.1% | 87.1% | 2/2 | 0/2 |
| log-02 | log | sieve | 5/42 | 86.6% | 87.1% | 2/2 | 0/2 |
| log-03 | log | sieve | 5/42 | 86.3% | 87.5% | 2/2 | 0/2 |
| log-04 | log | sieve | 5/42 | 86.9% | 87.3% | 2/2 | 0/2 |
| log-05 | log | sieve | 6/42 | 83.0% | 84.2% | 2/2 | 0/2 |
| test-01 | test | sieve | 10/74 | 87.3% | 87.5% | 4/4 | 0/4 |
| test-02 | test | sieve | 6/73 | 91.1% | 92.2% | 3/3 | 0/3 |
| test-03 | test | sieve | 9/74 | 89.1% | 88.4% | 4/4 | 0/4 |
| test-04 | test | sieve | 9/73 | 86.7% | 85.5% | 3/3 | 0/3 |
| test-05 | test | sieve | 6/73 | 90.6% | 88.9% | 3/3 | 0/3 |

聚合：

- **scx 证据保留率 = 70/70 = 100.00%**
- **truncation 基线证据保留率 = 0/70 = 0.00%**
- **平均字符压减 = 82.01%**
- **平均 token 估算压减 = 79.93%**
- 全部 25 切片 `mode=sieve`（无 passthrough/failopen）。

分 kind（scx 证据 / 字符压减 / token 压减）：

| kind | 证据 | 字符压减 | token压减 |
|---|---|---|---|
| build | 17/17 | 80.6% | 80.9% |
| stack | 15/15 | 71.7% | 62.0% |
| diff | 11/11 | 83.0% | 81.6% |
| log | 10/10 | 85.8% | 86.7% |
| test | 17/17 | 89.0% | 88.5% |

## 判定实现双轨（工具侧 / Rust 协议侧）

- **工具侧（`bin/scx-eval contain`）**：把证据行原文与压缩正文做 **substring
  包含**判定（`ev in body_text`）。宽松、快，适合编排统计。
- **协议侧（`scx::contain` + `tests/contain.rs`）**：做 **整行逐字节等值**
  判定（证据行原文必须作为一行存活于输出行集），更严格；红黄蓝断言批
  （blue 全保留 / red 硬上限守卫 / amber failopen 与汇总自洽）是判定语义的
  可执行规格，任何实现改动都必须先过它。
- 两者在 contain5 全 25 片上结果一致（70/70 = 100%）；协议侧更严，今后
  以 `scx::contain` 为判定唯一事实源。

## 边界与偏差

- stack 类压减偏低（字符 71.7% / token 62.0%）：堆栈帧行普遍较长且 `at` 帧链
  与 seed 邻近，context 窗保留了更多上下文行；这是 query 命中位置决定的，非缺陷。
- diff-01 压减最低（48.7%）：该切片证据行位于多个重复 hunk 脚手架中间，
  命中区域较宽。
- token 压减与字符压减存在数个百分点差，因为该估算不是 LLM tokenizer，仅作量级参考。
- 本结果为合成语料上的自测；要对外引用，需补真实事故语料与第三方盲审。
