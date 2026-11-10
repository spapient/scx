# 把 5 万行日志压进 12ms，证据一行不丢：一个零依赖的上下文压缩器

> 版本 0.11.0 已发布：crates.io / GitHub Releases / Homebrew / npm 四平台同步。

## 解决什么问题：上下文太贵

LLM 的输入按 token 计费，上下文窗口有上限。长日志、长代码、长文档要喂给 LLM 时，直接整段塞进去，费用和窗口压力都扛不住——**scx 解决的问题就是这个：在保留关键证据的前提下，把输入变小**。

## 为什么不能直接压缩

压缩文本不难，难在压缩后还能用：

- **通用摘要**：LLM 改写原文，输出不可信。排障、审计、代码问答的场景里，改写过的内容不能当作证据
- **随机抽样**：可能恰好丢掉根因行，等于没压
- **向量检索**：为一次上下文处理搭 embedding 服务，重

所以约束定成一条纪律：

> **只删整行，不改写原文；证据行及其最小邻域无条件保留，预算只能约束可选内容。**

输出永远是原文的行子集，一字不改，带原始行号；删哪些行由查询和预算决定，证据行绝不丢。

## 用法

```bash
scx compress app.log --query "database timeout connection pool" --ratio 0.35
# 命中的行全部保住，噪声裁掉，输出带原始行号
```

处理流程：

```
query + input
  ├─ 1. IDF 加权词面命中，确定证据行
  ├─ 2. 证据行及其 ±context 邻域无条件保留
  ├─ 3. 只在种子邻域内用剩余预算填充可选行
  ├─ 4. --max-tokens 硬上限：证据地板优先，超限如实报告
  └─ 5. 输出原文行子集与统计，不重写、不截断
```

其他参数：

- `--force "re:ERROR|FATAL"`：强制保留指定模式的行（手写 Thompson NFA 有限正则子集，无回溯灾难）
- `--max-tokens 2000`：硬 token 上限，超限如实报 `overflow:evidence` / `overflow:fill`，绝不静默丢证据

## 效果数据

作者侧自测（Apple M5 Pro，口径写进文档，非第三方评测）：

| 工作负载 | 结果 |
|---|---|
| 5 万行 / 233 万字符日志压缩 | 中位 11.7–12.0 ms |
| 187 万字符 / 5.2 万行 | 无损 ~50 ms，压 61.5%；`--ratio 0.3` → 75.4%，`0.2` → 77.9% |
| 证据保留（70 条证据行） | 70/70 = 100%（截断基线 0/70），默认档压减约 82% |
| Hard 层 MRR@10 vs 词面基线 | +0.0669（paired bootstrap 95% CI [0.0238, 0.1176]，p=0.0020） |

5 万行日志 12ms 压完，读取速度对排障链路没有感知。证据保留 70/70 和 MRR 提升说明证据优先不是口号，是可量化的。

## 为什么零依赖

第三方 crate 数量是 0（JSON parser、TOML 子集、SHA-256、正则子集全部手写），把确定性当契约：

> **同输入、同查询、同配置，输出逐字节一致。**

37 个 golden 用例对 stdout/stderr/退出码逐字节差分，每次变更的硬红线。`--locked` 离线可构建，压缩主链无网络请求、无随机源、无运行时依赖。

代码规模约 4.9 万行纯 Rust / 96 文件，554 个 `#[test]`，评测门禁 `xtask ops check` 全绿。

## Agent 工作流集成

同一二进制内置 MCP stdio server：

```bash
scx setup                # 一键注册 Claude / Codex / Cursor（幂等 + 写前备份 + tools/list 自检）
scx mcp                  # 手动进入 stdio server 模式
npx -y @spapient/scx     # npm 装，兼容千问办公的 MCP 接入
```

长日志/长代码先压再喂，关键证据行原样保留，是 Agent 上下文治理的直接用法。

## 安装

```bash
# crates.io
cargo install scx
# macOS Homebrew
brew tap spapient/scx && brew install scx
# npm（含 MCP 入口）
npx -y @spapient/scx
```

## 边界

- **词面相关优先**：查询和文本零共享词时 fail-open 回退原文，不承诺 embedding 级语义检索
- **压缩率有地板**：证据优先决定了输出存在下限，`--ratio` 不是硬性字符上限
- **非幂等**：对已压缩文本再压缩会继续裁减——累积场景必须对原始文本全量重压缩
- **pre-1.0**：CLI 和接口仍可能演进，生产集成建议固定 commit

仓库：[https://github.com/spapient/scx](https://github.com/spapient/scx)

有真实场景（排障、长代码问答、Agent 上下文治理）的欢迎试用、提 issue 或拍砖。
