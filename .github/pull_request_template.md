## 变更摘要

<!-- 说明问题、改动和为什么这样改。 -->

## 验证

<!-- 粘贴实际执行的命令和结果；未执行的检查请明确写“未执行”。 -->

- [ ] `cargo fmt -- --check`
- [ ] `cargo clippy --locked --all-targets -- -D warnings`
- [ ] `cargo test --locked`
- [ ] `cargo build --release --locked`
- [ ] `./bin/scx-eval selftest`（涉及评测工具或语料时）
- [ ] 新增/修改行为有对应测试

## 风险与回滚

<!-- 影响范围、兼容性、性能或数据风险；如何回滚。 -->

## 敏感信息检查

- [ ] 未提交真实凭据、私钥、客户数据或未公开事故材料
- [ ] 新增日志、样例或评估产物已人工检查
- [ ] 涉及漏洞时已按 SECURITY.md 私下报告，而不是在本 PR 披露
