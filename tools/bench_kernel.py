#!/usr/bin/env python3
"""bench_kernel.py — 算子开发生成评测（P6l，生成型全自动机器验证）。

范式：给模型"任务描述 + 签名约束"，模型生成算子函数（C/Rust/C++）；
与固定测试驱动拼接后**真实编译 + 运行 + 断言**验证（clang/rustc/
clang++）。对比 full（原始任务描述）vs scx（压缩后任务描述）的通过率。
压缩 query = 任务描述文本。gold 无手工标注——通过/失败由编译器与断言
机器判定。
"""
import argparse
import csv
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from bench import (  # noqa: E402
    load_registry,
    resolve_model,
    load_model_mlx,
    wrap_chat,
    scx_compress,
)

CC = {
    "C": (["clang", "-O0", "-w", "-x", "c"], "c"),
    "Rust": (["rustc", "--edition", "2021", "--crate-type", "bin"], "rs"),
    "C++": (["clang++", "-std=c++17", "-O0", "-w"], "cpp"),
}


def extract_code(raw: str, lang: str) -> str:
    """模型输出 → 可编译代码。

    1) 优先取首个围栏块内容；
    2) 无围栏：截到幻觉续写标记（<|endoftext|>、Human:/Assistant: 行），
       再整体 strip（3B 常见把训练语料的角色结构续写出来）。
    """
    if "```" in raw:
        parts = raw.split("```")
        for p in parts[1:]:
            body = p.split("\n", 1)[1] if "\n" in p else p
            body = body.strip()
            if body:
                return body
    for mark in ("<|endoftext|>", "\nHuman:", "\nAssistant:"):
        i = raw.find(mark)
        if i >= 0:
            raw = raw[:i]
    return raw.strip()


# 语言工程规范块：真实工程算子任务 prompt 本来就含接口约定/验收标准；
# 同时拉长上下文使 scx 压缩有意义（SMALL_TOK=400 以下不裁剪）。
LANG_SPEC = {
    "C": ("Engineering context. Target platform: macOS, clang C99. Put all definitions at the "
          "translation-unit top level. Include any required standard headers yourself. "
          "Do NOT define main(); the test harness provides it. Output ONLY the function "
          "definition - no explanation, no commentary, no notes. The signature must match the "
          "required signature exactly (parameter types, return type, const qualifiers). Handle "
          "edge inputs correctly: empty arrays, empty strings, zero values, empty lists. "
          "No global mutable state. No unrelated helper functions. Common pitfalls to avoid: "
          "buffer overrun, uninitialized locals, returning a pointer to a local array, integer "
          "overflow truncation, dereferencing NULL without a check, forgetting the string "
          "terminator. Style: 4-space indent, self-explanatory names, max line 80 cols, "
          "early return instead of deep nesting. Wrap the code in ```c fences if you wish. Review checklist before answering: the definition compiles standalone; every parameter is used or intentionally ignored; loop bounds hold for the worst case; no out-of-bounds access; the return value matches the contract for all edge inputs; the output contains no scratch notes or commentary."),
    "Rust": ("Engineering context. Target platform: rustc 2021 edition. Give complete definitions "
             "for functions, structs and impl blocks. Bring in standard library items with an "
             "explicit use when needed. Do NOT define main(); the test harness provides it. "
             "Output ONLY the definition - no explanation, no commentary, no notes. The "
             "signature must match the required signature exactly (slice/reference parameters, "
             "return type, generics bounds). Handle edge inputs correctly: empty slices, zero "
             "values, string boundaries. Respect ownership and borrowing - never return a "
             "dangling reference. Avoid unnecessary panic/unwrap branches; return the agreed "
             "value instead. No unrelated helper functions. Common pitfalls to avoid: mutating "
             "data while it is immutably borrowed, closure capture type mismatches, unwrapping "
             "a None at runtime, integer overflow without saturating/checked arithmetic, "
             "declaring main inside a fn, struct field names that differ from the contract. "
             "Style: 4-space indent, self-explanatory names, max line 80 cols, early return. "
             "Wrap the code in ```rust fences if you wish. Review checklist before answering: the definition compiles standalone; every parameter is used or intentionally ignored; loop bounds hold for the worst case; no out-of-bounds access; the return value matches the contract for all edge inputs; the output contains no scratch notes or commentary."),
    "C++": ("Engineering context. Target platform: clang++ -std=c++17. Give complete definitions "
            "for functions, classes and templates. Include required standard headers yourself "
            "(vector, string, etc). Do NOT define main(); the test harness provides it. Output "
            "ONLY the definition - no explanation, no commentary, no notes. The signature must "
            "match the required signature exactly (template parameters, reference types, return "
            "type, const qualifiers). Handle edge inputs correctly: empty containers, empty "
            "strings, zero values. Class members (including static members) must be defined "
            "in-class or out-of-class - never leave a bare declaration. Prefer standard library "
            "containers and algorithms over hand-rolled pointer management. No unrelated helper "
            "functions. Common pitfalls to avoid: static member without out-of-class definition "
            "(linker failure), template definition separated from declaration, returning a "
            "reference to a local, calling front()/back() on an empty container, comparing "
            "signed and unsigned integers, using a vector iterator after invalidation, missing "
            "const qualifiers on member functions. Style: 4-space indent, self-explanatory "
            "names, max line 80 cols, early return. Wrap the code in ```cpp fences if you wish. Review checklist before answering: the definition compiles standalone; every parameter is used or intentionally ignored; loop bounds hold for the worst case; no out-of-bounds access; the return value matches the contract for all edge inputs; the output contains no scratch notes or commentary."),
}


def build_prompt(item: dict) -> str:
    """任务描述（中英）+ 签名 + 语言工程规范块（可被 scx 压缩的上下文）。"""
    sig = item["sig"].replace("\n", " ")
    sig_line = f"Signature: {sig}\n" if sig else ""
    return (
        f"You are a senior {item['lang']} engineer. Output ONLY compilable code, no explanation.\n\n"
        f"Task: {item['task']}\n"
        f"Task (EN): {item['en_task']}\n\n{sig_line}\n"
        f"{LANG_SPEC[item['lang']]}\n"
        f"Answer with the complete compilable implementation that satisfies every requirement above. The final answer is only the code itself, nothing else. Verification contract: your answer is compiled together with a fixed test driver and run; every assertion must hold. Compile errors, link errors, missing symbols, and assertion failures all count as wrong answers. The driver declares the target signature before main; your definition may appear before or after it. Keep the definition self-contained: no extra files, no external crates, no platform-specific extensions. If a standard header is needed, include it at the top of your answer. Prefer simplicity: a straightforward correct implementation scores over an elaborate one."
    )


def check(gen_code: str, test_code: str, lang: str, workdir: Path) -> bool:
    """拼接生成代码 + 测试驱动，编译并运行；全部通过返回 True。

    C/C++ 的 include/原型必须位于生成代码之前（模型实现可能依赖
    <string.h> 等头），故 test 拆为「头部（include+原型）+ 生成代码 +
    main 段」；Rust 无此顺序约束，直接 生成代码 + 完整 test。
    """
    ext = CC[lang][1]
    src = workdir / f"k.{ext}"
    if lang == "Rust":
        src.write_text(gen_code + "\n\n" + test_code, encoding="utf-8")
    else:
        marker = test_code.find("int main")
        head = test_code[:marker].rstrip() + "\n"
        body = test_code[marker:]
        src.write_text(head + gen_code + "\n\n" + body, encoding="utf-8")
    bin_path = workdir / "k.out"
    try:
        proc = subprocess.run(
            CC[lang][0] + [str(src), "-o", str(bin_path)],
            capture_output=True, text=True, timeout=30,
        )
        if proc.returncode != 0:
            return False
        run = subprocess.run([str(bin_path)], capture_output=True, text=True, timeout=10)
        return run.returncode == 0
    except subprocess.TimeoutExpired:
        return False


def api_run(prompt: str, max_new: int = 640) -> str:
    """在线 OpenAI 兼容轨（DeepSeek 官方，deepseek-flash）。key 走环境变量。"""
    key = os.environ.get("DEEPSEEK_API_KEY", "")
    if not key:
        raise SystemExit("DEEPSEEK_API_KEY env missing for --api deepseek")
    body = {
        "model": "deepseek-flash",
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": max_new,
        "temperature": 0.1,
    }
    import json
    import urllib.request
    req = urllib.request.Request(
        "https://api.deepseek.com/chat/completions",
        data=json.dumps(body).encode(),
        headers={"Authorization": f"Bearer {key}", "Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=180) as r:
        data = json.loads(r.read())
    return data["choices"][0]["message"]["content"]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", default="config/bench/datasets/bench_kernel.csv")
    ap.add_argument("--model", default="qwen2.5-3b-instruct")
    ap.add_argument("--api", default="", help="在线轨: deepseek（默认空 = 本地 MLX）")
    ap.add_argument("--api-conc", type=int, default=4, help="在线轨并发请求数")
    ap.add_argument("--scx-bin", default="target/release/scx")
    ap.add_argument("--quantize", type=int, default=8)
    ap.add_argument("--scx-extra", default="--trim 1 --trim-ratio 0.3 --trim-op-keep 1 --num-line-keep 1",
                    metavar="ARGS")
    ap.add_argument("--csv", default="", help="逐题明细导出")
    ap.add_argument("--tag", default="", help="日志标签")
    args = ap.parse_args()

    scx_extra = tuple(args.scx_extra.split()) if args.scx_extra else ()
    with open(args.data, encoding="utf-8") as fh:
        qa = list(csv.DictReader(fh))
    # CSV 以真换行存多行文本（P6l 重建）：不得再做 \n 字面还原，
    # 否则会把 printf("OK\n") 的转义 \n 破坏成真换行（C/C++ 全编译挂根因）。

    online = args.api == "deepseek"
    tok = None
    if online:
        model_name = f"DeepSeek-V4.1-Flash (API, conc={args.api_conc})"
    else:
        registry = load_registry()
        spec = resolve_model(registry, args.model)
        model_name = spec.display()
        print(f"[load] {model_name} ...", flush=True)
        from mlx_lm import generate as mlx_generate
        model, tok = load_model_mlx(spec, quantize=args.quantize)

    def run(prompt: str, max_new: int = 640) -> str:
        if online:
            return api_run(prompt, max_new)
        # 必须走 chat 模板（instruct 模型裸 prompt 会复读任务/续写幻觉）。
        return mlx_generate(model, tok, wrap_chat(tok, prompt), max_tokens=max_new, verbose=False)

    def ntok(s: str) -> int:
        return len(tok.encode(s)) if tok is not None else len(s) // 3

    # 生成：在线轨并发出请求；本地轨主线程串行（mlx_lm 不支持子线程 GPU stream）。
    import concurrent.futures as cf
    gen = {}
    scx_map = {}
    if online:
        jobs = []
        for i, item in enumerate(qa):
            prompt = build_prompt(item)
            jobs.append((i, "full", prompt))
            scx_out = scx_compress(args.scx_bin, prompt, item["task"], extra=scx_extra)
            scx_map[i] = scx_out
            jobs.append((i, "scx", scx_out))
        t0_all = time.time()
        with cf.ThreadPoolExecutor(max_workers=args.api_conc) as ex:
            futs = {ex.submit(run, p): (i, tr) for (i, tr, p) in jobs}
            done = 0
            for f in cf.as_completed(futs):
                i, tr = futs[f]
                gen[(i, tr)] = f.result()
                done += 1
                if done % 4 == 0 or done == len(futs):
                    print(f"[gen {done}/{len(futs)}] {time.time()-t0_all:.0f}s", flush=True)
    else:
        for i, item in enumerate(qa):
            prompt = build_prompt(item)
            gen[(i, "full")] = run(prompt)
            scx_out = scx_compress(args.scx_bin, prompt, item["task"], extra=scx_extra)
            scx_map[i] = scx_out
            gen[(i, "scx")] = run(scx_out)

    rows = []
    for i, item in enumerate(qa):
        lang = item["lang"]
        task = item["task"]
        test = item["test"]
        prompt = build_prompt(item)
        workdir = Path(tempfile.mkdtemp(prefix="kbench_"))

        t0 = time.time()
        full_code = extract_code(gen[(i, "full")], lang)
        scx_out = scx_map[i]
        scx_tok = ntok(scx_out)
        full_tok = ntok(prompt)
        scx_code = extract_code(gen[(i, "scx")], lang)
        full_ok = check(full_code, test, lang, workdir)
        scx_ok = check(scx_code, test, lang, workdir)
        dt = time.time() - t0

        row = {
            "lang": lang, "task": task[:24],
            "full": full_ok, "scx": scx_ok,
            "full_tok": full_tok, "scx_tok": scx_tok,
            "compressed": scx_out != prompt,
        }
        rows.append(row)
        print(f"[{i+1}/{len(qa)}] {lang:4s} {task[:20]:22s} full={'P' if full_ok else 'F'} scx={'P' if scx_ok else 'F'} "
              f"cut={100*(1-scx_tok/full_tok):.1f}% {dt:.1f}s", flush=True)

    n = len(rows)
    full_p = sum(1 for r in rows if r["full"]) / n
    scx_p = sum(1 for r in rows if r["scx"]) / n
    tok_ratio = sum(r["scx_tok"] for r in rows) / sum(r["full_tok"] for r in rows)
    passed = sum(1 for r in rows if r["full"])
    kept = sum(1 for r in rows if r["full"] and r["scx"])
    print(f"\n=== 算子开发生成评测（{model_name}, {n} 题, {scx_extra}）===")
    print(f"full : pass={full_p:.3f} ({full_p*n:.0f}/{n})")
    print(f"scx  : pass={scx_p:.3f} ({scx_p*n:.0f}/{n})  tok_ratio={tok_ratio:.3f}  cut={100*(1-tok_ratio):.1f}%")
    print(f"保持率（full 通过中 scx 仍通过）: {kept}/{passed} = {kept/max(passed,1):.3f}")
    by_lang = {}
    for r in rows:
        b = by_lang.setdefault(r["lang"], {"full": 0, "scx": 0, "n": 0})
        b["n"] += 1
        b["full"] += 1 if r["full"] else 0
        b["scx"] += 1 if r["scx"] else 0
    for l, b in by_lang.items():
        print(f"  {l:5s} full {b['full']}/{b['n']}  scx {b['scx']}/{b['n']}")

    if args.csv:
        with open(args.csv, "w", newline="") as fh:
            w = csv.DictWriter(fh, fieldnames=["lang", "task", "full", "scx", "full_tok", "scx_tok", "compressed"])
            w.writeheader()
            for r in rows:
                w.writerow(r)
        print(f"[csv] {args.csv}", flush=True)


if __name__ == "__main__":
    main()
