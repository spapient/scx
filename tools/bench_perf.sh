#!/bin/bash
# scx 压缩性能基线（P7-C 可复现门禁，macOS/Linux，零依赖）
# 用法: tools/bench_perf.sh [--release-build]
# 输出: 输入大小/行数、三档（lossless/r0.3/r0.2）3 次最优 real 秒 + 压缩率 cut%
# 语料: 项目源码(src/**/*.rs) + 评测集(config/bench/datasets/*.csv question 列) 拼接
set -u
cd "$(dirname "$0")/.."

BIN=target/release/scx
if [ ! -x "$BIN" ] || [ "${1:-}" = "--release-build" ]; then
  cargo build --release --locked
fi

TMP_IN="$(mktemp -t scx_perf.XXXXXX)"
trap 'rm -f "$TMP_IN"' EXIT
{
  find src -name '*.rs' -print0 | sort -z | xargs -0 cat
  cat docs/plan.md README.md 2>/dev/null || true
  python3 - <<'PY'
import csv, glob
for f in sorted(glob.glob("config/bench/datasets/*.csv")):
    try:
        for row in csv.reader(open(f, encoding="utf-8")):
            if len(row) >= 2 and row[0].strip():
                print(row[0])
    except FileNotFoundError:
        pass
PY
} > "$TMP_IN"

CHARS=$(wc -c < "$TMP_IN" | tr -d ' ')
LINES=$(wc -l < "$TMP_IN" | tr -d ' ')
echo "== scx perf baseline =="
echo "input: ${CHARS} chars / ${LINES} lines"

run() { # ratio label
  local r="$1" label="$2" best="" out real cut
  for i in 1 2 3; do
    out="$(mktemp -t scx_perf_out.XXXXXX)"
    /usr/bin/time -p "$BIN" --query "fn compress" --ratio "$r" \
        --trim 1 --trim-ratio "$r" --trim-op-keep 1 --num-line-keep 1 \
        "$TMP_IN" > "$out" 2>/tmp/scx_perf_err.txt
    real=$(grep '^real' /tmp/scx_perf_err.txt | awk '{print $2}')
    if [ -z "$best" ] || awk -v a="$real" -v b="$best" 'BEGIN { exit !(a < b) }'; then
      best="$real"
    fi
    cut=$(awk -v i="$CHARS" -v o="$(wc -c < "$out" | tr -d ' ')" \
        'BEGIN { printf "%.1f%%", (1 - o / i) * 100 }')
    rm -f "$out"
  done
  printf '%-12s best=%-8s cut=%s\n' "$label" "$best" "$cut"
}

run 1.0   "lossless"
run 0.3   "r0.3"
run 0.2   "r0.2"
