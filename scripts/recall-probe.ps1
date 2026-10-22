# Deterministic recall diagnostics for the synthetic eval-real fixture.
#
# These probes are NOT independent M4 evidence: targets are derived from the
# fixture's known case ids, and the multi-rel labels are topic rules, not human
# adjudication. They are for regression hunting and A/B mechanism checks only.
param(
    [string]$Csv = (Join-Path $PSScriptRoot "..\eval-real\cases.csv"),
    [string]$MemDir = (Join-Path $PSScriptRoot "..\eval-real\mem"),
    [string]$Binary = (Join-Path $PSScriptRoot "..\rs\target\release\scx.exe"),
    [string]$OutDir = (Join-Path $env:TEMP "scx-recall-probe")
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $Csv)) { throw "missing CSV: $Csv" }
if (-not (Test-Path -LiteralPath $MemDir)) { throw "missing mem dir: $MemDir" }
if (-not (Test-Path -LiteralPath $Binary)) { throw "missing binary: $Binary" }
if (-not (Test-Path -LiteralPath $OutDir)) {
    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
}

$rows = @(Import-Csv -LiteralPath $Csv -Encoding UTF8)
$idByQuery = @{}
foreach ($r in $rows) {
    $idByQuery[[string]$r.query] = [string]$r.case_id
}

function Write-Json([string]$Path, $Value) {
    $json = $Value | ConvertTo-Json -Depth 10
    [IO.File]::WriteAllText($Path, $json, (New-Object Text.UTF8Encoding $false))
}

function Add-Case($Cases, [string]$Query, [string]$Layer, [object[]]$Targets) {
    $rel = [ordered]@{}
    $expected = @()
    foreach ($t in $Targets) {
        $cid = $idByQuery[[string]$t.q]
        if ([string]::IsNullOrWhiteSpace($cid)) {
            throw "unknown target query: $($t.q)"
        }
        $rel[$cid] = [int]$t.r
        $expected += $cid
    }
    $Cases.Add([pscustomobject][ordered]@{
        query = $Query
        layer = $Layer
        expected = $expected
        rel = $rel
        judge = [ordered]@{ by = "probe"; date = "2026-10-23T00:00:00Z" }
    }) | Out-Null
}

# 1) Truncation/robustness set: first 2/3 tokens + digits removed, target known.
$hard = New-Object System.Collections.ArrayList
foreach ($r in $rows) {
    $q = [string]$r.query
    $id = [string]$r.case_id
    $parts = @($q -split '\s+' | Where-Object { $_ })
    $variants = @()
    if ($parts.Count -ge 2) {
        $variants += ($parts[0..1] -join ' ')
        if ($parts.Count -ge 3) { $variants += ($parts[0..2] -join ' ') }
    }
    $variants += ($q -replace '[0-9]+', '')
    $variants = @($variants | Select-Object -Unique)
    foreach ($v in $variants) {
        Add-Case $hard $v.Trim() ([string]$r.layer) @(@{ q = $q; r = 2 })
    }
}
$hardPath = Join-Path $OutDir "hard.json"
Write-Json $hardPath ([ordered]@{ cases = @($hard) })

# 2) Small multi-rel diagnostic: one generic query spans several topics.
$multi = New-Object System.Collections.ArrayList
$defs = @(
    @{ q = "timeout"; targets = @(@{q="database query timeout after 30 seconds";r=2},@{q="storage read timeout on device sda";r=2},@{q="证书过期导致 tls 握手失败";r=2}) },
    @{ q = "disk"; targets = @(@{q="disk full inode exhausted";r=2},@{q="文件系统只读 read-only 导致写入失败";r=1},@{q="storage read timeout on device sda";r=1}) },
    @{ q = "memory"; targets = @(@{q="Java 进程内存耗尽 out of memory 崩溃";r=2},@{q="oom killer invoked";r=2},@{q="垃圾回收 gc 长时间停顿导致应用卡顿";r=1}) },
    @{ q = "connection"; targets = @(@{q="db pool exhausted conn";r=2},@{q="connection reset peer";r=1},@{q="upstream connection refused on 10.0.3.7 port 5432";r=1}) },
    @{ q = "test failure"; targets = @(@{q="pytest test_api assertion failed with status 500";r=2},@{q="jest 前端测试用例执行超时";r=2},@{q="flaky integration test";r=1}) },
    @{ q = "permission"; targets = @(@{q="部署脚本缺少 sudo 执行权限";r=2},@{q="镜像仓库 pull access denied 鉴权失败";r=1}) },
    @{ q = "certificate"; targets = @(@{q="ssl certificate expired";r=2},@{q="证书过期导致 tls 握手失败";r=1}) },
    @{ q = "build failure"; targets = @(@{q="cargo compilation failed for hello crate";r=2},@{q="maven package not found";r=1},@{q="return value mismatched types";r=1}) },
    @{ q = "config"; targets = @(@{q="服务启动缺少 config 配置项";r=2}) },
    @{ q = "rate limit"; targets = @(@{q="接口调用 rate limit 限流需要退避重试";r=2},@{q="queue overflow backlog";r=1}) }
)
foreach ($d in $defs) {
    Add-Case $multi ([string]$d.q) "semantic" @($d.targets)
}
$multiPath = Join-Path $OutDir "multi.json"
Write-Json $multiPath ([ordered]@{ cases = @($multi) })

Write-Output "diagnostic only (synthetic targets/rules, not M4 human evidence)"
Write-Output "hard=$hardPath cases=$($hard.Count)"
& $Binary --eval $hardPath --mem-dir $MemDir
if ($LASTEXITCODE -ne 0) { throw "hard probe failed: exit $LASTEXITCODE" }

Write-Output "multi=$multiPath cases=$($multi.Count)"
& $Binary --eval $multiPath --mem-dir $MemDir
if ($LASTEXITCODE -ne 0) { throw "multi probe failed: exit $LASTEXITCODE" }
