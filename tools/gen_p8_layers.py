#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""P8-C 跨层难度递进对照集（5 组 × L1/L2/L3 三问 = 15 题）。

同一主题三问递进：L1 语义理解 → L2 性能/复杂度 → L3 架构/设计权衡。
验证 scx 压缩在不同难度层的保持率差异（泛化性）。

产物：config/bench/datasets/bench_layers15.csv（列 question,answer）。
"""
from __future__ import annotations

import csv
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "config/bench/datasets/bench_layers15.csv"
N = 15


def node(src: str) -> str:
    return subprocess.run(["node", "-e", src], check=True,
                          capture_output=True, text=True).stdout.strip()


T = []  # (layer, theme, question, gold, verify_src or None)


def add(layer: str, theme: str, q: str, gold: str, verify=None) -> None:
    T.append((layer, theme, q, gold, verify))


# ===== 组 1: CUDA 规约（reduce）=====
add("L1", "cuda_reduce", """以下 CUDA 规约代码中，块 0 的 g[0] 是多少（d[0..15] = 0..15）？
```cuda
__global__ void k(int* g, int* d) {
    __shared__ int s[16];
    s[threadIdx.x] = d[threadIdx.x];
    __syncthreads();
    for (int stride = 8; stride > 0; stride >>= 1) {
        if (threadIdx.x < stride) s[threadIdx.x] += s[threadIdx.x + stride];
        __syncthreads();
    }
    if (threadIdx.x == 0) g[0] = s[0];
}
k<<<1, 16>>>();
```""", "120", None)

add("L2", "cuda_reduce", """上述规约代码中，如果 s 数组改为 stride 访问 s[threadIdx.x * 2]（线程访问间隔 2），会引入什么问题？""",
    "共享内存 bank conflict（性能下降）", None)

add("L3", "cuda_reduce", """要规约 100 万个 int（超出单块共享内存），最合理的分层架构是什么？""",
    "每块规约出部分和 → 块间用原子加或二次 kernel 聚合", None)

# ===== 组 2: JS 闭包/状态 =====
add("L1", "js_closure", """以下 JS 代码输出什么？
```js
function f() { let n = 0; return () => ++n; }
const a = f();
console.log(a(), a());
```""", "1 2", """function f() { let n = 0; return () => ++n; }
const a = f();
console.log(a(), a());""")

add("L2", "js_closure", """大量使用闭包捕获大对象时，主要的运行时开销是什么？""",
    "闭包环境对象保留引用（GC 压力/内存占用）", None)

add("L3", "js_closure", """在高频调用场景（如每帧创建回调）中，管理闭包状态的更优架构模式是什么？""",
    "函数复用/对象复用（避免每帧新建闭包）+ 状态外置", None)

# ===== 组 3: SQL 索引 =====
add("L1", "sql_index", """给定表 t(id, v)，以下查询返回几行？
```sql
SELECT * FROM t WHERE v = 3;
```（t: (1,3),(2,3),(3,1)）""",
    "2", """sqlite3 x.db "CREATE TABLE t(id INT, v INT); INSERT INTO t VALUES (1,3),(2,3),(3,1); SELECT COUNT(*) FROM t WHERE v = 3;" """)

add("L2", "sql_index", """对 v 列建 B+ 树索引后，WHERE v = 3 的查找复杂度从什么降为什么？""",
    "全表扫描 O(n) → 索引查找 O(log n)", None)

add("L3", "sql_index", """高写入表上为每列都建索引，主要权衡是什么？""",
    "查询加速 vs 写入/存储开销（索引维护成本）", None)

# ===== 组 4: 正则 =====
add("L1", "regex", """以下正则对 'aaa' 的匹配结果是什么？
```python
import re
len(re.findall(r'a{2}', 'aaa'))
```""", "1", """import re
print(len(re.findall(r'a{2}', 'aaa')))""")

add("L2", "regex", """正则 (a+)+b 对长串 'aaaa...a'（无 b）会发生什么？""",
    "灾难性回溯（指数级回退，卡死）", None)

add("L3", "regex", """对不可信输入做解析时，避免灾难性回溯的工程手段是什么？""",
    "有限状态机（无回溯）+ 超时/长度上限", None)

# ===== 组 5: 配置分层 =====
add("L1", "config", """以下 TOML 中，[servers] 下 alpha 的 ip 是什么？
```toml
[servers.alpha]
ip = "10.0.0.1"
```""", "10.0.0.1", """import tomllib
print(tomllib.loads('[servers.alpha]\\nip = "10.0.0.1"')['servers']['alpha']['ip'])""")

add("L2", "config", """大型应用启动时重复解析同一配置文件，主要开销是什么？""",
    "文件 I/O + 解析 CPU 开销（可缓存/懒加载）", None)

add("L3", "config", """生产级配置系统的推荐分层结构是什么？""",
    "默认值 → 环境覆盖 → 本地覆盖 → 密钥单独注入（十二因子）", None)


def validate() -> tuple[int, int]:
    ok = 0
    for layer, theme, q, gold, src in T:
        assert len(q) > 10 and len(gold) > 0, "空题"
        if src is not None:
            if src.startswith("import") or src.startswith("print"):
                out = subprocess.run(["/tmp/scx-eval/bin/python", "-c", src],
                                     check=True, capture_output=True, text=True).stdout.strip()
            elif src.startswith("sqlite3"):
                import tempfile
                with tempfile.TemporaryDirectory() as td:
                    out = subprocess.run(["bash", "-c", f"cd {td} && {src}"],
                                         check=True, capture_output=True, text=True).stdout.strip()
            else:
                out = node(src)
            want = gold.split("（")[0].strip()
            assert out == want, f"mismatch: {out!r} != {want!r} | {q[:50]}"
            ok += 1
    return len(T), ok


def main() -> None:
    n, ok = validate()
    assert n == N, f"题量 {n} != {N}"
    with open(OUT, "w", newline="", encoding="utf-8") as fh:
        w = csv.writer(fh)
        w.writerow(["question", "answer"])
        for layer, theme, q, gold, _ in T:
            w.writerow([f"【{layer}】{q}", gold])
    print(f"written {OUT}: {n} 题（5 组 × L1/L2/L3，工具验证 {ok}）")


if __name__ == "__main__":
    main()
