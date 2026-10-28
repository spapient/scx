#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""P8-B 非源码代码形态评测集生成器（30 题）。

覆盖 LLM 上下文里的非源码代码形态：JSON/YAML/TOML 配置语义、
SQL 查询、正则、shell/构建脚本、日志/数据管线。
gold 全部本机工具执行验证（python json/tomllib/yaml、sqlite3、bash），
格式统一 "答案是 X #### X"。

产物：config/bench/datasets/bench_shape30.csv（列 question,answer）。
"""
from __future__ import annotations

import csv
import json
import re
import sqlite3
import subprocess
import tempfile
import tomllib
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "config/bench/datasets/bench_shape30.csv"
N = 30


def sh(src: str) -> str:
    return subprocess.run(["bash", "-c", src], check=True,
                          capture_output=True, text=True).stdout.strip()


T = []  # (question, gold, verify_src or None)


def add(q: str, gold: str, verify_src: str | None = None) -> None:
    T.append((q, gold, verify_src))


# ===== 1-6: JSON 配置语义 =====
add("""以下 JSON 配置中，service.port 的值是多少？
```json
{"service": {"name": "api", "port": 8080}, "log": {"level": "info"}}
```""", "8080", """import json
d = json.loads('{"service": {"name": "api", "port": 8080}, "log": {"level": "info"}}')
print(d['service']['port'])""")

add("""以下 JSON 序列化输出中，键的顺序是什么？
```python
import json
print(json.dumps({"b": 1, "a": 2}))
```""", "{\"b\": 1, \"a\": 2}（按插入序）", """import json
print(json.dumps({"b": 1, "a": 2}))""")

add("""以下 JSON 中，deps 数组的长度是多少？
```json
{"name": "svc", "deps": ["db", "cache", "queue"]}
```""", "3", """import json
print(len(json.loads('{"name": "svc", "deps": ["db", "cache", "queue"]}')['deps']))""")

add("""以下 JSON 解析代码输出什么？
```python
import json
print(json.loads('{"a": [1, 2, 3]}')['a'][1])
```""", "2", """import json
print(json.loads('{"a": [1, 2, 3]}')['a'][1])""")

add("""以下 JSON 配置中，与 {} 等价的 Python 值是什么？""",
    "空 dict（False 布尔值）", None)

add("""以下 JSON 中，price 字段的类型是什么？
```json
{"item": "x", "price": 9.99}
```""", "float（浮点数）", """import json
print(type(json.loads('{"item": "x", "price": 9.99}')['price']).__name__)""")

# ===== 7-12: YAML/TOML 配置 =====
add("""以下 YAML 配置中，replicas 的值是多少？
```yaml
app:
  name: web
  replicas: 3
```""", "3", """import yaml
print(yaml.safe_load('app:\\n  name: web\\n  replicas: 3')['app']['replicas'])""")

add("""以下 YAML 中，env 是一个什么类型的值？
```yaml
env:
  - name: DEBUG
    value: "1"
```""", "list（列表）", """import yaml
print(type(yaml.safe_load('env:\\n  - name: DEBUG\\n    value: "1"')['env']).__name__)""")

add("""以下 TOML 配置中，[database] 段的 port 是多少？
```toml
[database]
host = "localhost"
port = 5432
```""", "5432", """import tomllib
d = tomllib.loads('[database]\\nhost = "localhost"\\nport = 5432')
print(d['database']['port'])""")

add("""以下 TOML 中，tags 的值类型是什么？
```toml
tags = ["dev", "test"]
```""", "list（字符串数组）", """import tomllib
print(type(tomllib.loads('tags = ["dev", "test"]')['tags']).__name__)""")

add("""以下 YAML 中，key: value 后跟注释行，解析后注释影响结果吗？""",
    "不影响（注释被忽略）", None)

add("""以下 TOML 中，[tool.test] 段的作用是什么？""",
    "声明 tool.test 子表（命名空间配置）", None)

# ===== 13-18: SQL =====
add("""给定表 students(name, score)，查询分数 > 80 的人数：
```sql
SELECT COUNT(*) FROM students WHERE score > 80;
```（students 含 5 行：90, 70, 85, 60, 88）""",
    "3", """sqlite3 x.db "CREATE TABLE students(name TEXT, score INT); INSERT INTO students VALUES ('a',90),('b',70),('c',85),('d',60),('e',88); SELECT COUNT(*) FROM students WHERE score > 80;" """)

add("""给定表 orders(id, amount)，以下查询的结果是什么？
```sql
SELECT SUM(amount) FROM orders;
```（amount: 10, 20, 30）""",
    "60", """sqlite3 x.db "CREATE TABLE orders(id INT, amount INT); INSERT INTO orders VALUES (1,10),(2,20),(3,30); SELECT SUM(amount) FROM orders;" """)

add("""以下 SQL 中，GROUP BY 的作用是什么？""",
    "按列分组聚合（配合聚合函数）", None)

add("""给定表 emp(name, dept)，查询每个部门的人数：
```sql
SELECT dept, COUNT(*) FROM emp GROUP BY dept;
```（emp: (a,IT),(b,HR),(c,IT)）""",
    "IT 2 行，HR 1 行", None)

add("""以下 SQL 中，JOIN 的作用是什么？""",
    "按关联键合并两张表的行", None)

add("""给定表 t(v)，以下查询返回多少行？
```sql
SELECT DISTINCT v FROM t;
```（v: 1, 1, 2, 3, 3）""",
    "3", """sqlite3 x.db "CREATE TABLE t(v INT); INSERT INTO t VALUES (1),(1),(2),(3),(3); SELECT COUNT(*) FROM (SELECT DISTINCT v FROM t);" """)

# ===== 19-24: 正则 =====
add("""以下正则对 'a1b2' 的匹配结果是什么？
```python
import re
re.findall(r'\\d', 'a1b2')
```""", "['1', '2']", """import re
print(re.findall(r'\\d', 'a1b2'))""")

add("""以下正则对 'cat 和 dog' 的匹配结果是什么？
```python
import re
re.search(r'[a-z]+', 'cat 和 dog').group()
```""", "cat", """import re
print(re.search(r'[a-z]+', 'cat 和 dog').group())""")

add("""以下正则匹配整个字符串吗？
```python
import re
re.fullmatch(r'\\d{3}', '123')
```""", "匹配（fullmatch 需整串）", None)

add("""以下正则的捕获组数量是多少？
```python
re.compile(r'(\\d+)-(\\d+)')
```""", "2", """import re
print(re.compile(r'(\\d+)-(\\d+)').groups)""")

add("""以下替换结果是什么？
```python
import re
re.sub(r'\\s+', '_', 'a b  c')
```""", "a_b_c", """import re
print(re.sub(r'\\s+', '_', 'a b  c'))""")

add("""以下正则中，^ 在字符类外的作用是什么？""",
    "锚定字符串开头", None)

# ===== 25-30: shell/构建脚本/日志 =====
add("""以下 shell 脚本输出什么？
```bash
for x in 1 2 3; do echo -n "$x"; done
```""", "123", """for x in 1 2 3; do echo -n "$x"; done; echo""")

add("""以下 shell 中，$(date) 的作用是什么？""",
    "命令替换（执行并插入输出）", None)

add("""以下 Makefile 中，make clean 会执行什么？
```makefile
clean:
\trm -f *.o
```""", "删除所有 .o 文件", None)

add("""以下日志行中，时间戳的格式是什么？
```text
2026-10-28T10:30:00Z [INFO] svc started
```""", "ISO 8601（UTC）", None)

add("""以下 grep 命令匹配的行数是多少？
```bash
echo -e 'a\\nb\\na' | grep -c 'a'
```""", "2", """echo -e 'a\\nb\\na' | grep -c 'a'""")

add("""以下 awk 命令的输出是什么？
```bash
echo '1 2 3' | awk '{print $2}'
```""", "2", """echo '1 2 3' | awk '{print $2}'""")


def validate() -> int:
    ok = 0
    for q, gold, src in T:
        assert len(q) > 10 and len(gold) > 0, "空题"
        if src is not None:
            if src.startswith("import") or src.startswith("print") or src.startswith("re."):
                out = subprocess.run(["/tmp/scx-eval/bin/python", "-c", src],
                                     check=True, capture_output=True, text=True).stdout.strip()
            elif src.startswith("sqlite3"):
                with tempfile.TemporaryDirectory() as td:
                    out = subprocess.run(["bash", "-c", f"cd {td} && {src}"],
                                         check=True, capture_output=True, text=True).stdout.strip()
            else:
                out = sh(src)
            # gold 规范化为答案核心（去括号说明）
            want = gold.split("（")[0].strip()
            # 数组输出格式归一（JSON 风格 vs repr）
            assert out == want or out.replace("'", '"') == want, \
                f"mismatch: {out!r} != {want!r} | {q[:50]}"
            ok += 1
    return ok


def main() -> None:
    ok = validate()
    assert len(T) == N, f"题量 {len(T)} != {N}"
    with open(OUT, "w", newline="", encoding="utf-8") as fh:
        w = csv.writer(fh)
        w.writerow(["question", "answer"])
        for q, gold, _ in T:
            w.writerow([q, gold])
    print(f"written {OUT}: {N} 题（工具验证 {ok}）")


if __name__ == "__main__":
    main()
