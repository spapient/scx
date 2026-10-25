#!/bin/sh
# scx 本机构建（macOS / Linux / WSL）：cargo build --release 并安装到 bin/
# 用法：sh build.sh
# 构建目标目录放在库外（缓存复用），避免污染技能目录。
set -e
cd "$(dirname "$0")"
TD="${SCX_TARGET_DIR:-${TMPDIR:-/tmp}/scx-cargo-target}"
cargo build --release --locked --target-dir "$TD"
mkdir -p bin
cp "$TD/release/scx" bin/scx
chmod +x bin/scx
echo "installed: bin/scx (cargo target dir: $TD)"
