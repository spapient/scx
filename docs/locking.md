# 记忆库锁序契约（Lock Order Contract）

多进程共享同一 `--mem-dir` 时，记忆库用锁文件做跨进程互斥（`src/memory/lock.rs`，
`create_new` + token + 超时/陈旧回收）。**锁序是契约**：任何路径同时持有多把锁时，
必须按下面的固定顺序获取；`acquire_all` 是唯一允许的多锁入口，禁止逐把散取。

## 锁清单

| 锁名（相对 mem-dir 根） | 保护对象 | 粒度 |
|---|---|---|
| `cases/{id}.lock` | 单个 case 目录（store 去重 / rate / delete 串行化） | 最细 |
| `index.lock` | 索引构建 / 作废（rebuild ↔ delete 互斥） | 细 |
| `meta.lock` | `meta.toml` read-modify-write（gen / case_count / df / params / l1） | 粗 |
| `qmap.lock` | `qmap.toml` 查询缓存 read-modify-write | 粗 |
| `fingerprints.lock` | `index/fingerprints.toml` 指纹索引 | 最粗（独立路径） |

## 固定顺序（LOCK ORDER）

```
cases/{id}.lock  →  index.lock  →  meta.lock  →  qmap.lock  →  fingerprints.lock
```

规则：
1. **细粒度在前，粗粒度在后**。任何多锁路径必须按此序获取。
2. **多锁一律走 `lock::acquire_all(root, &[...])`**：按序一次取齐；任一把失败时
   已持锁随守卫 `Drop` 逆序自动释放，绝不半持锁返回。
3. **已持锁后禁止再次获取**：持 `meta.lock` 时写 meta 一律调 `*_locked` 变体
   （`meta::bump_gen_locked` / `store::bump_case_count_locked`），否则自死锁。
4. 单锁路径不受序约束（只拿一把），但**不得**在持锁后改为获取更高序的锁。

## 当前唯一多锁路径

- `maintenance::delete_case`：`acquire_all([cases/{id}.lock, index.lock, meta.lock,
  qmap.lock])` → 删目录 → `invalidate_locked` → `bump_case_count_locked` →
  `invalidate_fp_index` → `bump_gen_locked` → 删 qmap。

新增多锁路径前先更新本表；违例 = 死锁风险，评审门禁拦截。

## 违规后果

两路径按相反序取锁 → 互相等待形成环 → 各自在 `LOCK_WAIT`(10s) 超时后报
`WouldBlock`（不是永久挂死，但功能不可用）。固定序保证环不可能出现。
