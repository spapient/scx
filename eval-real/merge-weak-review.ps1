# 把人工填写的 312 行弱相关边界（1/0）合并进 compact 机器草案。
# target/同来源=2、异桶=0 仍由草案提供；人工只覆盖同桶跨事故边界。
param(
    [string]$Draft = (Join-Path $PSScriptRoot "v2-judge-compact-draft.tsv"),
    [string]$Weak = (Join-Path $PSScriptRoot "v2-judge-review-weak-fei.tsv"),
    [string]$Out = (Join-Path $PSScriptRoot "v2-judge-compact-filled.tsv")
)
$ErrorActionPreference = "Stop"

$draftRows = @(Import-Csv -LiteralPath $Draft -Delimiter "`t" -Encoding UTF8)
$weakRows = @(Import-Csv -LiteralPath $Weak -Delimiter "`t" -Encoding UTF8)
$override = @{}
foreach ($w in $weakRows) {
    $rel = ([string]$w.rel).Trim()
    if ($rel -eq "" -or $rel -eq "s") { continue }
    if ($rel -notin @("0", "1")) { throw "weak rel must be 0/1/s, got '$rel'" }
    $override[("{0}|{1}" -f $w.case_index, $w.candidate)] = $rel
}
$n = 0
foreach ($d in $draftRows) {
    $k = ("{0}|{1}" -f $d.case_index, $d.candidate)
    if ($override.ContainsKey($k)) { $d.rel = $override[$k]; $n++ }
}
$draftRows | Export-Csv -LiteralPath $Out -Delimiter "`t" -NoTypeInformation -Encoding UTF8
Write-Output ("merged {0}/{1} weak overrides -> {2}" -f $n, $weakRows.Count, $Out)
