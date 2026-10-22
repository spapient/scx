# 解析 `--eval-verbose` 输出，逐 case 比较 A/B 中 expected 的排名。
# 只读诊断：不改 baseline / mem。输出 CSV 供差异归因。
param(
    [string]$Verbose = (Join-Path $PSScriptRoot "v2-eval-verbose.txt"),
    [string]$Baseline = (Join-Path $PSScriptRoot "v2-baseline-draft.json"),
    [string]$Out = (Join-Path $PSScriptRoot "v2-rank-diff.csv")
)
$ErrorActionPreference = "Stop"

$base = Get-Content -LiteralPath $Baseline -Raw -Encoding UTF8 | ConvertFrom-Json
$cases = @($base.cases)
$lines = Get-Content -LiteralPath $Verbose -Encoding UTF8
$rows = New-Object System.Collections.ArrayList
$ci = -1
$side = ""
$ranks = @{ A = @{}; B = @{} }

function Flush-Case {
    if ($script:ci -lt 0) { return }
    $c = $script:cases[$script:ci]
    $target = [string]$c.expected[0]
    $tail = ($target -split '_')[-1]
    $a = if ($script:ranks.A.ContainsKey($tail)) { [int]$script:ranks.A[$tail] } else { 0 }
    $b = if ($script:ranks.B.ContainsKey($tail)) { [int]$script:ranks.B[$tail] } else { 0 }
    [void]$script:rows.Add([pscustomobject]@{
        case_index = $script:ci
        layer = [string]$c.layer
        query = [string]$c.query
        target = $tail
        A_rank = $a
        B_rank = $b
        A_miss = ($a -eq 0)
        B_miss = ($b -eq 0)
        delta_rank = (($(if ($a -eq 0) { 11 } else { $a })) - ($(if ($b -eq 0) { 11 } else { $b })))
    })
}

foreach ($line in $lines) {
    if ($line -match '^case (\d+)/(\d+) q="(.*)"$') {
        Flush-Case
        $script:ci = [int]$matches[1] - 1
        $script:ranks = @{ A = @{}; B = @{} }
        $script:side = ""
        continue
    }
    if ($line -match '^\s+A: (.*)$') { $script:side = "A"; $body = $matches[1] }
    elseif ($line -match '^\s+B: (.*)$') { $script:side = "B"; $body = $matches[1] }
    else { continue }
    foreach ($part in ($body -split '\s+\|\s+')) {
        if ($part -match '^\s*(\d+)\)\s+([0-9a-f]{8,})') {
            $script:ranks[$script:side][$matches[2]] = [int]$matches[1]
        }
    }
}
Flush-Case

$rows | Export-Csv -LiteralPath $Out -NoTypeInformation -Encoding UTF8
Write-Output ("wrote {0} rows -> {1}" -f $rows.Count, $Out)
Write-Output "A_miss_only vs B_miss_only --"
$rows | Group-Object layer | ForEach-Object {
    $g = $_.Group
    $am = @($g | Where-Object { $_.A_miss -and -not $_.B_miss }).Count
    $bm = @($g | Where-Object { $_.B_miss -and -not $_.A_miss }).Count
    $both = @($g | Where-Object { $_.A_miss -and $_.B_miss }).Count
    Write-Output ("{0}: n={1} A_miss_only={2} B_miss_only={3} both_miss={4}" -f $_.Name, $g.Count, $am, $bm, $both)
}
