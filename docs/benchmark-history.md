# Benchmark 历史日志

由历史 benchmark 运行日志合并归档；原始分散文件从 git 历史恢复。

## bench_ablation_fillfloor.log

=== default (fill-floor 0) ===
full : acc=0.600 (30/50)  ctx_tok=1332
scx  : acc=0.540 (27/50)  ctx_tok=1242  tok_ratio=0.932
trunc: acc=0.080 (4/50)  ctx_tok=同 scx
scx 保持率（full 答对中 scx 仍答对）: 25/30 = 0.833
压缩率分桶（cut = 1 - scx_tok/full_tok）:
  cut<2%: 16 样本
  2-10%: 20 样本
  10-35%: 14 样本
注：1/50 样本 scx 未压缩（透传/失败），已按 fail-open 计
=== fill-floor 2 ===
full : acc=0.600 (30/50)  ctx_tok=1332
scx  : acc=0.420 (21/50)  ctx_tok=1122  tok_ratio=0.843
trunc: acc=0.020 (1/50)  ctx_tok=同 scx
scx 保持率（full 答对中 scx 仍答对）: 20/30 = 0.667
压缩率分桶（cut = 1 - scx_tok/full_tok）:
  cut<2%: 1 样本
  2-10%: 18 样本
  10-35%: 29 样本
=== fill-floor 4 ===
full : acc=0.600 (30/50)  ctx_tok=1332
scx  : acc=0.400 (20/50)  ctx_tok=993  tok_ratio=0.746
trunc: acc=0.000 (0/50)  ctx_tok=同 scx
scx 保持率（full 答对中 scx 仍答对）: 18/30 = 0.600
压缩率分桶（cut = 1 - scx_tok/full_tok）:
  2-10%: 3 样本
  10-35%: 38 样本

## bench_arch_both_run1.log

[load] Qwen2.5-3B-Instruct（千问） ...
[INFO] Quantized model with 8.501 bits per weight.
[load] 8-bit 量化完成（Qwen2.5-3B-Instruct（千问））
[gen 1/10] rust_arch    cut=37% 24s
[gen 2/10] perf_opt     cut=40% 24s
[gen 3/10] gpu_tiling   cut=33% 24s
[gen 4/10] kv_cache     cut=40% 24s
[gen 5/10] scheduler    cut=38% 19s
[gen 6/10] allocator    cut=39% 24s
[gen 7/10] code_review  cut=36% 27s
[gen 8/10] log_sys      cut=39% 39s
[gen 9/10] test_strategy cut=17% 47s
[gen 10/10] kv_batch     cut=39% 33s
[judge] qwen     0/full: 2.5
[judge] qwen     0/scx: 3.0
[judge] deepseek 0/full: 2.0
[judge] qwen     1/full: 2.0
[judge] deepseek 0/scx: 2.0
[judge] qwen     1/scx: 2.5
[judge] deepseek 1/full: 2.0
[judge] qwen     2/full: 2.5
[judge] deepseek 2/full: 2.0
[judge] qwen     2/scx: 2.5
[judge] deepseek 2/scx: 2.5
[judge] deepseek 1/scx: 3.0
[judge] qwen     3/full: 3.0
[judge] deepseek 3/full: 2.0
[judge] qwen     3/scx: 2.5
[judge] qwen     4/full: 3.0
[judge] deepseek 3/scx: 2.0
[judge] qwen     4/scx: 2.5
[judge] deepseek 4/full: 2.5
[judge] qwen     5/full: 2.5
[judge] deepseek 4/scx: 1.5
[judge] qwen     5/scx: 2.5
[judge] deepseek 5/full: 2.0
[judge] qwen     6/full: 3.0
[judge] deepseek 5/scx: 3.0
[judge] qwen     6/scx: 3.0
[judge] deepseek 6/full: 1.0
[judge] qwen     7/full: 2.5
[judge] deepseek 6/scx: 3.0
[judge] deepseek 7/full: 2.0
[judge] qwen     7/scx: 2.5
[judge] qwen     8/full: 2.5
[judge] deepseek 8/full: 1.5
[judge] deepseek 7/scx: 2.5
[judge] qwen     8/scx: 2.5
[judge] deepseek 8/scx: 2.0
[judge] qwen     9/full: 2.5
[judge] qwen     9/scx: 2.5
[judge] deepseek 9/scx: 1.5
[judge] deepseek 9/full: 2.0
[rejudge] deepseek 5/full max-delta=0.5
[rejudge] deepseek 5/scx max-delta=0.5
[rejudge] deepseek 2/full max-delta=0.5
[rejudge] deepseek 2/scx max-delta=0.5

=== P6n 架构文档 rubric 评测（Qwen2.5-3B-Instruct（千问） 生成本地 / judge=DeepSeek-V4.1-Flash 在线, 10 题, ('--trim', '1', '--trim-ratio', '0.3', '--structure-anchors', 'md', '--sym-line-keep', '1')）===
维度均分（0-5）：
维度           full    scx  scx/full
structure    1.90   2.30     1.21x
accuracy     0.80   1.05     1.31x
tradeoff     0.55   0.80     1.45x
feasibility   0.65   0.70     1.08x
clarity      1.40   1.70     1.21x
TOTAL        1.06   1.31     1.24x
保持率（scx 总分 / full 总分）: 1.31 / 1.06 = 1.236

=== P6n 架构文档 rubric 评测（Qwen2.5-3B-Instruct（千问） 生成本地 / judge=Qwen-flash 在线, 10 题, ('--trim', '1', '--trim-ratio', '0.3', '--structure-anchors', 'md', '--sym-line-keep', '1')）===
维度均分（0-5）：
维度           full    scx  scx/full
structure    2.60   2.60     1.00x
accuracy     2.20   2.20     1.00x
tradeoff     1.90   1.95     1.03x
feasibility   2.20   1.95     0.89x
clarity      3.05   3.10     1.02x
TOTAL        2.39   2.36     0.99x
保持率（scx 总分 / full 总分）: 2.36 / 2.39 = 0.987
压缩率（均值）: 35.7%

=== 双 judge 交叉一致性（同答案两模型打分差）===
维度             均值差     最大差
structure     0.60     2.0
accuracy      1.27     3.0
tradeoff      1.25     2.0
feasibility    1.40     2.5
clarity       1.52     3.0
综合            1.21     3.0

单 judge 复打一致性（4 次, max 分差）: 均值 0.50 / 最大 0.5
[csv] bench_arch_both_v1.csv

## bench_arch_both_run2.log

[load] Qwen2.5-3B-Instruct（千问） ...
[INFO] Quantized model with 8.501 bits per weight.
[load] 8-bit 量化完成（Qwen2.5-3B-Instruct（千问））
[gen 1/10] rust_arch    cut=37% 24s
[gen 2/10] perf_opt     cut=40% 24s
[gen 3/10] gpu_tiling   cut=33% 24s
[gen 4/10] kv_cache     cut=40% 24s
[gen 5/10] scheduler    cut=38% 19s
[gen 6/10] allocator    cut=39% 24s
[gen 7/10] code_review  cut=36% 25s
[gen 8/10] log_sys      cut=39% 29s
[gen 9/10] test_strategy cut=17% 36s
[gen 10/10] kv_batch     cut=39% 32s
[judge] deepseek 0/scx: 2.0
[judge] deepseek 0/full: 2.0
[judge] deepseek 1/full: 1.5
[judge] qwen     0/full: 2.5
[judge] qwen     0/scx: 2.5
[judge] qwen     1/full: 1.0
[judge] qwen     1/scx: 2.5
[judge] deepseek 1/scx: 2.5
[judge] deepseek 2/full: 2.5
[judge] deepseek 2/scx: 1.5
[judge] deepseek 3/full: 2.0
[judge] qwen     2/full: 1.0
[judge] qwen     2/scx: 1.5
[judge] deepseek 3/scx: 1.5
[judge] qwen     3/full: 2.0
[judge] deepseek 4/full: 2.0
[judge] qwen     3/scx: 1.5
[judge] deepseek 4/scx: 3.0
[judge] qwen     4/full: 2.0
[judge] deepseek 5/full: 2.0
[judge] qwen     5/full: 1.5
[judge] deepseek 5/scx: 2.0
[judge] qwen     4/scx: 3.5
[judge] deepseek 6/full: 2.0
[judge] qwen     5/scx: 2.0
[judge] qwen     6/full: 2.0
[judge] deepseek 7/full: 2.5
[judge] qwen     6/scx: 2.0
[judge] deepseek 6/scx: PARSE-FAIL
[judge] deepseek 7/scx: 2.0
[judge] qwen     7/full: 2.0
[judge] deepseek 8/full: 1.5
[judge] qwen     7/scx: 2.0
[judge] qwen     8/full: 1.5
[judge] qwen     8/scx: 2.0
[judge] deepseek 8/scx: 2.0
[judge] deepseek 9/full: 2.0
[judge] deepseek 9/scx: 2.0
[judge] qwen     9/scx: 1.5
[judge] qwen     9/full: 1.0
[rejudge] deepseek 5/full max-delta=0.5
[rejudge] deepseek 5/scx max-delta=0.5
[rejudge] deepseek 2/full max-delta=0.5
[rejudge] deepseek 2/scx max-delta=1.0

=== P6n 架构文档 rubric 评测（Qwen2.5-3B-Instruct（千问） 生成本地 / judge=DeepSeek-V4.1-Flash 在线, 10 题, ('--trim', '1', '--trim-ratio', '0.3', '--structure-anchors', 'md', '--sym-line-keep', '1')）===
维度均分（0-5）：
维度           full    scx  scx/full
structure    2.00   2.06     1.03x
accuracy     0.90   1.06     1.17x
tradeoff     0.75   0.67     0.89x
feasibility   0.60   0.72     1.20x
clarity      1.55   1.61     1.04x
TOTAL        1.16   1.22     0.95x
保持率（scx 总分 / full 总分）: 1.22 / 1.16 = 1.054

=== P6n 架构文档 rubric 评测（Qwen2.5-3B-Instruct（千问） 生成本地 / judge=Qwen-flash 在线, 10 题, ('--trim', '1', '--trim-ratio', '0.3', '--structure-anchors', 'md', '--sym-line-keep', '1')）===
维度均分（0-5）：
维度           full    scx  scx/full
structure    1.65   2.10     1.27x
accuracy     1.00   1.20     1.20x
tradeoff     0.85   0.85     1.00x
feasibility   0.85   0.80     0.94x
clarity      1.75   2.05     1.17x
TOTAL        1.22   1.40     1.15x
保持率（scx 总分 / full 总分）: 1.40 / 1.22 = 1.148
压缩率（均值）: 35.7%

=== 双 judge 交叉一致性（同答案两模型打分差）===
维度             均值差     最大差
structure     0.32     1.5
accuracy      0.29     1.0
tradeoff      0.24     0.5
feasibility    0.26     1.0
clarity       0.37     1.0
综合            0.29     1.5

单 judge 复打一致性（4 次, max 分差）: 均值 0.62 / 最大 1.0
[csv] bench_arch_both_v2.csv

## bench_arch_run1.log

[load] Qwen2.5-3B-Instruct（千问） ...
[INFO] Quantized model with 8.501 bits per weight.
[load] 8-bit 量化完成（Qwen2.5-3B-Instruct（千问））
[gen 1/10] rust_arch    cut=37% 24s
[gen 2/10] perf_opt     cut=40% 25s
[gen 3/10] gpu_tiling   cut=33% 25s
[gen 4/10] kv_cache     cut=40% 26s
[gen 5/10] scheduler    cut=38% 21s
[gen 6/10] allocator    cut=39% 24s
[gen 7/10] code_review  cut=36% 26s
[gen 8/10] log_sys      cut=39% 29s
[gen 9/10] test_strategy cut=17% 29s
[gen 10/10] kv_batch     cut=39% 32s
[judge] 2/full: PARSE-FAIL
[judge] 0/full: PARSE-FAIL
[judge] 1/full: PARSE-FAIL
[judge] 4/full: PARSE-FAIL
[judge] 3/full: PARSE-FAIL
[judge] 5/full: PARSE-FAIL
[judge] 7/full: PARSE-FAIL
[judge] 6/full: PARSE-FAIL
[judge] 8/full: PARSE-FAIL
[judge] 0/scx: PARSE-FAIL
[judge] 1/scx: PARSE-FAIL
[judge] 9/full: PARSE-FAIL
[judge] 2/scx: PARSE-FAIL
[judge] 4/scx: PARSE-FAIL
[judge] 3/scx: PARSE-FAIL
[judge] 5/scx: PARSE-FAIL
[judge] 6/scx: PARSE-FAIL
[judge] 7/scx: PARSE-FAIL
[judge] 8/scx: PARSE-FAIL
[judge] 9/scx: PARSE-FAIL

=== P6n 架构文档 rubric 评测（Qwen2.5-3B-Instruct（千问） 生成本地 / judge=DeepSeek-V4.1-Flash 在线, 10 题, ('--trim', '1', '--trim-ratio', '0.3', '--structure-anchors', 'md', '--sym-line-keep', '1')）===
维度均分（0-5）：
维度           full    scx  scx/full
structure    0.00   0.00     0.00x
accuracy     0.00   0.00     0.00x
tradeoff     0.00   0.00     0.00x
feasibility   0.00   0.00     0.00x
clarity      0.00   0.00     0.00x
Traceback (most recent call last):
  File "/Users/fei/code/scx/tools/bench_arch.py", line 244, in <module>
    main()
  File "/Users/fei/code/scx/tools/bench_arch.py", line 221, in main
    print(f"{'TOTAL':10s} {ta/n_full:6.2f} {tb/n_scx:6.2f} {tb/ta if ta else 0:8.2f}x")
                           ~~^~~~~~~
ZeroDivisionError: division by zero

## bench_arch_run2.log

[load] Qwen2.5-3B-Instruct（千问） ...
[INFO] Quantized model with 8.501 bits per weight.
[load] 8-bit 量化完成（Qwen2.5-3B-Instruct（千问））
[gen 1/10] rust_arch    cut=37% 25s
[gen 2/10] perf_opt     cut=40% 25s

## bench_arch_v3_run.log

[load] Qwen2.5-3B-Instruct（千问） ...
[INFO] Quantized model with 8.501 bits per weight.
[load] 8-bit 量化完成（Qwen2.5-3B-Instruct（千问））
[gen 1/20] rust_arch      cut=0% 42s
[gen 2/20] perf_opt       cut=0% 48s
[gen 3/20] gpu_tiling     cut=0% 48s
[gen 4/20] kv_cache       cut=0% 48s
[gen 5/20] scheduler      cut=0% 32s
[gen 6/20] allocator      cut=0% 52s
[gen 7/20] code_review    cut=0% 51s
[gen 8/20] log_sys        cut=0% 48s
[gen 9/20] test_strategy  cut=0% 48s
[gen 10/20] kv_batch       cut=0% 48s
[gen 11/20] cache_coherence cut=0% 48s
[gen 12/20] consensus      cut=0% 48s
[gen 13/20] compiler_ir    cut=0% 48s
[gen 14/20] net_stack      cut=0% 42s
[gen 15/20] gpu_mempool    cut=0% 48s
[gen 16/20] err_arch       cut=0% 48s
[gen 17/20] hashmap        cut=0% 47s
[gen 18/20] build_sys      cut=0% 47s
[gen 19/20] lsm_store      cut=0% 45s
[gen 20/20] observability  cut=0% 48s
[judge] deepseek 0/full@0.3: 2.0
[judge] deepseek 0/full@0.2: 2.0
[judge] qwen     0/full@0.3: 2.0
[judge] deepseek 0/full@0.4: 2.5
[judge] qwen     0/full@0.2: 2.0
[judge] deepseek 0/scx@0.2: 2.5
[judge] deepseek 0/scx@0.3: 2.0
[judge] qwen     0/full@0.4: 2.0
[judge] deepseek 0/scx@0.4: 2.0
[judge] qwen     0/scx@0.2: 2.0
[judge] deepseek 1/full@0.2: 2.0
[judge] qwen     0/scx@0.3: 2.5
[judge] qwen     0/scx@0.4: 2.0
[judge] deepseek 1/full@0.3: 2.0
[judge] qwen     1/full@0.3: 2.0
[judge] qwen     1/full@0.2: 2.0
[judge] deepseek 1/full@0.4: 2.5
[judge] deepseek 1/scx@0.2: 2.5
[judge] deepseek 1/scx@0.3: 2.0
[judge] qwen     1/full@0.4: 2.0
[judge] qwen     1/scx@0.2: 1.0
[judge] deepseek 1/scx@0.4: 2.5
[judge] qwen     1/scx@0.3: 2.0
[judge] qwen     1/scx@0.4: 1.5
[judge] deepseek 2/full@0.2: 2.0
[judge] deepseek 2/full@0.3: 2.0
[judge] qwen     2/full@0.3: 2.0
[judge] qwen     2/full@0.2: 1.5
[judge] deepseek 2/full@0.4: 2.0
[judge] deepseek 2/scx@0.2: 1.5
[judge] qwen     2/full@0.4: 1.0
[judge] deepseek 2/scx@0.3: 2.0
[judge] deepseek 2/scx@0.4: 2.0
[judge] qwen     2/scx@0.3: 1.5
[judge] deepseek 3/full@0.2: 2.0
[judge] qwen     2/scx@0.2: 1.0
[judge] deepseek 3/full@0.3: 2.5
[judge] qwen     2/scx@0.4: 1.0
[judge] qwen     3/full@0.2: 2.0
[judge] deepseek 3/full@0.4: 2.0
[judge] deepseek 3/scx@0.2: 2.0
[judge] qwen     3/full@0.3: 1.5
[judge] qwen     3/full@0.4: 2.0
[judge] deepseek 3/scx@0.3: 3.0
[judge] qwen     3/scx@0.2: 2.0
[judge] deepseek 3/scx@0.4: 1.5
[judge] qwen     3/scx@0.3: 1.5
[judge] deepseek 4/full@0.2: 2.0
[judge] deepseek 4/full@0.3: 2.0
[judge] qwen     3/scx@0.4: 2.0
[judge] deepseek 4/full@0.4: 2.0
[judge] qwen     4/full@0.2: 2.0
[judge] qwen     4/full@0.3: 1.5
[judge] deepseek 4/scx@0.2: 2.0
[judge] deepseek 4/scx@0.3: 2.0
[judge] qwen     4/full@0.4: 2.0
[judge] qwen     4/scx@0.2: 2.0
[judge] deepseek 4/scx@0.4: 2.0
[judge] deepseek 5/full@0.2: 2.0
[judge] qwen     4/scx@0.3: 2.5
[judge] qwen     4/scx@0.4: 2.0
[judge] deepseek 5/full@0.3: 2.0
[judge] qwen     5/full@0.2: 1.5
[judge] deepseek 5/full@0.4: 2.0
[judge] qwen     5/full@0.3: 1.5
[judge] deepseek 5/scx@0.2: 2.0
[judge] qwen     5/full@0.4: 1.5
[judge] qwen     5/scx@0.2: 1.0
[judge] deepseek 5/scx@0.3: 2.5
[judge] deepseek 5/scx@0.4: 1.5
[judge] deepseek 6/full@0.2: 1.5
[judge] qwen     5/scx@0.3: 1.0
[judge] qwen     5/scx@0.4: 1.5
[judge] deepseek 6/full@0.3: 2.0
[judge] deepseek 6/full@0.4: 2.0
[judge] qwen     6/full@0.2: 2.0
[judge] qwen     6/full@0.4: 2.0
[judge] qwen     6/full@0.3: 2.0
[judge] deepseek 6/scx@0.2: 1.5
[judge] deepseek 6/scx@0.3: 2.0
[judge] qwen     6/scx@0.2: 1.0
[judge] deepseek 6/scx@0.4: 1.5
[judge] deepseek 7/full@0.2: 2.0
[judge] qwen     6/scx@0.3: 1.5
[judge] qwen     6/scx@0.4: 1.5
[judge] deepseek 7/full@0.3: 1.0
[judge] deepseek 7/full@0.4: 2.0
[judge] qwen     7/full@0.2: 1.5
[judge] deepseek 7/scx@0.2: 2.0
[judge] qwen     7/full@0.3: 1.5
[judge] deepseek 7/scx@0.3: 2.0
[judge] qwen     7/full@0.4: 2.0
[judge] qwen     7/scx@0.2: 1.0
[judge] deepseek 7/scx@0.4: 2.0
[judge] deepseek 8/full@0.2: 1.0
[judge] qwen     7/scx@0.3: 1.5
[judge] deepseek 8/full@0.3: 1.5
[judge] qwen     8/full@0.3: 1.0
[judge] qwen     8/full@0.2: 1.0
[judge] deepseek 8/full@0.4: 1.5
[judge] deepseek 8/scx@0.2: 1.0
[judge] qwen     8/full@0.4: 1.0
[judge] qwen     8/scx@0.2: 1.0
[judge] deepseek 8/scx@0.3: 1.5
[judge] deepseek 8/scx@0.4: 2.0
[judge] qwen     8/scx@0.3: 1.0
[judge] qwen     8/scx@0.4: 1.5
[judge] deepseek 9/full@0.2: 2.0
[judge] deepseek 9/full@0.3: 1.5
[judge] qwen     9/full@0.2: 2.0
[judge] deepseek 9/full@0.4: 1.5
[judge] qwen     7/scx@0.4: 1.5
[judge] deepseek 9/scx@0.2: 2.0
[judge] qwen     9/full@0.3: 1.5
[judge] deepseek 9/scx@0.3: 2.0
[judge] qwen     9/full@0.4: 1.0
[judge] qwen     9/scx@0.2: 2.0
[judge] deepseek 9/scx@0.4: 2.0
[judge] qwen     9/scx@0.3: 1.5
[judge] qwen     9/scx@0.4: 2.0
[judge] deepseek 10/full@0.3: 1.5
[judge] qwen     10/full@0.2: 1.0
[judge] qwen     10/full@0.3: 1.0
[judge] deepseek 10/full@0.4: 2.0
[judge] qwen     10/full@0.4: 1.0
[judge] deepseek 10/scx@0.2: 1.5
[judge] deepseek 10/scx@0.3: 1.0
[judge] qwen     10/scx@0.2: 1.5
[judge] qwen     10/scx@0.3: 1.0
Traceback (most recent call last):
  File "/Users/fei/.local/share/uv/python/cpython-3.12.15-macos-aarch64-none/lib/python3.12/urllib/request.py", line 1357, in do_open
    h.request(req.get_method(), req.selector, req.data, headers,
  File "/Users/fei/.local/share/uv/python/cpython-3.12.15-macos-aarch64-none/lib/python3.12/http/client.py", line 1386, in request
    self._send_request(method, url, body, headers, encode_chunked)
  File "/Users/fei/.local/share/uv/python/cpython-3.12.15-macos-aarch64-none/lib/python3.12/http/client.py", line 1432, in _send_request
    self.endheaders(body, encode_chunked=encode_chunked)
  File "/Users/fei/.local/share/uv/python/cpython-3.12.15-macos-aarch64-none/lib/python3.12/http/client.py", line 1381, in endheaders
    self._send_output(message_body, encode_chunked=encode_chunked)
  File "/Users/fei/.local/share/uv/python/cpython-3.12.15-macos-aarch64-none/lib/python3.12/http/client.py", line 1141, in _send_output
    self.send(msg)
  File "/Users/fei/.local/share/uv/python/cpython-3.12.15-macos-aarch64-none/lib/python3.12/http/client.py", line 1085, in send
    self.connect()
  File "/Users/fei/.local/share/uv/python/cpython-3.12.15-macos-aarch64-none/lib/python3.12/http/client.py", line 1527, in connect
    self.sock = self._context.wrap_socket(self.sock,
                ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/fei/.local/share/uv/python/cpython-3.12.15-macos-aarch64-none/lib/python3.12/ssl.py", line 457, in wrap_socket
    return self.sslsocket_class._create(
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/fei/.local/share/uv/python/cpython-3.12.15-macos-aarch64-none/lib/python3.12/ssl.py", line 1066, in _create
    self.do_handshake()
  File "/Users/fei/.local/share/uv/python/cpython-3.12.15-macos-aarch64-none/lib/python3.12/ssl.py", line 1344, in do_handshake
    self._sslobj.do_handshake()
TimeoutError: [Errno 60] Operation timed out

During handling of the above exception, another exception occurred:

Traceback (most recent call last):
  File "/Users/fei/code/scx/tools/bench_arch.py", line 338, in <module>
    main()
  File "/Users/fei/code/scx/tools/bench_arch.py", line 211, in main
    sc, raw = f.result()
              ^^^^^^^^^^
  File "/Users/fei/.local/share/uv/python/cpython-3.12.15-macos-aarch64-none/lib/python3.12/concurrent/futures/_base.py", line 449, in result
    return self.__get_result()
           ^^^^^^^^^^^^^^^^^^^
  File "/Users/fei/.local/share/uv/python/cpython-3.12.15-macos-aarch64-none/lib/python3.12/concurrent/futures/_base.py", line 401, in __get_result
    raise self._exception
  File "/Users/fei/.local/share/uv/python/cpython-3.12.15-macos-aarch64-none/lib/python3.12/concurrent/futures/thread.py", line 59, in run
    result = self.fn(*self.args, **self.kwargs)
             ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/fei/code/scx/tools/bench_arch.py", line 119, in judge_answer
    raw = api_run(prompt, max_new=cfg["max_new"], judge=judge)
          ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/fei/code/scx/tools/bench_arch.py", line 140, in api_call
    with urllib.request.urlopen(req, timeout=240) as r:
         ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/fei/.local/share/uv/python/cpython-3.12.15-macos-aarch64-none/lib/python3.12/urllib/request.py", line 215, in urlopen
    return opener.open(url, data, timeout)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/fei/.local/share/uv/python/cpython-3.12.15-macos-aarch64-none/lib/python3.12/urllib/request.py", line 515, in open
    response = self._open(req, data)
               ^^^^^^^^^^^^^^^^^^^^^
  File "/Users/fei/.local/share/uv/python/cpython-3.12.15-macos-aarch64-none/lib/python3.12/urllib/request.py", line 532, in _open
    result = self._call_chain(self.handle_open, protocol, protocol +
             ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/fei/.local/share/uv/python/cpython-3.12.15-macos-aarch64-none/lib/python3.12/urllib/request.py", line 492, in _call_chain
    result = func(*args)
             ^^^^^^^^^^^
  File "/Users/fei/.local/share/uv/python/cpython-3.12.15-macos-aarch64-none/lib/python3.12/urllib/request.py", line 1405, in https_open
    return self.do_open(http.client.HTTPSConnection, req,
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "/Users/fei/.local/share/uv/python/cpython-3.12.15-macos-aarch64-none/lib/python3.12/urllib/request.py", line 1360, in do_open
    raise URLError(err)
urllib.error.URLError: <urlopen error [Errno 60] Operation timed out>

## bench_arch_v3_run2.log

[load] Qwen2.5-3B-Instruct（千问） ...
[INFO] Quantized model with 8.501 bits per weight.
[load] 8-bit 量化完成（Qwen2.5-3B-Instruct（千问））
[gen 1/20] rust_arch      cut=0% 42s
[gen 2/20] perf_opt       cut=0% 49s
[gen 3/20] gpu_tiling     cut=0% 49s
[gen 4/20] kv_cache       cut=0% 49s
[gen 5/20] scheduler      cut=0% 32s
[gen 6/20] allocator      cut=0% 50s
[gen 7/20] code_review    cut=0% 51s
[gen 8/20] log_sys        cut=0% 48s
[gen 9/20] test_strategy  cut=0% 49s
[gen 10/20] kv_batch       cut=0% 48s
[gen 11/20] cache_coherence cut=0% 48s
[gen 12/20] consensus      cut=0% 48s
[gen 13/20] compiler_ir    cut=0% 50s
[gen 14/20] net_stack      cut=0% 45s
[gen 15/20] gpu_mempool    cut=0% 50s
[gen 16/20] err_arch       cut=0% 50s
[gen 17/20] hashmap        cut=0% 50s

## bench_arch_v3_run3.log

[load] Qwen2.5-3B-Instruct（千问） ...
[INFO] Quantized model with 8.501 bits per weight.
[load] 8-bit 量化完成（Qwen2.5-3B-Instruct（千问））
[gen 1/20] rust_arch      cut=0% 43s
[gen 2/20] perf_opt       cut=0% 50s

## bench_arch_v3_run4.log

[load] Qwen2.5-3B-Instruct（千问） ...
[INFO] Quantized model with 8.501 bits per weight.
[load] 8-bit 量化完成（Qwen2.5-3B-Instruct（千问））
[gen 1/20] rust_arch      cut=0% 48s
[gen 2/20] perf_opt       cut=0% 66s
[gen 3/20] gpu_tiling     cut=0% 50s
[gen 4/20] kv_cache       cut=0% 49s
[gen 5/20] scheduler      cut=0% 32s
[gen 6/20] allocator      cut=0% 49s
[gen 7/20] code_review    cut=0% 49s
[gen 8/20] log_sys        cut=0% 48s

## bench_arch_v3_run5.log

[load] Qwen2.5-3B-Instruct（千问） ...
[INFO] Quantized model with 8.501 bits per weight.
[load] 8-bit 量化完成（Qwen2.5-3B-Instruct（千问））
[gen 1/20] rust_arch      cut=0% 42s
[gen 2/20] perf_opt       cut=0% 48s
[gen 3/20] gpu_tiling     cut=0% 49s
[gen 4/20] kv_cache       cut=0% 50s
[gen 5/20] scheduler      cut=0% 33s

## bench_arch_v3_run6.log

[load] Qwen2.5-3B-Instruct（千问） ...
[INFO] Quantized model with 8.501 bits per weight.
[load] 8-bit 量化完成（Qwen2.5-3B-Instruct（千问））
[gen 1/20] rust_arch      cut=63% 43s
[gen 2/20] perf_opt       cut=70% 41s
[gen 3/20] gpu_tiling     cut=64% 46s
[gen 4/20] kv_cache       cut=72% 49s
[gen 5/20] scheduler      cut=75% 45s
[gen 6/20] allocator      cut=69% 49s
[gen 7/20] code_review    cut=80% 49s
[gen 8/20] log_sys        cut=66% 36s
[gen 9/20] test_strategy  cut=44% 48s
[gen 10/20] kv_batch       cut=72% 43s
[gen 11/20] cache_coherence cut=60% 48s
[gen 12/20] consensus      cut=62% 45s
[gen 13/20] compiler_ir    cut=63% 47s
[gen 14/20] net_stack      cut=70% 36s
[gen 15/20] gpu_mempool    cut=68% 43s
[gen 16/20] err_arch       cut=61% 48s
[gen 17/20] hashmap        cut=68% 42s
[gen 18/20] build_sys      cut=62% 44s
[gen 19/20] lsm_store      cut=63% 42s
[gen 20/20] observability  cut=64% 40s
[judge] deepseek 0/full@0.2: 2.0
[judge] deepseek 0/full@0.3: 1.5
[judge] qwen     0/full@0.2: 1.5
[judge] deepseek 0/full@0.4: 2.0
[judge] qwen     0/full@0.3: 2.0
[judge] deepseek 0/scx@0.2: 1.0
[judge] qwen     0/full@0.4: 2.0
[judge] deepseek 0/scx@0.3: 1.0
[judge] qwen     0/scx@0.2: 1.0
[judge] deepseek 0/scx@0.4: 1.0
[judge] qwen     0/scx@0.3: 1.5
[judge] deepseek 1/full@0.2: 2.5
[judge] qwen     0/scx@0.4: 2.0
[judge] deepseek 1/full@0.3: 2.5
[judge] qwen     1/full@0.3: 1.5
[judge] deepseek 1/full@0.4: 3.0
[judge] deepseek 1/scx@0.2: 1.5
[judge] qwen     1/full@0.4: 1.5
[judge] deepseek 1/scx@0.3: 2.0
[judge] qwen     1/scx@0.2: 1.5
[judge] deepseek 1/scx@0.4: 1.5
[judge] qwen     1/scx@0.3: 1.0
[judge] deepseek 2/full@0.2: 2.0
[judge] qwen     1/scx@0.4: 1.0
[judge] qwen     1/full@0.2: 1.5
[judge] deepseek 2/full@0.3: 2.0
[judge] qwen     2/full@0.2: 2.0
[judge] qwen     2/full@0.3: 1.5
[judge] deepseek 2/full@0.4: 2.5
[judge] deepseek 2/scx@0.2: 0.5
[judge] deepseek 2/scx@0.3: 1.0
[judge] qwen     2/scx@0.2: 0.5
[judge] qwen     2/scx@0.3: 0.5
[judge] deepseek 2/scx@0.4: 1.5
[judge] qwen     2/full@0.4: 2.0
[judge] qwen     2/scx@0.4: 1.0
[judge] deepseek 3/full@0.2: 2.5
[judge] deepseek 3/full@0.3: 2.5
[judge] qwen     3/full@0.2: 1.5
[judge] deepseek 3/full@0.4: 2.0
[judge] deepseek 3/scx@0.2: 1.0
[judge] qwen     3/full@0.3: 2.0
[judge] qwen     3/full@0.4: 2.5
[judge] qwen     3/scx@0.2: 1.0
[judge] deepseek 3/scx@0.3: 1.0
[judge] deepseek 3/scx@0.4: 1.0
[judge] deepseek 4/full@0.2: 2.0
[judge] qwen     3/scx@0.3: 1.0
[judge] qwen     3/scx@0.4: 1.0
[judge] deepseek 4/full@0.3: 2.0
[judge] qwen     4/full@0.2: 2.0
[judge] qwen     4/full@0.3: 1.5
[judge] deepseek 4/full@0.4: 2.5
[judge] deepseek 4/scx@0.2: 1.5
[judge] qwen     4/scx@0.2: 1.5
[judge] deepseek 4/scx@0.3: 1.0
[judge] qwen     4/full@0.4: 1.5
[judge] deepseek 4/scx@0.4: 1.5
[judge] qwen     4/scx@0.4: 1.0
[judge] qwen     4/scx@0.3: 1.0
[judge] deepseek 5/full@0.2: 2.0
[judge] deepseek 5/full@0.3: 2.0
[judge] deepseek 5/full@0.4: 1.0
[judge] qwen     5/full@0.3: 1.5
[judge] deepseek 5/scx@0.2: 1.0
[judge] qwen     5/full@0.2: 1.0
[judge] deepseek 5/scx@0.3: 1.0
[judge] qwen     5/scx@0.2: 1.0
[judge] qwen     5/scx@0.3: 1.0
[judge] deepseek 5/scx@0.4: 1.0
[judge] qwen     5/full@0.4: 1.5
[judge] deepseek 6/full@0.2: 1.5
[judge] qwen     5/scx@0.4: 1.0
[judge] deepseek 6/full@0.3: 1.5
[judge] deepseek 6/full@0.4: 2.0
[judge] qwen     6/full@0.3: 1.0
[judge] deepseek 6/scx@0.2: 1.0
[judge] qwen     6/full@0.4: 1.5
[judge] deepseek 6/scx@0.3: 1.0
[judge] qwen     6/scx@0.2: 1.0
[judge] qwen     6/scx@0.3: 1.0
[judge] deepseek 6/scx@0.4: 1.0
[judge] deepseek 7/full@0.2: 2.0
[judge] qwen     6/scx@0.4: 1.0
[judge] qwen     7/full@0.2: 2.0
[judge] deepseek 7/full@0.3: 2.0
[judge] deepseek 7/full@0.4: 2.0
[judge] qwen     7/full@0.3: 1.5
[judge] qwen     6/full@0.2: 1.5
[judge] deepseek 7/scx@0.2: 1.0
[judge] qwen     7/full@0.4: 1.5
[judge] qwen     7/scx@0.2: 0.5
[judge] deepseek 7/scx@0.3: 1.0
[judge] deepseek 7/scx@0.4: 1.0
[judge] deepseek 8/full@0.2: 1.5
[judge] qwen     7/scx@0.3: 1.0
[judge] qwen     7/scx@0.4: 1.0
[judge] deepseek 8/full@0.3: 1.5
[judge] qwen     8/full@0.3: 1.0
[judge] deepseek 8/full@0.4: 1.0
[judge] qwen     8/full@0.2: 1.0
[judge] deepseek 8/scx@0.2: 1.0
[judge] qwen     8/full@0.4: 1.5
[judge] qwen     8/scx@0.2: 1.5
[judge] deepseek 8/scx@0.3: 1.5
[judge] deepseek 8/scx@0.4: 2.0
[judge] qwen     8/scx@0.3: 1.0
[judge] qwen     8/scx@0.4: 1.0
[judge] deepseek 9/full@0.2: 2.0
[judge] deepseek 9/full@0.3: 1.5
[judge] deepseek 9/full@0.4: 2.0
[judge] qwen     9/full@0.3: 1.5
[judge] qwen     9/full@0.2: 1.0
[judge] qwen     9/full@0.4: 1.5
[judge] deepseek 9/scx@0.2: 1.0
[judge] deepseek 9/scx@0.3: 1.0
[judge] qwen     9/scx@0.2: 0.5
[judge] qwen     9/scx@0.3: 1.0
[judge] deepseek 9/scx@0.4: 1.5
[judge] deepseek 10/full@0.2: 2.0
[judge] qwen     9/scx@0.4: 0.5
[judge] deepseek 10/full@0.3: 1.5
[judge] qwen     10/full@0.2: 1.0
[judge] deepseek 10/full@0.4: 1.5
[judge] qwen     10/full@0.3: 1.0
[judge] deepseek 10/scx@0.2: 1.0
[judge] qwen     10/scx@0.2: 0.5
[judge] deepseek 10/scx@0.3: 1.0
[judge] deepseek 10/scx@0.4: 1.0
[judge] qwen     10/full@0.4: 1.0
[judge] qwen     10/scx@0.3: 1.0
[judge] deepseek 11/full@0.2: 1.0
[judge] deepseek 11/full@0.3: 1.0
[judge] qwen     10/scx@0.4: 1.0
[judge] deepseek 11/full@0.4: 1.0
[judge] qwen     11/full@0.2: 0.5
[judge] deepseek 11/scx@0.2: 1.0
[judge] qwen     11/full@0.3: 1.0
[judge] qwen     11/full@0.4: 1.0
[judge] deepseek 11/scx@0.3: 1.5
[judge] deepseek 11/scx@0.4: 1.0
[judge] qwen     11/scx@0.2: 1.0
[judge] qwen     11/scx@0.3: 1.0
[judge] deepseek 12/full@0.2: 2.0
[judge] qwen     12/full@0.2: 1.0
[judge] qwen     11/scx@0.4: 1.0
[judge] deepseek 12/full@0.3: 1.5
[judge] deepseek 12/full@0.4: 1.5
[judge] deepseek 12/scx@0.2: 1.0
[judge] qwen     12/full@0.4: 1.0
[judge] qwen     12/full@0.3: 1.0
[judge] deepseek 12/scx@0.3: 0.5
[judge] qwen     12/scx@0.2: 0.5
[judge] deepseek 12/scx@0.4: 1.0
[judge] deepseek 13/full@0.2: 2.5
[judge] qwen     12/scx@0.3: 0.5
[judge] qwen     12/scx@0.4: 1.0
[judge] deepseek 13/full@0.3: 2.0
[judge] qwen     13/full@0.2: 1.0
[judge] deepseek 13/full@0.4: 2.0
[judge] deepseek 13/scx@0.2: 0.5
[judge] qwen     13/full@0.3: 2.0
[judge] qwen     13/scx@0.2: 1.0
[judge] deepseek 13/scx@0.3: 1.0
[judge] deepseek 13/scx@0.4: 1.0
[judge] qwen     13/full@0.4: 2.0
[judge] qwen     13/scx@0.3: 0.5
[judge] deepseek 14/full@0.2: 1.5
[judge] deepseek 14/full@0.3: 2.0
[judge] qwen     13/scx@0.4: 1.0
[judge] qwen     14/full@0.2: 1.0
[judge] deepseek 14/full@0.4: 1.5
[judge] deepseek 14/scx@0.2: 1.0
[judge] qwen     14/full@0.3: 1.0
[judge] qwen     14/full@0.4: 1.0
[judge] deepseek 14/scx@0.3: 1.0
[judge] qwen     14/scx@0.2: 1.0
[judge] qwen     14/scx@0.3: 0.5
[judge] deepseek 14/scx@0.4: 1.0
[judge] deepseek 15/full@0.2: 1.5
[judge] qwen     14/scx@0.4: 1.5
[judge] deepseek 15/full@0.3: 1.5
[judge] qwen     15/full@0.2: 1.0
[judge] deepseek 15/full@0.4: 2.0
[judge] deepseek 15/scx@0.2: 1.0
[judge] qwen     15/full@0.3: 1.0
[judge] qwen     15/full@0.4: 1.0
[judge] deepseek 15/scx@0.3: 1.0
[judge] deepseek 15/scx@0.4: 1.5
[judge] qwen     15/scx@0.2: 1.0
[judge] qwen     15/scx@0.3: 1.0
[judge] qwen     15/scx@0.4: 1.0
[judge] deepseek 16/full@0.2: 2.0
[judge] deepseek 16/full@0.3: 2.0
[judge] qwen     16/full@0.2: 1.5
[judge] qwen     16/full@0.3: 1.0
[judge] deepseek 16/full@0.4: 2.0
[judge] deepseek 16/scx@0.2: 1.0
[judge] deepseek 16/scx@0.3: 1.5
[judge] qwen     16/full@0.4: 0.5
[judge] qwen     16/scx@0.2: 0.5
[judge] deepseek 16/scx@0.4: 1.0
[judge] deepseek 17/full@0.2: 1.5
[judge] qwen     16/scx@0.3: 1.0
[judge] qwen     16/scx@0.4: 1.0
[judge] deepseek 17/full@0.3: 1.5
[judge] qwen     17/full@0.2: 1.0
[judge] deepseek 17/full@0.4: 2.0
[judge] qwen     17/full@0.3: 1.0
[judge] deepseek 17/scx@0.2: 1.0
[judge] qwen     17/full@0.4: 1.0
[judge] deepseek 17/scx@0.3: 1.0
[judge] qwen     17/scx@0.2: 0.5
[judge] deepseek 17/scx@0.4: 1.5
[judge] deepseek 18/full@0.2: 1.5
[judge] qwen     17/scx@0.3: 1.0
[judge] qwen     17/scx@0.4: 1.0
[judge] deepseek 18/full@0.3: 2.0
[judge] qwen     18/full@0.3: 2.0
[judge] qwen     18/full@0.2: 1.5
[judge] deepseek 18/full@0.4: 2.0
[judge] deepseek 18/scx@0.2: 1.0
[judge] deepseek 18/scx@0.3: 1.0
[judge] qwen     18/full@0.4: 2.0
[judge] qwen     18/scx@0.3: 1.0
[judge] deepseek 18/scx@0.4: 1.0
[judge] deepseek 19/full@0.2: 2.0
[judge] qwen     19/full@0.2: 1.0
[judge] qwen     18/scx@0.4: 0.5
[judge] deepseek 19/full@0.3: 2.0
[judge] deepseek 19/full@0.4: 1.5
[judge] qwen     19/full@0.3: 1.0
[judge] deepseek 19/scx@0.2: 1.5
[judge] qwen     19/full@0.4: 1.0
[judge] deepseek 19/scx@0.3: 1.0
[judge] qwen     19/scx@0.2: 1.0
[judge] qwen     18/scx@0.2: 1.0
[judge] deepseek 19/scx@0.4: 1.0
[judge] qwen     19/scx@0.3: 1.0
[judge] qwen     19/scx@0.4: 1.0
[rejudge] deepseek 10/full max-delta=0.5
[rejudge] deepseek 10/scx max-delta=0.5
[rejudge] deepseek 4/full max-delta=0.5
[rejudge] deepseek 4/scx max-delta=0.5

=== P6n 架构文档 rubric 评测（Qwen2.5-3B-Instruct（千问） 生成本地 / judge=DeepSeek-V4.1-Flash 在线, 20 题, ratios=['0.2', '0.3', '0.4']）===
轨                structure  accuracy  tradeoff feasibility  clarity   TOTAL    cut
Traceback (most recent call last):
  File "/Users/fei/code/scx/tools/bench_arch.py", line 406, in <module>
    main()
  File "/Users/fei/code/scx/tools/bench_arch.py", line 328, in main
    a = agg[j][tr][rt][d]
        ~~~~~~~~~~^^^^
KeyError: None

## bench_arch_v3_run7.log

[load] Qwen2.5-3B-Instruct（千问） ...
[INFO] Quantized model with 8.501 bits per weight.
[load] 8-bit 量化完成（Qwen2.5-3B-Instruct（千问））
[resume] 从 bench_arch_plans.json 加载 20 题生成结果
[judge] deepseek 0/full@0.3: 2.0
[judge] deepseek 0/full@0.2: 2.0
[judge] deepseek 0/full@0.4: 2.5
[judge] qwen     0/full@0.2: 2.5
[judge] qwen     0/full@0.3: 2.0
[judge] deepseek 0/scx@0.2: 1.0
[judge] qwen     0/scx@0.2: 1.0
[judge] deepseek 0/scx@0.3: 1.5
[judge] qwen     0/full@0.4: 2.0
[judge] deepseek 0/scx@0.4: 1.5
[judge] qwen     0/scx@0.3: 1.5
[judge] qwen     0/scx@0.4: 1.5
[judge] deepseek 1/full@0.2: 2.0
[judge] qwen     1/full@0.2: 1.0
[judge] deepseek 1/full@0.3: 2.0
[judge] deepseek 1/full@0.4: 2.0
[judge] qwen     1/full@0.3: 1.5
[judge] deepseek 1/scx@0.2: 1.5
[judge] qwen     1/scx@0.2: 1.5
[judge] qwen     1/full@0.4: 1.5
[judge] deepseek 1/scx@0.3: 1.5
[judge] deepseek 1/scx@0.4: 2.0
[judge] qwen     1/scx@0.3: 1.5
[judge] qwen     1/scx@0.4: 1.0
[judge] deepseek 2/full@0.2: 2.0
[judge] qwen     2/full@0.3: 2.0
[judge] deepseek 2/full@0.3: 2.5
[judge] qwen     2/full@0.2: 1.5
[judge] deepseek 2/full@0.4: 2.0
[judge] deepseek 2/scx@0.2: 1.0
[judge] deepseek 2/scx@0.3: 1.0
[judge] qwen     2/full@0.4: 2.0
[judge] qwen     2/scx@0.2: 1.0
[judge] deepseek 2/scx@0.4: 1.0
[judge] qwen     2/scx@0.3: 0.5
[judge] deepseek 3/full@0.2: 2.0
[judge] qwen     2/scx@0.4: 1.0
[judge] deepseek 3/full@0.3: 2.0
[judge] qwen     3/full@0.2: 1.5
[judge] qwen     3/full@0.3: 2.0
[judge] deepseek 3/scx@0.2: 0.5
[judge] deepseek 3/full@0.4: 2.0
[judge] qwen     3/full@0.4: 2.0
[judge] deepseek 3/scx@0.3: 1.0
[judge] deepseek 3/scx@0.4: 1.0
[judge] qwen     3/scx@0.2: 0.5
[judge] qwen     3/scx@0.3: 0.5
[judge] deepseek 4/full@0.2: 3.0
[judge] qwen     3/scx@0.4: 1.0
[judge] deepseek 4/full@0.3: 3.0
[judge] qwen     4/full@0.2: 1.5
[judge] deepseek 4/full@0.4: 2.5
[judge] qwen     4/full@0.3: 2.0
[judge] deepseek 4/scx@0.2: 1.0
[judge] qwen     4/full@0.4: 2.0
[judge] deepseek 4/scx@0.3: 1.0
[judge] qwen     4/scx@0.3: 1.0
[judge] deepseek 4/scx@0.4: 1.0
[judge] deepseek 5/full@0.2: 1.5
[judge] qwen     4/scx@0.4: 1.0
[judge] deepseek 5/full@0.3: 2.0
[judge] qwen     5/full@0.3: 1.5
[judge] qwen     5/full@0.2: 1.5
[judge] deepseek 5/full@0.4: 2.0
[judge] deepseek 5/scx@0.2: 0.5
[judge] qwen     5/full@0.4: 1.5
[judge] qwen     4/scx@0.2: 1.0
[judge] deepseek 5/scx@0.3: 1.0
[judge] qwen     5/scx@0.2: 0.5
[judge] qwen     5/scx@0.3: 1.0
[judge] deepseek 5/scx@0.4: 1.5
[judge] deepseek 6/full@0.2: 1.5
[judge] qwen     5/scx@0.4: 1.0
[judge] qwen     6/full@0.2: 1.5
[judge] deepseek 6/full@0.3: 1.0
[judge] deepseek 6/full@0.4: 2.0
[judge] qwen     6/full@0.3: 1.0
[judge] deepseek 6/scx@0.2: 0.5
[judge] deepseek 6/scx@0.3: 1.0
[judge] qwen     6/scx@0.2: 0.5
[judge] deepseek 6/scx@0.4: 1.0
[judge] qwen     6/scx@0.3: 1.0
[judge] deepseek 7/full@0.2: 2.0
[judge] qwen     6/scx@0.4: 1.0
[judge] deepseek 7/full@0.3: 2.5
[judge] qwen     7/full@0.3: 1.5
[judge] qwen     7/full@0.2: 2.5
[judge] deepseek 7/full@0.4: 2.0
[judge] deepseek 7/scx@0.2: 1.0
[judge] qwen     7/full@0.4: 1.5
[judge] qwen     7/scx@0.2: 0.5
[judge] deepseek 7/scx@0.3: 1.0
[judge] deepseek 7/scx@0.4: 1.0
[judge] qwen     6/full@0.4: 1.5
[judge] qwen     7/scx@0.3: 0.5
[judge] qwen     7/scx@0.4: 1.0
[judge] deepseek 8/full@0.3: 1.5
[judge] deepseek 8/full@0.2: 1.0
[judge] qwen     8/full@0.2: 1.0
[judge] deepseek 8/full@0.4: 1.5
[judge] qwen     8/full@0.3: 1.5
[judge] qwen     8/full@0.4: 1.0
[judge] deepseek 8/scx@0.2: 2.0
[judge] deepseek 8/scx@0.3: 2.0
[judge] deepseek 8/scx@0.4: 1.5
[judge] qwen     8/scx@0.2: 1.0
[judge] qwen     8/scx@0.3: 1.0
[judge] deepseek 9/full@0.2: 1.5
[judge] deepseek 9/full@0.3: 1.5
[judge] qwen     8/scx@0.4: 1.0
[judge] qwen     9/full@0.2: 1.5
[judge] deepseek 9/full@0.4: 2.0
[judge] qwen     9/full@0.3: 1.5
[judge] deepseek 9/scx@0.2: 0.5
[judge] deepseek 9/scx@0.3: 0.5
[judge] qwen     9/scx@0.2: 1.0
[judge] qwen     9/full@0.4: 1.5
[judge] deepseek 9/scx@0.4: 1.0
[judge] qwen     9/scx@0.3: 0.5
[judge] qwen     9/scx@0.4: 1.0
[judge] deepseek 10/full@0.2: 1.5
[judge] deepseek 10/full@0.3: 2.0
[judge] deepseek 10/full@0.4: 1.5
[judge] qwen     10/full@0.2: 1.0
[judge] deepseek 10/scx@0.2: 0.5
[judge] qwen     10/full@0.3: 1.0
[judge] deepseek 10/scx@0.3: 1.0
[judge] qwen     10/full@0.4: 1.5
[judge] qwen     10/scx@0.2: 0.0
[judge] deepseek 10/scx@0.4: 1.5
[judge] qwen     10/scx@0.3: 0.5
[judge] deepseek 11/full@0.2: 1.0
[judge] qwen     10/scx@0.4: 0.5
[judge] deepseek 11/full@0.3: 1.0
[judge] deepseek 11/full@0.4: 1.0
[judge] qwen     11/full@0.3: 1.0
[judge] qwen     11/full@0.2: 1.0
[judge] deepseek 11/scx@0.2: 1.0
[judge] qwen     11/full@0.4: 1.0
[judge] deepseek 11/scx@0.3: 1.5
[judge] qwen     11/scx@0.2: 1.0
[judge] deepseek 11/scx@0.4: 1.5
[judge] deepseek 12/full@0.2: 1.0
[judge] qwen     11/scx@0.4: 1.0
[judge] deepseek 12/full@0.3: 1.5
[judge] qwen     12/full@0.2: 1.5
[judge] qwen     11/scx@0.3: 1.0
[judge] qwen     12/full@0.3: 1.0
[judge] deepseek 12/full@0.4: 1.0
[judge] deepseek 12/scx@0.2: 1.0
[judge] deepseek 12/scx@0.3: 1.0
[judge] qwen     12/scx@0.2: 0.5
[judge] qwen     12/full@0.4: 1.5
[judge] deepseek 12/scx@0.4: 1.0
[judge] qwen     12/scx@0.3: 0.5
[judge] deepseek 13/full@0.2: 2.5
[judge] qwen     12/scx@0.4: 0.5
[judge] deepseek 13/full@0.3: 2.0
[judge] qwen     13/full@0.2: 2.0
[judge] deepseek 13/full@0.4: 2.0
[judge] deepseek 13/scx@0.2: 1.0
[judge] qwen     13/scx@0.2: 0.5
[judge] deepseek 13/scx@0.3: 1.0
[judge] qwen     13/full@0.4: 2.0
[judge] deepseek 13/scx@0.4: 1.0
[judge] qwen     13/full@0.3: 2.0
[judge] qwen     13/scx@0.3: 1.0
[judge] qwen     13/scx@0.4: 0.5
[judge] deepseek 14/full@0.2: 1.0
[judge] deepseek 14/full@0.3: 2.0
[judge] qwen     14/full@0.2: 1.0
[judge] deepseek 14/full@0.4: 1.5
[judge] qwen     14/full@0.3: 1.0
[judge] deepseek 14/scx@0.2: 1.0
[judge] qwen     14/scx@0.2: 0.5
[judge] deepseek 14/scx@0.3: 0.5
[judge] qwen     14/full@0.4: 1.0
[judge] qwen     14/scx@0.3: 0.5
[judge] deepseek 14/scx@0.4: 1.0
[judge] deepseek 15/full@0.2: 1.5
[judge] qwen     14/scx@0.4: 1.0

## bench_arch_v3_run8.log

[load] Qwen2.5-3B-Instruct（千问） ...
[INFO] Quantized model with 8.501 bits per weight.
[load] 8-bit 量化完成（Qwen2.5-3B-Instruct（千问））
[resume] 从 bench_arch_plans.json 加载 20 题生成结果
[resume] judge 缓存 181 条
[judge] deepseek 15/full@0.3: 2.0
[judge] deepseek 15/full@0.4: 2.0
[judge] qwen     15/full@0.3: 1.0
[judge] qwen     15/full@0.2: 1.0
[judge] deepseek 15/scx@0.2: 1.5
[judge] qwen     15/full@0.4: 1.0
[judge] deepseek 15/scx@0.3: 1.5
[judge] deepseek 15/scx@0.4: 1.5
[judge] qwen     15/scx@0.2: 1.0
[judge] deepseek 16/full@0.2: 1.5
[judge] qwen     15/scx@0.3: 1.0
[judge] qwen     15/scx@0.4: 1.0
[judge] deepseek 16/full@0.3: 2.0
[judge] deepseek 16/full@0.4: 2.0
[judge] qwen     16/full@0.3: 1.0
[judge] qwen     16/full@0.2: 1.5
[judge] qwen     16/full@0.4: 1.0
[judge] deepseek 16/scx@0.2: 1.0
[judge] deepseek 16/scx@0.3: 2.0
[judge] deepseek 16/scx@0.4: 1.0
[judge] qwen     16/scx@0.2: 1.0
[judge] qwen     16/scx@0.3: 1.0
[judge] deepseek 17/full@0.2: 1.0
[judge] qwen     16/scx@0.4: 1.0
[judge] qwen     17/full@0.2: 1.0
[judge] deepseek 17/full@0.3: 1.0
[judge] deepseek 17/full@0.4: 1.0
[judge] qwen     17/full@0.3: 1.0
[judge] deepseek 17/scx@0.2: 1.0
[judge] qwen     17/scx@0.2: 1.0
[judge] qwen     17/full@0.4: 1.0
[judge] deepseek 17/scx@0.3: 1.0
[judge] deepseek 17/scx@0.4: 1.5
[judge] qwen     17/scx@0.3: 1.0
[judge] deepseek 18/full@0.2: 1.5
[judge] qwen     17/scx@0.4: 1.0
[judge] deepseek 18/full@0.3: 2.0
[judge] qwen     18/full@0.3: 1.5
[judge] qwen     18/full@0.2: 1.0
[judge] deepseek 18/full@0.4: 2.0
[judge] deepseek 18/scx@0.2: 1.0
[judge] deepseek 18/scx@0.3: 1.0
[judge] qwen     18/full@0.4: 1.5
[judge] qwen     18/scx@0.2: 1.0
[judge] deepseek 18/scx@0.4: 1.0
[judge] deepseek 19/full@0.2: 2.0
[judge] qwen     18/scx@0.3: 1.0
[judge] qwen     19/full@0.2: 1.0
[judge] deepseek 19/full@0.3: 2.0
[judge] qwen     18/scx@0.4: 0.5
[judge] deepseek 19/full@0.4: 1.5
[judge] qwen     19/full@0.3: 1.5
[judge] qwen     19/full@0.4: 1.0
[judge] deepseek 19/scx@0.2: 1.0
[judge] deepseek 19/scx@0.3: 1.0
[judge] qwen     19/scx@0.2: 1.0
[judge] qwen     19/scx@0.3: 0.5
[judge] deepseek 19/scx@0.4: 1.0
[judge] qwen     19/scx@0.4: 1.0
[rejudge] deepseek 10/full max-delta=0.5
[rejudge] deepseek 10/scx max-delta=0.5
[rejudge] deepseek 4/full max-delta=0.5
[rejudge] deepseek 4/scx max-delta=0.5

=== P6n 架构文档 rubric 评测（Qwen2.5-3B-Instruct（千问） 生成本地 / judge=DeepSeek-V4.1-Flash 在线, 20 题, ratios=['0.2', '0.3', '0.4']）===
轨                structure  accuracy  tradeoff feasibility  clarity   TOTAL    cut
full                  1.65      0.88      0.60       0.68     1.48    1.05     0%
scx r=0.2             0.97      0.53      0.15       0.42     1.10    0.64    66%
scx r=0.3             1.15      0.75      0.25       0.53     1.25    0.79    59%
scx r=0.4             1.23      0.72      0.42       0.55     1.20    0.82    52%
保持率 scx(r=0.2)/full = 0.64 / 1.05 = 0.602
保持率 scx(r=0.3)/full = 0.79 / 1.05 = 0.744
保持率 scx(r=0.4)/full = 0.82 / 1.05 = 0.782

=== P6n 架构文档 rubric 评测（Qwen2.5-3B-Instruct（千问） 生成本地 / judge=Qwen-flash (3.8) 在线, 20 题, ratios=['0.2', '0.3', '0.4']）===
轨                structure  accuracy  tradeoff feasibility  clarity   TOTAL    cut
full                  1.40      0.97      0.68       0.80     1.80    1.13     0%
scx r=0.2             0.80      0.78      0.25       0.35     1.45    0.72    66%
scx r=0.3             0.85      0.88      0.38       0.55     1.57    0.84    59%
scx r=0.4             0.93      0.90      0.45       0.50     1.55    0.86    52%
保持率 scx(r=0.2)/full = 0.72 / 1.13 = 0.642
保持率 scx(r=0.3)/full = 0.84 / 1.13 = 0.748
保持率 scx(r=0.4)/full = 0.86 / 1.13 = 0.765
压缩率（r=0.2）: 65.9%

=== 双 judge 交叉一致性（同答案两模型打分差）===
--- 档位 0.2 ---
维度             均值差     最大差      方向一致率
structure     0.31     1.5        75%
accuracy      0.28     1.0        62%
tradeoff      0.19     1.0        70%
feasibility    0.25     0.5        68%
clarity       0.44     1.0        71%
综合            0.29     1.5
--- 档位 0.3 ---
维度             均值差     最大差      方向一致率
structure     0.38     1.0        85%
accuracy      0.25     1.0        72%
tradeoff      0.21     1.0        72%
feasibility    0.25     0.5        66%
clarity       0.46     1.0        65%
综合            0.31     1.0
--- 档位 0.4 ---
维度             均值差     最大差      方向一致率
structure     0.35     1.0        65%
accuracy      0.24     1.0        65%
tradeoff      0.21     1.0        63%
feasibility    0.17     0.5        61%
clarity       0.36     1.0        61%
综合            0.27     1.0

单 judge 复打一致性（4 次, max 分差）: 均值 0.50 / 最大 0.5
judge 解析失败题: ['rust_arch', 'perf_opt', 'gpu_tiling', 'kv_cache', 'scheduler', 'allocator', 'code_review', 'log_sys', 'test_strategy', 'kv_batch', 'cache_coherence', 'consensus', 'compiler_ir', 'net_stack', 'gpu_mempool', 'err_arch', 'hashmap', 'build_sys', 'lsm_store', 'observability']
[csv] bench_arch_v3.csv

## bench_c60_n50.log

full : acc=0.680 (34/50)  ctx_tok=966
scx  : acc=0.680 (34/50)  ctx_tok=738  tok_ratio=0.764
scx 保持率（full 答对中 scx 仍答对）: 30/34 = 0.882

## bench_code50_02.log

full : acc=0.950 (19/20)  ctx_tok=759
scx  : acc=1.000 (20/20)  ctx_tok=517  tok_ratio=0.681
scx 保持率（full 答对中 scx 仍答对）: 19/19 = 1.000
压缩率分桶（cut = 1 - scx_tok/full_tok）:

## bench_code60_03.log

full : acc=0.980 (49/50)  ctx_tok=759
scx  : acc=0.940 (47/50)  ctx_tok=534  tok_ratio=0.703
scx 保持率（full 答对中 scx 仍答对）: 47/49 = 0.959

## bench_code60_anchor_reg.log

full : acc=0.980 (49/50)  ctx_tok=759
scx  : acc=0.880 (44/50)  ctx_tok=524  tok_ratio=0.691
scx 保持率（full 答对中 scx 仍答对）: 44/49 = 0.898

## bench_code60_anchor_reg2.log

full : acc=0.980 (49/50)  ctx_tok=759
scx  : acc=0.880 (44/50)  ctx_tok=524  tok_ratio=0.691
scx 保持率（full 答对中 scx 仍答对）: 44/49 = 0.898

## bench_code60_n50.log

full : acc=0.980 (49/50)  ctx_tok=759
scx  : acc=0.940 (47/50)  ctx_tok=518  tok_ratio=0.682
scx 保持率（full 答对中 scx 仍答对）: 47/49 = 0.959
压缩率分桶（cut = 1 - scx_tok/full_tok）:
  10-35%: 50 样本

## bench_code60_n50b.log

full : acc=0.980 (49/50)  ctx_tok=759
scx  : acc=0.940 (47/50)  ctx_tok=518  tok_ratio=0.682
scx 保持率（full 答对中 scx 仍答对）: 47/49 = 0.959

## bench_code60_rollback.log

full : acc=0.980 (49/50)  ctx_tok=759
scx  : acc=0.940 (47/50)  ctx_tok=518  tok_ratio=0.682
scx 保持率（full 答对中 scx 仍答对）: 47/49 = 0.959

## bench_code_02.log

full : acc=0.950 (19/20)  ctx_tok=759
scx  : acc=1.000 (20/20)  ctx_tok=517  tok_ratio=0.681
scx 保持率（full 答对中 scx 仍答对）: 19/19 = 1.000
压缩率分桶（cut = 1 - scx_tok/full_tok）:

## bench_code_04.log

full : acc=0.950 (19/20)  ctx_tok=759
scx  : acc=0.900 (18/20)  ctx_tok=540  tok_ratio=0.712
scx 保持率（full 答对中 scx 仍答对）: 18/19 = 0.947

## bench_code_final_n20.log

full : acc=0.600 (30/50)  ctx_tok=1332
scx  : acc=0.620 (31/50)  ctx_tok=965  tok_ratio=0.725
scx 保持率（full 答对中 scx 仍答对）: 27/30 = 0.900

## bench_code_keep_n20.log

full : acc=0.600 (30/50)  ctx_tok=1332
scx  : acc=0.620 (31/50)  ctx_tok=965  tok_ratio=0.725
scx 保持率（full 答对中 scx 仍答对）: 27/30 = 0.900

## bench_code_ops_n20.log

full : acc=0.600 (30/50)  ctx_tok=1332
scx  : acc=0.580 (29/50)  ctx_tok=997  tok_ratio=0.748
scx 保持率（full 答对中 scx 仍答对）: 26/30 = 0.867

## bench_code_ratio_scan.log

=== CODE n=20 code-keep=1 trim-ratio=0.4 ===
full : acc=0.950 (19/20)  ctx_tok=759
scx  : acc=0.950 (19/20)  ctx_tok=729  tok_ratio=0.961
scx 保持率（full 答对中 scx 仍答对）: 19/19 = 1.000
=== CODE n=20 code-keep=1 trim-ratio=0.3 ===
full : acc=0.950 (19/20)  ctx_tok=759
scx  : acc=0.950 (19/20)  ctx_tok=712  tok_ratio=0.939
scx 保持率（full 答对中 scx 仍答对）: 19/19 = 1.000
=== CODE n=20 code-keep=1 trim-ratio=0.2 ===
full : acc=0.950 (19/20)  ctx_tok=759
scx  : acc=0.950 (19/20)  ctx_tok=697  tok_ratio=0.918
scx 保持率（full 答对中 scx 仍答对）: 18/19 = 0.947

## bench_cpp60_n50.log

full : acc=0.880 (44/50)  ctx_tok=1011
scx  : acc=0.820 (41/50)  ctx_tok=773  tok_ratio=0.765
scx 保持率（full 答对中 scx 仍答对）: 39/44 = 0.886

## bench_gsm8k_anchor_reg.log

full : acc=0.600 (30/50)  ctx_tok=1332
scx  : acc=0.620 (31/50)  ctx_tok=965  tok_ratio=0.725
scx 保持率（full 答对中 scx 仍答对）: 27/30 = 0.900

## bench_gsm8k_default_n50.log

full : acc=0.700 (14/20)  ctx_tok=701
scx  : acc=0.500 (10/20)  ctx_tok=649  tok_ratio=0.927
scx 保持率（full 答对中 scx 仍答对）: 9/14 = 0.643

## bench_java60_n50.log

full : acc=0.760 (38/50)  ctx_tok=991
scx  : acc=0.760 (38/50)  ctx_tok=765  tok_ratio=0.772
scx 保持率（full 答对中 scx 仍答对）: 33/38 = 0.868

## bench_kernel_ds_run1.log

[gen 4/40] 5s
[gen 8/40] 8s
[gen 12/40] 12s
[gen 16/40] 15s
[gen 20/40] 16s
[gen 24/40] 20s
[gen 28/40] 23s
[gen 32/40] 26s
[gen 36/40] 29s
[gen 40/40] 32s
[1/20] C    请用 C 语言实现函数 str_rev：   full=F scx=F cut=57.3% 0.1s
[2/20] C    请用 C 语言实现函数 arr_sum：   full=F scx=F cut=57.0% 0.0s
[3/20] C    请用 C 语言实现函数 count_bi   full=F scx=F cut=69.2% 0.0s
[4/20] C    请用 C 语言实现函数 list_len   full=F scx=F cut=66.9% 0.0s
[5/20] C    请用 C 语言实现函数 max3：返回三   full=F scx=F cut=70.7% 0.0s
[6/20] C    请用 C 语言实现函数 is_prime   full=F scx=F cut=70.4% 0.0s
[7/20] C    请用 C 语言实现函数 gcd：计算两个   full=F scx=F cut=70.9% 0.0s
[8/20] C    请用 C 语言实现函数 to_upper   full=F scx=F cut=70.6% 0.0s
[9/20] Rust 请用 Rust 实现函数 vec_sum   full=P scx=P cut=57.9% 0.9s
[10/20] Rust 请用 Rust 实现函数 count_e   full=P scx=P cut=69.7% 0.6s
[11/20] Rust 请用 Rust 实现函数 swap_pa   full=P scx=P cut=69.8% 0.6s
[12/20] Rust 请用 Rust 实现结构体 Rect 及   full=F scx=P cut=57.7% 0.5s
[13/20] Rust 请用 Rust 实现函数 first_c   full=P scx=P cut=70.0% 0.6s
[14/20] Rust 请用 Rust 实现函数 fib：计算第   full=F scx=F cut=69.9% 0.1s
[15/20] C++  请用 C++ 实现模板函数 tmax：返   full=F scx=F cut=69.4% 0.0s
[16/20] C++  请用 C++ 实现类 Counter：构   full=F scx=F cut=58.3% 0.0s
[17/20] C++  请用 C++ 实现函数 vsum：返回    full=F scx=F cut=58.8% 0.3s
[18/20] C++  请用 C++ 实现函数 sum_squa   full=F scx=F cut=58.5% 0.3s
[19/20] C++  请用 C++ 实现函数 slen：返回    full=F scx=F cut=69.8% 0.3s
[20/20] C++  请用 C++ 实现函数 arev：原地反   full=F scx=F cut=70.1% 0.1s

=== 算子开发生成评测（DeepSeek-V4.1-Flash (API, conc=4), 20 题, ('--trim', '1', '--trim-ratio', '0.3', '--trim-op-keep', '1', '--num-line-keep', '1', '--sym-line-keep', '1')）===
full : pass=0.200 (4/20)
scx  : pass=0.250 (5/20)  tok_ratio=0.345  cut=65.5%
保持率（full 通过中 scx 仍通过）: 4/4 = 1.000
  C     full 0/8  scx 0/8
  Rust  full 4/6  scx 5/6
  C++   full 0/6  scx 0/6
[csv] bench_kernel_ds_v1.csv

## bench_kernel_ds_run2.log

[gen 4/40] 4s
[gen 8/40] 6s
[gen 12/40] 10s
[gen 16/40] 14s
[gen 20/40] 16s
[gen 24/40] 18s
[gen 28/40] 22s
[gen 32/40] 25s
[gen 36/40] 27s
[gen 40/40] 30s
[1/20] C    请用 C 语言实现函数 str_rev：   full=P scx=P cut=57.3% 0.9s
[2/20] C    请用 C 语言实现函数 arr_sum：   full=P scx=P cut=57.0% 0.7s
[3/20] C    请用 C 语言实现函数 count_bi   full=P scx=P cut=69.2% 0.7s
[4/20] C    请用 C 语言实现函数 list_len   full=F scx=F cut=66.9% 0.1s
[5/20] C    请用 C 语言实现函数 max3：返回三   full=P scx=P cut=70.7% 0.7s
[6/20] C    请用 C 语言实现函数 is_prime   full=P scx=F cut=70.4% 0.4s
[7/20] C    请用 C 语言实现函数 gcd：计算两个   full=F scx=P cut=70.9% 0.4s
[8/20] C    请用 C 语言实现函数 to_upper   full=F scx=P cut=70.6% 0.4s
[9/20] Rust 请用 Rust 实现函数 vec_sum   full=F scx=P cut=57.9% 0.4s
[10/20] Rust 请用 Rust 实现函数 count_e   full=P scx=P cut=69.7% 0.6s
[11/20] Rust 请用 Rust 实现函数 swap_pa   full=P scx=P cut=69.8% 0.6s
[12/20] Rust 请用 Rust 实现结构体 Rect 及   full=P scx=P cut=57.7% 0.5s
[13/20] Rust 请用 Rust 实现函数 first_c   full=P scx=P cut=70.0% 0.6s
[14/20] Rust 请用 Rust 实现函数 fib：计算第   full=F scx=F cut=69.9% 0.1s
[15/20] C++  请用 C++ 实现模板函数 tmax：返   full=P scx=P cut=69.4% 0.7s
[16/20] C++  请用 C++ 实现类 Counter：构   full=F scx=F cut=58.3% 0.1s
[17/20] C++  请用 C++ 实现函数 vsum：返回    full=P scx=P cut=58.8% 1.0s
[18/20] C++  请用 C++ 实现函数 sum_squa   full=P scx=P cut=58.5% 1.0s
[19/20] C++  请用 C++ 实现函数 slen：返回    full=F scx=P cut=69.8% 0.6s
[20/20] C++  请用 C++ 实现函数 arev：原地反   full=P scx=P cut=70.1% 0.8s

=== 算子开发生成评测（DeepSeek-V4.1-Flash (API, conc=4), 20 题, ('--trim', '1', '--trim-ratio', '0.3', '--trim-op-keep', '1', '--num-line-keep', '1', '--sym-line-keep', '1')）===
full : pass=0.650 (13/20)
scx  : pass=0.800 (16/20)  tok_ratio=0.345  cut=65.5%
保持率（full 通过中 scx 仍通过）: 12/13 = 0.923
  C     full 5/8  scx 6/8
  Rust  full 4/6  scx 5/6
  C++   full 4/6  scx 5/6
[csv] bench_kernel_ds_v2.csv

## bench_kernel_run.log

[16/20] C++  请用 C++ 实现类 Counter：构   full=F scx=F cut=0.0% 18.3s
[17/20] C++  请用 C++ 实现函数 vsum：返回    full=F scx=F cut=0.0% 18.8s
[18/20] C++  请用 C++ 实现函数 sum_squa   full=F scx=F cut=0.0% 18.4s
[19/20] C++  请用 C++ 实现函数 slen：返回    full=F scx=F cut=0.0% 17.9s
[20/20] C++  请用 C++ 实现函数 arev：原地反   full=F scx=F cut=0.0% 17.4s

=== 算子开发生成评测（Qwen2.5-3B-Instruct（千问） via MLX, 20 题, ('--trim', '1', '--trim-ratio', '0.3', '--trim-op-keep', '1', '--num-line-keep', '1')）===
full : pass=0.250 (5/20)
scx  : pass=0.250 (5/20)  tok_ratio=1.000  cut=0.0%
保持率（full 通过中 scx 仍通过）: 5/5 = 1.000
  C     full 0/8  scx 0/8
  Rust  full 5/6  scx 5/6
  C++   full 0/6  scx 0/6
[csv] bench_kernel_qwen.csv
===== NANBEIGE =====
[16/20] C++  请用 C++ 实现类 Counter：构   full=F scx=F cut=0.0% 37.1s
[17/20] C++  请用 C++ 实现函数 vsum：返回    full=F scx=F cut=0.0% 10.9s
[18/20] C++  请用 C++ 实现函数 sum_squa   full=F scx=F cut=0.0% 54.2s
[19/20] C++  请用 C++ 实现函数 slen：返回    full=F scx=F cut=0.0% 2.5s
[20/20] C++  请用 C++ 实现函数 arev：原地反   full=F scx=F cut=0.0% 37.2s

=== 算子开发生成评测（Nanbeige4.2-3B（南北阁） via MLX, 20 题, ('--trim', '1', '--trim-ratio', '0.3', '--trim-op-keep', '1', '--num-line-keep', '1')）===
full : pass=0.000 (0/20)
scx  : pass=0.000 (0/20)  tok_ratio=1.000  cut=0.0%
保持率（full 通过中 scx 仍通过）: 0/0 = 0.000
  C     full 0/8  scx 0/8
  Rust  full 0/6  scx 0/6
  C++   full 0/6  scx 0/6
[csv] bench_kernel_nb.csv

## bench_kernel_run2.log

[18/20] C++  请用 C++ 实现函数 sum_squa   full=F scx=F cut=0.0% 3.0s
[19/20] C++  请用 C++ 实现函数 slen：返回    full=F scx=F cut=0.0% 1.4s
[20/20] C++  请用 C++ 实现函数 arev：原地反   full=F scx=F cut=0.0% 2.4s

=== 算子开发生成评测（Qwen2.5-3B-Instruct（千问） via MLX, 20 题, ('--trim', '1', '--trim-ratio', '0.3', '--trim-op-keep', '1', '--num-line-keep', '1')）===
full : pass=0.100 (2/20)
scx  : pass=0.100 (2/20)  tok_ratio=0.979  cut=2.1%
保持率（full 通过中 scx 仍通过）: 2/2 = 1.000
  C     full 0/8  scx 0/8
  Rust  full 2/6  scx 2/6
  C++   full 0/6  scx 0/6
[csv] bench_kernel_qwen.csv
===== NANBEIGE =====
[18/20] C++  请用 C++ 实现函数 sum_squa   full=F scx=F cut=0.0% 40.5s
[19/20] C++  请用 C++ 实现函数 slen：返回    full=F scx=F cut=0.0% 27.5s
[20/20] C++  请用 C++ 实现函数 arev：原地反   full=F scx=F cut=0.0% 39.8s

=== 算子开发生成评测（Nanbeige4.2-3B（南北阁） via MLX, 20 题, ('--trim', '1', '--trim-ratio', '0.3', '--trim-op-keep', '1', '--num-line-keep', '1')）===
full : pass=0.000 (0/20)
scx  : pass=0.000 (0/20)  tok_ratio=0.973  cut=2.7%
保持率（full 通过中 scx 仍通过）: 0/0 = 0.000
  C     full 0/8  scx 0/8
  Rust  full 0/6  scx 0/6
  C++   full 0/6  scx 0/6
[csv] bench_kernel_nb.csv

## bench_kernel_run3.log

[load] Qwen2.5-3B-Instruct（千问） ...
[INFO] Quantized model with 8.501 bits per weight.
[load] 8-bit 量化完成（Qwen2.5-3B-Instruct（千问））
[1/20] C    请用 C 语言实现函数 str_rev：   full=F scx=F cut=59.6% 5.5s
[2/20] C    请用 C 语言实现函数 arr_sum：   full=F scx=F cut=56.1% 2.4s
[3/20] C    请用 C 语言实现函数 count_bi   full=F scx=F cut=67.1% 1.7s
[4/20] C    请用 C 语言实现函数 list_len   full=F scx=F cut=64.6% 2.4s
[5/20] C    请用 C 语言实现函数 max3：返回三   full=F scx=F cut=68.8% 2.1s
[6/20] C    请用 C 语言实现函数 is_prime   full=F scx=F cut=58.8% 3.6s
[7/20] C    请用 C 语言实现函数 gcd：计算两个   full=F scx=F cut=60.5% 1.6s
[8/20] C    请用 C 语言实现函数 to_upper   full=F scx=F cut=60.0% 2.2s
[9/20] Rust 请用 Rust 实现函数 vec_sum   full=F scx=F cut=58.0% 2.1s
[10/20] Rust 请用 Rust 实现函数 count_e   full=P scx=F cut=69.1% 2.4s
[11/20] Rust 请用 Rust 实现函数 swap_pa   full=P scx=F cut=67.5% 3.6s
[12/20] Rust 请用 Rust 实现结构体 Rect 及   full=P scx=P cut=58.7% 2.6s
[13/20] Rust 请用 Rust 实现函数 first_c   full=F scx=F cut=70.2% 1.1s
[14/20] Rust 请用 Rust 实现函数 fib：计算第   full=P scx=P cut=66.5% 2.3s
[15/20] C++  请用 C++ 实现模板函数 tmax：返   full=F scx=F cut=62.2% 4.7s
[16/20] C++  请用 C++ 实现类 Counter：构   full=F scx=F cut=59.2% 2.7s
[17/20] C++  请用 C++ 实现函数 vsum：返回    full=F scx=F cut=60.1% 4.4s
[18/20] C++  请用 C++ 实现函数 sum_squa   full=F scx=F cut=59.9% 4.6s
[19/20] C++  请用 C++ 实现函数 slen：返回    full=F scx=F cut=68.9% 2.8s
[20/20] C++  请用 C++ 实现函数 arev：原地反   full=F scx=F cut=69.0% 2.7s

=== 算子开发生成评测（Qwen2.5-3B-Instruct（千问） via MLX, 20 题, ('--trim', '1', '--trim-ratio', '0.3', '--trim-op-keep', '1', '--num-line-keep', '1')）===
full : pass=0.200 (4/20)
scx  : pass=0.100 (2/20)  tok_ratio=0.368  cut=63.2%
保持率（full 通过中 scx 仍通过）: 2/4 = 0.500
  C     full 0/8  scx 0/8
  Rust  full 4/6  scx 2/6
  C++   full 0/6  scx 0/6
[csv] bench_kernel_qwen_v3.csv

## bench_kernel_run4.log

[load] Qwen2.5-3B-Instruct（千问） ...
[INFO] Quantized model with 8.501 bits per weight.
[load] 8-bit 量化完成（Qwen2.5-3B-Instruct（千问））
[1/20] C    请用 C 语言实现函数 str_rev：   full=F scx=F cut=0.0% 6.8s
[2/20] C    请用 C 语言实现函数 arr_sum：   full=F scx=F cut=0.0% 3.1s
[3/20] C    请用 C 语言实现函数 count_bi   full=F scx=F cut=0.0% 1.9s
[4/20] C    请用 C 语言实现函数 list_len   full=F scx=F cut=0.0% 2.5s
[5/20] C    请用 C 语言实现函数 max3：返回三   full=F scx=F cut=0.0% 2.6s
[6/20] C    请用 C 语言实现函数 is_prime   full=F scx=F cut=0.0% 3.6s
[7/20] C    请用 C 语言实现函数 gcd：计算两个   full=F scx=F cut=0.0% 1.6s
[8/20] C    请用 C 语言实现函数 to_upper   full=F scx=F cut=0.0% 2.3s
[9/20] Rust 请用 Rust 实现函数 vec_sum   full=F scx=F cut=0.0% 2.1s

## bench_kernel_run5.log

[load] Qwen2.5-3B-Instruct（千问） ...
[INFO] Quantized model with 8.501 bits per weight.
[load] 8-bit 量化完成（Qwen2.5-3B-Instruct（千问））
[1/20] C    请用 C 语言实现函数 str_rev：   full=F scx=F cut=0.0% 6.2s
[2/20] C    请用 C 语言实现函数 arr_sum：   full=F scx=F cut=0.0% 3.1s
[3/20] C    请用 C 语言实现函数 count_bi   full=F scx=F cut=0.0% 1.9s
[4/20] C    请用 C 语言实现函数 list_len   full=F scx=F cut=0.0% 2.5s
[5/20] C    请用 C 语言实现函数 max3：返回三   full=F scx=F cut=0.0% 2.6s
[6/20] C    请用 C 语言实现函数 is_prime   full=F scx=F cut=0.0% 3.6s
[7/20] C    请用 C 语言实现函数 gcd：计算两个   full=F scx=F cut=0.0% 1.6s
[8/20] C    请用 C 语言实现函数 to_upper   full=F scx=F cut=0.0% 2.3s
[9/20] Rust 请用 Rust 实现函数 vec_sum   full=P scx=P cut=0.0% 2.5s
[10/20] Rust 请用 Rust 实现函数 count_e   full=P scx=P cut=0.0% 2.4s
[11/20] Rust 请用 Rust 实现函数 swap_pa   full=P scx=P cut=0.0% 3.2s
[12/20] Rust 请用 Rust 实现结构体 Rect 及   full=P scx=P cut=0.0% 2.4s
[13/20] Rust 请用 Rust 实现函数 first_c   full=P scx=P cut=0.0% 1.7s
[14/20] Rust 请用 Rust 实现函数 fib：计算第   full=P scx=P cut=0.0% 3.6s
[15/20] C++  请用 C++ 实现模板函数 tmax：返   full=F scx=F cut=0.0% 7.5s
[16/20] C++  请用 C++ 实现类 Counter：构   full=F scx=F cut=0.0% 3.6s
[17/20] C++  请用 C++ 实现函数 vsum：返回    full=F scx=F cut=0.0% 2.7s
[18/20] C++  请用 C++ 实现函数 sum_squa   full=F scx=F cut=0.0% 3.1s
[19/20] C++  请用 C++ 实现函数 slen：返回    full=F scx=F cut=0.0% 1.9s
[20/20] C++  请用 C++ 实现函数 arev：原地反   full=F scx=F cut=0.0% 2.6s

=== 算子开发生成评测（Qwen2.5-3B-Instruct（千问） via MLX, 20 题, ('--trim', '1', '--trim-ratio', '0.3', '--trim-op-keep', '1', '--num-line-keep', '1', '--sym-line-keep', '1')）===
full : pass=0.300 (6/20)
scx  : pass=0.300 (6/20)  tok_ratio=1.000  cut=0.0%
保持率（full 通过中 scx 仍通过）: 6/6 = 1.000
  C     full 0/8  scx 0/8
  Rust  full 6/6  scx 6/6
  C++   full 0/6  scx 0/6
[csv] bench_kernel_qwen_v5.csv

## bench_kernel_run6.log

[load] Qwen2.5-3B-Instruct（千问） ...
[INFO] Quantized model with 8.501 bits per weight.
[load] 8-bit 量化完成（Qwen2.5-3B-Instruct（千问））
[1/20] C    请用 C 语言实现函数 str_rev：   full=F scx=F cut=56.8% 4.9s
[2/20] C    请用 C 语言实现函数 arr_sum：   full=F scx=F cut=56.1% 2.5s
[3/20] C    请用 C 语言实现函数 count_bi   full=F scx=F cut=67.1% 1.7s
[4/20] C    请用 C 语言实现函数 list_len   full=F scx=F cut=64.6% 2.4s
[5/20] C    请用 C 语言实现函数 max3：返回三   full=F scx=F cut=68.8% 2.1s
[6/20] C    请用 C 语言实现函数 is_prime   full=F scx=F cut=66.7% 3.6s
[7/20] C    请用 C 语言实现函数 gcd：计算两个   full=F scx=F cut=68.7% 1.5s
[8/20] C    请用 C 语言实现函数 to_upper   full=F scx=F cut=67.8% 2.0s
[9/20] Rust 请用 Rust 实现函数 vec_sum   full=P scx=F cut=56.6% 2.1s
[10/20] Rust 请用 Rust 实现函数 count_e   full=P scx=P cut=67.3% 2.1s
[11/20] Rust 请用 Rust 实现函数 swap_pa   full=P scx=F cut=66.2% 2.7s
[12/20] Rust 请用 Rust 实现结构体 Rect 及   full=P scx=P cut=56.7% 2.5s
[13/20] Rust 请用 Rust 实现函数 first_c   full=P scx=F cut=68.9% 1.6s
[14/20] Rust 请用 Rust 实现函数 fib：计算第   full=P scx=F cut=65.3% 2.8s
[15/20] C++  请用 C++ 实现模板函数 tmax：返   full=F scx=F cut=68.5% 4.6s
[16/20] C++  请用 C++ 实现类 Counter：构   full=F scx=F cut=59.2% 2.7s
[17/20] C++  请用 C++ 实现函数 vsum：返回    full=F scx=F cut=60.1% 4.4s
[18/20] C++  请用 C++ 实现函数 sum_squa   full=F scx=F cut=59.9% 4.6s
[19/20] C++  请用 C++ 实现函数 slen：返回    full=F scx=F cut=68.9% 2.8s
[20/20] C++  请用 C++ 实现函数 arev：原地反   full=F scx=F cut=69.0% 2.7s

=== 算子开发生成评测（Qwen2.5-3B-Instruct（千问） via MLX, 20 题, ('--trim', '1', '--trim-ratio', '0.3', '--trim-op-keep', '1', '--num-line-keep', '1', '--sym-line-keep', '1')）===
full : pass=0.300 (6/20)
scx  : pass=0.100 (2/20)  tok_ratio=0.359  cut=64.1%
保持率（full 通过中 scx 仍通过）: 2/6 = 0.333
  C     full 0/8  scx 0/8
  Rust  full 6/6  scx 2/6
  C++   full 0/6  scx 0/6
[csv] bench_kernel_qwen_v6.csv

## bench_kernel_run7.log

[load] Qwen2.5-3B-Instruct（千问） ...
[INFO] Quantized model with 8.501 bits per weight.
[load] 8-bit 量化完成（Qwen2.5-3B-Instruct（千问））
[1/20] C    请用 C 语言实现函数 str_rev：   full=F scx=F cut=56.1% 5.3s
[2/20] C    请用 C 语言实现函数 arr_sum：   full=F scx=F cut=55.5% 2.5s
[3/20] C    请用 C 语言实现函数 count_bi   full=F scx=F cut=67.1% 1.7s
[4/20] C    请用 C 语言实现函数 list_len   full=F scx=F cut=63.7% 2.4s
[5/20] C    请用 C 语言实现函数 max3：返回三   full=F scx=F cut=68.1% 2.1s
[6/20] C    请用 C 语言实现函数 is_prime   full=F scx=F cut=66.4% 3.5s
[7/20] C    请用 C 语言实现函数 gcd：计算两个   full=F scx=F cut=68.0% 1.5s
[8/20] C    请用 C 语言实现函数 to_upper   full=F scx=F cut=67.6% 2.1s
[9/20] Rust 请用 Rust 实现函数 vec_sum   full=P scx=P cut=55.9% 2.3s
[10/20] Rust 请用 Rust 实现函数 count_e   full=P scx=P cut=66.7% 1.9s
[11/20] Rust 请用 Rust 实现函数 swap_pa   full=P scx=F cut=65.4% 2.8s
[12/20] Rust 请用 Rust 实现结构体 Rect 及   full=P scx=P cut=55.0% 2.5s
[13/20] Rust 请用 Rust 实现函数 first_c   full=P scx=P cut=68.0% 1.6s
[14/20] Rust 请用 Rust 实现函数 fib：计算第   full=P scx=P cut=64.5% 3.1s
[15/20] C++  请用 C++ 实现模板函数 tmax：返   full=F scx=F cut=67.5% 4.6s
[16/20] C++  请用 C++ 实现类 Counter：构   full=F scx=F cut=58.0% 3.9s
[17/20] C++  请用 C++ 实现函数 vsum：返回    full=F scx=F cut=59.1% 3.0s
[18/20] C++  请用 C++ 实现函数 sum_squa   full=F scx=F cut=58.9% 3.2s
[19/20] C++  请用 C++ 实现函数 slen：返回    full=F scx=F cut=68.5% 2.2s
[20/20] C++  请用 C++ 实现函数 arev：原地反   full=F scx=F cut=68.1% 2.3s

=== 算子开发生成评测（Qwen2.5-3B-Instruct（千问） via MLX, 20 题, ('--trim', '1', '--trim-ratio', '0.3', '--trim-op-keep', '1', '--num-line-keep', '1', '--sym-line-keep', '1')）===
full : pass=0.300 (6/20)
scx  : pass=0.250 (5/20)  tok_ratio=0.367  cut=63.3%
保持率（full 通过中 scx 仍通过）: 5/6 = 0.833
  C     full 0/8  scx 0/8
  Rust  full 6/6  scx 5/6
  C++   full 0/6  scx 0/6
[csv] bench_kernel_qwen_v7.csv

## bench_kernel_run8.log

[load] Qwen2.5-3B-Instruct（千问） ...
[INFO] Quantized model with 8.501 bits per weight.
[load] 8-bit 量化完成（Qwen2.5-3B-Instruct（千问））
[1/20] C    请用 C 语言实现函数 str_rev：   full=P scx=F cut=56.1% 0.9s
[2/20] C    请用 C 语言实现函数 arr_sum：   full=F scx=P cut=55.5% 0.4s
[3/20] C    请用 C 语言实现函数 count_bi   full=P scx=P cut=67.1% 0.6s
[4/20] C    请用 C 语言实现函数 list_len   full=F scx=F cut=63.7% 0.1s
[5/20] C    请用 C 语言实现函数 max3：返回三   full=P scx=P cut=68.1% 0.6s
[6/20] C    请用 C 语言实现函数 is_prime   full=P scx=P cut=66.4% 0.6s
[7/20] C    请用 C 语言实现函数 gcd：计算两个   full=P scx=P cut=68.0% 0.7s
[8/20] C    请用 C 语言实现函数 to_upper   full=P scx=P cut=67.6% 0.6s
[9/20] Rust 请用 Rust 实现函数 vec_sum   full=P scx=P cut=55.9% 0.8s
[10/20] Rust 请用 Rust 实现函数 count_e   full=P scx=P cut=66.7% 0.5s
[11/20] Rust 请用 Rust 实现函数 swap_pa   full=P scx=F cut=65.4% 0.8s
[12/20] Rust 请用 Rust 实现结构体 Rect 及   full=P scx=P cut=55.0% 0.7s
[13/20] Rust 请用 Rust 实现函数 first_c   full=P scx=P cut=68.0% 0.5s
[14/20] Rust 请用 Rust 实现函数 fib：计算第   full=P scx=P cut=64.5% 0.7s
[15/20] C++  请用 C++ 实现模板函数 tmax：返   full=F scx=P cut=67.5% 0.6s
[16/20] C++  请用 C++ 实现类 Counter：构   full=F scx=F cut=58.0% 0.4s
[17/20] C++  请用 C++ 实现函数 vsum：返回    full=P scx=P cut=59.1% 1.0s
[18/20] C++  请用 C++ 实现函数 sum_squa   full=P scx=P cut=58.9% 1.0s
[19/20] C++  请用 C++ 实现函数 slen：返回    full=P scx=P cut=68.5% 0.9s
[20/20] C++  请用 C++ 实现函数 arev：原地反   full=F scx=F cut=68.1% 0.1s

=== 算子开发生成评测（Qwen2.5-3B-Instruct（千问）, 20 题, ('--trim', '1', '--trim-ratio', '0.3', '--trim-op-keep', '1', '--num-line-keep', '1', '--sym-line-keep', '1')）===
full : pass=0.750 (15/20)
scx  : pass=0.750 (15/20)  tok_ratio=0.367  cut=63.3%
保持率（full 通过中 scx 仍通过）: 13/15 = 0.867
  C     full 6/8  scx 6/8
  Rust  full 6/6  scx 5/6
  C++   full 3/6  scx 4/6
[csv] bench_kernel_qwen_v8.csv

## bench_py60_02a.log

full : acc=0.620 (31/50)  ctx_tok=950
scx  : acc=0.540 (27/50)  ctx_tok=681  tok_ratio=0.717
scx 保持率（full 答对中 scx 仍答对）: 23/31 = 0.742

## bench_py60_03.log

full : acc=0.620 (31/50)  ctx_tok=950
scx  : acc=0.640 (32/50)  ctx_tok=687  tok_ratio=0.723
scx 保持率（full 答对中 scx 仍答对）: 25/31 = 0.806

## bench_py60_03a.log

full : acc=0.620 (31/50)  ctx_tok=950
scx  : acc=0.640 (32/50)  ctx_tok=691  tok_ratio=0.728
scx 保持率（full 答对中 scx 仍答对）: 26/31 = 0.839

## bench_py60_fb.log

full : acc=0.620 (31/50)  ctx_tok=950
scx  : acc=0.680 (34/50)  ctx_tok=687  tok_ratio=0.723
scx 保持率（full 答对中 scx 仍答对）: 27/31 = 0.871

## bench_py60_n50.log

full : acc=0.620 (31/50)  ctx_tok=950
scx  : acc=0.580 (29/50)  ctx_tok=675  tok_ratio=0.711
scx 保持率（full 答对中 scx 仍答对）: 22/31 = 0.710
压缩率分桶（cut = 1 - scx_tok/full_tok）:

## bench_qwen_n100.log

[load] MLX 加载模型 Qwen2.5-3B-Instruct（千问） (/Users/fei/model/qwen2.5-3b-instruct) ...
[INFO] Quantized model with 8.501 bits per weight.
[load] 8-bit 量化完成（Qwen2.5-3B-Instruct（千问））
Using the latest cached version of the dataset since openai/gsm8k couldn't be found on the Hugging Face Hub (offline mode is enabled).
Found the latest cached dataset configuration 'main' at /Users/fei/.cache/huggingface/datasets/openai___gsm8k/main/0.0.0/740312add88f781978c0658806c59bc2815b9866 (last modified on Wed Oct 28 00:01:26 2026).
[1/100]
[5/100]
[10/100]
[15/100]
[20/100]
[25/100]
[30/100]
[35/100]
[40/100]
[45/100]
[50/100]
[55/100]
[60/100]
[65/100]
[70/100]
[75/100]
[80/100]
[85/100]
[90/100]
[95/100]
[100/100]
[csv] 明细已导出: /Users/fei/code/scx/bench_qwen_n100.csv

=== GSM8K 8-shot 三方对照（Qwen2.5-3B-Instruct（千问） via MLX, n=100, seed=20261027）===
full : acc=0.480 (48/100)  ctx_tok=1332
scx  : acc=0.500 (50/100)  ctx_tok=1245  tok_ratio=0.935
trunc: acc=0.100 (10/100)  ctx_tok=同 scx
scx 保持率（full 答对中 scx 仍答对）: 42/48 = 0.875
压缩率分桶（cut = 1 - scx_tok/full_tok）:
  cut<2%: 34 样本
  2-10%: 40 样本
  10-35%: 26 样本
注：4/100 样本 scx 未压缩（透传/失败），已按 fail-open 计

## bench_rs60_fb.log

full : acc=0.800 (40/50)  ctx_tok=900
scx  : acc=0.740 (37/50)  ctx_tok=661  tok_ratio=0.734
scx 保持率（full 答对中 scx 仍答对）: 35/40 = 0.875

## bench_rs60_n50.log

full : acc=0.800 (40/50)  ctx_tok=900
scx  : acc=0.700 (35/50)  ctx_tok=661  tok_ratio=0.734
scx 保持率（full 答对中 scx 仍答对）: 34/40 = 0.850

## bench_trim_060_065.log

=== trim-ratio 0.6 ===
=== GSM8K 8-shot 三方对照（Qwen2.5-3B-Instruct（千问） via MLX, n=50, seed=20261027）===
full : acc=0.600 (30/50)  ctx_tok=1332
scx  : acc=0.560 (28/50)  ctx_tok=1026  tok_ratio=0.770
trunc: 已跳过（--no-trunc）
scx 保持率（full 答对中 scx 仍答对）: 27/30 = 0.900
压缩率分桶（cut = 1 - scx_tok/full_tok）:
  10-35%: 47 样本
  >35%: 3 样本
=== trim-ratio 0.65 ===

=== GSM8K 8-shot 三方对照（Qwen2.5-3B-Instruct（千问） via MLX, n=50, seed=20261027）===
full : acc=0.600 (30/50)  ctx_tok=1332
scx  : acc=0.600 (30/50)  ctx_tok=1060  tok_ratio=0.796
trunc: 已跳过（--no-trunc）
scx 保持率（full 答对中 scx 仍答对）: 27/30 = 0.900
压缩率分桶（cut = 1 - scx_tok/full_tok）:
  10-35%: 50 样本

## bench_trim_final_05.log

scx  : acc=0.620 (31/50)  ctx_tok=965  tok_ratio=0.725
trunc: 已跳过（--no-trunc）
scx 保持率（full 答对中 scx 仍答对）: 27/30 = 0.900
压缩率分桶（cut = 1 - scx_tok/full_tok）:
  10-35%: 43 样本
  >35%: 7 样本

## bench_trim_final_n100.log

[load] MLX 加载模型 Qwen2.5-3B-Instruct（千问） (/Users/fei/model/qwen2.5-3b-instruct) ...
[INFO] Quantized model with 8.501 bits per weight.
[load] 8-bit 量化完成（Qwen2.5-3B-Instruct（千问））
Using the latest cached version of the dataset since openai/gsm8k couldn't be found on the Hugging Face Hub (offline mode is enabled).
Found the latest cached dataset configuration 'main' at /Users/fei/.cache/huggingface/datasets/openai___gsm8k/main/0.0.0/740312add88f781978c0658806c59bc2815b9866 (last modified on Wed Oct 28 00:01:26 2026).
[1/100]
[5/100]
[10/100]
[15/100]
[20/100]
[25/100]
[30/100]
[35/100]
[40/100]
[45/100]
[50/100]
[55/100]
[60/100]
[65/100]
[70/100]
[75/100]
[80/100]
[85/100]
[90/100]
[95/100]
[100/100]

=== GSM8K 8-shot 三方对照（Qwen2.5-3B-Instruct（千问） via MLX, n=100, seed=20261027）===
full : acc=0.500 (50/100)  ctx_tok=1332
scx  : acc=0.590 (59/100)  ctx_tok=968  tok_ratio=0.727
trunc: 已跳过（--no-trunc）
scx 保持率（full 答对中 scx 仍答对）: 47/50 = 0.940
压缩率分桶（cut = 1 - scx_tok/full_tok）:
  10-35%: 87 样本
  >35%: 13 样本

## bench_trim_floor_n50.log

=== floor=1 trim=0.5 ===
full : acc=0.600 (30/50)  ctx_tok=1332
scx  : acc=0.540 (27/50)  ctx_tok=869  tok_ratio=0.653
scx 保持率（full 答对中 scx 仍答对）: 25/30 = 0.833
=== floor=1 trim=0.6 ===
full : acc=0.600 (30/50)  ctx_tok=1332
scx  : acc=0.580 (29/50)  ctx_tok=917  tok_ratio=0.688
scx 保持率（full 答对中 scx 仍答对）: 25/30 = 0.833

## bench_trim_floor_sweep_n20.log

=== trim0.5 + floor=1 ===
full : acc=0.600 (12/20)  ctx_tok=1334
scx  : acc=0.550 (11/20)  ctx_tok=884  tok_ratio=0.663
scx 保持率（full 答对中 scx 仍答对）: 10/12 = 0.833
=== trim0.5 + floor=2 ===
full : acc=0.600 (12/20)  ctx_tok=1334
scx  : acc=0.450 (9/20)  ctx_tok=875  tok_ratio=0.656
scx 保持率（full 答对中 scx 仍答对）: 8/12 = 0.667
=== trim0.5 + floor=3 ===
full : acc=0.600 (12/20)  ctx_tok=1334
scx  : acc=0.200 (4/20)  ctx_tok=800  tok_ratio=0.600
scx 保持率（full 答对中 scx 仍答对）: 4/12 = 0.333

## bench_trim_keep_05.log

=== GSM8K 8-shot 三方对照（Qwen2.5-3B-Instruct（千问） via MLX, n=50, seed=20261027）===
full : acc=0.600 (30/50)  ctx_tok=1332
scx  : acc=0.580 (29/50)  ctx_tok=959  tok_ratio=0.720
trunc: 已跳过（--no-trunc）
scx 保持率（full 答对中 scx 仍答对）: 26/30 = 0.867
压缩率分桶（cut = 1 - scx_tok/full_tok）:
  10-35%: 41 样本
  >35%: 9 样本

## bench_trim_keep_055_06.log

=== KEEP-KERNEL trim-ratio 0.55 ===
=== GSM8K 8-shot 三方对照（Qwen2.5-3B-Instruct（千问） via MLX, n=50, seed=20261027）===
full : acc=0.600 (30/50)  ctx_tok=1332
scx  : acc=0.560 (28/50)  ctx_tok=989  tok_ratio=0.743
trunc: 已跳过（--no-trunc）
scx 保持率（full 答对中 scx 仍答对）: 26/30 = 0.867
压缩率分桶（cut = 1 - scx_tok/full_tok）:
  10-35%: 47 样本
  >35%: 3 样本
=== KEEP-KERNEL trim-ratio 0.6 ===
=== GSM8K 8-shot 三方对照（Qwen2.5-3B-Instruct（千问） via MLX, n=50, seed=20261027）===
full : acc=0.600 (30/50)  ctx_tok=1332
scx  : acc=0.600 (30/50)  ctx_tok=1009  tok_ratio=0.757
trunc: 已跳过（--no-trunc）
scx 保持率（full 答对中 scx 仍答对）: 27/30 = 0.900
压缩率分桶（cut = 1 - scx_tok/full_tok）:
  10-35%: 47 样本
  >35%: 3 样本

## bench_trim_keep_05_norm.log

scx  : acc=0.560 (28/50)  ctx_tok=966  tok_ratio=0.725
trunc: 已跳过（--no-trunc）
scx 保持率（full 答对中 scx 仍答对）: 26/30 = 0.867
压缩率分桶（cut = 1 - scx_tok/full_tok）:
  10-35%: 43 样本
  >35%: 7 样本

## bench_trim_keep_05_rerun.log

=== GSM8K 8-shot 三方对照（Qwen2.5-3B-Instruct（千问） via MLX, n=50, seed=20261027）===
full : acc=0.600 (30/50)  ctx_tok=1332
scx  : acc=0.580 (29/50)  ctx_tok=959  tok_ratio=0.720
trunc: 已跳过（--no-trunc）
scx 保持率（full 答对中 scx 仍答对）: 26/30 = 0.867
压缩率分桶（cut = 1 - scx_tok/full_tok）:
  10-35%: 41 样本
  >35%: 9 样本

## bench_trim_n50.log

=== trim 1 (ratio 0.5) n=50 ===
full : acc=0.600 (30/50)  ctx_tok=1332
scx  : acc=0.540 (27/50)  ctx_tok=953  tok_ratio=0.716
trunc: acc=0.000 (0/50)  ctx_tok=同 scx
scx 保持率（full 答对中 scx 仍答对）: 26/30 = 0.867
压缩率分桶（cut = 1 - scx_tok/full_tok）:
  10-35%: 41 样本

## bench_trim_newkern_05.log

=== GSM8K 8-shot 三方对照（Qwen2.5-3B-Instruct（千问） via MLX, n=50, seed=20261027）===
full : acc=0.600 (30/50)  ctx_tok=1332
scx  : acc=0.540 (27/50)  ctx_tok=985  tok_ratio=0.739
trunc: 已跳过（--no-trunc）
scx 保持率（full 答对中 scx 仍答对）: 26/30 = 0.867
压缩率分桶（cut = 1 - scx_tok/full_tok）:
  10-35%: 45 样本
  >35%: 5 样本

## bench_trim_newkern_06_065.log

=== NEW-KERNEL trim-ratio 0.6 ===
=== GSM8K 8-shot 三方对照（Qwen2.5-3B-Instruct（千问） via MLX, n=50, seed=20261027）===
full : acc=0.600 (30/50)  ctx_tok=1332
scx  : acc=0.540 (27/50)  ctx_tok=1035  tok_ratio=0.777
trunc: 已跳过（--no-trunc）
scx 保持率（full 答对中 scx 仍答对）: 25/30 = 0.833
压缩率分桶（cut = 1 - scx_tok/full_tok）:
  10-35%: 48 样本
  >35%: 2 样本
=== NEW-KERNEL trim-ratio 0.65 ===
=== GSM8K 8-shot 三方对照（Qwen2.5-3B-Instruct（千问） via MLX, n=50, seed=20261027）===
full : acc=0.600 (30/50)  ctx_tok=1332
scx  : acc=0.600 (30/50)  ctx_tok=1052  tok_ratio=0.790
trunc: 已跳过（--no-trunc）
scx 保持率（full 答对中 scx 仍答对）: 26/30 = 0.867
压缩率分桶（cut = 1 - scx_tok/full_tok）:
  10-35%: 49 样本
  >35%: 1 样本

## bench_trim_ratio_sweep.log

=== trim-ratio 0.2 ===
=== GSM8K 8-shot 三方对照（Qwen2.5-3B-Instruct（千问） via MLX, n=50, seed=20261027）===
full : acc=0.600 (30/50)  ctx_tok=1332
scx  : acc=0.480 (24/50)  ctx_tok=816  tok_ratio=0.613
trunc: 已跳过（--no-trunc）
scx 保持率（full 答对中 scx 仍答对）: 22/30 = 0.733
压缩率分桶（cut = 1 - scx_tok/full_tok）:
  10-35%: 15 样本
  >35%: 35 样本
=== trim-ratio 0.3 ===
=== GSM8K 8-shot 三方对照（Qwen2.5-3B-Instruct（千问） via MLX, n=50, seed=20261027）===
full : acc=0.600 (30/50)  ctx_tok=1332
scx  : acc=0.540 (27/50)  ctx_tok=866  tok_ratio=0.650
trunc: 已跳过（--no-trunc）
scx 保持率（full 答对中 scx 仍答对）: 25/30 = 0.833
压缩率分桶（cut = 1 - scx_tok/full_tok）:
  10-35%: 27 样本
  >35%: 23 样本
=== trim-ratio 0.5 ===
=== GSM8K 8-shot 三方对照（Qwen2.5-3B-Instruct（千问） via MLX, n=50, seed=20261027）===
full : acc=0.600 (30/50)  ctx_tok=1332
scx  : acc=0.540 (27/50)  ctx_tok=953  tok_ratio=0.716
trunc: 已跳过（--no-trunc）
scx 保持率（full 答对中 scx 仍答对）: 26/30 = 0.867
压缩率分桶（cut = 1 - scx_tok/full_tok）:
  10-35%: 41 样本
  >35%: 9 样本
=== trim-ratio 0.7 ===

=== GSM8K 8-shot 三方对照（Qwen2.5-3B-Instruct（千问） via MLX, n=50, seed=20261027）===
full : acc=0.600 (30/50)  ctx_tok=1332
scx  : acc=0.600 (30/50)  ctx_tok=1056  tok_ratio=0.792
trunc: 已跳过（--no-trunc）
scx 保持率（full 答对中 scx 仍答对）: 26/30 = 0.867
压缩率分桶（cut = 1 - scx_tok/full_tok）:
  10-35%: 50 样本

## bench_zh_keep_n26.log

[load] MLX 加载模型 Qwen2.5-3B-Instruct（千问） (/Users/fei/model/qwen2.5-3b-instruct) ...
[INFO] Quantized model with 8.501 bits per weight.
[load] 8-bit 量化完成（Qwen2.5-3B-Instruct（千问））
[data] 自定义集 bench_zh_cn40.csv: 40 行（demo 14 / test 26）
[1/26]
[5/26]
[10/26]
[15/26]
[20/26]
[25/26]

=== GSM8K 8-shot 三方对照（Qwen2.5-3B-Instruct（千问） via MLX, n=26, seed=20261027）===
full : acc=0.923 (24/26)  ctx_tok=1181
scx  : acc=0.923 (24/26)  ctx_tok=281  tok_ratio=0.238
trunc: 已跳过（--no-trunc）
scx 保持率（full 答对中 scx 仍答对）: 24/24 = 1.000
压缩率分桶（cut = 1 - scx_tok/full_tok）:
  >35%: 26 样本

## bench_zh_n26.log

[load] MLX 加载模型 Qwen2.5-3B-Instruct（千问） (/Users/fei/model/qwen2.5-3b-instruct) ...
[INFO] Quantized model with 8.501 bits per weight.
[load] 8-bit 量化完成（Qwen2.5-3B-Instruct（千问））
[data] 自定义集 bench_zh_cn40.csv: 40 行（demo 14 / test 26）
[1/26]
[5/26]
[10/26]
[15/26]
[20/26]
[25/26]
[csv] 明细已导出: bench_zh_n26.csv

=== GSM8K 8-shot 三方对照（Qwen2.5-3B-Instruct（千问） via MLX, n=26, seed=20261027）===
full : acc=0.923 (24/26)  ctx_tok=1181
scx  : acc=0.923 (24/26)  ctx_tok=279  tok_ratio=0.237
trunc: 已跳过（--no-trunc）
scx 保持率（full 答对中 scx 仍答对）: 24/24 = 1.000
压缩率分桶（cut = 1 - scx_tok/full_tok）:
  >35%: 26 样本

## bench_zh_n30.log

[load] MLX 加载模型 Qwen2.5-3B-Instruct（千问） (/Users/fei/model/qwen2.5-3b-instruct) ...
[INFO] Quantized model with 8.501 bits per weight.
[load] 8-bit 量化完成（Qwen2.5-3B-Instruct（千问））
[data] 自定义集 bench_zh_cn30.csv: 30 行（demo 8 / test 22）
[1/30]
[5/30]
[10/30]
[15/30]
[20/30]
[csv] 明细已导出: bench_zh_n30.csv

=== GSM8K 8-shot 三方对照（Qwen2.5-3B-Instruct（千问） via MLX, n=22, seed=20261027）===
full : acc=0.955 (21/22)  ctx_tok=720
scx  : acc=0.955 (21/22)  ctx_tok=720  tok_ratio=1.000
trunc: acc=0.955 (21/22)  ctx_tok=同 scx
scx 保持率（full 答对中 scx 仍答对）: 21/21 = 1.000
压缩率分桶（cut = 1 - scx_tok/full_tok）:
  cut<2%: 22 样本
注：22/22 样本 scx 未压缩（透传/失败），已按 fail-open 计
