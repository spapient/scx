#!/usr/bin/env python3
"""#66 端到端问答保真评测（对标 LLMLingua-2 的「压缩→问答」协议）。

协议：长代码 prompt（真实工程文件 + 问题）分别以 原版 / scx 压缩 / scx+semantic
压缩 交给双在线旗舰作答，与参考答案判定一致，比较 原版 acc / 压缩 acc /
语义压缩 acc，并报告 token 压缩率与保真率（压缩 acc / 原版 acc）。
- 数据：bench_long_*（7 个真实工程文件 + 91 题代码事实问答，gold 经真实工具链确认）
- 压缩：scx --query <单词语> --ratio 0.35；comp_sem 模式追加 --semantic weights.json
- query 一律单词，保持与蒸馏训练（单 term 特征）口径一致
"""
from __future__ import annotations
import argparse, csv, json, os, re, subprocess, sys, tempfile, time
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

ROOT = Path("/Users/fei/code/scx")
sys.path.insert(0, str(ROOT / "tools"))
from bench_code import scx_compress, check  # noqa: E402


FENCE = {"rust_cache": "rust", "scx_trim": "rust", "c_pool": "c", "c_sched": "c",
         "py_config": "python", "cpp_matrix": "cpp", "java_queue": "java"}
EXT = {"rust_cache": "rs", "scx_trim": "rs", "c_pool": "c", "c_sched": "c",
       "py_config": "py", "cpp_matrix": "cpp", "java_queue": "java"}

_KEYS: dict[str, str] = {}


def _key(model: str) -> str:
    """按需取 key：deepseek 走 env（~/.zshrc source），qwen 走 keychain；不落盘。"""
    if model not in _KEYS:
        if model == "deepseek":
            k = os.environ.get("DEEPSEEK_API_KEY")
            if not k:
                out = subprocess.run(["/bin/zsh", "-lc", "source ~/.zshrc && echo -n $DEEPSEEK_API_KEY"],
                                     capture_output=True, text=True, timeout=30)
                k = out.stdout.strip()
            _KEYS[model] = k or ""
        else:
            out = subprocess.run(["security", "find-generic-password", "-s", "scx-qwen-api", "-w"],
                                 capture_output=True, text=True, timeout=30)
            _KEYS[model] = out.stdout.strip() or ""
    return _KEYS[model]


def _call(model: str, prompt: str) -> str:
    """curl -4 硬超时调用（规避 macOS urllib TLS 冻结）；失败重试 2 次后返回空串。"""
    url = ("https://api.deepseek.com/chat/completions" if model == "deepseek"
           else "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions")
    mdl = "deepseek-chat" if model == "deepseek" else "qwen3.8-flash"
    body = json.dumps({"model": mdl, "messages": [{"role": "user", "content": prompt}],
                       "max_tokens": 512, "temperature": 0.1})
    key = _key(model)
    if not key:
        return ""
    for _ in range(3):
        try:
            out = subprocess.run(
                ["curl", "-4", "-sS", "--max-time", "60", "-X", "POST", url,
                 "-H", f"Authorization: Bearer {key}", "-H", "Content-Type: application/json",
                 "-d", body],
                capture_output=True, text=True, timeout=90)
            if out.returncode != 0:
                time.sleep(2)
                continue
            r = json.loads(out.stdout)
            return r["choices"][0]["message"]["content"] or ""
        except Exception:
            time.sleep(2)
    return ""


def build_prompt(code: str, q: str, fence: str) -> str:
    return (
        "以下是一段真实工程代码，请阅读代码后回答后面的问题。\n\n"
        f"```{fence}\n{code}\n```\n\n"
        f"问题：{q}\n"
        "回答（只输出答案）："
    )


def _tokens(text: str) -> int:
    """粗 token 估计（英文/代码 1 token≈4 字符；CJK 1 token≈1.5 字符）。"""
    ascii_n = len(re.findall(r"[ -~]", text))
    cjk_n = len(text) - ascii_n
    return ascii_n // 4 + cjk_n


def _top_idents(code: str, k: int = 4) -> list[str]:
    """主文件高频标识符（非停用、非纯数字、长度≥2）：query 附加词，命中更多证据行。"""
    words = re.findall(r"[A-Za-z_][A-Za-z0-9_]{1,}", code)
    stop = {"the", "and", "for", "with", "from", "use", "let", "mut", "fn", "pub",
            "struct", "impl", "enum", "trait", "return", "if", "else", "match",
            "while", "loop", "self", "int", "char", "void", "static", "const",
            "this", "new", "null", "true", "false", "std", "main", "print"}
    cnt: dict[str, int] = {}
    for w in words:
        if w not in stop and not w.isdigit():
            cnt[w] = cnt.get(w, 0) + 1
    return [w for w, _ in sorted(cnt.items(), key=lambda kv: -kv[1])[:k]]


def load_prompts(units_dir: Path, qa: list[dict]) -> list[tuple[str, str, str]]:
    """每题 = 主 unit 文件 + 2 个无关工程文件拼接（保证 >BUDGET_MIN/ratio ≈ 571 tokens）。
    query = 问题文本 + 主文件高频标识符（证据行命中保护 + 语义裁剪，对标 LLMLingua-2 中档压缩）。
    返回 (prompt, query, gold)。"""
    names = list(EXT)
    out = []
    for i, r in enumerate(qa):
        unit = r["unit"]
        code = (Path(units_dir) / f"{unit}.{EXT[unit]}").read_text(encoding="utf-8")
        others = [n for n in names if n != unit]
        for j in range(2):
            extra = others[(i + j * 3) % len(others)]
            code += "\n\n" + (Path(units_dir) / f"{extra}.{EXT[extra]}").read_text(encoding="utf-8")
        prompt = build_prompt(code, r["question"], FENCE[unit])
        idents = " ".join(_top_idents(code))
        query = f"{r['question']} {idents}".strip()
        out.append((prompt, query, r["answer"]))
    return out


def _compress(scx_bin: str, prompt: str, query: str, semantic: bool = False) -> str:
    """直连 CLI 压缩（包装 scx_compress 内置 ratio 0.35 且拒绝重复 --ratio，故不走包装）。
    温和保真档：ratio 0.5 + fill-floor 2 + 证据行保护；comp_sem 轨附加 --semantic。
    每调用独立临时文件（6 workers 并发安全）。"""
    fd, path = tempfile.mkstemp(suffix=".txt")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            fh.write(prompt)
        extra = ["--trim", "1", "--trim-ratio", "0.3", "--zh-keep", "1", "--fill-floor", "2"]
        if semantic:
            extra += ["--semantic", str(ROOT / "config" / "semantic" / "weights.json")]
        proc = subprocess.run(
            [scx_bin, path, "--query", query,
             "--ratio", "0.5", "--context", "1", *extra],
            capture_output=True, text=True, timeout=120)
        if proc.returncode != 0 or not proc.stdout.strip():
            return prompt
        lines = proc.stdout.splitlines()
        cut = 0
        for k, ln in enumerate(lines):
            if ln.lstrip().startswith("compressed:"):
                cut = k
                break
        else:
            return prompt
        out = "\n".join(lines[:cut])
        return out if out.strip() else prompt
    except Exception:
        return prompt
    finally:
        try:
            os.unlink(path)
        except OSError:
            pass


def _one(idx: int, prompt: str, q: str, gold: str, scx_bin: str) -> dict:
    comp = _compress(scx_bin, prompt, q)
    compressed = comp != prompt
    ok_ds_orig = check(_call("deepseek", prompt), gold)
    ok_qw_orig = check(_call("qwen", prompt), gold)
    ok_ds_comp = check(_call("deepseek", comp), gold) if compressed else False
    ok_qw_comp = check(_call("qwen", comp), gold) if compressed else False
    ok_ds_sem = ok_qw_sem = False
    sem_path = ROOT / "config" / "semantic" / "weights.json"
    if compressed and sem_path.exists():
        comp_sem = _compress(scx_bin, prompt, q, semantic=True)
        if comp_sem != prompt:
            outs = [_call("deepseek", comp_sem), _call("qwen", comp_sem)]
            ok_ds_sem, ok_qw_sem = [bool(o and check(o, gold)) for o in outs]
    return {
        "idx": idx,
        "compressed": compressed,
        "ok_ds_orig": ok_ds_orig,
        "ok_ds_comp": ok_ds_comp,
        "ok_ds_sem": ok_ds_sem,
        "ok_qw_orig": ok_qw_orig,
        "ok_qw_comp": ok_qw_comp,
        "ok_qw_sem": ok_qw_sem,
        "src_tokens": _tokens(prompt),
        "comp_tokens": _tokens(comp),
    }


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--units", default=str(ROOT / "bench_long"))
    ap.add_argument("--scx-bin", default="target/release/scx")
    ap.add_argument("--sem", default=str(ROOT / "config/semantic/weights.json"))
    ap.add_argument("--workers", type=int, default=6)
    ap.add_argument("--csv", default="", help="逐题明细导出")
    args = ap.parse_args()

    qa = []
    for f in ("bench_long_qa.csv", "bench_long_cx.csv", "bench_long_nl30.csv",
              "bench_long_ck30.csv", "bench_long_q30.csv"):
        for r in csv.DictReader(open(ROOT / "config/bench/datasets" / f, encoding="utf-8")):
            gold = r.get("answer") or r.get("gold") or r.get("scx")
            if r.get("unit") and r.get("question") and gold:
                qa.append({"unit": r["unit"], "question": r["question"], "answer": gold})
    qa = qa[:60]
    sem_path = Path(args.sem)
    have_sem = sem_path.exists()
    prompts = load_prompts(Path(args.units), qa)
    print(f"== 端到端保真（bench_long 长代码拼接）：{len(qa)} 题 | query=问题+高频标识符 ratio=0.5 "
          f"sem={'有' if have_sem else '无'} ==")

    rows = []
    t0 = time.time()
    with ThreadPoolExecutor(max_workers=args.workers) as ex:
        futs = []
        for i, (prompt, q, gold) in enumerate(prompts):
            futs.append(ex.submit(_one, i, prompt, q, gold, args.scx_bin))
        for f in as_completed(futs):
            rows.append(f.result())
            done = len(rows)
            if done % 10 == 0:
                el = time.time() - t0
                print(f"  进度 {done}/{len(qa)} ({el / done:.1f}s/题)", flush=True)
    rows.sort(key=lambda r: r["idx"])

    agg: dict[str, dict] = {}
    tok_src = tok_comp = 0
    for r in rows:
        for m, pfx in (("deepseek", "ds"), ("qwen", "qw")):
            for mode in ("orig", "comp", "comp_sem"):
                key = f"{mode}|{m}"
                agg.setdefault(key, {"ok": 0, "n": 0})
                agg[key]["n"] += 1
                if r.get(f"ok_{pfx}_{mode}"):
                    agg[key]["ok"] += 1
        if r["compressed"]:
            tok_src += r["src_tokens"]
            tok_comp += r["comp_tokens"]

    def acc(key):
        a = agg[key]
        return a["ok"] / a["n"] if a["n"] else 0.0

    n_comp = sum(1 for r in rows if r["compressed"])
    print(f"\n压缩生效样本：{n_comp}/{len(rows)}")
    print("=== 汇总 ===")
    for m in ("deepseek", "qwen"):
        a_o = acc(f"orig|{m}")
        a_c = acc(f"comp|{m}")
        a_s = acc(f"comp_sem|{m}")
        print(f"{m:>8}  orig {agg[f'orig|{m}']['ok']}/{agg[f'orig|{m}']['n']}={a_o:.3f} | "
              f"comp {agg[f'comp|{m}']['ok']}/{agg[f'comp|{m}']['n']}={a_c:.3f} "
              f"(保真 {a_c / a_o if a_o else 0:.2f}) | "
              f"comp_sem {agg[f'comp_sem|{m}']['ok']}/{agg[f'comp_sem|{m}']['n']}={a_s:.3f} "
              f"(保真 {a_s / a_o if a_o else 0:.2f})")
    if tok_src:
        print(f"\ntoken 压缩率（压缩样本 {n_comp} 题）: {tok_comp}/{tok_src} = {tok_comp / tok_src:.2%}")
    if args.csv:
        with open(args.csv, "w", encoding="utf-8", newline="") as fh:
            w = csv.DictWriter(fh, fieldnames=["idx", "compressed", "ok_ds_orig", "ok_ds_comp",
                                               "ok_ds_sem", "ok_qw_orig", "ok_qw_comp", "ok_qw_sem",
                                               "src_tokens", "comp_tokens"])
            w.writeheader()
            w.writerows(rows)
        print(f"明细已写入 {args.csv}")
    return 0


if __name__ == "__main__":
    main()
