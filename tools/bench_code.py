#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""P8-D 代码能力评测集泛化验证器（双在线模型，full vs scx 保持率）。

用法：
  DEEPSEEK_API_KEY=... QWEN_API_KEY=... /tmp/scx-eval/bin/python tools/bench_code.py \
      --data config/bench/datasets/bench_cuda60.csv --model both \
      --csv config/bench/results/bench_code_cuda60.csv --resume

指标（与 bench_long 同口径）：
  full_acc  / scx_acc  : 模型回答命中 gold 的比例（normalize 宽松比对）
  保持率              : full 答对中 scx 仍答对的比例
  cut%                : 字符口径压缩率（在线无本地 tokenizer）
"""
from __future__ import annotations

import argparse
import csv
import json
import os
import re
import sys
import time
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

sys.path.insert(0, str(ROOT / "tools"))
from bench import scx_compress  # noqa: E402  正确封装：文件+query 调用+装饰块截断

JUDGE_CFG = {
    "deepseek": {
        "url": "https://api.deepseek.com/chat/completions",
        "model": "deepseek-flash",
        "env": "DEEPSEEK_API_KEY",
    },
    "qwen": {
        "url": "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions",
        "model": "qwen3.8-flash",
        "env": "QWEN_API_KEY",
    },
}

ANSWER_RE = re.compile(r"答案是\s*(.*?)\s*####\s*(.*?)\s*$", re.S)


def api_call(prompt: str, model: str, max_new: int = 0) -> str:
    # deepseek-flash 带 reasoning：思考会吃光小 max_tokens 导致空 content，
    # 必须给足（bench_arch 同注）；qwen3.8-flash 同样带思考。
    if max_new <= 0:
        max_new = 2048 if model == "deepseek" else 1024
    cfg = JUDGE_CFG[model]
    key = os.environ.get(cfg["env"], "")
    if not key:
        raise SystemExit(f"{cfg['env']} env missing for model={model}")
    body = {"model": cfg["model"], "messages": [{"role": "user", "content": prompt}],
            "max_tokens": max_new, "temperature": 0.1}
    if model == "deepseek":
        # 关 reasoning：思考会吃光 max_tokens 致空响应且每题 30-40s；
        # 实测 reasoning 关闭后 1.2s 返回同等质量答案，速度 20-30x。
        body["reasoning"] = {"enabled": False}
    last = None
    for attempt in range(5):
        try:
            req = urllib.request.Request(
                cfg["url"], data=json.dumps(body).encode(),
                headers={"Authorization": f"Bearer {key}", "Content-Type": "application/json"},
            )
            with urllib.request.urlopen(req, timeout=240) as r:
                content = json.loads(r.read())["choices"][0]["message"]["content"]
            if content and content.strip():
                return content
            last = RuntimeError("empty model content")
        except Exception as e:  # noqa: BLE001
            last = e
        time.sleep(3 * (attempt + 1))
    return ""


CONTEXT = {
    "cuda": """CUDA 编程模型：kernel 在 GPU 上以线程网格（grid）执行，网格由线程块（block）组成，
每块由线程（thread）组成，通过 blockIdx/threadIdx/gridDim/blockDim 索引定位全局线程 ID。
内存层级包括全局内存（设备主存，容量大延迟高）、共享内存（块内线程共享的高速存储）、
常量/纹理缓存与线程私有寄存器。块内同步用 __syncthreads，跨块同步需原子操作或多次 kernel。
性能关键：全局访问要合并（相邻线程相邻地址）、共享内存避免 bank conflict、warp 内天然同步、
warp divergence 使两个分支串行执行、occupancy 受寄存器/共享内存限制。""",
    "js": """JavaScript 运行时：单线程事件循环，同步代码先执行，微任务（Promise.then）在每轮
宏任务（setTimeout 等）之前全部执行。函数是第一类对象，闭包捕获词法作用域变量并持有引用。
this 取决于调用方式（方法调用/普通调用/箭头函数词法绑定）。原型链实现继承，
class 是语法糖。Promise 三种状态、async/await 是语法糖。对象键枚举顺序：整数键升序优先。
浅拷贝（Object.assign/展开）共享嵌套对象。GC 处理不可达对象。""",
    "shape": """配置与数据处理：JSON 为通用数据交换格式（对象/数组/数字/字符串/布尔/null，
键序为插入序，Python json 解析数字为 int/float）。YAML 用缩进表达层级，列表用 - 前缀。
TOML 用 [表] 分段，键值对赋值。SQL 查询数据库表，聚合函数配 GROUP BY，WHERE 过滤，
DISTINCT 去重。正则表达式匹配文本，\\d 数字、[a-z] 字符类、+ 一个或多个、锚点 ^ $ 定界。
shell 脚本命令替换 $(...) 执行并插入输出，grep 过滤行、awk 按列处理。日志常用 ISO 8601 时间戳。""",
    "layers": """代码工程素养：L1 语义层理解代码行为（输出/结果/类型）；L2 性能层分析复杂度、
缓存、访存、内存与并发开销；L3 架构层做工程设计决策（模块划分、分层、状态管理、
可扩展性与权衡）。生产级工程强调：正确性可验证、性能可测量、架构可演进。""",
}


def build_prompt(q: str, kind: str = "cuda") -> str:
    ctx = CONTEXT.get(kind, CONTEXT["cuda"])
    head = "你是资深软件工程师，负责代码理解、正确性推理与性能分析。\n\n"
    tail = (
        f"\n\n问题：{q}\n\n"
        "作答要求：先给出简短推理，最后一行必须用 \"答案是 X #### X\" 格式作答"
        "（X 为最终答案，与推理同行分隔）。"
    )
    # 领域背景补充到总长 ≥1000 字符，保证 scx 触发压缩且有足量可裁内容
    prompt = head + ctx + tail
    block = "\n技术要点补充：" + ctx
    while len(prompt) < 1000:
        prompt = head + ctx + block + tail
        ctx = ctx + block
    return prompt


def scx_compress_prompt(prompt: str, query_hint: str, scx_bin: str, extra: str) -> tuple[str, bool]:
    out = scx_compress(scx_bin, prompt, query_hint, extra=tuple(extra.split()))
    return out, out != prompt


def norm(s: str) -> str:
    """gold/答案归一化：小写、去空白标点、去括号说明、数字中英同化。"""
    s = s.lower().strip()
    s = s.split("（")[0]
    s = re.sub(r"[\s\.,;:'\"`_\-\[\]{}()（）]", "", s)
    # 数字中英同化（长串先替换："10" 先于 "1"）
    for d, c in (("10", "十"), ("0", "零"), ("1", "一"), ("2", "二"),
                 ("3", "三"), ("4", "四"), ("5", "五"), ("6", "六"),
                 ("7", "七"), ("8", "八"), ("9", "九")):
        s = s.replace(d, c)
    # 去 "答案是/结果是" 前缀
    s = re.sub(r"^(答案|结果是|答案为)", "", s)
    return s


def extract_answer(raw: str) -> str:
    """从模型回答抽取最终答案（答案是 X #### X 或末尾句）。"""
    m = ANSWER_RE.search(raw.strip())
    if m:
        return m.group(1).strip()
    # 兜底：取最后一行（去 "答案是" 前缀）
    lines = [l for l in raw.strip().splitlines() if l.strip()]
    last = lines[-1] if lines else raw
    return re.sub(r"^答案是\s*", "", last).strip()


def check(raw: str, gold: str) -> bool:
    a_raw = extract_answer(raw)
    ans = norm(a_raw)
    if not ans:
        return False
    # 候选答案：gold 主句 + 括号内内容（"（寄存器/局部内存）" → 答其一即对）
    cands = [gold] + re.findall(r"[（(]([^）)]+)[)）]", gold)
    for c in cands:
        for sub in re.split(r"[/\s]+", c):
            if not sub:
                continue
            sn = norm(sub)
            if not sn:
                continue
            if sn == ans or sn in ans or ans in sn:
                return True
            # 英文/数字标识符子串（中英混排粘连免疫）
            if re.search(r"[A-Za-z0-9]", sub):
                core = re.sub(r"[_\s()（）]", "", sub).lower()
                if core and core in re.sub(r"[_\s]", "", a_raw).lower():
                    return True
    # 数字集合（原文提取）
    na = set(re.findall(r"\d+", a_raw))
    ng = set(re.findall(r"\d+", gold))
    if ng and na and ng <= na:
        return True
    # 中文关键短语：gold 主段任一 ≥2 字连续段出现在答案
    main_seg = re.findall(r"[\u4e00-\u9fff]{2,}", gold.split("（")[0])
    ans_seg = re.findall(r"[\u4e00-\u9fff]{2,}", ans)
    if main_seg and ans_seg:
        if any(s in x for s in main_seg for x in ans_seg):
            return True
    return False


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", required=True)
    ap.add_argument("--model", default="both", choices=["deepseek", "qwen", "both"])
    ap.add_argument("--scx-bin", default="target/release/scx")
    ap.add_argument("--scx-extra",
                    default="--trim 1 --trim-ratio 0.3 --trim-op-keep 1 --num-line-keep 1 --sym-line-keep 1")
    ap.add_argument("--csv", default="", help="逐题明细导出")
    ap.add_argument("--resume", action="store_true")
    ap.add_argument("--workers", type=int, default=6)
    args = ap.parse_args()

    qa = list(csv.DictReader(open(args.data, encoding="utf-8")))
    models = ["deepseek", "qwen"] if args.model == "both" else [args.model]
    # 压缩召回 query：短领域关键词（terms≈4-6），按评测集类型给 hint
    hint_map = {
        "cuda": "CUDA 内核 线程 共享内存 原子 warp",
        "js": "JavaScript 输出 闭包 异步 原型 类型",
        "shape": "配置 解析 JSON SQL 正则 输出",
        "layers": "代码 语义 性能 架构 设计",
    }
    name = Path(args.data).stem
    qhint = next((h for k, h in hint_map.items() if k in name), "代码 输出 结果 行为 类型")
    print(f"== 评测 {args.data} {len(qa)} 题 × {models} | query_hint={qhint!r} ==")

    rows = []
    csv_fh = None
    csv_w = None
    done = set()
    if args.csv and args.resume and Path(args.csv).exists():
        with open(args.csv, encoding="utf-8") as fh:
            for r in csv.DictReader(fh):
                rows.append(r)
                done.add((r["model"], r["idx"]))
        csv_fh = open(args.csv, "a", newline="")
        csv_w = csv.DictWriter(csv_fh, fieldnames=csv.DictReader(
            open(args.csv, encoding="utf-8")).fieldnames)
    else:
        csv_fh = open(args.csv, "w", newline="") if args.csv else None
        csv_w = csv.DictWriter(csv_fh, fieldnames=["model", "idx", "question", "full", "scx",
                                                    "full_ans", "scx_ans",
                                                    "gold", "full_chars", "scx_chars", "compressed"])
        if csv_w:
            csv_w.writeheader()
            csv_fh.flush()

    def run_one(model: str, idx: int, item: dict) -> dict:
        q = item["question"]
        gold = item["answer"]
        full_prompt = build_prompt(q)
        scx_out, compressed = scx_compress_prompt(full_prompt, qhint, args.scx_bin, args.scx_extra)
        ans_full = api_call(full_prompt, model)
        ans_scx = api_call(scx_out, model)
        row = {
            "model": model, "idx": idx, "question": q[:80],
            "full": check(ans_full, gold), "scx": check(ans_scx, gold),
            "full_ans": ans_full[:200], "scx_ans": ans_scx[:200],
            "gold": gold[:60], "full_chars": len(full_prompt), "scx_chars": len(scx_out),
            "compressed": compressed,
        }
        if csv_w is not None:
            csv_w.writerow(row)
            csv_fh.flush()
        if (idx + 1) % 10 == 0 or idx == 0:
            print(f"  [{idx+1}/{len(qa)}] {model}: full={row['full']} scx={row['scx']} "
                  f"cut={(1 - len(scx_out) / len(full_prompt)) * 100:.1f}%", flush=True)
        return row

    for model in models:
        todo = [(model, i, it) for i, it in enumerate(qa) if (model, str(i)) not in done]
        if not todo:
            print(f"  {model}: 全部已跑（--resume 跳过）")
            continue
        with ThreadPoolExecutor(max_workers=args.workers) as ex:
            for row in ex.map(lambda t: run_one(*t), todo):
                rows.append(row)

    if csv_fh is not None:
        csv_fh.close()

    for model in models:
        sub = [r for r in rows if r["model"] == model]
        n = len(sub)
        if n == 0:
            continue
        full_p = sum(1 for r in sub if r["full"])
        scx_p = sum(1 for r in sub if r["scx"])
        kept = sum(1 for r in sub if r["full"] and r["scx"])
        chars = [(r["full_chars"], r["scx_chars"]) for r in sub if r.get("full_chars")]
        cut = 100 * (1 - sum(c[1] for c in chars) / sum(c[0] for c in chars)) if chars else 0
        nc = sum(1 for r in sub if not r["compressed"])
        print(f"\n=== {model} | {Path(args.data).name} {n} 题 ===")
        print(f"full : acc={full_p / n:.3f} ({full_p}/{n})")
        print(f"scx  : acc={scx_p / n:.3f} ({scx_p}/{n})  cut={cut:.1f}%（字符口径）")
        print(f"保持率（full 答对中 scx 仍答对）: {kept}/{max(full_p,1)} = {kept / max(full_p, 1):.3f}")
        if nc:
            print(f"注：{nc}/{n} 题未压缩（fail-open）")


if __name__ == "__main__":
    main()
