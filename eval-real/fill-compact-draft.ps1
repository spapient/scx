# 为 compact 包填机器草案：target / 同来源 = 2，其余 = 0。
# 明确不是人工证据；裁决人必须逐条审改后再用 judge-audit.ps1 -Import。
param(
    [string]$Packet = (Join-Path $PSScriptRoot "v2-judge-compact.tsv"),
    [string]$Baseline = (Join-Path $PSScriptRoot "v2-baseline.json"),
    [string]$MemDir = (Join-Path $PSScriptRoot "v2-mem"),
    [string]$Out = (Join-Path $PSScriptRoot "v2-judge-compact-draft.tsv")
)
$ErrorActionPreference = "Stop"

$src = @{}
foreach ($d in Get-ChildItem -LiteralPath (Join-Path $MemDir "cases") -Directory) {
    $cj = Join-Path $d.FullName "case.json"
    if (-not (Test-Path -LiteralPath $cj)) { continue }
    $c = Get-Content -LiteralPath $cj -Raw -Encoding UTF8 | ConvertFrom-Json
    $s = ""
    foreach ($line in ([string]$c.text -split "`n")) {
        if ($line -match '来源:\s*(https?://\S+)') { $s = $matches[1]; break }
    }
    $src[[string]$c.id] = $s
}
$base = Get-Content -LiteralPath $Baseline -Raw -Encoding UTF8 | ConvertFrom-Json
$targets = @($base.cases | ForEach-Object { [string]$_.expected[0] })
$rows = @(Import-Csv -LiteralPath $Packet -Delimiter "`t" -Encoding UTF8)
foreach ($r in $rows) {
    $ci = [int]$r.case_index
    $t = $targets[$ci]
    $same = ($src.ContainsKey([string]$r.candidate) -and $src.ContainsKey($t) -and $src[[string]$r.candidate] -ne "" -and $src[[string]$r.candidate] -eq $src[$t])
    $r.rel = if ([string]$r.candidate -eq $t -or $same) { "2" } else { "0" }
}
$rows | Export-Csv -LiteralPath $Out -Delimiter "`t" -NoTypeInformation -Encoding UTF8
Write-Output ("wrote draft rel -> {0} ({1} rows)" -f $Out, $rows.Count)
