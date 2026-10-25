# scx 证据保留同场对照：scx vs 头截断 vs 随机抽样（contain5）

> **性质声明：这是 scx 作者侧的自测（self-hosted），不是第三方独立评测。**
> 语料全部为合成噪音（`note: synthetic`），不含真实事故、真实公司、真实凭据。
> 与 SuperCompress evidence containment 的真实对比**待第三方盲审**——本页不引用、
> 不编造任何 SuperCompress 数字；SuperCompress B5 的历史口径差异见
> [containment.md](containment.md)。

## 环境与方法（2026-10-26）

- 机器：Apple M5 Pro（Mac17,9），macOS 27.0.1，arm64
- 二进制：`target/release/scx` v0.5.1（release profile，`cargo build --release --locked`
  成功产物，2026-10-26 02:07 重新链接；测量期间工作区有并行会话的 src 重构，
  重构前后两版二进制在本语料上 scx 侧聚合指标逐位一致）
- 参数：`--ratio 0.35 --context 1`；语料 `config/contain5.corpus.json`
  （25 切片，SHA256 `a077c35eedcdf1faa766b0ded9cc25e3039b2728fab926a9d994baa2c369b681`，
  未改动）
- 随机基线种子：`contain-rand-v1`（逐切片 `random.Random(f"{seed}:{slice_id}")`，
  同语料同种子逐字节可复现）
- 计时：每切片每方法重复 5 轮取中位，再对 25 切片取均值；`time.perf_counter` wall-clock

## 三指标对照

| 方法 | 证据存活率（70 条必需证据行） | 字符压减率 | 单切片 wall-clock |
|---|---:|---:|---:|
| **scx**（release 二进制） | **70/70 = 100.00%** | **82.01%**（token 估算 79.93%） | **4.19 ms**（切片间中位的均值；范围 3.87–4.60 ms） |
| 头截断基线（同等 kept 字节预算） | 0/70 = 0.00% | ≈82.01%（构造上同预算） | ≈0.3 µs（进程内 Python） |
| 随机抽样基线（同等 kept 字节预算，固定种子） | 8/70 = 11.43% | ≈82.01%（构造上同预算） | ≈34 µs（进程内 Python） |

逐切片明细见 [`contain5-compare.json`](contain5-compare.json)（SHA256
`7852eeb9796d7607cf7233201d573f776f0d23961d70145782c17a7a08b5672e`）。

## 口径说明

- **同场同预算**：两基线的 kept 字节数都等于 scx 该切片的实际输出字节数，
  因此三行压减率被构造为近似相等——对照的差异只体现在**证据存活率**与
  **延迟**上，不是在压减率上。
- **随机抽样**：行索引按种子洗牌后整行贪心装入预算，余量由下一行的字节前缀
  补齐；选中行按原始顺序输出。证据行被整条抽中即存活，否则丢失；8/70 即
  纯随机在 ~18% kept 率下的期望命中量级，与 scx 的 query 感知选择形成对照。
- **延迟口径不对称（必须读）**：scx 每次切片是**独立 release 子进程**端到端
  wall-clock（含进程启动 + 临时文件写入）；两基线是评测器进程内纯 Python
  计算、不起子进程。该列回答"用户在 shell 里各花多少时间"，不是两算法内核
  的纯 CPU 对照；scx 的内核级延迟见 [benchmarks.md](benchmarks.md)。
- scx 列与冻结结论 [containment.md](containment.md) 逐项一致（70/70、0/70、
  82.01%、79.93%），本次新增的只有随机基线与延迟两列。
- 确定性：同语料同参数连跑两次，证据存活率三行逐字节一致；wall-clock 随系统
  负载漂移数百分点，属正常。

## 复现命令

```bash
cargo build --release --locked
python3 tools/scx.py contain config/contain5.corpus.json \
    --out docs/contain5-compare.json
# 可选：--seed contain-rand-v1（默认）--timing-rounds 5（默认）
```

## SuperCompress 对照状态

- 本机有 node v22，但 SuperCompress v1 compiler 未 vendor、未安装；本次**未离线
  跑**其对照，也不引用其任何数字。
- 真实同场对比需：真实事故语料 + 第三方盲审 + 双方可执行入口，当前**均不具备**，
  明确列为待办；本页数字不构成对 SuperCompress 的任何结论。
