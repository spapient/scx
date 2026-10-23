# 校验 real-cases.csv 的 target 是否进入 judge-packet-real.tsv 的 top-k 候选。
param(
    [string]$Csv = (Join-Path $PSScriptRoot "real-cases.csv"),
    [string]$Packet = (Join-Path $PSScriptRoot "judge-packet-real.tsv")
)
$rows = @(Import-Csv -LiteralPath $Csv -Encoding UTF8)
$pkt = @(Import-Csv -LiteralPath $Packet -Delimiter "`t" -Encoding UTF8)
$byCase = @{}
foreach ($p in $pkt) {
    $ci = [int]$p.case_index
    if (-not $byCase.ContainsKey($ci)) { $byCase[$ci] = @{} }
    $byCase[$ci][[string]$p.candidate] = $true
}
$missing = @()
for ($i = 0; $i -lt $rows.Count; $i++) {
    $target = [string]$rows[$i].case_id
    if (-not $byCase.ContainsKey($i)) {
        $missing += "case[$i] no rows at all (query=$($rows[$i].query))"
    } elseif (-not $byCase[$i].ContainsKey($target)) {
        $missing += "case[$i] target not in top-k (query=$($rows[$i].query) target=$target)"
    }
}
Write-Output ("rows={0} distinct_case_index={1} targets_missing={2}" -f $pkt.Count, $byCase.Count, $missing.Count)
foreach ($m in $missing) { Write-Output ("  MISS " + $m) }
