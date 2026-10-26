# SCX 治理层路线图：对齐代码现状的重写版（v0.2）

> 目标版本：0.6.7 → 后续治理能力（续接已交付治理线 P0–P3，编号 P4–P7）。
> 本版取代 v0.1：v0.1 把 9 项"缺口"当未开工，实际其中 6 项已在
> `src/bucket.rs` / `src/meter.rs` 落地，命名（`PayloadKind`/`BucketStats`）
> 与编号（另起 P1–P4）均与代码脱节。§0 给出对齐表，后续全部复用既有类型，
> 不造第二套并行系统。
>
> 状态：方案定稿（未开工）。版本号：治理路线 P4–P7 草案。

## 0. 现状对齐（v0.1 的 9 缺口 → 真实状态）

| # | v0.1 缺口 | v0.1 命名 | 真实状态（代码证据） |
| --- | --- | --- | --- |
| 1 | Token 计量（△） | `TokenMetric` | 已有 `terms::estimate`（`src/terms.rs:329`，ASCII 4 字/token、CJK 0.6 token/字、other 2.5 字/token）+ `meter::count`（向上取整）。**无 calibrated/BPE**。 |
| 2 | 载荷分类（○） | `PayloadKind` | **已实现** `bucket::ContentType`（`src/bucket.rs:13`：StaticContract/SystemDefinition/SessionMessage/ToolOutput + `default_ratio` 1.0/0.8/0.5/0.2）。 |
| 3 | 分桶统计（△） | `BucketStats` + `Outcome.buckets` | **已实现** `meter::BucketStat`（`src/meter.rs:57`）+ `Snapshot.buckets`（`src/meter.rs:144`）。 |
| 4 | 快照差分（○） | `Snapshot`/`Diff` | **已实现** `meter::Snapshot`/`Diff`（`src/meter.rs:174`）+ `bucket::Snapshot`（`src/bucket.rs:69`）。缺 payload/config 指纹（见 P5）。 |
| 5 | 运行时留存（△） | `ev_retention` | **未实现**。`meter::Metrics.retention` 是"高价值桶留存率"，非证据留存，语义不同。 |
| 6 | MCP 生命周期（○） | 4 新工具 | **半实现**。`mcp.rs:25` 已有 `compress_context`/`connect_account`/`usage_summary`；缺 snapshot/diff/ledger_summary（见 P5）。 |
| 7 | 收益账本（○） | `Ledger` | **已实现** `meter::Ledger`（`src/meter.rs:183`，JSONL round-trip）+ `bucket::append_record`。缺聚合 summarize（见 P5）。 |
| 8 | 脱敏（○） | `RedactConfig` | **半实现**。`memory/util.rs` 的 `sanitize`/`redact`（密钥/token/连接串脱敏）面向记忆库落盘，非压缩入口、非规则表（见 P7）。 |
| 9 | 会话增量（○） | `SessionState` | **未实现**。见 §3 缓办。 |

**编号冲突说明**：治理线已有编号 P0–P3（`src/lib.rs:48-53` 模块注释 + governance 提交链
`2862054`/`3fedc0a`/`c33c07a`），与本方案续接的 P4–P7 同线；`docs/plan.md` 的 P1–P4
是**另一条内核路线**（compress 阶段拆分/评测协议/记忆库），两者勿混淆。下表为治理线全貌：

| 治理线 | 已交付（代码） | 本方案 |
| --- | --- | --- |
| P0 | `hook.rs` PostToolUse 自动拦截 | — |
| P1 | `bucket.rs` 类型路由 + 计量桶 + 前后快照 | — |
| P2 | 类型化计量（typed metering，并入 bucket/meter） | — |
| P3 | `meter.rs` 计量 + 效果审计 + `Ledger` | — |
| P4–P7 | — | 见 §2（运行时留存 / 指纹+MCP+聚合 / 骨架保护 / 脱敏） |

## 1. 设计原则（沿用 v0.1，全部成立）

1. **零第三方依赖**：所有新算子 std 实现；可选 feature 同 `semantic` 模式（cfg 排除，默认路径不变）。
2. **确定性**：不引入随机源与网络；同输入 + 同配置 = 同输出；种子与哈希固定。
3. **配置化声明**：新算子只以配置表存在（`policy.rs` 扩展），禁止硬编码策略；默认值改动必须过 golden + Gauge 门。
4. **fail-open 安全**：分类不确定、快照写入失败、证据校验缺失一律降级到现状行为，绝不静默丢弃证据或改变输出。
5. **门禁冻结**：每阶段新增专项测试入 CI；golden **32 例**逐字节零漂移；Gauge/fidelity 复跑不退化。
6. **阶段化交付**：P4 → P7 每阶段独立 commit、过门才进下一阶段。
7. **复用既有类型**：不引入 `PayloadKind`/`BucketStats` 等平行命名，一律在 `ContentType`/`BucketStat`/`Snapshot`/`Ledger` 上扩展。

## 2. 净新增工作（P4–P7）

### P4：运行时留存代理 `ev_retention`

**目标**：为每次压缩补一个轻量"信息留存"度量，弥合运行时与离线 fidelity 之间的空白。

**现状**：`Outcome`（`src/lib.rs:195`）有 `kept/total/evidence`，无留存率；fidelity 的
ev/ans 是离线深度评测（`config/fidelity.corpus.json` + `config/ci-fidelity.json`）。

**方案（两级，确定性）**：

1. **显式证据（权威）**：入口可选 `evidence`（保留行号列表，或必须保留的子串列表）
   → 逐字校验压缩输出，`ev_retention = 保留证据数 / 总证据数`。CLI `--evidence`、
   MCP `compress_context` 增 `evidence` 参数、`wrap` 透传。
2. **轻量默认（无 evidence）**：输入行 hash 集合 vs 输出行 hash 集合的保留率
   （行级近似）。压缩"只删不改、保留行逐字原文"，故差集 = 被删行。

**诚实性约束（关键）**：行 hash 保留率度量的是"删除率"，**不等于**信息留存
（删一行 filler 与删一行关键证据权重不同）。因此"默认代理"的合法性必须由
相关性门背书——见门禁。无 evidence 时不得宣称 `ev_retention` 等于 fidelity 语义。

**数据结构**：`Outcome` 增 `pub ev_retention: f64`，默认 1.0（passthrough/fail-open
恒 1.0，与现状单桶行为逐字节一致）。

**门禁**：
- 默认代理 vs fidelity 全量在冻结语料（`config/fidelity.corpus.json`）上的相关性 **≥ 0.9**；
- evidence 缺省时行为与现状逐字节一致（golden 32 零漂移）；
- 显式 evidence 单测：构造证据列表断言 `ev_retention` 逐位正确。

**验收**：`Outcome.ev_retention` 全链路可用（CLI/MCP/wrap）；相关性 ≥0.9 冻结。

### P5：快照指纹 + MCP 生命周期 + 观测聚合

**目标**：让已交付的 `meter`/`bucket`/`Ledger` 具备内容寻址、可追溯与可聚合，形成闭环。

**5.1 快照指纹（扩展 `meter::Snapshot`，非新模块）**

```rust
pub struct Snapshot {
    pub id: String,                // 改为 sha256(payload_sha256 + config_fingerprint)[..16]
    pub round: u64,
    pub payload_sha256: String,    // 新增：输入 payload sha256（hex）
    pub config_fingerprint: String,// 新增：policy 表 + 版本哈希
    pub buckets: Vec<BucketStat>,
    pub tokens_in: f64,
    pub tokens_out: f64,
    pub metrics: Metrics,
    pub budget: Budget,
}
```

- `id` 由调用方传入改为内容派生（确定性、可复现）；`round` 保留。
- 这是 meter（P3 模块）的演进，不影响 compress 内核与 golden；`Ledger::by_id` 语义不变。

**5.2 MCP 生命周期工具（`mcp.rs` tools/list 静态表新增）**

| 工具 | 作用 | 关键参数 |
| --- | --- | --- |
| `context_snapshot` | 计量 + 前置快照（不压缩） | text, kind?, evidence? |
| `compress_context` | 标准压缩入口（已有，加 kind/evidence） | query, text, kind?, evidence? |
| `context_diff` | 查询最近一次快照差分 | id? |
| `ledger_summary` | 聚合收益（按 kind/天） | window? |

**5.3 观测聚合（`ledger --summarize --json`）**

```json
{"window":"24h","by_kind":[{"kind":"tool","tokens_saved":...,"ratio":...,"ev_retention":...}],
 "total":{"tokens_saved":...,"est_cost_saved_usd":...,"calls":...}}
```

- `est_cost_saved_usd`：token 单价声明式配置（`ledger.price` 表，默认空 = 不估算，绝不编造价格）。
- 输出确定性排序（kind 字典序 + 时间窗归一）。

**门禁**：快照确定性单测（同输入同配置 → 同 id/同指纹）；MCP 协议测试
（initialize → snapshot → compress → diff → ledger_summary 全生命周期）；
聚合数学测试（构造账本断言汇总）。

**验收**：MCP 4 工具全链路可用；`ledger --summarize --json` 契约冻结；golden 零漂移。

### P6：骨架保护（structure anchors）

**目标**：只做**骨架保护**，不做语义理解——保证压缩后 JSON/XML/IDoc 仍可被标准解析器解析。

**现状**：行级删除不破坏单行语法，但会删结构边界行（`{`、`}`、`<tag>`），导致下游解析失败。

**方案（范围控制）**：
- 结构边界行（JSON 花括号、XML/IDoc 起止标签、缩进层级首行）标记为**结构锚点**，
  优先级高于普通噪声行、**低于证据行**；
- 配置声明：`structure_anchors: [json, xml, idoc, yaml]`（默认 off = 现状逐字节不变）；
- 与现有 `--structure 1`（fence/import/堆栈）并存，互不干扰。

**门禁**：构造 JSON/XML/IDoc 语料，压缩后仍可被标准解析器解析（parse 成功 +
证据保留率 ≥ 0.95）。

**验收**：JSON/XML/IDoc 解析门冻结；默认 off 路径逐字节不变。

### P7：脱敏规则化 `RedactConfig`

**目标**：企业（ERP/SAP 等）场景的 PII/密钥治理；压缩前脱敏，审计留痕。

**设计**：`RedactConfig` 规则表（独立 toml，`--redact <rules.toml>`）：

```rust
pub struct RedactRule {
    pub name: &'static str,      // aws_key / conn_str / email / phone / id_card ...
    pub pattern: &'static str,   // 复用现有 regex.rs 子集，或确定性手写匹配器
    pub action: RedactAction,    // Mask | Replace | Hash
    pub payload: &'static str,   // mask 字符 / 替换文案 / hash 盐
}
```

- **正则约束（必须写清）**：现有 `src/regex.rs` 是手写 Thompson NFA 子集，
  **不支持 `.` 通配、`()` 分组、`\d` 简写、`{}`**（见 `src/regex.rs:9-13`）。
  高置信模式（密钥/连接串，形如 `AKIA...`、`postgres://`、`Bearer `）可用现有子集
  或复用 `memory/util.rs` 的 `sanitize` 逻辑；身份类（身份证/手机）用确定性手写匹配器
  （校验位/长度规则），**不引入第三方 regex crate**。
- 入口：压缩**前**对输入脱敏，压缩后输出即脱敏文本；证据行 hash 校验在脱敏后语义上做。
- 审计日志：命中规则 + 次数 + 规则版本写入 `Ledger`（`kind:"redact"`）；脱敏不可逆（hash 单向），
  原文不落盘（快照仅存脱敏后 hash）。

**门禁（分档，不搞一刀切 99%/0.5%）**：
- 高置信模式（密钥/连接串）：检出率 ≥ 99%、误报率 ≤ 0.5%；
- 身份类（身份证/手机/邮箱）：检出率 ≥ 90%、误报率 ≤ 3%。

**验收**：高置信模式门冻结；脱敏 + 快照审计链路可用；`--redact` 未启用时逐字节不变。

## 3. 缓办与砍掉（v0.1 中收益薄/不可行/投机的项）

| 项 | v0.1 位置 | 处置 | 理由 |
| --- | --- | --- | --- |
| BPE 精确器 | P1 缺口 1 | **砍掉**，移出路线图 | ① vocab 授权需确认；② 零依赖下手写 byte-level BPE 拿不到 Claude/GPT 精确 merge rank，就退化成"另一个估算器"；③ 压缩器绝对 token 精度只影响 `--max-tokens` 与成本估算，而成本默认不估算。将来真需要时，独立 feature + 第三方 crate 授权后再议。 |
| Calibrated 校准表 | P1 缺口 1 | **已移除** | 三次方向（隐式分类/词数密度/换行特征）均撞共线（words↔alnum VIF≈596）与线性误设地板（留出 MAE 4.66 不回退）；无消费方需要低绝对 MAE。改以 `--max-tokens` 保守上界（`BUDGET_SAFETY_FACTOR`，`tools/calibrate.py` 定分位数）保预算安全。 |
| SessionState 会话增量 | P4 缺口 9 | **缓办** | 等价性门（增量==全量逐字节）维护成本高，且前提是真实多轮场景存在。等 P4/P5 落地、有真实收益数据后再立项。 |
| token 级裁剪 | 风险 3 | **维持砍掉** | 保持整行语义与结构安全；与 LLMLingua-2 的压缩率差距仍在（1.1–2.4x vs 2–20x），追平需带结构保护的 token 级算子，单独立项。 |

## 4. 门禁与验收矩阵

| 阶段 | 新增门禁（CI 步骤） | 关键阈值 | 兼容门（必须保持） |
| --- | --- | --- | --- |
| P4 | 留存相关性、evidence 单测 | 相关性 ≥0.9 | golden 32 零漂移、Gauge/fidelity 复跑 |
| P5 | 快照确定性、MCP 生命周期、聚合数学 | 全链路 PASS、schema 冻结 | 同上 |
| P6 | 骨架解析 | parse PASS、证据保留率 ≥0.95 | 同上 |
| P7 | 脱敏检出/误报（分档） | 高置信 99%/0.5%、身份类 90%/3% | 同上 |

## 5. 风险与开放问题

1. **留存代理的自欺风险**：行 hash 保留率 ≠ 信息留存；唯一护栏是 P4 的相关性门 ≥0.9。
   若实测不达标，默认代理不可信，只能依赖显式 evidence——这会削弱"轻量默认"的价值。
   对策：相关性门不过则本阶段不交付，如实上报。
2. **分类误判**：`bucket::ContentType` 隐式推断把契约误判为工具输出 → 可能被裁剪；
   fail-open 到 ToolOutput 是安全侧但损失价值梯度收益。对策：显式 `--kind` 标注优先，隐式只作缺省。
3. **快照 id 改内容派生**：影响已消费 `meter::Snapshot.id` 的外部调用方；meter 为 P3 新模块、
   无 golden 依赖，破坏面可控，但需在 P5 单独 commit 并写明变更。
4. **脱敏正则能力边界**：现有 regex.rs 子集不足以覆盖全部 PII 模式；身份类靠手写匹配器，
   召回/误报双降。若需完整 regex，须先决策"是否引入 regex crate"（与零依赖红线冲突），
   本方案默认**不引入**，只做高置信模式。
5. **前端可视化**：本方案只交付数据契约（聚合 JSON + schema），前端实现另行立项，不进入内核。

## 6. 与既有资产的关系

- **复用**：`bucket::ContentType`（分类 + 价值梯度）、`meter::BucketStat/Snapshot/Diff/Ledger`
  （计量/快照/账本）、`terms::estimate/count`（唯一计量口径）、`ops::render`（JSON 渲染）、
  `memory/util.rs` 脱敏思路（P7 起点）、`scx.py manifest/report`（冻结机制）、
  Gauge/fidelity/contain（评测基线）、记忆库 lock 机制（若 P4/P5 需要会话状态）。
- **不改变**：压缩内核默认路径、零依赖与 `#![forbid(unsafe_code)]`、CLI 现有参数语义、
  `Outcome`/`Mode` 的 v0.4.1 统计行字符串（`Mode::as_str` 冒号分段格式）。
