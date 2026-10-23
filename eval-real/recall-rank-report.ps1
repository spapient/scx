# 从盲测包计算 target 排名分布（包内 candidate 顺序 = 召回排名）。
param(
    [string]$Csv = (Join-Path $PSScriptRoot "real-cases.csv"),
    [string]$Packet = (Join-Path $PSScriptRoot "judge-packet-real.tsv")
)
$rows = @(Import-Csv -LiteralPath $Csv -Encoding UTF8)
$pkt = @(Import-Csv -LiteralPath $Packet -Delimiter "`t" -Encoding UTF8)
$byCase = @{}
foreach ($p in $pkt) {
    $ci = [int]$p.case_index
    if (-not $byCase.ContainsKey($ci)) { $byCase[$ci] = New-Object System.Collections.ArrayList }
    [void]$byCase[$ci].Add([string]$p.candidate)
}
$stats = @{ literal = @(0,0,0); paraphrase = @(0,0,0); semantic = @(0,0,0) }
for ($i = 0; $i -lt $rows.Count; $i++) {
    $target = [string]$rows[$i].case_id
    $layer = [string]$rows[$i].layer
    $rank = -1
    if ($byCase.ContainsKey($i)) {
        $idx = $byCase[$i].IndexOf($target)
        if ($idx -ge 0) { $rank = $idx + 1 }
    }
    if ($rank -ge 1 -and $rank -le 1) { $stats[$layer][0]++ }
    if ($rank -ge 1 -and $rank -le 5) { $stats[$layer][1]++ }
    if ($rank -ge 1 -and $rank -le 10) { $stats[$layer][2]++ }
    if ($rank -gt 5) {
        Write-Output ("  low-rank case[{0}] {1} rank={2} query={3}" -f $i, $layer, $rank, $rows[$i].query)
    }
}
Write-Output ("layer      r@1  r@5  r@10")
foreach ($l in @("literal","paraphrase","semantic")) {
    $n = @($rows | Where-Object { $_.layer -eq $l }).Count
    Write-Output ("{0,-10} {1,3}/{4} {2,3}/{4} {3,3}/{4}" -f $l, $stats[$l][0], $stats[$l][1], $stats[$l][2], $n)
}
