# Cluster-aware sensitivity for a ranked evaluation.
#
# The confirm5 corpus stores five sibling cases per incident. A case-level
# bootstrap treats those sibling rows as independent and therefore understates
# uncertainty. This script re-runs the MRR delta test by resampling incidents,
# not individual cases. It is a sensitivity check; it does not replace the
# pre-registered case-level primary analysis.
param(
    [string]$RankDiff = (Join-Path $PSScriptRoot "confirm5-rank-diff.csv"),
    [string]$Strata = (Join-Path $PSScriptRoot "confirm5-strata.csv"),
    [string]$Out = (Join-Path $PSScriptRoot "confirm5-significance-cluster.csv"),
    [int]$ClusterSize = 5,
    [int]$Boot = 10000,
    [int]$Perm = 10000,
    [double]$Mde = 0.05,
    [int]$Seed = 20261023
)
$ErrorActionPreference = "Stop"
if ($ClusterSize -lt 1) { throw "ClusterSize must be positive" }

$rows = @(Import-Csv -LiteralPath $RankDiff -Encoding UTF8 | ForEach-Object {
    $ar = [int]$_.A_rank
    $br = [int]$_.B_rank
    [pscustomobject]@{
        case_index = [int]$_.case_index
        layer = [string]$_.layer
        a_mrr = $(if ($ar -gt 0) { 1.0 / $ar } else { 0.0 })
        b_mrr = $(if ($br -gt 0) { 1.0 / $br } else { 0.0 })
        delta = $(if ($ar -gt 0) { 1.0 / $ar } else { 0.0 }) - $(if ($br -gt 0) { 1.0 / $br } else { 0.0 })
        incident = [int][math]::Floor([int]$_.case_index / $ClusterSize)
    }
})
$tier = @{}
foreach ($s in Import-Csv -LiteralPath $Strata -Encoding UTF8) {
    $tier[[int]$s.case_index] = [string]$s.tier
}
$hard = @($rows | Where-Object { $tier[$_.case_index] -eq "hard" })
if ($hard.Count -eq 0) { throw "no hard cases found" }

$clusters = @($hard | Group-Object incident)
$nCases = $hard.Count
$nClusters = $clusters.Count
$obs = ($hard | Measure-Object delta -Average).Average
$aMrr = ($hard | Measure-Object a_mrr -Average).Average
$bMrr = ($hard | Measure-Object b_mrr -Average).Average

$rng = New-Object System.Random $Seed
$bootMeans = [System.Collections.Generic.List[double]]::new()
for ($b = 0; $b -lt $Boot; $b++) {
    $sum = 0.0
    $count = 0
    for ($i = 0; $i -lt $nClusters; $i++) {
        $g = $clusters[$rng.Next($nClusters)].Group
        foreach ($r in $g) {
            $sum += [double]$r.delta
            $count++
        }
    }
    $bootMeans.Add($sum / $count)
}
$sorted = @($bootMeans | Sort-Object)
$lo = [double]$sorted[[int][math]::Floor(0.025 * ($Boot - 1))]
$hi = [double]$sorted[[int][math]::Ceiling(0.975 * ($Boot - 1))]

$ge = 0
for ($p = 0; $p -lt $Perm; $p++) {
    $sum = 0.0
    $count = 0
    foreach ($g in $clusters) {
        $sign = if ($rng.NextDouble() -ge 0.5) { 1.0 } else { -1.0 }
        foreach ($r in $g.Group) {
            $sum += $sign * [double]$r.delta
            $count++
        }
    }
    if ([math]::Abs($sum / $count) -ge [math]::Abs($obs)) { $ge++ }
}
$pval = ($ge + 1.0) / ($Perm + 1.0)

$result = [pscustomobject]@{
    group = "tier:hard:cluster"
    n_cases = $nCases
    n_clusters = $nClusters
    A_mrr = [math]::Round($aMrr, 4)
    B_mrr = [math]::Round($bMrr, 4)
    delta = [math]::Round($obs, 4)
    ci_lo = [math]::Round($lo, 4)
    ci_hi = [math]::Round($hi, 4)
    perm_p = [math]::Round($pval, 4)
    mde = $Mde
    advantage = (($lo -gt 0) -and ($obs -ge $Mde) -and ($pval -lt 0.05))
}
@($result) | Export-Csv -LiteralPath $Out -NoTypeInformation -Encoding UTF8
$result | Format-Table -AutoSize
Write-Output ("wrote {0}" -f $Out)
Write-Output ("cluster sensitivity: {0} cases / {1} incidents; resampled incidents with replacement" -f $nCases, $nClusters)
