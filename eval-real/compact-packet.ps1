# 把 top-10 盲测包压缩成人工可审的候选集：
# - 每条 case 保留 target + 2 个同来源兄弟 + 3 个同桶跨事故 near-miss + 1 个异桶对照；
# - literal 只保留 target + 1 个对照（literal 目标已由脚本给 rel=2，人工只需抽检）；
# - 行内确定性打乱，避免 target 永远在首行暴露。
param(
    [string]$Packet = (Join-Path $PSScriptRoot "v2-judge-packet.tsv"),
    [string]$Baseline = (Join-Path $PSScriptRoot "v2-baseline.json"),
    [string]$MemDir = (Join-Path $PSScriptRoot "v2-mem"),
    [string]$Out = (Join-Path $PSScriptRoot "v2-judge-compact.tsv")
)
$ErrorActionPreference = "Stop"

function Get-Key([string]$s) {
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        $bytes = $sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($s))
        ([BitConverter]::ToString($bytes)).Replace("-", "")
    } finally { $sha.Dispose() }
}

$info = @{}
$tail = @{}
foreach ($d in Get-ChildItem -LiteralPath (Join-Path $MemDir "cases") -Directory) {
    $cj = Join-Path $d.FullName "case.json"
    if (-not (Test-Path -LiteralPath $cj)) { continue }
    $c = Get-Content -LiteralPath $cj -Raw -Encoding UTF8 | ConvertFrom-Json
    $src = ""
    foreach ($line in ([string]$c.text -split "`n")) {
        if ($line -match '来源:\s*(https?://\S+)') { $src = $matches[1]; break }
    }
    $id = [string]$c.id
    $info[$id] = @{ src = $src; bucket = [string]$c.bucket }
    $tail[($id -split '_')[-1]] = $id
}

$base = Get-Content -LiteralPath $Baseline -Raw -Encoding UTF8 | ConvertFrom-Json
$cases = @($base.cases)
$rows = @(Import-Csv -LiteralPath $Packet -Delimiter "`t" -Encoding UTF8)
$byCase = @{}
foreach ($r in $rows) {
    $ci = [int]$r.case_index
    if (-not $byCase.ContainsKey($ci)) { $byCase[$ci] = New-Object System.Collections.ArrayList }
    [void]$byCase[$ci].Add($r)
}

$rowsOut = New-Object System.Collections.ArrayList
$row = 1
for ($ci = 0; $ci -lt $cases.Count; $ci++) {
    $target = [string]$cases[$ci].expected[0]
    $ti = $info[$target]
    $layer = [string]$cases[$ci].layer
    $pool = @($byCase[$ci])
    $picked = New-Object System.Collections.ArrayList
    $seen = @{}
    foreach ($r in $pool) {
        $full = if ($tail.ContainsKey([string]$r.candidate)) { $tail[[string]$r.candidate] } else { [string]$r.candidate }
        if ($full -eq $target) {
            [void]$picked.Add($r); $seen[$full] = $true; break
        }
    }
    $same = @($pool | Where-Object {
        $f = if ($tail.ContainsKey([string]$_.candidate)) { $tail[[string]$_.candidate] } else { [string]$_.candidate }
        $f -ne $target -and $info.ContainsKey($f) -and $info[$f].src -eq $ti.src
    } | Select-Object -First 2)
    $near = @($pool | Where-Object {
        $f = if ($tail.ContainsKey([string]$_.candidate)) { $tail[[string]$_.candidate] } else { [string]$_.candidate }
        $f -ne $target -and $info.ContainsKey($f) -and $info[$f].src -ne $ti.src -and $info[$f].bucket -eq $ti.bucket
    } | Select-Object -First 3)
    $contrast = @($pool | Where-Object {
        $f = if ($tail.ContainsKey([string]$_.candidate)) { $tail[[string]$_.candidate] } else { [string]$_.candidate }
        $f -ne $target -and $info.ContainsKey($f) -and $info[$f].bucket -ne $ti.bucket
    } | Select-Object -First 1)
    $take = @($picked) + @($same) + @($near) + $(if ($layer -ne "literal") { @($contrast) } else { @() })
    if ($layer -eq "literal" -and $take.Count -eq 1) {
        $other = @($pool | Where-Object {
            $f = if ($tail.ContainsKey([string]$_.candidate)) { $tail[[string]$_.candidate] } else { [string]$_.candidate }
            $f -ne $target
        } | Select-Object -First 1)
        $take += $other
    }
    $ordered = @($take | Sort-Object { Get-Key ("{0}:{1}" -f $ci, $_.candidate) })
    foreach ($r in $ordered) {
        [void]$rowsOut.Add([pscustomobject]@{
            row = $row; case_index = $ci; layer = $r.layer; query = $r.query
            candidate = $r.candidate; summary = $r.summary; fix = $r.fix; rel = ""
        })
        $row++
    }
}
$rowsOut | Export-Csv -LiteralPath $Out -Delimiter "`t" -NoTypeInformation -Encoding UTF8
Write-Output ("wrote {0} compact rows ({1} cases) -> {2}" -f $rowsOut.Count, $cases.Count, $Out)
