# v2 预注册统计：MRR@10 为主，paired bootstrap 95% CI + permutation p。
# 输入：v2-rank-diff.csv（A/B 目标排名）+ v2-strata.csv（难度分层）。
# 判定：hard 层 ΔMRR 的 CI 下界 > 0、Δ >= MDE(0.05) 且 permutation p < 0.05
# 才可写“优势”；否则只能写“未观察到劣化/无显著优势”。
param(
    [string]$RankDiff = (Join-Path $PSScriptRoot "v2-rank-diff.csv"),
    [string]$Strata = (Join-Path $PSScriptRoot "v2-strata.csv"),
    [string]$Out = (Join-Path $PSScriptRoot "v2-significance.csv"),
    [int]$Boot = 10000,
    [int]$Perm = 10000,
    [double]$Mde = 0.05,
    [int]$Seed = 20261023
)
$ErrorActionPreference = "Stop"

$rows = @(Import-Csv -LiteralPath $RankDiff -Encoding UTF8)
$strat = @{}
foreach ($s in Import-Csv -LiteralPath $Strata -Encoding UTF8) { $strat[[int]$s.case_index] = $s }
foreach ($r in $rows) {
    $r | Add-Member -NotePropertyName tier -NotePropertyValue $strat[[int]$r.case_index].tier -Force
    $ar = [int]$r.A_rank; $br = [int]$r.B_rank
    $r | Add-Member -NotePropertyName a_mrr -NotePropertyValue $(if ($ar -gt 0) { 1.0 / $ar } else { 0.0 }) -Force
    $r | Add-Member -NotePropertyName b_mrr -NotePropertyValue $(if ($br -gt 0) { 1.0 / $br } else { 0.0 }) -Force
    $r | Add-Member -NotePropertyName delta -NotePropertyValue ($r.a_mrr - $r.b_mrr) -Force
}

function Analyze([string]$Name, $Data) {
    $d = @($Data)
    if ($d.Count -eq 0) { return }
    $obs = ($d | Measure-Object delta -Average).Average
    $deltas = [double[]]@($d | ForEach-Object { [double]$_.delta })
    $rng = New-Object System.Random $Seed
    $bootMeans = New-Object 'double[]' $Boot
    for ($b = 0; $b -lt $Boot; $b++) {
        $sum = 0.0
        for ($i = 0; $i -lt $deltas.Length; $i++) {
            $idx = [int][math]::Floor($rng.NextDouble() * $deltas.Length)
            $sum += $deltas[$idx]
        }
        $bootMeans[$b] = $sum / $deltas.Length
    }
    [Array]::Sort($bootMeans)
    $lo = $bootMeans[[int][math]::Floor(0.025 * ($Boot - 1))]
    $hi = $bootMeans[[int][math]::Ceiling(0.975 * ($Boot - 1))]
    $ge = 0
    for ($p = 0; $p -lt $Perm; $p++) {
        $sum = 0.0
        foreach ($x in $deltas) {
            if ($rng.NextDouble() -ge 0.5) { $sum += $x } else { $sum -= $x }
        }
        if ([math]::Abs($sum / $deltas.Length) -ge [math]::Abs($obs)) { $ge++ }
    }
    $pval = ($ge + 1.0) / ($Perm + 1.0)
    [pscustomobject]@{
        group = $Name
        n = $d.Count
        A_mrr = [math]::Round(($d | Measure-Object a_mrr -Average).Average, 4)
        B_mrr = [math]::Round(($d | Measure-Object b_mrr -Average).Average, 4)
        delta = [math]::Round($obs, 4)
        ci_lo = [math]::Round($lo, 4)
        ci_hi = [math]::Round($hi, 4)
        perm_p = [math]::Round($pval, 4)
        mde = $Mde
        advantage = (($lo -gt 0) -and ($obs -ge $Mde) -and ($pval -lt 0.05))
    }
}

$results = New-Object System.Collections.ArrayList
foreach ($g in ($rows | Group-Object layer)) { [void]$results.Add((Analyze $g.Name $g.Group)) }
foreach ($g in ($rows | Group-Object tier)) { [void]$results.Add((Analyze ("tier:" + $g.Name) $g.Group)) }
[void]$results.Add((Analyze "all" $rows))
$results | Export-Csv -LiteralPath $Out -NoTypeInformation -Encoding UTF8
$results | Format-Table -AutoSize
Write-Output ("wrote {0} -> {1}" -f $results.Count, $Out)
Write-Output "decision rule: hard CI_lo>0, delta>=MDE and p<0.05 => advantage; otherwise no established advantage."
