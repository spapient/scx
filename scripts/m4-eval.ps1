# M4 自动标注回归评估（可复现语料，Windows PowerShell）
#
# 用途：固定 24-case 语料 → --annotate → --eval，输出三层 A/B 指标与
# 自检漏斗（emitted / self-check dropped / no-synonym / no-root）。
#
# 口径警告：自动标注的 paraphrase/semantic 变体先经 A 管线自检放行，
# 因此该结果是“A 已能召回的变体上的回归集”，不是独立质量证据，
# 不能单独用来证明 A 优于 B，也不能替代人工抽样复核。
#
# 用法：powershell -ExecutionPolicy Bypass -File scripts/m4-eval.ps1
$ErrorActionPreference = "Stop"

function Write-Utf8([string]$path, [string]$text) {
    [IO.File]::WriteAllText($path, $text, (New-Object Text.UTF8Encoding $false))
}

$scx = Join-Path $PSScriptRoot "..\rs\target\release\scx.exe"
if (-not (Test-Path $scx)) {
    # 自举脚本把 release 二进制安装到 bin/；两种构建路径都支持。
    $scx = Join-Path $PSScriptRoot "..\bin\scx.exe"
}
if (-not (Test-Path $scx)) {
    Write-Error "未找到 release 二进制：先运行 cd rs && cargo build --release --locked，或 sh build.sh"
}

# 只清理脚本自己的临时目录；先验证解析结果确实位于 $env:TEMP 下。
$mem = Join-Path $env:TEMP "scx-m4-eval"
$resolved = [IO.Path]::GetFullPath($mem)
$tempRoot = [IO.Path]::GetFullPath($env:TEMP)
if (-not $resolved.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase)) {
    Write-Error "拒绝清理非 TEMP 路径: $resolved"
}
if (Test-Path $resolved) { Remove-Item -Recurse -Force -LiteralPath $resolved }
New-Item -ItemType Directory -Force $resolved | Out-Null

# 24-case 可复现语料：16 英文运维场景 + 8 中文/中英混排场景。
$cases = @(
    @{ q = "db timeout conn_id"; t = "ERROR db timeout conn_id=fkab pool exhausted`nat Pool.java:214" },
    @{ q = "db write errno no space"; t = "ERROR db write errno=28 no space left`nat Disk.java:77" },
    @{ q = "cargo build compile fail"; t = "error: could not compile hello`nat Pool.java:214" },
    @{ q = "disk full inode"; t = "ERROR disk full inode exhausted`nat Fs.java:9" },
    @{ q = "out of memory heap"; t = "FATAL out of memory heap 4g`nat Mem.java:12" },
    @{ q = "connection reset peer"; t = "ERROR connection reset by peer errno=104`nat Net.java:3" },
    @{ q = "dns lookup failed host"; t = "ERROR dns lookup failed host=api.example.com`nat Net.java:8" },
    @{ q = "tls handshake timeout"; t = "ERROR tls handshake timeout cert expired`nat Tls.java:5" },
    @{ q = "deadlock detected"; t = "ERROR deadlock detected wait chain`nat Lock.java:11" },
    @{ q = "queue overflow backlog"; t = "ERROR queue overflow backlog limit`nat Q.java:2" },
    @{ q = "command not found kubectl"; t = "bash: line 3: command not found: kubectl`nat deploy.sh" },
    @{ q = "port already in use"; t = "ERROR port 8080 already in use`nat Sock.java:1" },
    @{ q = "maven package not found"; t = "error: package foo does not exist`nat Build.java:214" },
    @{ q = "pytest failed test_api"; t = "FAILED test_api - AssertionError: status 500`nat test_api.py:42" },
    @{ q = "rust mismatched types"; t = "error[E0308]: mismatched types`nat main.rs:10" },
    @{ q = "disk io read timeout"; t = "ERROR io error read timeout dev=sda`nat Fs.java:4" },
    @{ q = "存储空间不足"; t = "ERROR disk full 存储空间不足`nat Fs.java:9" },
    @{ q = "连接pool最小空闲不足"; t = "ERROR pool min idle below threshold`nat Pool.java:214" },
    @{ q = "数据库连接超时"; t = "ERROR db timeout conn_id=fkab pool exhausted`nat Pool.java:214" },
    @{ q = "磁盘inode耗尽"; t = "ERROR disk full inode exhausted`nat Fs.java:9" },
    @{ q = "网络连接被重置"; t = "ERROR connection reset by peer errno=104`nat Net.java:3" },
    @{ q = "依赖包找不到"; t = "error: package foo does not exist`nat Build.java:214" },
    @{ q = "内存溢出"; t = "FATAL out of memory heap 4g`nat Mem.java:12" },
    @{ q = "队列积压"; t = "ERROR queue overflow backlog limit`nat Q.java:2" }
)

$i = 0
# Windows PowerShell 5.1 会把原生 stderr 当错误记录；这里显式看 exit code。
$ErrorActionPreference = "Continue"
foreach ($c in $cases) {
    $log = Join-Path $resolved "in$i.txt"
    Write-Utf8 $log ($c.t + "`n")
    & $scx --query $c.q --store 1 --mem-dir $resolved $log 2>$null | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "store failed: $($c.q)" }
    $i++
}

$baseline = Join-Path $resolved "baseline.json"
& $scx --annotate --seed 0 --annotate-max 0 --annotate-out $baseline --mem-dir $resolved
if ($LASTEXITCODE -ne 0) { throw "annotate failed" }
& $scx --eval $baseline --mem-dir $resolved
if ($LASTEXITCODE -ne 0) { throw "eval failed" }

Write-Host ""
Write-Host "口径：自动标注在 A 的 top-k 内自检放行；结果是回归集，不是独立质量证据。" -ForegroundColor Yellow
Write-Host "库仍在 $resolved（可删可留）。"
