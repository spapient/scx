#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""多语言高级特性评测集生成器（60 题/语言，每题特性互不重复）。

gold 一律用本机真实编译器/解释器执行验证（不靠手算），保证答案正确性：
  python3 / rustc / clang++ / clang / javac

产物：bench_{py,rs,cpp,c,java}60.csv，列 question,answer，
格式与 bench_code60.csv 一致（few-shot demo + 围栏代码 + "答案是 X #### X"）。
"""
from __future__ import annotations

import csv
import os
import shutil
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def _exec(cmd: list[str], timeout: int = 30) -> str:
    out = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
    if out.returncode != 0:
        raise RuntimeError(f"{cmd[0]} failed: {out.stderr[-400:]}")
    return out.stdout.strip()


def py_gold(code: str) -> str:
    return _exec(["python3", "-c", code])


def rs_gold(code: str) -> str:
    with tempfile.TemporaryDirectory() as td:
        src = Path(td) / "m.rs"
        src.write_text(code, encoding="utf-8")
        _exec(["rustc", "--edition", "2021", "-O", str(src), "-o", f"{td}/m"])
        return _exec([f"{td}/m"])


def cpp_gold(code: str) -> str:
    with tempfile.TemporaryDirectory() as td:
        src = Path(td) / "m.cpp"
        src.write_text(code, encoding="utf-8")
        _exec(["clang++", "-O2", "-std=c++17", str(src), "-o", f"{td}/m"])
        return _exec([f"{td}/m"])


def c_gold(code: str) -> str:
    with tempfile.TemporaryDirectory() as td:
        src = Path(td) / "m.c"
        src.write_text(code, encoding="utf-8")
        _exec(["clang", "-O2", "-std=c11", str(src), "-o", f"{td}/m"])
        return _exec([f"{td}/m"])


def java_gold(code: str) -> str:
    if not shutil.which("javac") or not shutil.which("java"):
        raise RuntimeError("javac/java 不可用（JDK 未安装），跳过 Java 生成")
    with tempfile.TemporaryDirectory() as td:
        src = Path(td) / "M.java"
        src.write_text(code, encoding="utf-8")
        _exec(["javac", str(src)])
        return _exec(["java", "-cp", td, "M"])


LANGS = {
    "py":   {"hdr": "以下 Python 代码输出什么？", "fence": "python", "gold": py_gold},
    "rs":   {"hdr": "以下 Rust 代码输出什么？",   "fence": "rust",   "gold": rs_gold},
    "cpp":  {"hdr": "以下 C++ 代码输出什么？",    "fence": "cpp",    "gold": cpp_gold},
    "c":    {"hdr": "以下 C 代码输出什么？",      "fence": "c",      "gold": c_gold},
    "java": {"hdr": "以下 Java 代码输出什么？",   "fence": "java",   "gold": java_gold},
}

# 每题 (特性名, 完整可执行代码, 中文推理)。gold 由运行时代入，推理里的
# 数字/值必须与真实输出一致（生成时用 gold 插值，不手写具体数值）。
# 设计约束：单 print、标量输出（int/str/bool/float），3B 模型可推。

PY = [
("生成器 yield", "def g():\n    yield 1\n    yield 2\nit = g()\nprint(next(it) + next(it))", "生成器依次产出 1、2，next 两次相加得"),
("装饰器", "def deco(f):\n    return lambda: f() + 1\n@deco\ndef f():\n    return 1\nprint(f())", "deco 包装后返回值加 1，"),
("闭包 nonlocal", "def outer():\n    x = 0\n    def inner():\n        nonlocal x\n        x += 1\n        return x\n    return inner\nf = outer()\nf()\nprint(f())", "inner 闭包引用 x，调用两次后 x=2，"),
("MRO 方法解析", 'class A:\n    def m(self):\n        return "A"\nclass B(A):\n    pass\nclass C(A):\n    def m(self):\n        return "C"\nclass D(B, C):\n    pass\nprint(D().m())', "D(B,C) 的 MRO 中 C 先于 A，取 C 的实现，输出"),
("super 链", 'class A:\n    def m(self):\n        return "A"\nclass B(A):\n    def m(self):\n        return "B" + super().m()\nclass C(B):\n    pass\nprint(C().m())', "C 继承 B，B.m 拼接 B 与 A 的结果，输出"),
("异常 finally", "try:\n    x = 1 // 0\nexcept ZeroDivisionError:\n    x = 5\nfinally:\n    x += 1\nprint(x)", "除零触发异常 x=5，finally 加 1，"),
("自定义异常", 'class E(Exception):\n    pass\ntry:\n    raise E("x")\nexcept E as e:\n    print(len(str(e)))', "异常消息 x 长度 1，"),
("推导式过滤", "print(sum(x for x in range(10) if x % 3 == 0))", "0 到 9 中 3 的倍数 0+3+6+9=18，"),
("字典推导式", "d = {i: i * i for i in range(3)}\nprint(d[2])", "字典推导 {0:0,1:1,2:4}，d[2]=4，"),
("lambda 排序键", 'arr = ["bb", "a", "ccc"]\nprint(len(sorted(arr, key=len)[-1]))', "按长度排序最长 ccc 长度 3，"),
("可变默认参数", "def f(x, L=[]):\n    L.append(x)\n    return L\nprint(len(f(1)))", "默认列表 L 共享但只调一次，len=1，"),
("kwargs", "def f(**kw):\n    return len(kw)\nprint(f(a=1, b=2, c=3))", "3 个关键字参数，"),
("args 求和", "def f(*a):\n    return sum(a)\nprint(f(1, 2, 3))", "1+2+3=6，"),
("元组解包", "a, b = (1, 2)\nprint(a * 10 + b)", "a=1、b=2，1*10+2=12，"),
("切片步长", 's = "0123456789"\nprint(s[1:8:2])', "索引 1、3、5、7 取字符拼成，输出"),
("负索引", 's = "abcde"\nprint(s[-1] + s[-3])', "s[-1]=e、s[-3]=c 拼接，输出"),
("方法链", 'print("  abc  ".strip().upper())', "strip 去空白再 upper，输出"),
("bytes", 'b = b"ab"\nprint(len(b) + b[0])', "len=2，b[0]=97，2+97=99，"),
("集合交集", "a = {1, 2, 3}\nb = {2, 3, 4}\nprint(len(a & b))", "交集 {2,3} 长度 2，"),
("位运算", "print((6 & 3) | (8 >> 2))", "6&3=2，8>>2=2，2|2=2，"),
("逻辑短路", "def f():\n    return 0\ndef g():\n    return 1\nprint(f() and g() or 5)", "f()=0 为假 and 短路，or 取 5，"),
("链式比较", "x = 5\nprint(1 < x < 10 and x > 4)", "1<5<10 且 5>4 都成立，输出"),
("三目嵌套", 'x = 7\nprint("A" if x > 9 else "B" if x > 5 else "C")', "7>5 取中间分支，输出"),
("引用别名", "a = [1]\nb = a\nb.append(2)\nprint(len(a))", "b 与 a 指向同一列表，len=2，"),
("引用传参", "def f(x):\n    x.append(1)\na = []\nf(a)\nprint(len(a))", "函数内修改同一列表，len=1，"),
("元组内可变列表", "t = (1, [2])\nt[1].append(3)\nprint(len(t[1]))", "元组内列表可变，append 后 len=2，"),
("f-string", 'x = 4\nprint(f"{x * 3}")', "4*3=12，"),
("split", 's = "a,b,c"\nprint(len(s.split(",")))', "按逗号拆成 3 段，"),
("iter/next", "it = iter([1, 2, 3])\nnext(it)\nprint(next(it))", "next 跳过 1 后取 2，"),
("yield from", "def g():\n    yield from range(2)\n    yield 5\nprint(sum(g()))", "0+1+5=6，"),
("with 协议", "class C:\n    def __enter__(self):\n        return 3\n    def __exit__(self, *a):\n        pass\nwith C() as v:\n    print(v * 2)", "__enter__ 返回 3，3*2=6，"),
("类/实例属性", "class A:\n    x = 1\nobj = A()\nobj.x = 2\nprint(A.x + obj.x)", "类属性 1 + 实例属性 2 = 3，"),
("property", "class C:\n    @property\n    def v(self):\n        return 7\nprint(C().v)", "property 访问返回 7，"),
("staticmethod", "class C:\n    @staticmethod\n    def f(x):\n        return x * 2\nprint(C.f(4))", "静态方法 4*2=8，"),
("super 继承", "class A:\n    def m(self):\n        return 1\nclass B(A):\n    def m(self):\n        return super().m() + 1\nprint(B().m())", "B.m 调 A.m 加 1，1+1=2，"),
("str 魔法方法", 'class C:\n    def __str__(self):\n        return "hi"\nprint(len(str(C())))', "str 返回 hi 长度 2，"),
("多重赋值交换", "a, b = 1, 2\na, b = b, a\nprint(a - b)", "交换后 a=2、b=1，2-1=1，"),
("海象运算符", "n = 0\nwhile (m := n + 1) < 5:\n    n = m\nprint(n)", "m 递增到 4 后 5 不满足循环，n=4，"),
("enumerate", "s = 0\nfor i, v in enumerate([3, 4]):\n    s += i + v\nprint(s)", "(0+3)+(1+4)=8，"),
("zip 并行", "a = [1, 2]\nb = [3, 4]\nprint(sum(x + y for x, y in zip(a, b)))", "(1+3)+(2+4)=10，"),
("any/all", "print(any([False, True]) + all([1, 2]))", "any 为 True 记 1，all 为 True 记 1，共 2，"),
("reduce", "from functools import reduce\nprint(reduce(lambda a, b: a * b, [2, 3, 4]))", "2*3*4=24，"),
("Counter", 'from collections import Counter\nprint(Counter("aab").most_common(1)[0][1])', "a 出现次数最多为 2，"),
("defaultdict", "from collections import defaultdict\nd = defaultdict(int)\nd['k'] += 2\nprint(d['k'])", "默认 int 0 再加 2，"),
("deque", "from collections import deque\nq = deque([1])\nq.appendleft(2)\nprint(q[0])", "头部插入 2，q[0]=2，"),
("heapq", "import heapq\nh = [3, 1]\nheapq.heapify(h)\nprint(heapq.heappop(h))", "堆弹出最小元素 1，"),
("排序稳定性", 'arr = [("a", 2), ("b", 1)]\nprint(sorted(arr, key=lambda t: t[1])[0][0])', "按第二元素升序首个 b，输出"),
("递归记忆化", "def f(n):\n    return 1 if n < 2 else f(n - 1) + f(n - 2)\nprint(f(6))", "斐波那契 f(6)=13，"),
("生成器流水线", "def gen():\n    for i in range(4):\n        yield i * 2\nprint(sum(gen()))", "0+2+4+6=12，"),
("send 注入", "def g():\n    v = yield 1\n    yield v + 1\nx = g()\nnext(x)\nprint(x.send(5))", "send(5) 使 v=5，5+1=6，"),
("LEGB 作用域", "x = 1\ndef f():\n    x = 2\n    return x\nprint(f() + x)", "函数内 2 + 全局 1 = 3，"),
("nonlocal 修改", "def f():\n    x = 0\n    def g():\n        nonlocal x\n        x = 2\n    g()\n    return x\nprint(f())", "g 修改外层 x 为 2，"),
("global 修改", "x = 0\ndef f():\n    global x\n    x = 3\nf()\nprint(x)", "f 修改全局 x 为 3，"),
("utf-8 编码", 'print(len("中文".encode("utf-8")))', "中文 utf-8 每字 3 字节共 6，"),
("进制转换", 'print(int("ff", 16))', "十六进制 ff 等于 255，"),
("divmod", "q, r = divmod(17, 5)\nprint(q * 10 + r)", "17//5=3、17%5=2，3*10+2=32，"),
("pow 取模", "print(pow(3, 4, 5))", "3^4=81，81 mod 5 = 1，"),
("round 精度", "print(round(3.14159, 2))", "保留两位小数得 3.14，"),
("字符串重复", 's = "ab"\nprint(len(s * 2) * 2)', "ab 重复 2 次长 4，4*2=8，"),
("平方和推导", "print(sum(i * i for i in range(1, 4)))", "1+4+9=14，"),
]

RS = [
("所有权转移", 'fn main() {\n    let s = String::from("ab");\n    let t = s;\n    println!("{}", t.len());\n}', "所有权转移给 t，长度"),
("可变借用", 'fn main() {\n    let mut v = vec![1];\n    let r = &mut v;\n    r.push(2);\n    println!("{}", v.len());\n}', "可变借用 push 后长度"),
("clone", 'fn main() {\n    let a = vec![1];\n    let b = a.clone();\n    println!("{}", b.len());\n}', "克隆副本长度"),
("迭代器求和", 'fn main() {\n    let s: i32 = (1..=4).sum();\n    println!("{}", s);\n}', "1 到 4 求和"),
("闭包捕获", 'fn main() {\n    let x = 3;\n    let f = || x * 2;\n    println!("{}", f());\n}', "闭包捕获 x 翻倍"),
("map/filter 链", 'fn main() {\n    let s: i32 = (0..10).filter(|x| x % 2 == 0).map(|x| x * 2).sum();\n    println!("{}", s);\n}', "偶数翻倍后求和"),
("Option 解包", 'fn main() {\n    let x: Option<i32> = Some(5);\n    println!("{}", x.unwrap() + 1);\n}', "解包后加 1"),
("Result 解包", 'fn main() {\n    let r: Result<i32, String> = Ok(7);\n    println!("{}", r.unwrap());\n}', "Ok 直接解包"),
("match 分支", 'fn main() {\n    let x = 2;\n    let y = match x { 1 => 10, 2 => 20, _ => 0 };\n    println!("{}", y);\n}', "x=2 命中 20"),
("if let", 'fn main() {\n    let x = Some(4);\n    if let Some(v) = x {\n        println!("{}", v * 2);\n    }\n}', "解构 Some 翻倍"),
("枚举带数据", 'enum E { A(i32), B }\nfn main() {\n    let e = E::A(3);\n    let v = match e { E::A(n) => n, _ => 0 };\n    println!("{}", v);\n}', "枚举携带数据 3"),
("trait 实现", 'trait T { fn f(&self) -> i32; }\nstruct S;\nimpl T for S {\n    fn f(&self) -> i32 { 5 }\n}\nfn main() {\n    println!("{}", S.f());\n}', "trait 实现返回 5"),
("trait 默认方法", 'trait T { fn f(&self) -> i32 { 1 } }\nstruct S;\nimpl T for S {}\nfn main() {\n    println!("{}", S.f());\n}', "默认方法返回 1"),
("impl 方法", 'struct S { x: i32 }\nimpl S {\n    fn f(&self) -> i32 { self.x * 2 }\n}\nfn main() {\n    println!("{}", S { x: 3 }.f());\n}', "方法翻倍字段"),
("关联函数", 'struct S;\nimpl S {\n    fn val() -> i32 { 9 }\n}\nfn main() {\n    println!("{}", S::val());\n}', "关联函数返回 9"),
("元组结构体", 'struct P(i32, i32);\nfn main() {\n    let p = P(3, 4);\n    println!("{}", p.0 + p.1);\n}', "元组字段相加"),
("push_str", 'fn main() {\n    let mut s = String::from("ab");\n    s.push_str("cd");\n    println!("{}", s.len());\n}', "追加后长度"),
("format 宏", 'fn main() {\n    let s = format!("{}{}", 1, 2);\n    println!("{}", s.len());\n}', "拼接串长度"),
("数组切片", 'fn main() {\n    let v = [1, 2, 3, 4];\n    let s = &v[1..3];\n    println!("{}", s.len());\n}', "切片含 2 元素"),
("Vec 推入", 'fn main() {\n    let mut v = Vec::new();\n    v.push(1);\n    v.push(2);\n    println!("{}", v.len());\n}', "两次 push 长度"),
("Vec 索引", 'fn main() {\n    let v = vec![5, 6, 7];\n    println!("{}", v[1]);\n}', "索引 1 元素"),
("range 步长", 'fn main() {\n    let s: i32 = (0..10).step_by(3).sum();\n    println!("{}", s);\n}', "步长 3 求和"),
("while let", 'fn main() {\n    let mut v = vec![1, 2];\n    let mut s = 0;\n    while let Some(x) = v.pop() { s += x; }\n    println!("{}", s);\n}', "弹栈累加"),
("引用计数", 'use std::rc::Rc;\nfn main() {\n    let a = Rc::new(5);\n    let b = a.clone();\n    println!("{}", Rc::strong_count(&a));\n}', "克隆后强引用数 2"),
("泛型函数", 'fn f<T: std::ops::Add<Output = T>>(a: T, b: T) -> T { a + b }\nfn main() {\n    println!("{}", f(3, 4));\n}', "泛型加法"),
("泛型结构体", 'struct P<T> { x: T }\nfn main() {\n    let p = P { x: 3 };\n    println!("{}", p.x * 2);\n}', "泛型字段翻倍"),
("derive 派生", '#[derive(Clone)]\nstruct S { x: i32 }\nfn main() {\n    let a = S { x: 2 };\n    let b = a.clone();\n    println!("{}", b.x + 1);\n}', "派生 Clone 副本加 1"),
("match 范围", 'fn main() {\n    let x = 3;\n    let y = match x { 1 | 2 => 0, 3..=5 => 1, _ => 2 };\n    println!("{}", y);\n}', "3 落在范围命中 1"),
("数组长度", 'fn main() {\n    let a = [1, 2, 3];\n    println!("{}", a.len());\n}', "数组长度 3"),
("元组解构", 'fn main() {\n    let (a, b) = (3, 4);\n    println!("{}", a + b);\n}', "解构后相加"),
("函数指针", 'fn f(x: i32) -> i32 { x * 2 }\nfn main() {\n    let g: fn(i32) -> i32 = f;\n    println!("{}", g(4));\n}', "函数指针调用"),
("enumerate", 'fn main() {\n    let mut s = 0;\n    for (i, v) in vec![3, 4].iter().enumerate() { s += i + v; }\n    println!("{}", s);\n}', "索引与值累加"),
("collect", 'fn main() {\n    let v: Vec<i32> = (1..4).map(|x| x * x).collect();\n    println!("{}", v[1]);\n}', "平方收集后索引 1"),
("chars 计数", 'fn main() {\n    let s = "abc";\n    println!("{}", s.chars().count());\n}', "字符数 3"),
("字符转数字", 'fn main() {\n    let c = \'5\';\n    println!("{}", c.to_digit(10).unwrap());\n}', "字符 5 转数字"),
("布尔逻辑", 'fn main() {\n    println!("{}", (3 > 2) && (1 < 4));\n}', "两个条件都成立"),
("位运算", 'fn main() {\n    println!("{}", (6 & 3) | (8 >> 2));\n}', "6&3=2、8>>2=2 相或"),
("环绕加法", 'fn main() {\n    println!("{}", 255u8.wrapping_add(1));\n}', "u8 溢出环绕"),
("as 转换", 'fn main() {\n    println!("{}", 7u32 as i32 - 2);\n}', "转换后减 2"),
("取模整除", 'fn main() {\n    println!("{}", 17 / 5 + 17 % 5);\n}', "商 3 加余 2"),
("幂运算", 'fn main() {\n    println!("{}", 3i32.pow(2) + 1);\n}', "3 的平方加 1"),
("排序", 'fn main() {\n    let mut v = vec![3, 1, 2];\n    v.sort();\n    println!("{}", v[0]);\n}', "升序最小"),
("去重", 'fn main() {\n    let mut v = vec![1, 1, 2];\n    v.dedup();\n    println!("{}", v.len());\n}', "相邻去重后长度"),
("split 计数", 'fn main() {\n    let s = "a,b,c";\n    println!("{}", s.split(\',\').count());\n}', "逗号分 3 段"),
("trim", 'fn main() {\n    let s = "  ab  ";\n    println!("{}", s.trim().len());\n}', "去空白后长度 2"),
("HashMap", 'use std::collections::HashMap;\nfn main() {\n    let mut m = HashMap::new();\n    m.insert("k", 3);\n    println!("{}", m["k"]);\n}', "键 k 的值"),
("entry 或默认", 'use std::collections::HashMap;\nfn main() {\n    let mut m = HashMap::new();\n    *m.entry("k").or_insert(0) += 2;\n    println!("{}", m["k"]);\n}', "不存在则插入加 2"),
("reverse", 'fn main() {\n    let mut v = vec![1, 2];\n    v.reverse();\n    println!("{}", v[0]);\n}', "反转后首元素"),
("chain 连接", 'fn main() {\n    let s: i32 = vec![1, 2].into_iter().chain(vec![3]).sum();\n    println!("{}", s);\n}', "串联迭代求和"),
("shadowing", 'fn main() {\n    let x = 1;\n    let x = x + 2;\n    println!("{}", x);\n}', "遮蔽后新值"),
("作用域", 'fn main() {\n    let x = 5;\n    { let x = 10; }\n    println!("{}", x);\n}', "内层遮蔽不影响外层"),
("常量", 'fn main() {\n    const N: i32 = 6;\n    println!("{}", N * N);\n}', "常量平方"),
("元组比较", 'fn main() {\n    println!("{}", (1, 2) < (1, 3));\n}', "元组字典序"),
("Option map", 'fn main() {\n    let x = Some(3);\n    println!("{}", x.map(|v| v * 2).unwrap());\n}', "map 翻倍解包"),
("filter 计数", 'fn main() {\n    let c = (0..10).filter(|x| x % 2 == 0).count();\n    println!("{}", c);\n}', "偶数个数 5"),
("any 判断", 'fn main() {\n    println!("{}", (1..4).any(|x| x > 2));\n}', "存在大于 2"),
("平方和", 'fn main() {\n    let s: i32 = (1..4).map(|x| x * x).sum();\n    println!("{}", s);\n}', "1+4+9 平方和"),
("break 标签", 'fn main() {\n    let mut s = 0;\n    \'outer: for i in 0..3 {\n        for j in 0..3 {\n            if j == 2 { break \'outer; }\n            s += 1;\n        }\n    }\n    println!("{}", s);\n}', "j=2 时跳出外层"),
("min/max", 'fn main() {\n    println!("{}", 3.min(5) + 2.max(1));\n}', "取小加取大"),
("repeat 重复", 'fn main() {\n    let s = "ab".repeat(3);\n    println!("{}", s.len());\n}', "重复 3 次长度"),
]

CPP = [
("引用别名", '#include <iostream>\nint main() {\n    int x = 3;\n    int &r = x;\n    r = 5;\n    std::cout << x << "\\n";\n}', "引用修改原变量"),
("指针解引用", '#include <iostream>\nint main() {\n    int x = 7;\n    int *p = &x;\n    std::cout << *p + 1 << "\\n";\n}', "解引用加 1"),
("vector 长度", '#include <iostream>\n#include <vector>\nint main() {\n    std::vector<int> v;\n    v.push_back(1);\n    v.push_back(2);\n    std::cout << v.size() << "\\n";\n}', "两次 push 后大小"),
("string 拼接", '#include <iostream>\n#include <string>\nint main() {\n    std::string s = "ab";\n    s += "cd";\n    std::cout << s.size() << "\\n";\n}', "拼接后长度"),
("函数重载", '#include <iostream>\nint f(int x) { return x * 2; }\ndouble f(double x) { return x + 0.5; }\nint main() {\n    std::cout << f(3) << "\\n";\n}', "int 重载翻倍"),
("默认参数", '#include <iostream>\nint f(int x, int y = 3) { return x + y; }\nint main() {\n    std::cout << f(2) << "\\n";\n}', "缺省参数 3"),
("模板函数", '#include <iostream>\ntemplate <typename T> T f(T a, T b) { return a + b; }\nint main() {\n    std::cout << f(3, 4) << "\\n";\n}', "模板实例化求和"),
("构造函数", '#include <iostream>\nstruct S {\n    int x;\n    S(int v) : x(v) {}\n};\nint main() {\n    S s(4);\n    std::cout << s.x * 2 << "\\n";\n}', "构造初始化翻倍"),
("静态成员", '#include <iostream>\nstruct S {\n    static int cnt;\n};\nint S::cnt = 5;\nint main() {\n    std::cout << S::cnt + 1 << "\\n";\n}', "静态成员加 1"),
("继承调用", '#include <iostream>\nstruct A { int v = 3; };\nstruct B : A {};\nint main() {\n    B b;\n    std::cout << b.v * 2 << "\\n";\n}', "继承字段翻倍"),
("虚函数多态", '#include <iostream>\nstruct A { virtual int f() { return 1; } };\nstruct B : A { int f() override { return 2; } };\nint main() {\n    A *p = new B();\n    std::cout << p->f() << "\\n";\n    delete p;\n}', "虚分派取 B 实现"),
("运算符重载", '#include <iostream>\nstruct P { int x; };\nP operator+(P a, P b) { return {a.x + b.x}; }\nint main() {\n    P a{2}, b{3};\n    std::cout << (a + b).x << "\\n";\n}', "重载加法"),
("lambda 捕获", '#include <iostream>\nint main() {\n    int x = 3;\n    auto f = [x] { return x * 2; };\n    std::cout << f() << "\\n";\n}', "按值捕获翻倍"),
("sort 自定义", '#include <iostream>\n#include <vector>\n#include <algorithm>\nint main() {\n    std::vector<int> v{3, 1, 2};\n    std::sort(v.begin(), v.end());\n    std::cout << v[0] << "\\n";\n}', "升序最小"),
("map 访问", '#include <iostream>\n#include <map>\nint main() {\n    std::map<std::string, int> m;\n    m["k"] = 4;\n    std::cout << m["k"] << "\\n";\n}', "键 k 的值"),
("set 计数", '#include <iostream>\n#include <set>\nint main() {\n    std::set<int> s{1, 1, 2};\n    std::cout << s.size() << "\\n";\n}', "去重后大小"),
("auto 推断", '#include <iostream>\nint main() {\n    auto x = 4;\n    auto y = x * 2.0;\n    std::cout << (int)y << "\\n";\n}', "auto 推断 double 转 int"),
("范围 for", '#include <iostream>\nint main() {\n    int s = 0;\n    for (int v : {1, 2, 3}) s += v;\n    std::cout << s << "\\n";\n}', "范围 for 累加"),
("move 语义", '#include <iostream>\n#include <string>\nint main() {\n    std::string s = "abc";\n    std::string t = std::move(s);\n    std::cout << t.size() << "\\n";\n}', "移动后目标长度"),
("shared_ptr", '#include <iostream>\n#include <memory>\nint main() {\n    std::shared_ptr<int> a = std::make_shared<int>(3);\n    auto b = a;\n    std::cout << a.use_count() << "\\n";\n}', "两个共享引用"),
("pair", '#include <iostream>\n#include <utility>\nint main() {\n    std::pair<int, int> p{3, 4};\n    std::cout << p.first + p.second << "\\n";\n}', "pair 字段相加"),
("tuple get", '#include <iostream>\n#include <tuple>\nint main() {\n    auto t = std::make_tuple(2, 3);\n    std::cout << std::get<0>(t) * std::get<1>(t) << "\\n";\n}', "元组字段相乘"),
("find 定位", '#include <iostream>\n#include <string>\nint main() {\n    std::string s = "hello";\n    std::cout << s.find("l") << "\\n";\n}', "首个 l 索引"),
("substr", '#include <iostream>\n#include <string>\nint main() {\n    std::string s = "abcde";\n    std::cout << s.substr(1, 2).size() << "\\n";\n}', "子串长度"),
("stoi 转换", '#include <iostream>\n#include <string>\nint main() {\n    std::string s = "42";\n    std::cout << std::stoi(s) + 1 << "\\n";\n}', "字符串转 int 加 1"),
("sizeof", '#include <iostream>\nint main() {\n    int a[4];\n    std::cout << sizeof(a) / sizeof(a[0]) << "\\n";\n}', "数组元素个数"),
("引用传参", '#include <iostream>\nvoid f(int &x) { x += 3; }\nint main() {\n    int v = 2;\n    f(v);\n    std::cout << v << "\\n";\n}', "引用修改原值"),
("const 引用", '#include <iostream>\nint main() {\n    int x = 5;\n    const int &r = x;\n    std::cout << r * 2 << "\\n";\n}', "常量引用翻倍"),
("enum", '#include <iostream>\nenum Color { RED = 2, GREEN = 5 };\nint main() {\n    std::cout << GREEN + RED << "\\n";\n}', "枚举值相加"),
("switch", '#include <iostream>\nint main() {\n    int x = 2;\n    int y = 0;\n    switch (x) {\n        case 1: y = 10; break;\n        case 2: y = 20; break;\n        default: y = 0;\n    }\n    std::cout << y << "\\n";\n}', "命中 case 2"),
("位运算", '#include <iostream>\nint main() {\n    std::cout << ((6 & 3) | (8 >> 2)) << "\\n";\n}', "与或移位"),
("三目", '#include <iostream>\nint main() {\n    int x = 7;\n    std::cout << (x > 5 ? 1 : 0) << "\\n";\n}', "条件为真取 1"),
("短路", '#include <iostream>\nint main() {\n    int x = 0;\n    bool r = (x != 0) && (10 / x > 0);\n    std::cout << r << "\\n";\n}', "短路不触发除零"),
("循环累加", '#include <iostream>\nint main() {\n    int s = 0;\n    for (int i = 1; i <= 5; i++) s += i;\n    std::cout << s << "\\n";\n}', "1 到 5 累加"),
("嵌套循环", '#include <iostream>\nint main() {\n    int s = 0;\n    for (int i = 0; i < 3; i++)\n        for (int j = 0; j < 4; j++) s++;\n    std::cout << s << "\\n";\n}', "3 乘 4 次"),
("continue", '#include <iostream>\nint main() {\n    int s = 0;\n    for (int i = 1; i <= 5; i++) {\n        if (i % 2 == 0) continue;\n        s += i;\n    }\n    std::cout << s << "\\n";\n}', "跳过偶数累加"),
("do-while", '#include <iostream>\nint main() {\n    int x = 0, s = 0;\n    do { s += x; x++; } while (x < 3);\n    std::cout << s << "\\n";\n}', "先执行再判断"),
("指针算术", '#include <iostream>\nint main() {\n    int a[3] = {10, 20, 30};\n    int *p = a + 1;\n    std::cout << *p << "\\n";\n}', "指针偏移取元素"),
("结构体成员", '#include <iostream>\nstruct P { int x; int y; };\nint main() {\n    P p{3, 4};\n    std::cout << p.x * p.y << "\\n";\n}', "成员相乘"),
("函数指针", '#include <iostream>\nint f(int x) { return x * 2; }\nint main() {\n    int (*g)(int) = f;\n    std::cout << g(4) << "\\n";\n}', "函数指针调用"),
("递归阶乘", '#include <iostream>\nint f(int n) { return n <= 1 ? 1 : n * f(n - 1); }\nint main() {\n    std::cout << f(5) << "\\n";\n}', "5 的阶乘"),
("斐波那契", '#include <iostream>\nint f(int n) { return n < 2 ? n : f(n - 1) + f(n - 2); }\nint main() {\n    std::cout << f(7) << "\\n";\n}', "第 7 项斐波那契"),
("reverse", '#include <iostream>\n#include <string>\n#include <algorithm>\nint main() {\n    std::string s = "abc";\n    std::reverse(s.begin(), s.end());\n    std::cout << s.size() << "\\n";\n}', "反转后长度不变"),
("count_if", '#include <iostream>\n#include <vector>\n#include <algorithm>\nint main() {\n    std::vector<int> v{1, 2, 3, 4};\n    int c = std::count_if(v.begin(), v.end(), [](int x) { return x % 2 == 0; });\n    std::cout << c << "\\n";\n}', "偶数个数"),
("accumulate", '#include <iostream>\n#include <vector>\n#include <numeric>\nint main() {\n    std::vector<int> v{1, 2, 3, 4};\n    std::cout << std::accumulate(v.begin(), v.end(), 0) << "\\n";\n}', "全量累加"),
("排序取中", '#include <iostream>\n#include <vector>\n#include <algorithm>\nint main() {\n    std::vector<int> v{5, 1, 3};\n    std::sort(v.begin(), v.end());\n    std::cout << v[v.size() / 2] << "\\n";\n}', "排序后中位"),
("迭代器遍历", '#include <iostream>\n#include <vector>\nint main() {\n    std::vector<int> v{2, 4};\n    int s = 0;\n    for (auto it = v.begin(); it != v.end(); ++it) s += *it;\n    std::cout << s << "\\n";\n}', "迭代器累加"),
("erase 删除", '#include <iostream>\n#include <vector>\nint main() {\n    std::vector<int> v{1, 2, 3};\n    v.erase(v.begin() + 1);\n    std::cout << v.size() << "\\n";\n}', "删一个元素后大小"),
("insert 插入", '#include <iostream>\n#include <vector>\nint main() {\n    std::vector<int> v{1, 3};\n    v.insert(v.begin() + 1, 2);\n    std::cout << v[1] << "\\n";\n}', "中间插入元素"),
("operator[]", '#include <iostream>\n#include <map>\nint main() {\n    std::map<int, int> m;\n    m[1] = 10;\n    m[1] += 5;\n    std::cout << m[1] << "\\n";\n}', "下标更新值"),
("multiset", '#include <iostream>\n#include <set>\nint main() {\n    std::multiset<int> s{1, 1, 2};\n    std::cout << s.count(1) << "\\n";\n}', "1 出现次数"),
("priority_queue", '#include <iostream>\n#include <queue>\nint main() {\n    std::priority_queue<int> q;\n    q.push(3);\n    q.push(1);\n    std::cout << q.top() << "\\n";\n}', "最大堆顶"),
("deque 头插", '#include <iostream>\n#include <deque>\nint main() {\n    std::deque<int> q;\n    q.push_front(2);\n    q.push_back(3);\n    std::cout << q.front() << "\\n";\n}', "头插后首元素"),
("list 大小", '#include <iostream>\n#include <list>\nint main() {\n    std::list<int> l{1, 2, 3};\n    std::cout << l.size() << "\\n";\n}', "链表大小"),
("static_cast", '#include <iostream>\nint main() {\n    double d = 7.9;\n    int i = static_cast<int>(d);\n    std::cout << i << "\\n";\n}', "强转截断"),
("stringstream", '#include <iostream>\n#include <sstream>\n#include <string>\nint main() {\n    std::stringstream ss("42");\n    int v;\n    ss >> v;\n    std::cout << v / 2 << "\\n";\n}', "流解析整数"),
("rfind 反向查找", '#include <iostream>\n#include <string>\nint main() {\n    std::string s = "ababa";\n    std::cout << s.rfind("ba") << "\\n";\n}', "反向查找首个 ba"),
("reserve 容量", '#include <iostream>\n#include <vector>\nint main() {\n    std::vector<int> v;\n    v.reserve(8);\n    std::cout << v.capacity() << "\\n";\n}', "预留容量"),
("字符串比较", '#include <iostream>\n#include <string>\nint main() {\n    std::string s = "ab";\n    std::cout << (s == "ab" ? 1 : 0) << "\\n";\n}', "内容相等记 1"),
("back 尾字符", '#include <iostream>\n#include <string>\nint main() {\n    std::string s = "abc";\n    std::cout << s.back() - \'a\' + 1 << "\\n";\n}', "尾字符序号"),
]

C = [
("指针解引用", '#include <stdio.h>\nint main() {\n    int x = 7;\n    int *p = &x;\n    printf("%d\\n", *p + 1);\n}', "解引用加 1"),
("指针算术", '#include <stdio.h>\nint main() {\n    int a[3] = {10, 20, 30};\n    int *p = a + 1;\n    printf("%d\\n", *p);\n}', "指针偏移取元素"),
("数组求和", '#include <stdio.h>\nint main() {\n    int a[] = {1, 2, 3};\n    int s = 0;\n    for (int i = 0; i < 3; i++) s += a[i];\n    printf("%d\\n", s);\n}', "遍历累加"),
("sizeof 数组", '#include <stdio.h>\nint main() {\n    int a[5];\n    printf("%zu\\n", sizeof(a) / sizeof(a[0]));\n}', "元素个数"),
("结构体", '#include <stdio.h>\nstruct P { int x; int y; };\nint main() {\n    struct P p = {3, 4};\n    printf("%d\\n", p.x * p.y);\n}', "成员相乘"),
("typedef", '#include <stdio.h>\ntypedef struct { int v; } S;\nint main() {\n    S s = {5};\n    printf("%d\\n", s.v + 1);\n}', "typedef 结构体"),
("函数调用", '#include <stdio.h>\nint f(int x) { return x * 2; }\nint main() {\n    printf("%d\\n", f(4));\n}', "函数翻倍"),
("递归", '#include <stdio.h>\nint f(int n) { return n <= 1 ? 1 : n * f(n - 1); }\nint main() {\n    printf("%d\\n", f(5));\n}', "5 的阶乘"),
("位运算", '#include <stdio.h>\nint main() {\n    printf("%d\\n", (6 & 3) | (8 >> 2));\n}', "与或移位"),
("逻辑", '#include <stdio.h>\nint main() {\n    printf("%d\\n", (3 > 2) && (1 < 4));\n}', "两条件成立记 1"),
("三目", '#include <stdio.h>\nint main() {\n    int x = 7;\n    printf("%d\\n", x > 5 ? 1 : 0);\n}', "条件为真取 1"),
("for 循环", '#include <stdio.h>\nint main() {\n    int s = 0;\n    for (int i = 1; i <= 5; i++) s += i;\n    printf("%d\\n", s);\n}', "1 到 5 累加"),
("嵌套循环", '#include <stdio.h>\nint main() {\n    int s = 0;\n    for (int i = 0; i < 3; i++)\n        for (int j = 0; j < 4; j++) s++;\n    printf("%d\\n", s);\n}', "3 乘 4 次"),
("switch", '#include <stdio.h>\nint main() {\n    int x = 2, y = 0;\n    switch (x) {\n        case 1: y = 10; break;\n        case 2: y = 20; break;\n        default: y = 0;\n    }\n    printf("%d\\n", y);\n}', "命中 case 2"),
("enum", '#include <stdio.h>\nenum Color { RED = 2, GREEN = 5 };\nint main() {\n    printf("%d\\n", GREEN + RED);\n}', "枚举值相加"),
("宏", '#include <stdio.h>\n#define DOUBLE(x) ((x) * 2)\nint main() {\n    printf("%d\\n", DOUBLE(4));\n}', "宏展开翻倍"),
("strlen", '#include <stdio.h>\n#include <string.h>\nint main() {\n    char s[] = "hello";\n    printf("%zu\\n", strlen(s));\n}', "字符串长度"),
("strcmp", '#include <stdio.h>\n#include <string.h>\nint main() {\n    printf("%d\\n", strcmp("abc", "abd") < 0);\n}', "字典序比较"),
("字符数组", '#include <stdio.h>\nint main() {\n    char s[] = "ab";\n    printf("%d\\n", s[1] - \'a\' + 1);\n}', "字符差"),
("指针比较", '#include <stdio.h>\nint main() {\n    int a[2] = {1, 2};\n    int *p = a;\n    int *q = a + 1;\n    printf("%ld\\n", q - p);\n}', "指针距离"),
("函数指针", '#include <stdio.h>\nint f(int x) { return x * 2; }\nint main() {\n    int (*g)(int) = f;\n    printf("%d\\n", g(4));\n}', "函数指针调用"),
("数组传参", '#include <stdio.h>\nint sum(int a[], int n) {\n    int s = 0;\n    for (int i = 0; i < n; i++) s += a[i];\n    return s;\n}\nint main() {\n    int a[] = {1, 2, 3};\n    printf("%d\\n", sum(a, 3));\n}', "传数组求和"),
("多维数组", '#include <stdio.h>\nint main() {\n    int a[2][3] = {{1, 2, 3}, {4, 5, 6}};\n    int s = 0;\n    for (int i = 0; i < 2; i++)\n        for (int j = 0; j < 3; j++) s += a[i][j];\n    printf("%d\\n", s);\n}', "二维累加"),
("自增前后缀", '#include <stdio.h>\nint main() {\n    int x = 5;\n    int a = x++;\n    int b = ++x;\n    printf("%d\\n", a + b);\n}', "后置取 5，前置后 x=7 取 7"),
("取地址", '#include <stdio.h>\nint main() {\n    int x = 8;\n    int *p = &x;\n    *p += 2;\n    printf("%d\\n", x);\n}', "指针修改原值"),
("局部静态", '#include <stdio.h>\nint f() {\n    static int c = 0;\n    return ++c;\n}\nint main() {\n    f();\n    printf("%d\\n", f());\n}', "静态变量第二次为 2"),
("全局变量", '#include <stdio.h>\nint g = 3;\nint main() {\n    g += 4;\n    printf("%d\\n", g);\n}', "全局加 4"),
("类型转换", '#include <stdio.h>\nint main() {\n    double d = 7.9;\n    printf("%d\\n", (int)d);\n}', "强转截断"),
("取余", '#include <stdio.h>\nint main() {\n    printf("%d\\n", 17 % 5);\n}', "余数"),
("整数除法", '#include <stdio.h>\nint main() {\n    printf("%d\\n", 17 / 5);\n}', "整数商"),
("逗号表达式", '#include <stdio.h>\nint main() {\n    int x = (1, 2, 3);\n    printf("%d\\n", x);\n}', "逗号取末值"),
("union", '#include <stdio.h>\nunion U { int i; char c; };\nint main() {\n    union U u;\n    u.i = 65;\n    printf("%d\\n", (int)u.c);\n}', "union 共享内存"),
("指针数组", '#include <stdio.h>\nint main() {\n    int a[] = {1, 2, 3};\n    int *p[3];\n    for (int i = 0; i < 3; i++) p[i] = &a[i];\n    printf("%d\\n", *p[1]);\n}', "指针数组取元素"),
("strcat", '#include <stdio.h>\n#include <string.h>\nint main() {\n    char s[16] = "ab";\n    strcat(s, "cd");\n    printf("%zu\\n", strlen(s));\n}', "拼接后长度"),
("strcpy", '#include <stdio.h>\n#include <string.h>\nint main() {\n    char a[] = "xyz";\n    char b[16];\n    strcpy(b, a);\n    printf("%zu\\n", strlen(b));\n}', "复制后长度"),
("条件累加", '#include <stdio.h>\nint main() {\n    int s = 0;\n    for (int i = 1; i <= 10; i++)\n        if (i % 2 == 0) s += i;\n    printf("%d\\n", s);\n}', "偶数累加"),
("手写幂", '#include <stdio.h>\nint main() {\n    int r = 1;\n    for (int i = 0; i < 3; i++) r *= 2;\n    printf("%d\\n", r);\n}', "2 的 3 次方"),
("gcd", '#include <stdio.h>\nint gcd(int a, int b) { return b ? gcd(b, a % b) : a; }\nint main() {\n    printf("%d\\n", gcd(12, 8));\n}', "最大公约数"),
("斐波那契", '#include <stdio.h>\nint f(int n) { return n < 2 ? n : f(n - 1) + f(n - 2); }\nint main() {\n    printf("%d\\n", f(7));\n}', "第 7 项"),
("素数判断", '#include <stdio.h>\nint main() {\n    int n = 17, c = 0;\n    for (int i = 2; i * i <= n; i++)\n        if (n % i == 0) c++;\n    printf("%d\\n", c == 0);\n}', "17 是素数"),
("数字反转", '#include <stdio.h>\nint main() {\n    int n = 123, r = 0;\n    while (n) { r = r * 10 + n % 10; n /= 10; }\n    printf("%d\\n", r);\n}', "123 反转为 321"),
("位计数", '#include <stdio.h>\nint main() {\n    int x = 7, c = 0;\n    while (x) { c += x & 1; x >>= 1; }\n    printf("%d\\n", c);\n}', "7 二进制有 3 个 1"),
("指针交换", '#include <stdio.h>\nvoid swap(int *a, int *b) {\n    int t = *a; *a = *b; *b = t;\n}\nint main() {\n    int x = 2, y = 5;\n    swap(&x, &y);\n    printf("%d\\n", x - y);\n}', "交换后 5-2"),
("数组最大值", '#include <stdio.h>\nint main() {\n    int a[] = {3, 7, 2, 9};\n    int m = a[0];\n    for (int i = 1; i < 4; i++)\n        if (a[i] > m) m = a[i];\n    printf("%d\\n", m);\n}', "最大值 9"),
("二维求和", '#include <stdio.h>\nint main() {\n    int a[2][2] = {{1, 2}, {3, 4}};\n    int s = 0;\n    for (int i = 0; i < 2; i++)\n        for (int j = 0; j < 2; j++) s += a[i][j];\n    printf("%d\\n", s);\n}', "全部元素和"),
("字符计数", '#include <stdio.h>\nint main() {\n    char s[] = "hello";\n    int c = 0;\n    for (int i = 0; s[i]; i++) c++;\n    printf("%d\\n", c);\n}', "遍历计数"),
("乘法表行和", '#include <stdio.h>\nint main() {\n    int s = 0;\n    for (int j = 1; j <= 9; j++) s += 3 * j;\n    printf("%d\\n", s);\n}', "3 的乘法表求和"),
("do-while", '#include <stdio.h>\nint main() {\n    int x = 0, s = 0;\n    do { s += x; x++; } while (x < 3);\n    printf("%d\\n", s);\n}', "先执行再判断"),
("位移翻倍", '#include <stdio.h>\nint main() {\n    printf("%d\\n", 5 << 1);\n}', "左移一位翻倍"),
("掩码", '#include <stdio.h>\nint main() {\n    int x = 0x0F;\n    printf("%d\\n", (x & 0x0C) >> 2);\n}', "掩码取位"),
("结构体指针", '#include <stdio.h>\nstruct P { int x; };\nint main() {\n    struct P p = {6};\n    struct P *q = &p;\n    q->x += 2;\n    printf("%d\\n", p.x);\n}', "指针改成员"),
("指针数组遍历", '#include <stdio.h>\n#include <string.h>\nint main() {\n    char *a[] = {"a", "b", "c"};\n    int s = 0;\n    for (int i = 0; i < 3; i++) s += (int)strlen(a[i]);\n    printf("%d\\n", s);\n}', "各字符串长度和"),
("三元指针取元素", '#include <stdio.h>\nint main() {\n    int a[3] = {5, 6, 7};\n    int *p = a + 1;\n    printf("%d\\n", *p);\n}', "指针偏移取 6"),
("数组修改", '#include <stdio.h>\nint main() {\n    int a[] = {1, 2, 3};\n    a[0] += 3;\n    printf("%d\\n", a[0]);\n}', "下标修改"),
("break 跳出", '#include <stdio.h>\nint main() {\n    int s = 0;\n    for (int i = 0; i < 5; i++) {\n        if (i == 3) break;\n        s += i;\n    }\n    printf("%d\\n", s);\n}', "i=3 中断"),
("长度差", '#include <stdio.h>\n#include <string.h>\nint main() {\n    char a[] = "hello";\n    char b[] = "hi";\n    printf("%d\\n", (int)(strlen(a) - strlen(b)));\n}', "5 减 2"),
("位清零", '#include <stdio.h>\nint main() {\n    int x = 0x0F;\n    x &= ~0x0C;\n    printf("%d\\n", x);\n}', "清除两位"),
("宏平方", '#include <stdio.h>\n#define SQUARE(x) ((x) * (x))\nint main() {\n    printf("%d\\n", SQUARE(3) + 1);\n}', "宏平方加 1"),
("结构体数组", '#include <stdio.h>\nstruct P { int v; };\nint main() {\n    struct P arr[2] = {{3}, {5}};\n    printf("%d\\n", arr[1].v);\n}', "第二元素"),
("指针减法", '#include <stdio.h>\nint main() {\n    int a[4] = {0, 0, 0, 0};\n    int *p = a;\n    p += 3;\n    printf("%ld\\n", p - a);\n}', "指针距离 3"),
]

JAVA = [
("String 不可变", 'public class M {\n    public static void main(String[] args) {\n        String s = "abc";\n        s.toUpperCase();\n        System.out.println(s.length());\n    }\n}', "toUpperCase 不改原串"),
("StringBuilder", 'public class M {\n    public static void main(String[] args) {\n        StringBuilder b = new StringBuilder("ab");\n        b.append("cd");\n        System.out.println(b.length());\n    }\n}', "追加后长度"),
("substring", 'public class M {\n    public static void main(String[] args) {\n        String s = "abcde";\n        System.out.println(s.substring(1, 3).length());\n    }\n}', "子串长度"),
("indexOf", 'public class M {\n    public static void main(String[] args) {\n        String s = "hello";\n        System.out.println(s.indexOf("l"));\n    }\n}', "首个 l 索引"),
("数组", 'public class M {\n    public static void main(String[] args) {\n        int[] x = new int[3];\n        System.out.println(x.length);\n    }\n}', "数组长度"),
("ArrayList", 'import java.util.*;\npublic class M {\n    public static void main(String[] args) {\n        List<Integer> l = new ArrayList<>();\n        l.add(1);\n        l.add(2);\n        System.out.println(l.size());\n    }\n}', "两次添加后大小"),
("ArrayList 取值", 'import java.util.*;\npublic class M {\n    public static void main(String[] args) {\n        List<Integer> l = new ArrayList<>(Arrays.asList(5, 6, 7));\n        System.out.println(l.get(1));\n    }\n}', "索引 1 元素"),
("HashMap", 'import java.util.*;\npublic class M {\n    public static void main(String[] args) {\n        Map<String, Integer> m = new HashMap<>();\n        m.put("k", 3);\n        System.out.println(m.get("k"));\n    }\n}', "键 k 的值"),
("HashSet 大小", 'import java.util.*;\npublic class M {\n    public static void main(String[] args) {\n        Set<Integer> s = new HashSet<>();\n        s.add(1);\n        s.add(1);\n        s.add(2);\n        System.out.println(s.size());\n    }\n}', "去重后大小"),
("自动装箱", 'public class M {\n    public static void main(String[] args) {\n        Integer x = 5;\n        int y = x + 2;\n        System.out.println(y);\n    }\n}', "拆箱相加"),
("Integer 缓存", 'public class M {\n    public static void main(String[] args) {\n        Integer x = 100;\n        Integer y = 100;\n        System.out.println(x == y ? 1 : 0);\n    }\n}', "缓存范围内同一对象"),
("泛型 List", 'import java.util.*;\npublic class M {\n    public static void main(String[] args) {\n        List<String> l = new ArrayList<>();\n        l.add("ab");\n        System.out.println(l.get(0).length());\n    }\n}', "泛型元素长度"),
("类继承", 'class A { int v = 3; }\nclass B extends A {}\npublic class M {\n    public static void main(String[] args) {\n        B b = new B();\n        System.out.println(b.v * 2);\n    }\n}', "继承字段翻倍"),
("方法覆盖", 'class A { int f() { return 1; } }\nclass B extends A { int f() { return 2; } }\npublic class M {\n    public static void main(String[] args) {\n        A x = new B();\n        System.out.println(x.f());\n    }\n}', "多态取 B 实现"),
("super 调用", 'class A { int f() { return 3; } }\nclass B extends A { int f() { return super.f() + 1; } }\npublic class M {\n    public static void main(String[] args) {\n        System.out.println(new B().f());\n    }\n}', "父类加 1"),
("构造重载", 'class S {\n    int x;\n    S() { x = 1; }\n    S(int v) { x = v; }\n}\npublic class M {\n    public static void main(String[] args) {\n        System.out.println(new S(5).x);\n    }\n}', "参数构造 5"),
("static 方法", 'class S {\n    static int f(int x) { return x * 2; }\n}\npublic class M {\n    public static void main(String[] args) {\n        System.out.println(S.f(4));\n    }\n}', "静态方法调用"),
("static 变量", 'class S {\n    static int c = 0;\n    static int f() { return ++c; }\n}\npublic class M {\n    public static void main(String[] args) {\n        S.f();\n        System.out.println(S.f());\n    }\n}', "静态计数第二次 2"),
("final 变量", 'public class M {\n    public static void main(String[] args) {\n        final int x = 4;\n        System.out.println(x * 2);\n    }\n}', "常量翻倍"),
("接口默认方法", 'interface I {\n    default int f() { return 6; }\n}\nclass S implements I {}\npublic class M {\n    public static void main(String[] args) {\n        System.out.println(new S().f());\n    }\n}', "默认实现 6"),
("lambda", 'public class M {\n    interface F { int run(int x); }\n    public static void main(String[] args) {\n        F f = x -> x * 2;\n        System.out.println(f.run(4));\n    }\n}', "lambda 翻倍"),
("stream map", 'import java.util.*;\nimport java.util.stream.*;\npublic class M {\n    public static void main(String[] args) {\n        int s = Stream.of(1, 2, 3).mapToInt(x -> x * 2).sum();\n        System.out.println(s);\n    }\n}', "翻倍求和"),
("filter count", 'import java.util.*;\nimport java.util.stream.*;\npublic class M {\n    public static void main(String[] args) {\n        long c = Stream.of(1, 2, 3, 4).filter(x -> x % 2 == 0).count();\n        System.out.println(c);\n    }\n}', "偶数个数"),
("reduce 求和", 'import java.util.*;\nimport java.util.stream.*;\npublic class M {\n    public static void main(String[] args) {\n        int s = Stream.of(1, 2, 3, 4).reduce(0, (x, y) -> x + y);\n        System.out.println(s);\n    }\n}', "归约求和"),
("字符串拼接", 'public class M {\n    public static void main(String[] args) {\n        String s = "ab" + "cd";\n        System.out.println(s.length());\n    }\n}', "拼接长度"),
("equals", 'public class M {\n    public static void main(String[] args) {\n        String a = new String("x");\n        String b = new String("x");\n        System.out.println(a.equals(b) ? 1 : 0);\n    }\n}', "值相等"),
("三目", 'public class M {\n    public static void main(String[] args) {\n        int x = 7;\n        System.out.println(x > 5 ? 1 : 0);\n    }\n}', "条件为真取 1"),
("switch", 'public class M {\n    public static void main(String[] args) {\n        int x = 2, y = 0;\n        switch (x) {\n            case 1: y = 10; break;\n            case 2: y = 20; break;\n            default: y = 0;\n        }\n        System.out.println(y);\n    }\n}', "命中 case 2"),
("循环累加", 'public class M {\n    public static void main(String[] args) {\n        int s = 0;\n        for (int i = 1; i <= 5; i++) s += i;\n        System.out.println(s);\n    }\n}', "1 到 5 累加"),
("嵌套循环", 'public class M {\n    public static void main(String[] args) {\n        int s = 0;\n        for (int i = 0; i < 3; i++)\n            for (int j = 0; j < 4; j++) s++;\n        System.out.println(s);\n    }\n}', "3 乘 4 次"),
("continue", 'public class M {\n    public static void main(String[] args) {\n        int s = 0;\n        for (int i = 1; i <= 5; i++) {\n            if (i % 2 == 0) continue;\n            s += i;\n        }\n        System.out.println(s);\n    }\n}', "跳过偶数累加"),
("数组排序", 'import java.util.*;\npublic class M {\n    public static void main(String[] args) {\n        int[] x = {3, 1, 2};\n        Arrays.sort(x);\n        System.out.println(x[0]);\n    }\n}', "升序最小"),
("二分搜索", 'import java.util.*;\npublic class M {\n    public static void main(String[] args) {\n        int[] x = {1, 3, 5};\n        System.out.println(Arrays.binarySearch(x, 5));\n    }\n}', "命中索引 2"),
("char 运算", 'public class M {\n    public static void main(String[] args) {\n        char c = \'b\';\n        System.out.println(c - \'a\' + 1);\n    }\n}', "字符差"),
("取模", 'public class M {\n    public static void main(String[] args) {\n        System.out.println(17 % 5);\n    }\n}', "余数"),
("整除", 'public class M {\n    public static void main(String[] args) {\n        System.out.println(17 / 5);\n    }\n}', "整数商"),
("位运算", 'public class M {\n    public static void main(String[] args) {\n        System.out.println((6 & 3) | (8 >> 2));\n    }\n}', "与或移位"),
("Math.pow", 'public class M {\n    public static void main(String[] args) {\n        System.out.println((int)Math.pow(2, 3));\n    }\n}', "2 的 3 次方"),
("Math.max", 'public class M {\n    public static void main(String[] args) {\n        System.out.println(Math.max(3, 7) + Math.min(3, 7));\n    }\n}', "最大加最小"),
("递归阶乘", 'public class M {\n    static int f(int n) { return n <= 1 ? 1 : n * f(n - 1); }\n    public static void main(String[] args) {\n        System.out.println(f(5));\n    }\n}', "5 的阶乘"),
("斐波那契", 'public class M {\n    static int f(int n) { return n < 2 ? n : f(n - 1) + f(n - 2); }\n    public static void main(String[] args) {\n        System.out.println(f(7));\n    }\n}', "第 7 项"),
("多态转型", 'class A {}\nclass B extends A {}\npublic class M {\n    public static void main(String[] args) {\n        A x = new B();\n        System.out.println(x instanceof B ? 1 : 0);\n    }\n}', "运行时类型判断"),
("instanceof", 'class A {}\nclass B extends A {}\npublic class M {\n    public static void main(String[] args) {\n        A x = new A();\n        System.out.println(x instanceof B ? 1 : 0);\n    }\n}', "不是 B 类型"),
("抽象类", 'abstract class A { abstract int f(); }\nclass B extends A { int f() { return 4; } }\npublic class M {\n    public static void main(String[] args) {\n        System.out.println(new B().f() * 2);\n    }\n}', "实现抽象方法翻倍"),
("enum", 'public class M {\n    enum C { A(2), B(5); int v; C(int v) { this.v = v; } }\n    public static void main(String[] args) {\n        System.out.println(C.B.v + C.A.v);\n    }\n}', "枚举字段相加"),
("String.format", 'public class M {\n    public static void main(String[] args) {\n        String s = String.format("%d%d", 1, 2);\n        System.out.println(s.length());\n    }\n}', "格式化长度"),
("charAt", 'public class M {\n    public static void main(String[] args) {\n        String s = "abc";\n        System.out.println(s.charAt(1) - \'a\');\n    }\n}', "字符差"),
("ArrayList 删除", 'import java.util.*;\npublic class M {\n    public static void main(String[] args) {\n        List<Integer> l = new ArrayList<>(Arrays.asList(1, 2, 3));\n        l.remove(1);\n        System.out.println(l.get(1));\n    }\n}', "删除后取 3"),
("getOrDefault", 'import java.util.*;\npublic class M {\n    public static void main(String[] args) {\n        Map<String, Integer> m = new HashMap<>();\n        System.out.println(m.getOrDefault("k", 9));\n    }\n}', "缺省值 9"),
("迭代器", 'import java.util.*;\npublic class M {\n    public static void main(String[] args) {\n        List<Integer> l = Arrays.asList(2, 3);\n        int s = 0;\n        Iterator<Integer> it = l.iterator();\n        while (it.hasNext()) s += it.next();\n        System.out.println(s);\n    }\n}', "迭代累加"),
("parseInt", 'public class M {\n    public static void main(String[] args) {\n        System.out.println(Integer.parseInt("42") + 1);\n    }\n}', "解析加 1"),
("异常捕获", 'public class M {\n    public static void main(String[] args) {\n        int x = 0;\n        try { int y = 1 / 0; } catch (ArithmeticException e) { x = 5; }\n        System.out.println(x);\n    }\n}', "捕获后 x=5"),
("finally", 'public class M {\n    public static void main(String[] args) {\n        int x = 0;\n        try { int y = 1 / 0; } catch (ArithmeticException e) { x = 5; } finally { x++; }\n        System.out.println(x);\n    }\n}', "finally 加 1"),
("StringBuilder 反转", 'public class M {\n    public static void main(String[] args) {\n        StringBuilder b = new StringBuilder("abc");\n        System.out.println(b.reverse().length());\n    }\n}', "反转后长度不变"),
("字符数组排序", 'import java.util.*;\npublic class M {\n    public static void main(String[] args) {\n        char[] c = {\'b\', \'a\'};\n        Arrays.sort(c);\n        System.out.println(c[0] - \'a\' + 1);\n    }\n}', "升序最小 a"),
("trim 去空白", 'public class M {\n    public static void main(String[] args) {\n        String s = "  ab  ";\n        System.out.println(s.trim().length());\n    }\n}', "去空白后长度"),
("静态初始化块", 'class S {\n    static int v;\n    static { v = 6; }\n}\npublic class M {\n    public static void main(String[] args) {\n        System.out.println(S.v + 1);\n    }\n}', "静态块赋值"),
("多接口实现", 'interface I1 { default int a() { return 2; } }\ninterface I2 { default int b() { return 3; } }\nclass S implements I1, I2 {}\npublic class M {\n    public static void main(String[] args) {\n        S s = new S();\n        System.out.println(s.a() + s.b());\n    }\n}', "两接口方法相加"),
("泛型方法", 'public class M {\n    static <T> T id(T x) { return x; }\n    public static void main(String[] args) {\n        System.out.println(id(7) * 2);\n    }\n}', "泛型恒等翻倍"),
("replace 替换", 'public class M {\n    public static void main(String[] args) {\n        String s = "aXbXc";\n        System.out.println(s.replace("X", "").length());\n    }\n}', "去掉 X 后长度"),
]

def gen(lang: str, table: list[tuple[str, str, str]]) -> Path:
    meta = LANGS[lang]
    rows = []
    for name, code, reason in table:
        try:
            g = meta["gold"](code)
        except RuntimeError as e:
            print(f"[{lang}:{name}] SKIP gold verify: {e}")
            raise
        assert g and "\n" not in g, f"[{lang}:{name}] non-scalar: {g!r}"
        q = f'{meta["hdr"]}请用 "答案是 X #### X" 格式作答。\n```{meta["fence"]}\n{code}\n```'
        a = f"{reason}，{g}。答案是 {g} #### {g}"
        rows.append((q, a))
    out = ROOT / f"bench_{lang}60.csv"
    with open(out, "w", newline="", encoding="utf-8") as fh:
        w = csv.writer(fh)
        w.writerow(["question", "answer"])
        w.writerows(rows)
    rs = list(csv.DictReader(open(out, encoding="utf-8")))
    assert len(rs) == len(table) == 60, f"[{lang}] rows={len(rs)}"
    assert all(x["question"].count("```") == 2 for x in rs), f"[{lang}] fence broken"
    return out


if __name__ == "__main__":
    os.chdir(ROOT)
    for lang, table in [("py", PY), ("rs", RS), ("cpp", CPP), ("c", C), ("java", JAVA)]:
        try:
            p = gen(lang, table)
            print(f"[{lang}] -> {p} ({len(list(csv.DictReader(open(p))))} rows)")
        except RuntimeError as e:
            print(f"[{lang}] skipped: {e}")
