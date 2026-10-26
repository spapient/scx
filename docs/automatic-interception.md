# 自动拦截：让压缩发生在工具输出进上下文之前

`scx` 默认接入是「MCP 工具 + SKILL.md」——模型读到技能后**主动调用** `compress_context`。
但模型是否调用是概率性的。本节给出两条不依赖模型自觉的路径，按自动化程度排序。

> 状态：`scx wrap` 已实现并测试；宿主 hook 为**模板/思路**，未在真实宿主实测，
> 具体脚本需按宿主当前的 hook 协议调整。以下示例以 Claude Code 的 hook 形态为参考。

## 1. `scx wrap`（手动包装，已实现）

把任意命令的输出合并后压缩，一条命令完成，退出码沿用被包装命令：

```bash
scx wrap --query "compile error" -- cargo build
scx wrap --query "db timeout" --preserve errno -- make test
```

- 小输出（<40 行且 <600 token）自动 passthrough，原样透传。
- 大输出按 `--query`/`--force`/`--preserve` 筛选；正文写 stdout、效果卡写 stderr。
- 等价于 `CMD > /tmp/out 2>&1 && scx --query "..." /tmp/out`。

## 2. 宿主 hook（完全自动，模板）

思路：在宿主 Agent 的 **PostToolUse** 阶段拦截工具输出，若输出大就交给 `scx` 压缩后再回填。
脚本骨架（bash，需按宿主协议解析 JSON-RPC 的 `tool_response` 字段）：

```bash
#!/usr/bin/env bash
# 骨架：读 stdin 的 hook JSON，压缩大 tool_response，写回。未实测，需按协议调整。
set -euo pipefail
input="$(cat)"
# 1. 解析 tool_response 内容（字段名/路径取决于宿主 hook 协议）。
resp="$(printf '%s' "$input" | jq -r '.tool_response // empty')"
[ -z "$resp" ] && { printf '%s' "$input"; exit 0; }
# 2. 大输出才压缩（阈值自行定，如 40 行 / 600 token）。
lines="$(printf '%s' "$resp" | wc -l | tr -d ' ')"
if [ "$lines" -lt 40 ]; then printf '%s' "$input"; exit 0; fi
# 3. 压缩并把结果回填（query 可取自 hook 的 tool_name 或固定关键字）。
compressed="$(printf '%s' "$resp" | scx --query 'error timeout failed' --no-stats)"
printf '%s' "$input" | jq --arg c "$compressed" '.tool_response = $c'
```

配置（Claude Code 形态，`settings.json` 的 `hooks.PostToolUse`）：

```json
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Bash|Read|Grep",
        "hooks": [{ "type": "command", "command": "$HOME/.claude/hooks/scx-compress.sh" }]
      }
    ]
  }
}
```

**注意事项**：

- 这是**骨架**，`tool_response` 的 JSON 字段名、hook 的 stdin 结构、写回方式都会随宿主协议变化，落地前必须对照宿主文档实测。
- 只在「大工具输出」场景挂 hook（Bash/Read/Grep/日志类工具），避免对已经很小的输出加开销。
- `scx` 的压缩是确定性、本地、毫秒级的，hook 引入的延迟可忽略，但不要挂到高频小工具上。

## 与 MCP 的关系

- MCP（`scx setup` + `compress_context`）：模型**主动**、语义化地压缩一段文本。
- `wrap` / hook：**被动**、自动地压缩工具输出，不依赖模型自觉。

两者互补：hook 保证「大输出必压缩」，MCP 允许模型在「任何文本」上主动调压缩。
