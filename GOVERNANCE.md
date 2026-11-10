# 项目治理

本文件说明 `scx` 的决策方式、角色职责与访问连续性安排，
用于回答"谁做决定""谁能发布""维护者失联会怎样"这类问题。

## 项目性质

`scx` 是一个志愿者维护的开源项目，采用 MIT OR Apache-2.0 双许可证。
项目不设基金会、不设商业实体，没有付费支持承诺。

## 角色与职责

### 维护者（Maintainer）

当前：[@spapient](https://github.com/spapient)

职责：

- 对架构方向、公开契约（CLI 参数、JSON 字段、MCP schema）拥有最终决定权
- 审查并合并拉取请求，维护 `main` 分支保护设置
- 执行发布：版本号、标签、crates.io / npm / Homebrew / GitHub Release 四渠道
- 响应安全报告，按 [SECURITY.md](SECURITY.md) 协调披露
- 维护本治理文件与 [CHANGELOG.md](CHANGELOG.md)

### 评审者（Reviewer）

由维护者按需授予，通常授予持续提交高质量变更的贡献者。

职责：在维护者授权范围内审查 PR、运行门禁、给出合并建议。
评审者不持有发布凭据。

### 贡献者（Contributor）

任何提交 issue、文档或代码的人。职责与要求见 [CONTRIBUTING.md](CONTRIBUTING.md)。

贡献者通过 `Signed-off-by` 声明其有权提交该贡献（DCO），见贡献指南。

## 决策方式

1. **惰性共识（lazy consensus）**：常规变更由 PR 提出，维护者在合理期限内
   未提出反对即视为通过。
2. **公开契约变更需显式批准**：涉及 CLI 参数、JSON 字段、MCP schema、
   golden 基线的变更，必须由维护者明确批准，并同步更新文档与变更日志。
3. **争议裁决**：出现分歧时，先在 issue 中记录双方理由与取舍；
   维护者做最终裁决，并把裁决理由写入对应 issue 或 PR，便于日后追溯。
4. **不可逆操作**：`yank` 已发布版本、修改已发布标签、改写 `main` 历史，
   需在仓库 issue 中留下公开记录后再执行。

## 分支与发布

- `main` 受保护：禁止直接推送，要求至少一次评审与通过的 CI 检查项
  （`quality` 与三平台 `test`）。
- 维护者保留管理员绕过权限，仅用于仓库不可用等紧急场景；
  若启用绕过，须在同一日内补发 PR 或在 issue 中记录原因。
- 发布需同时满足：`cargo fmt --check`、`cargo clippy --all-targets --all-features -D warnings`、
  全特性矩阵构建、`cargo test --all-features` 全绿、可复现构建校验。
- 每个发布必须有唯一版本号、标签与人类可读的变更说明（见 `CHANGELOG.md`）。

## 发布凭据

发布依赖以下凭据，均由维护者掌管：

| 渠道 | 凭据 |
| --- | --- |
| crates.io | API token |
| npm `@spapient/scx` | npm token |
| Homebrew tap | GitHub 推送权限 |
| GitHub Release | GitHub token（`repo` 范围） |

凭据不进入仓库、不写入 CI 日志；CI 仅使用 `contents: read` 最小权限。

## 访问连续性

**已知风险**：项目当前只有一名维护者（bus factor = 1）。

为把"维护者失能导致项目停摆"的影响降到最低，采取以下措施：

1. **源码与构建可独立复现**：零第三方 crate 依赖，`Cargo.lock` 锁定，
   可复现构建已验证（两次独立目标目录构建产出相同 SHA-256）。
   任何第三方都能从公开源码重建出与发布资产一致的二进制。
2. **发布流程完全文档化**：本文件与 [CONTRIBUTING.md](CONTRIBUTING.md)、
   [docs/release-checklist.md](docs/release-checklist.md) 记录了四渠道发布步骤，
   具备 Rust 工具链经验的人可按文档独立完成发布。
3. **凭据交接预案**：维护者应把上述发布凭据的恢复方式（令牌重新签发流程、
   Homebrew tap 归属转移、crates.io 所有者变更）以离线方式记录并交由可信第三方保管，
   同时明确继任者。该预案不由仓库承载，避免凭据泄漏。
4. **一周内可继续运作**：在上述预案生效的前提下，继任者可在一周内
   创建或关闭 issue、接受变更、发布新版本。

若项目未来引入第二名常态化维护者，本节的 bus factor 风险即解除，
应同步更新此文件与 `main` 的保护规则。

## 行为准则

参与本项目须遵守 [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)。
违规报告渠道见该文件"执行"一节。

## 变更本文件

本文件的修改由维护者提出 PR，公开说明理由，合并后记入 `CHANGELOG.md`。
