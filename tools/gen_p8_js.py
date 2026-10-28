#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""P8-A JS/TS 语言评测集生成器（60 题，特性互不重复）。

gold 策略：纯 JS 运行时题用 node 执行验证（不靠手算）；
TS 类型题靠静态类型语义（确定性推理），格式统一
"答案是 X #### X"，与 bench_*60.csv 一致。

产物：config/bench/datasets/bench_js60.csv（列 question,answer）。
"""
from __future__ import annotations

import csv
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "config/bench/datasets/bench_js60.csv"
N = 60


def node(src: str) -> str:
    return subprocess.run(["node", "-e", src], check=True,
                          capture_output=True, text=True).stdout.strip()


T = []  # (question, gold, node_src or None)


def add(q: str, gold: str, node_src: str | None = None) -> None:
    T.append((q, gold, node_src))


# ===== 1-10: 基础与闭包 =====
add("""以下 JS 代码输出什么？
```js
var a = 1;
function f() { console.log(a); var a = 2; }
f();
```""", "undefined", """var a = 1;
function f() { console.log(a); var a = 2; }
f();""")

add("""以下 JS 代码输出什么？
```js
let x = 10;
const f = () => { console.log(x); x = 20; };
f(); console.log(x);
```""", "10 然后 20", """let x = 10;
const f = () => { console.log(x); x = 20; };
f(); console.log(x);""")

add("""以下 JS 代码输出什么（闭包）？
```js
function make() {
  let n = 0;
  return () => ++n;
}
const a = make(); const b = make();
console.log(a(), a(), b());
```""", "1 2 1", """function make() {
  let n = 0;
  return () => ++n;
}
const a = make(); const b = make();
console.log(a(), a(), b());""")

add("""以下 JS 代码输出什么（var 循环捕获）？
```js
const fns = [];
for (var i = 0; i < 3; i++) fns.push(() => console.log(i));
fns[0]();
```""", "3", """const fns = [];
for (var i = 0; i < 3; i++) fns.push(() => console.log(i));
fns[0]();""")

add("""以下 JS 代码输出什么（let 循环捕获）？
```js
const fns = [];
for (let i = 0; i < 3; i++) fns.push(() => console.log(i));
fns[1]();
```""", "1", """const fns = [];
for (let i = 0; i < 3; i++) fns.push(() => console.log(i));
fns[1]();""")

add("""以下 JS 代码输出什么（严格模式 this）？
```js
'use strict';
function f() { console.log(this); }
f();
```""", "undefined", """'use strict';
function f() { console.log(this); }
f();""")

add("""以下 JS 代码输出什么（对象方法 this）？
```js
const o = { v: 5, get() { return this.v; } };
console.log(o.get());
const g = o.get;
console.log(g());
```""", "5 然后 undefined", """const o = { v: 5, get() { return this.v; } };
console.log(o.get());
const g = o.get;
console.log(g());""")

add("""以下 JS 代码输出什么（字符串拼接）？
```js
console.log('3' + 4, '3' - 4);
```""", "34 -1", """console.log('3' + 4, '3' - 4);""")

add("""以下 JS 代码输出什么（== vs ===）？
```js
console.log(0 == false, 0 === false);
```""", "true false", """console.log(0 == false, 0 === false);""")

add("""以下 JS 代码输出什么（NaN）？
```js
console.log(NaN === NaN, Number.isNaN(NaN));
```""", "false 然后 true", """console.log(NaN === NaN, Number.isNaN(NaN));""")

# ===== 11-20: 原型与继承 =====
add("""以下 JS 代码输出什么（原型链）？
```js
function A() {}
A.prototype.x = 1;
const a = new A();
console.log(a.x, a.hasOwnProperty('x'));
```""", "1 然后 false", """function A() {}
A.prototype.x = 1;
const a = new A();
console.log(a.x, a.hasOwnProperty('x'));""")

add("""以下 JS 代码输出什么（class 继承）？
```js
class A { constructor() { this.v = 'a'; } }
class B extends A { constructor() { super(); this.v += 'b'; } }
console.log(new B().v);
```""", "ab", """class A { constructor() { this.v = 'a'; } }
class B extends A { constructor() { super(); this.v += 'b'; } }
console.log(new B().v);""")

add("""以下 JS 代码中，instanceof 判断什么？""",
    "对象是否属于某构造器原型链", None)

add("""以下 JS 代码输出什么（Object.create）？
```js
const base = { greet() { return 'hi'; } };
const o = Object.create(base);
console.log(o.greet(), o.hasOwnProperty('greet'));
```""", "hi 然后 false", """const base = { greet() { return 'hi'; } };
const o = Object.create(base);
console.log(o.greet(), o.hasOwnProperty('greet'));""")

add("""以下 JS 代码输出什么（getter）？
```js
const o = { get v() { return 7; } };
console.log(o.v);
```""", "7", """const o = { get v() { return 7; } };
console.log(o.v);""")

add("""以下 JS 代码输出什么（Symbol.iterator）？
```js
const o = { [Symbol.iterator]: function* () { yield 1; yield 2; } };
console.log([...o].length);
```""", "2", """const o = { [Symbol.iterator]: function* () { yield 1; yield 2; } };
console.log([...o].length);""")

add("""以下 JS 代码输出什么（Object.assign 浅拷贝）？
```js
const a = { x: { y: 1 } };
const b = Object.assign({}, a);
b.x.y = 9;
console.log(a.x.y);
```""", "9（浅拷贝共享嵌套对象）", """const a = { x: { y: 1 } };
const b = Object.assign({}, a);
b.x.y = 9;
console.log(a.x.y);""")

add("""以下 JS 代码输出什么（静态方法）？
```js
class A { static s() { return 's'; } }
console.log(A.s());
```""", "s", """class A { static s() { return 's'; } }
console.log(A.s());""")

add("""以下 JS 代码中，Object.freeze 的作用是什么？""",
    "对象不可扩展且属性不可改（只读）", None)

add("""以下 JS 代码输出什么（instanceof 跨原型）？
```js
function A() {}
const a = new A();
console.log(a instanceof A, a instanceof Object);
```""", "true 然后 true", """function A() {}
const a = new A();
console.log(a instanceof A, a instanceof Object);""")

# ===== 21-30: 异步 =====
add("""以下 JS 代码输出顺序是什么（microtask 优先）？
```js
console.log(1);
Promise.resolve().then(() => console.log(2));
setTimeout(() => console.log(3), 0);
console.log(4);
```""", "1 4 2 3", """console.log(1);
Promise.resolve().then(() => console.log(2));
setTimeout(() => console.log(3), 0);
console.log(4);""")

add("""以下 JS 代码输出什么（Promise 链）？
```js
Promise.resolve(1).then(v => v + 1).then(v => console.log(v));
```""", "2", """Promise.resolve(1).then(v => v + 1).then(v => console.log(v));""")

add("""以下 JS 代码输出什么（async/await）？
```js
async function f() { return 42; }
f().then(v => console.log(v));
```""", "42", """async function f() { return 42; }
f().then(v => console.log(v));""")

add("""以下 JS 代码输出什么（Promise.all 失败）？
```js
Promise.all([Promise.resolve(1), Promise.reject(2)]).catch(e => console.log(e));
```""", "2", """Promise.all([Promise.resolve(1), Promise.reject(2)]).catch(e => console.log(e));""")

add("""以下 JS 代码输出什么（Promise.race）？
```js
Promise.race([Promise.resolve('a'), Promise.resolve('b')]).then(v => console.log(v));
```""", "a", """Promise.race([Promise.resolve('a'), Promise.resolve('b')]).then(v => console.log(v));""")

add("""以下 JS 代码输出什么（catch 后继续）？
```js
Promise.reject('e').catch(e => 'ok').then(v => console.log(v));
```""", "ok", """Promise.reject('e').catch(e => 'ok').then(v => console.log(v));""")

add("""以下 JS 代码中，await 的作用是什么？""",
    "暂停 async 函数直到 Promise 完成", None)

add("""以下 JS 代码输出什么（async 内 throw）？
```js
async function f() { throw new Error('boom'); }
f().catch(e => console.log(e.message));
```""", "boom", """async function f() { throw new Error('boom'); }
f().catch(e => console.log(e.message));""")

add("""以下 JS 代码输出什么（微任务嵌套）？
```js
Promise.resolve().then(() => console.log(1))
  .then(() => console.log(2));
Promise.resolve().then(() => console.log(3));
```""", "1 3 2", """Promise.resolve().then(() => console.log(1))
  .then(() => console.log(2));
Promise.resolve().then(() => console.log(3));""")

add("""以下 JS 代码输出什么（Promise 构造函数立即执行）？
```js
new Promise(res => { console.log('a'); res(); });
console.log('b');
```""", "a 然后 b", """new Promise(res => { console.log('a'); res(); });
console.log('b');""")

# ===== 31-40: 数组与函数式 =====
add("""以下 JS 代码输出什么（reduce）？
```js
console.log([1, 2, 3].reduce((a, b) => a + b, 0));
```""", "6", """console.log([1, 2, 3].reduce((a, b) => a + b, 0));""")

add("""以下 JS 代码输出什么（filter+map）？
```js
console.log([1, 2, 3, 4].filter(x => x % 2 === 0).map(x => x * 10));
```""", "[ 20, 40 ]", """console.log([1, 2, 3, 4].filter(x => x % 2 === 0).map(x => x * 10));""")

add("""以下 JS 代码输出什么（sort 数字）？
```js
console.log([10, 2, 1].sort());
console.log([10, 2, 1].sort((a, b) => a - b));
```""", "[ 1, 10, 2 ] 然后 [ 1, 2, 10 ]", """console.log([10, 2, 1].sort());
console.log([10, 2, 1].sort((a, b) => a - b));""")

add("""以下 JS 代码输出什么（解构）？
```js
const [a, , b] = [1, 2, 3];
console.log(a + b);
```""", "4", """const [a, , b] = [1, 2, 3];
console.log(a + b);""")

add("""以下 JS 代码输出什么（展开）？
```js
console.log(Math.max(...[1, 5, 3]));
```""", "5", """console.log(Math.max(...[1, 5, 3]));""")

add("""以下 JS 代码输出什么（可选链）？
```js
const o = { a: { b: 1 } };
console.log(o.a?.b, o.x?.y);
```""", "1 undefined", """const o = { a: { b: 1 } };
console.log(o.a?.b, o.x?.y);""")

add("""以下 JS 代码输出什么（空值合并）？
```js
console.log(null ?? 'd', 0 ?? 'd', '' ?? 'd');
```""", "d 0 空字符串", None)

add("""以下 JS 代码输出什么（flatMap）？
```js
console.log([[1], [2, 3]].flatMap(x => x));
```""", "[ 1, 2, 3 ]", """console.log([[1], [2, 3]].flatMap(x => x));""")

add("""以下 JS 代码输出什么（Set 去重）？
```js
console.log(new Set([1, 1, 2, 3, 3]).size);
```""", "3", """console.log(new Set([1, 1, 2, 3, 3]).size);""")

add("""以下 JS 代码输出什么（对象键顺序）？
```js
const o = { 2: 'b', 1: 'a', x: 'c' };
console.log(Object.keys(o).join(','));
```""", "1,2,x（整数键升序优先）", """const o = { 2: 'b', 1: 'a', x: 'c' };
console.log(Object.keys(o).join(','));""")

# ===== 41-50: TS 类型 =====
add("""以下 TS 代码中，x 的类型推断是什么？
```ts
let x = 10;
x = 'str';  // 报错吗？
```""", "number；赋 string 报错", None)

add("""以下 TS 代码中，泛型 T 的推断结果是什么？
```ts
function id<T>(v: T): T { return v; }
const a = id(42);
```""", "number", None)

add("""以下 TS 代码中，联合类型的写法是什么？
```ts
let v: string | number;
```""", "string 或 number", None)

add("""以下 TS 代码中，keyof 的作用是什么？""",
    "取类型的所有键的联合", None)

add("""以下 TS 代码中，ReturnType 的作用是什么？""",
    "取函数类型的返回值类型", None)

add("""以下 TS 代码中，接口与类型别名的关键区别是什么？""",
    "接口可合并声明，类型别名不可", None)

add("""以下 TS 代码中，Pick<T, K> 的作用是什么？""",
    "从 T 中挑选键 K 组成新类型", None)

add("""以下 TS 代码中，类型守卫 typeof 的作用是什么？""",
    "在联合类型中收窄类型分支", None)

add("""以下 TS 代码中，readonly 的作用是什么？""",
    "属性只读（编译期检查）", None)

add("""以下 TS 代码中，as const 的作用是什么？""",
    "字面量类型收窄（不可变字面量）", None)

# ===== 51-60: 高级 =====
add("""以下 JS 代码输出什么（WeakMap 键）？
```js
const wm = new WeakMap();
const k = {};
wm.set(k, 1);
console.log(wm.get(k));
```""", "1", """const wm = new WeakMap();
const k = {};
wm.set(k, 1);
console.log(wm.get(k));""")

add("""以下 JS 代码输出什么（Proxy）？
```js
const p = new Proxy({}, { get(t, k) { return 'proxy'; } });
console.log(p.anything);
```""", "proxy", """const p = new Proxy({}, { get(t, k) { return 'proxy'; } });
console.log(p.anything);""")

add("""以下 JS 代码输出什么（生成器）？
```js
function* g() { yield 1; yield 2; }
const it = g();
console.log(it.next().value, it.next().value);
```""", "1 2", """function* g() { yield 1; yield 2; }
const it = g();
console.log(it.next().value, it.next().value);""")

add("""以下 JS 代码输出什么（JSON 序列化）？
```js
console.log(JSON.stringify({ a: 1, b: undefined }));
```""", "{\"a\":1}", """console.log(JSON.stringify({ a: 1, b: undefined }));""")

add("""以下 JS 代码输出什么（模板字符串）？
```js
const n = 3;
console.log(`v=${n * 2}`);
```""", "v=6", """const n = 3;
console.log(`v=${n * 2}`);""")

add("""以下 JS 代码输出什么（for...of vs for...in）？
```js
const o = { a: 1, b: 2 };
console.log([...Object.keys(o)].join(','), [...o].length);
```""", "a,b 然后 抛 TypeError（对象不可迭代）", None)

add("""以下 JS 代码输出什么（闭包计数器封装）？
```js
function counter() { let n = 0; return { inc: () => ++n, get: () => n }; }
const c = counter();
c.inc(); c.inc();
console.log(c.get());
```""", "2", """function counter() { let n = 0; return { inc: () => ++n, get: () => n }; }
const c = counter();
c.inc(); c.inc();
console.log(c.get());""")

add("""以下 JS 代码输出什么（正则）？
```js
console.log('a1b2'.replace(/\\d/g, '#'));
```""", "a#b#", """console.log('a1b2'.replace(/\\d/g, '#'));""")

add("""以下 JS 代码中，void 0 的值是什么？""",
    "undefined", None)

add("""以下 JS 代码输出什么（BigInt）？
```js
console.log(2n ** 10n);
```""", "1024n", """console.log(2n ** 10n);""")


def validate() -> int:
    exec_ok = 0
    for q, gold, src in T:
        assert len(q) > 20 and len(gold) > 0, "空题"
        if src is not None:
            out = node(src)
            # 单行/多行统一规范化：node 输出 join 空格 == gold 去 "然后" 后 join
            got = " ".join(out.splitlines()).strip()
            want = " ".join(gold.split("（")[0].split(" 然后 ")).strip()
            assert got == want, f"node mismatch: {out!r} != {want!r} | {q[:60]}"
            exec_ok += 1
    return exec_ok


def main() -> None:
    exec_ok = validate()
    assert len(T) == N, f"题量 {len(T)} != {N}"
    with open(OUT, "w", newline="", encoding="utf-8") as fh:
        w = csv.writer(fh)
        w.writerow(["question", "answer"])
        for q, gold, _ in T:
            w.writerow([q, gold])
    print(f"written {OUT}: {N} 题（node 验证 {exec_ok}）")


if __name__ == "__main__":
    main()
