#!/usr/bin/env python3
"""测试用向量 provider：按 scx 的 Q/L 行协议输出定长 4 维向量。

stdin：首行 `Q\t<query>`，随后 `L\t<text>`；stdout：每个输入行一个向量。
设置 `STUB_LOG` 时把每条输入的首字段追加记录，用于验证缓存命中后不再编码候选。
"""
import os
import sys

log = os.environ.get("STUB_LOG")

for line in sys.stdin:
    line = line.rstrip("\n")
    if not line:
        continue
    if log:
        with open(log, "a", encoding="utf-8") as f:
            f.write(line.split("\t", 1)[0] + "\n")
    text = line.split("\t", 1)[1] if "\t" in line else ""
    print(f"{len(text)} 1 0 0", flush=True)
