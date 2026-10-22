# 双裁决人一致性：比对两个已回填 baseline（两名裁决人、同一 query/候选池）。
# 输出：共同候选数、原始一致率、Cohen's kappa（0/1/2 三类）、逐条分歧清单。
# 用法：powershell -NoProfile -ExecutionPolicy Bypass -File scripts\judge-agreement.ps1 -A a.json -B b.json
param(
    [Parameter(Mandatory = $true)][string]$A,
    [Parameter(Mandatory = $true)][string]$B
)

$ErrorActionPreference = "Stop"

function Read-Cases([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { throw "missing: $Path" }
    (Get-Content -LiteralPath $Path -Raw -Encoding UTF8 | ConvertFrom-Json).cases
}

$ca = @(Read-Cases $A)
$cb = @(Read-Cases $B)
if ($ca.Count -ne $cb.Count) { throw "baseline case counts differ: $($ca.Count) vs $($cb.Count)" }

$conf = @{}   # "0,0" -> count，3x3 混淆矩阵
$agree = 0
$total = 0
$disagree = @()

for ($i = 0; $i -lt $ca.Count; $i++) {
    $ra = $ca[$i].rel
    $rb = $cb[$i].rel
    foreach ($p in $ra.PSObject.Properties) {
        $id = $p.Name
        $raVal = [int]$p.Value
        $rbProp = $rb.PSObject.Properties | Where-Object Name -eq $id
        if ($null -eq $rbProp) { continue }  # 只比共同裁决过的候选
        $rbVal = [int]$rbProp.Value
        $key = "$raVal,$rbVal"
        if (-not $conf.ContainsKey($key)) { $conf[$key] = 0 }
        $conf[$key]++
        $total++
        if ($raVal -eq $rbVal) {
            $agree++
        } else {
            $disagree += [pscustomobject]@{ case = $i; query = [string]$ca[$i].query; candidate = $id; A = $raVal; B = $rbVal }
        }
    }
}

if ($total -eq 0) {
    Write-Output "judge-agreement: no common adjudicated candidates"
    exit 0
}

$raw = $agree / $total

# Cohen's kappa：3 类（0/1/2）。pe = Σ (行和×列和) / n^2
$rowSum = @(0, 0, 0)
$colSum = @(0, 0, 0)
foreach ($k in @($conf.Keys)) {
    $parts = $k -split ','
    $r = [int]$parts[0]; $c = [int]$parts[1]; $n = $conf[$k]
    $rowSum[$r] += $n
    $colSum[$c] += $n
}
$pe = 0.0
for ($j = 0; $j -lt 3; $j++) { $pe += ($rowSum[$j] * $colSum[$j]) }
$pe = $pe / ($total * $total)
$kappa = if ($pe -ge 1.0) { 1.0 } else { ($raw - $pe) / (1.0 - $pe) }

function Get-Conf([string]$k) { if ($conf.ContainsKey($k)) { $conf[$k] } else { 0 } }
Write-Output ("judge-agreement: common={0} raw={1:F3} kappa={2:F3}" -f $total, $raw, $kappa)
$off = (Get-Conf '2,1') + (Get-Conf '2,0') + (Get-Conf '1,2') + (Get-Conf '1,0') + (Get-Conf '0,2') + (Get-Conf '0,1')
Write-Output ("  matrix(rows=A,cols=B) 2,2={0} 1,1={1} 0,0={2} off-diag={3}" -f
    (Get-Conf '2,2'), (Get-Conf '1,1'), (Get-Conf '0,0'), $off)
if ($disagree.Count -gt 0) {
    Write-Output "disagreements:"
    foreach ($d in $disagree) {
        Write-Output ("  case={0} q=`"{1}`" cand={2} A={3} B={4}" -f $d.case, $d.query, $d.candidate, $d.A, $d.B)
    }
} else {
    Write-Output "disagreements: 0"
}
