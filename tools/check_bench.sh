#!/bin/bash
# scx 评测闭环门禁（P7-E）：一条命令校验 代码门禁 + 定稿结果不可变基线。
# 用法: tools/check_bench.sh
# 校验:
#   1. cargo test --locked --lib（341）
#   2. cargo test --locked --features semantic --lib（350）
#   3. cargo test --locked --test golden（零漂移）
#   4. clippy 全 targets 零告警 + fmt 干净
#   5. eval-manifest.json：frozen 结果 sha256 + 行数匹配；present 文件存在且行数达标
# 任一失败 → 非零退出（CI 可直接挂接）。
set -uo pipefail
cd "$(dirname "$0")/.."

FAIL=0
step() { printf '\n== %s ==\n' "$1"; }
fail() { FAIL=1; echo "FAIL: $1"; }

step "cargo test --locked --lib"
cargo test --locked --lib 2>&1 | tail -3 | grep -q "0 failed" || fail "lib tests"

step "cargo test --locked --features semantic --lib"
cargo test --locked --features semantic --lib 2>&1 | tail -3 | grep -q "0 failed" || fail "semantic tests"

step "cargo test --locked --test golden"
cargo test --locked --test golden 2>&1 | tail -3 | grep -q "0 failed" || fail "golden tests"

step "clippy --all-targets"
W=$(cargo clippy --locked --all-targets 2>&1 | grep -cE "^warning:|^error")
[ "$W" = "0" ] || fail "clippy $W warnings"

step "fmt --check"
cargo fmt --check >/dev/null 2>&1 || fail "fmt not clean"

step "eval-manifest 冻结校验"
MANIFEST=config/bench/eval-manifest.json
[ -f "$MANIFEST" ] || fail "missing $MANIFEST"
/tmp/scx-eval/bin/python - "$MANIFEST" <<'PY' || fail "manifest integrity"
import hashlib, json, os, sys
m = json.load(open(sys.argv[1], encoding="utf-8"))
ok = True
for name, want in m.get("frozen", {}).items():
    p = os.path.join("config/bench/results", name)
    if not os.path.exists(p):
        print(f"  frozen {name}: MISSING"); ok = False; continue
    data = open(p, "rb").read()
    rows = data.count(b"\n") + (0 if data.endswith(b"\n") else 1)
    h = hashlib.sha256(data).hexdigest()
    for key, val in (("sha256", h), ("rows", rows)):
        if str(want.get(key)) != str(val):
            print(f"  frozen {name}: {key} mismatch want={want.get(key)} got={val}"); ok = False
for item in m.get("present", []):
    p = os.path.join("config/bench/results", item["file"])
    if not os.path.exists(p):
        print(f"  present {item['file']}: MISSING"); ok = False; continue
    rows = sum(1 for _ in open(p, encoding="utf-8"))
    if rows < item.get("min_rows", 2):
        print(f"  present {item['file']}: rows={rows} < {item.get('min_rows',2)}"); ok = False
print("  " + ("OK: frozen + present 全部匹配" if ok else "FAIL"))
sys.exit(0 if ok else 1)
PY

step "A2 保真断言（fidelity_thresholds 分档）"
/tmp/scx-eval/bin/python tools/check_fidelity.py || fail "A2 fidelity"

if [ "$FAIL" = "0" ]; then
  echo; echo "check_bench: 全部通过 ✓"
else
  echo; echo "check_bench: 存在失败项"; exit 1
fi
