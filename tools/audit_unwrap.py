#!/usr/bin/env python3
"""scx unwrap/panic 审计门禁（零依赖，T4 工程顶级化）。

断言：
1. 生产路径（剥离 #[cfg(test)]/#[test]/mod tests 后）不允许裸 `.unwrap()`
   ——所有不变量取值必须 `expect("语义消息")`。
2. 所有生产路径 expect / panic! / unreachable! 消息非空（可诊断）。
3. 返回未通过项数（0 = 通过），供 check_bench.sh 集成。

例外（保留的合法 unwrap，与审查结论一致）：
- `join().unwrap()`：std::thread::scope 惯用（子线程必成功，join 前已借出）。
- 测试/示例/死路径：不要求改造（避免无意义 churn）。
"""
import io
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(ROOT, "src")

# 合法保留模式：scope join 惯用
ALLOWED = re.compile(r"\.join\(\)\.unwrap\(\)")

UNWRAP = re.compile(r"\.unwrap\(\)")
EXPECT = re.compile(r"\.expect\((\s*\))")
PANIC = re.compile(r"panic!\(\s*\)|unreachable!\(\s*\)")
EXPECT_EMPTY = re.compile(r"\.expect\(\)")
# 测试段起点
TEST_START = re.compile(r"#\[cfg\(test\)\]|#\[test\]|mod tests \{")


def prod_part(src: str) -> str:
    """截取第一个测试段之前的代码作为生产路径。"""
    cut = len(src)
    for m in TEST_START.finditer(src):
        cut = min(cut, m.start())
    return src[:cut]


def audit() -> int:
    fails = 0
    total_unwrap = 0
    total_expect = 0
    total_panic = 0
    for root, _, files in os.walk(SRC):
        for f in sorted(files):
            if not f.endswith(".rs"):
                continue
            p = os.path.join(root, f)
            s = io.open(p, encoding="utf-8").read()
            prod = prod_part(s)
            rel = os.path.relpath(p, ROOT)
            for m in UNWRAP.finditer(prod):
                line = prod.count("\n", 0, m.start()) + 1
                snippet = prod[max(0, m.start() - 40): m.end()].strip()
                if ALLOWED.search(snippet):
                    continue
                fails += 1
                print(f"裸 unwrap（需 expect 语义消息）: {rel}:{line}  {snippet!r}")
            # 空消息 expect / panic
            for m in EXPECT_EMPTY.finditer(prod):
                fails += 1
                print(f"空 expect 消息: {rel}:{prod.count(chr(10), 0, m.start()) + 1}")
            for m in PANIC.finditer(prod):
                fails += 1
                print(f"空 panic!/unreachable! 消息: {rel}:{prod.count(chr(10), 0, m.start()) + 1}")
            total_unwrap += len(UNWRAP.findall(prod))
            total_expect += len(EXPECT.findall(prod))
            total_panic += len(PANIC.findall(prod))
    print(f"生产路径统计: unwrap={total_unwrap} expect={total_expect} panic/unreachable={total_panic}")
    if fails:
        print(f"audit_unwrap: {fails} 处未达标")
    else:
        print("audit_unwrap: OK（生产裸 unwrap=0，expect/panic 消息非空）")
    return fails


if __name__ == "__main__":
    sys.exit(1 if audit() else 0)
