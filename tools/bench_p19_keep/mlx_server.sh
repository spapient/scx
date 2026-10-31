#!/bin/bash
# MLX 3B judge server 一键起停（日常探索用 8bit 档）
# 用法: ./mlx_server.sh start|stop|status [port]
set -euo pipefail
PORT="${2:-8123}"
MODEL="${MLX_MODEL:-/Users/fei/model/qwen2.5-3b-instruct-mlx-8bit}"
case "${1:-}" in
  start)
    if lsof -ti :"$PORT" >/dev/null 2>&1; then echo "server 已在 $PORT 运行"; exit 0; fi
    nohup ~/scx-eval-mlx/bin/python -m mlx_lm.server --model "$MODEL" --kv-bits 8 --port "$PORT" \
      > /tmp/mlx_server_8bit.log 2>&1 &
    echo "启动中 pid=$!（模型 $MODEL，端口 $PORT）"
    for i in 1 2 3 4; do
      sleep 5
      lsof -i :"$PORT" 2>/dev/null | grep -q LISTEN && { echo "OK: 监听 $PORT"; exit 0; }
    done
    echo "启动失败，看 /tmp/mlx_server_8bit.log"; exit 1
    ;;
  stop)
    lsof -ti :"$PORT" 2>/dev/null | xargs -r kill -9 2>/dev/null || true
    echo "已停止 $PORT"
    ;;
  status)
    lsof -i :"$PORT" 2>/dev/null | grep LISTEN || echo "未运行"
    ;;
  *)
    echo "用法: $0 start|stop|status [port]"; exit 1
    ;;
esac
