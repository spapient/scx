# 从 compact 包中抽出唯一需要人判 1/0 的“同桶跨事故”候选。
# target / 同来源=2 是裁决规则；异桶=0；这里只留弱相关边界。
param(
    [string]$Packet = (Join-Path $PSScriptRoot "v2-judge-compact.tsv"),
    [string]$Baseline = (Join-Path $PSScriptRoot "v2-baseline.json"),
    [string]$MemDir = (Join-Path $PSScriptRoot "v2-mem"),
    [string]$Out = (Join-Path $PSScriptRoot "v2-judge-review-weak.tsv")
)
$ErrorActionPreference = "Stop"

$info = @{}
$tail = @{}
foreach ($d in Get-ChildItem -LiteralPath (Join-Path $MemDir "cases") -Directory) {
    $cj = Join-Path $d.FullName "case.json"
    if (-not (Test-Path -LiteralPath $cj)) { continue }
    $c = Get-Content -LiteralPath $cj -Raw -Encoding UTF8 | ConvertFrom-Json
    $s = ""
    foreach ($line in ([string]$c.text -split "`n")) {
        if ($line -match '来源:\s*(https?://\S+)') { $s = $matches[1]; break }
    }
    $id = [string]$c.id
    $info[$id] = @{ src = $s; bucket = [string]$c.bucket }
    $tail[($id -split '_')[-1]] = $id
}
$base = Get-Content -LiteralPath $Baseline -Raw -Encoding UTF8 | ConvertFrom-Json
$targets = @($base.cases | ForEach-Object { [string]$_.expected[0] })
$rows = @(Import-Csv -LiteralPath $Packet -Delimiter "`t" -Encoding UTF8)
$results = New-Object System.Collections.ArrayList
foreach ($r in $rows) {
    $ci = [int]$r.case_index
    $t = $targets[$ci]
    $full = if ($tail.ContainsKey([string]$r.candidate)) { $tail[[string]$r.candidate] } else { [string]$r.candidate }
    if (-not $info.ContainsKey($full)) { continue }
    $sameSrc = ($info[$full].src -eq $info[$t].src)
    $sameBucket = ($info[$full].bucket -eq $info[$t].bucket)
    if ($full -ne $t -and -not $sameSrc -and $sameBucket) {
        [void]$results.Add([pscustomobject]@{
            case_index = $ci; layer = $r.layer; query = $r.query
            candidate = $r.candidate; summary = $r.summary; fix = $r.fix
            rel = ""; note = "same-bucket cross-incident: 1=related but not reusable, 0=unrelated"
        })
    }
}
$results | Export-Csv -LiteralPath $Out -Delimiter "`t" -NoTypeInformation -Encoding UTF8
Write-Output ("wrote {0} weak-boundary rows -> {1}" -f $results.Count, $Out)
