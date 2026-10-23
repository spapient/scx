# 由已裁决 baseline 回填盲测包：保留候选/摘要/fix，写 rel 列。
param(
    [string]$Packet = (Join-Path $PSScriptRoot "judge-packet-real.tsv"),
    [string]$Baseline = (Join-Path $PSScriptRoot "real-baseline-arb.json"),
    [string]$Out = (Join-Path $PSScriptRoot "judge-packet-filled.tsv")
)
$ErrorActionPreference = "Stop"
$cases = @((Get-Content -LiteralPath $Baseline -Raw -Encoding UTF8 | ConvertFrom-Json).cases)
$rows = @(Import-Csv -LiteralPath $Packet -Delimiter "`t" -Encoding UTF8)
$sb = New-Object System.Text.StringBuilder
[void]$sb.AppendLine("row`tcase_index`tlayer`tquery`tcandidate`tsummary`tfix`trel")
foreach ($r in $rows) {
    $ci = [int]$r.case_index
    $rel = 0
    $p = $cases[$ci].rel.PSObject.Properties[[string]$r.candidate]
    if ($p) { $rel = [int]$p.Value }
    [void]$sb.AppendLine(
        ("{0}`t{1}`t{2}`t{3}`t{4}`t{5}`t{6}`t{7}" -f
            $r.row, $r.case_index, $r.layer, $r.query, $r.candidate, $r.summary, $r.fix, $rel)
    )
}
[IO.File]::WriteAllText($Out, $sb.ToString(), (New-Object Text.UTF8Encoding $false))
$d = @{ 0 = 0; 1 = 0; 2 = 0 }
foreach ($r in $rows) {
    $ci = [int]$r.case_index
    $p = $cases[$ci].rel.PSObject.Properties[[string]$r.candidate]
    $v = if ($p) { [int]$p.Value } else { 0 }
    $d[$v]++
}
Write-Output ("filled: rel2={0} rel1={1} rel0={2} -> {3}" -f $d[2], $d[1], $d[0], $Out)
