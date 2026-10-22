# v2 难度分层 + near-miss 干扰统计。
# 难度：query 与目标 case 正文的 token Jaccard（easy >0.5 / medium 0.2-0.5 / hard <0.2）。
# near-miss：A top-10 中与目标同场景桶、但来源不同的 case（跨事故同域干扰）。
# 说明：Jaccard 是代理指标，不替代人工难度判断；中文按 CJK unigram+bigram 切分。
param(
    [string]$Baseline = (Join-Path $PSScriptRoot "v2-baseline-draft.json"),
    [string]$MemDir = (Join-Path $PSScriptRoot "v2-mem"),
    [string]$Verbose = (Join-Path $PSScriptRoot "v2-eval-verbose.txt"),
    [string]$Out = (Join-Path $PSScriptRoot "v2-strata.csv")
)
$ErrorActionPreference = "Stop"

$stop = @("the","a","an","of","to","and","in","on","for","with","by","from","after","before","was","were","is","are","be","that","this","it","as","at","into","during","caused","cause","causing")
function Get-Tokens([string]$s) {
    $set = New-Object 'System.Collections.Generic.HashSet[string]'
    foreach ($m in [regex]::Matches($s.ToLower(), '[a-z0-9][a-z0-9_\-\.]*')) {
        $t = $m.Value.Trim('.', '-', '_')
        if ($t.Length -ge 2 -and $stop -notcontains $t) { [void]$set.Add($t) }
    }
    foreach ($m in [regex]::Matches($s, '[\u4e00-\u9fff]+')) {
        $run = $m.Value
        for ($i = 0; $i -lt $run.Length; $i++) {
            [void]$set.Add($run.Substring($i, 1))
            if ($i + 1 -lt $run.Length) { [void]$set.Add($run.Substring($i, 2)) }
        }
    }
    $set
}
function Jaccard([string]$a, [string]$b) {
    $ta = Get-Tokens $a; $tb = Get-Tokens $b
    if ($ta.Count -eq 0 -and $tb.Count -eq 0) { return 0.0 }
    $inter = 0
    foreach ($t in $ta) { if ($tb.Contains($t)) { $inter++ } }
    $union = $ta.Count + $tb.Count - $inter
    if ($union -eq 0) { return 0.0 }
    [math]::Round($inter / $union, 4)
}
function Get-Tier([double]$j) {
    if ($j -gt 0.5) { "easy" } elseif ($j -ge 0.2) { "medium" } else { "hard" }
}

# 文档元数据
$info = @{}
$tailMap = @{}
foreach ($d in Get-ChildItem -LiteralPath (Join-Path $MemDir "cases") -Directory) {
    $cj = Join-Path $d.FullName "case.json"
    if (-not (Test-Path -LiteralPath $cj)) { continue }
    $c = Get-Content -LiteralPath $cj -Raw -Encoding UTF8 | ConvertFrom-Json
    $src = ""
    foreach ($line in ([string]$c.text -split "`n")) {
        if ($line -match '来源:\s*(https?://\S+)') { $src = $matches[1]; break }
    }
    $body = @(([string]$c.text -split "`n") | Where-Object { $_ -notmatch '^公开复盘' }) -join "`n"
    $info[[string]$c.id] = @{ bucket = [string]$c.bucket; src = $src; body = $body; query = [string]$c.query }
    $tailMap[([string]$c.id -split '_')[-1]] = [string]$c.id
}

# A top-10（来自 --eval-verbose）
$aRanks = @{}
$ci = -1
foreach ($line in Get-Content -LiteralPath $Verbose -Encoding UTF8) {
    if ($line -match '^case (\d+)/') { $ci = [int]$matches[1] - 1; $aRanks[$ci] = New-Object System.Collections.ArrayList; continue }
    if ($line -match '^\s+A: (.*)$' -and $ci -ge 0) {
        foreach ($part in ($matches[1] -split '\s+\|\s+')) {
            if ($part -match '^\s*(\d+)\)\s+([0-9a-f]{8,})') {
                [void]$aRanks[$ci].Add([pscustomobject]@{ rank = [int]$matches[1]; id = $matches[2] })
            }
        }
    }
}

$base = Get-Content -LiteralPath $Baseline -Raw -Encoding UTF8 | ConvertFrom-Json
$cases = @($base.cases)
$rows = New-Object System.Collections.ArrayList
for ($i = 0; $i -lt $cases.Count; $i++) {
    $c = $cases[$i]
    $target = [string]$c.expected[0]
    $ti = $info[$target]
    # 难度按“查询 vs 目标 case 的 query 摘要”计算；根因正文是中文补充说明，
    # 混入会把所有短 query 都压成 hard，失去分层意义。
    $body = $ti.query
    $j = Jaccard ([string]$c.query) $body
    $nm = New-Object System.Collections.ArrayList
    foreach ($h in $aRanks[$i]) {
        $full = if ($tailMap.ContainsKey($h.id)) { $tailMap[$h.id] } else { $h.id }
        if ($full -eq $target) { continue }
        $hi = $info[$full]
        if ($null -eq $hi) { continue }
        if ($hi.bucket -eq $ti.bucket -and $hi.src -ne $ti.src) {
            [void]$nm.Add(("{0}@r{1}" -f $h.id, $h.rank))
        }
    }
    [void]$rows.Add([pscustomobject]@{
        case_index = $i
        layer = [string]$c.layer
        query = [string]$c.query
        target = $target
        target_bucket = $ti.bucket
        jaccard = $j
        tier = Get-Tier $j
        near_miss_count = $nm.Count
        near_miss_ids = ($nm -join ";")
    })
}
$rows | Export-Csv -LiteralPath $Out -NoTypeInformation -Encoding UTF8
Write-Output ("wrote {0} rows -> {1}" -f $rows.Count, $Out)
foreach ($g in ($rows | Group-Object layer)) {
    $e = @($g.Group | Where-Object { $_.tier -eq "easy" }).Count
    $m = @($g.Group | Where-Object { $_.tier -eq "medium" }).Count
    $h = @($g.Group | Where-Object { $_.tier -eq "hard" }).Count
    $nm = @($g.Group | Where-Object { $_.near_miss_count -gt 0 }).Count
    Write-Output ("{0}: n={1} easy={2} medium={3} hard={4} with_near_miss={5}" -f $g.Name, $g.Count, $e, $m, $h, $nm)
}
