#!/bin/bash
# P22 本地冒烟/正式运行启动器（由 run_p22.py 包装，参数透传）。
set -u
cd /Users/fei/code/scx/tools/bench_p22_compact
export SCX_P22_LLM_LOG="${SCX_P22_LLM_LOG:-/tmp/scx_bench_compact/llm_calls.jsonl}"
exec /tmp/scx-eval/bin/python run_p22.py "$@"
