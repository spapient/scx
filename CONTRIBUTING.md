# 贡献指南

感谢参与 `scx`。本项目优先保证正确性、证据可追溯性与确定性，其次才是性能与体积。

## 提交变更

1. 从最新 `main` 创建分支，例如 `feat/recall-bucket` 或 `fix/cache-window`。
2. 保持变更聚焦；不要在同一个 PR 中混合无关重构、格式化与行为修改。
3. 新增或修改行为时补充测试；涉及 golden 输出时按 README 的流程审查并重冻基线。
4. 提交 PR 并填写模板中的验证命令、风险与回滚方式。

`main` 受分支保护：不接受直接推送，至少需要一次维护者评审（CODEOWNERS）和通过的 CI。维护者自己的紧急变更仍应通过 PR 留痕，除非仓库处于不可用状态。

## 本地验证

构建缓存放在仓库外，避免技能扫描把 `target/` 当成仓库内容：

```bash
export CARGO_TARGET_DIR="${TMPDIR:-/tmp}/scx-cargo-target"
cargo fmt -- --check
cargo clippy --locked --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps
cargo test --locked
cargo build --release --locked
./tools/scx.py selftest
```

Windows PowerShell：

```powershell
$env:CARGO_TARGET_DIR = "$env:TEMP\scx-cargo-target"
cargo fmt -- --check
cargo clippy --locked --all-targets -- -D warnings
$env:RUSTDOCFLAGS = "-D warnings"
cargo doc --locked --no-deps
cargo test --locked
cargo build --release --locked
python tools/scx.py selftest
```

PR 描述中请粘贴实际执行的命令与结果；未执行的检查不要写成已通过。

## 设计约束

- 保留行只删不改；证据行与种子邻域不可被预算挤掉。
- 筛选引擎保持纯函数、确定性、无随机源；CLI 只做 I/O 与参数解析。
- 记忆库召回与评估改动不得让未标注数据冒充人工证据。
- 性能结论必须带基线、环境、重复次数与波动范围；单次运行不作为结论。
- 不引入未经说明的网络访问、遥测或第三方依赖。

## 数据、秘密与许可证

- 不要提交真实 token、密码、私钥、客户数据或未公开事故材料。
- 测试夹具使用明显虚构的值；发现误提交的秘密时立即撤销凭据并联系维护者。
- 贡献内容以仓库的 MIT OR Apache-2.0 双许可证发布。

## 安全漏洞

不要用公开 issue 报告漏洞，请按 [SECURITY.md](SECURITY.md) 私下提交。
