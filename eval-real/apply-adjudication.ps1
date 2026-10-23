# 把裁决人的 rel 规则物化为 baseline：目标=2；同一公开事故（同来源 URL）的其他 case=2；
# 可选弱相关清单=1；其余=0。规则由裁决人给出，本脚本只做确定性应用并写 judge 元数据。
#
# 用法：
#   powershell -NoProfile -ExecutionPolicy Bypass -File eval-real\apply-adjudication.ps1 `
#     -Packet eval-real\judge-packet-real.tsv -Baseline eval-real\real-baseline.json `
#     -Out eval-real\real-baseline-fei.json -Judge fei
# 可选 -WeakFile：每行 `case_index:hash16`（或 `case_index:hash16=1`），标 rel=1。
param(
    [string]$Packet = (Join-Path $PSScriptRoot "judge-packet-real.tsv"),
    [string]$Csv = (Join-Path $PSScriptRoot "real-cases.csv"),
    [string]$MemDir = (Join-Path $PSScriptRoot "real-mem"),
    [string]$Baseline = (Join-Path $PSScriptRoot "real-baseline.json"),
    [string]$Out = (Join-Path $PSScriptRoot "real-baseline-judged.json"),
    [string]$Judge = "",
    [string]$WeakFile = "",
    [ValidateSet("none", "samebucket")]
    [string]$WeakRule = "none",
    # 目标未进候选包时仍写 rel（默认拒绝，保持 v1 可复现口径）。
    # 仅用于诊断/难例：目标未被召回本身就是 MRR=0，不能伪装成命中。
    [switch]$AllowMissingTarget
)

$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($Judge)) { throw "-Judge <姓名> required" }

function Get-MetaById([string]$Root) {
    $map = @{}
    $casesDir = Join-Path $Root "cases"
    if (-not (Test-Path -LiteralPath $casesDir)) { return $map }
    foreach ($d in Get-ChildItem -LiteralPath $casesDir -Directory) {
        $cj = Join-Path $d.FullName "case.json"
        if (-not (Test-Path -LiteralPath $cj)) { continue }
        try {
            $c = Get-Content -LiteralPath $cj -Raw | ConvertFrom-Json
            $id = [string]$c.id
            $src = ""
            foreach ($line in ([string]$c.text -split "`n")) {
                if ($line -match '来源:\s*(https?://\S+)') { $src = $matches[1]; break }
            }
            $map[$id] = @{ src = $src; bucket = [string]$c.bucket }
        } catch { }
    }
    $map
}

$rows = @(Import-Csv -LiteralPath $Csv -Encoding UTF8)
$pkt = @(Import-Csv -LiteralPath $Packet -Delimiter "`t" -Encoding UTF8)
$idMeta = Get-MetaById $MemDir

$weak = @{}
if ($WeakFile) {
    if (-not (Test-Path -LiteralPath $WeakFile)) { throw "missing weak file: $WeakFile" }
    foreach ($line in Get-Content -LiteralPath $WeakFile -Encoding UTF8) {
        $t = $line.Trim()
        if ($t -eq "" -or $t.StartsWith("#")) { continue }
        if ($t -match '^(\d+):([0-9a-f]{8,})(?:=1)?$') {
            $ci = [int]$matches[1]
            $h = $matches[2]
            if (-not $weak.ContainsKey($ci)) { $weak[$ci] = @{} }
            $weak[$ci][$h] = $true
        } else {
            throw "bad weak line: $line"
        }
    }
}

# packet 按 case_index 分组（保留行序）
$byCase = @{}
foreach ($p in $pkt) {
    $ci = [int]$p.case_index
    if (-not $byCase.ContainsKey($ci)) { $byCase[$ci] = New-Object System.Collections.ArrayList }
    [void]$byCase[$ci].Add($p)
}

$base = Get-Content -LiteralPath $Baseline -Raw -Encoding UTF8 | ConvertFrom-Json
$cases = @($base.cases)
$dist = @{ 2 = 0; 1 = 0; 0 = 0 }
$now = [DateTime]::UtcNow.ToString("yyyy-MM-ddTHH:mm:ssZ")

for ($ci = 0; $ci -lt $cases.Count; $ci++) {
    if (-not $byCase.ContainsKey($ci)) { throw "packet missing case_index=$ci" }
    $target = [string]$rows[$ci].case_id
    $tSrc = if ($idMeta.ContainsKey($target)) { $idMeta[$target].src } else { "" }
    $tBucket = if ($idMeta.ContainsKey($target)) { $idMeta[$target].bucket } else { "" }
    $rel = [ordered]@{}
    foreach ($p in $byCase[$ci]) {
        $cand = [string]$p.candidate
        $cSrc = if ($idMeta.ContainsKey($cand)) { $idMeta[$cand].src } else { "" }
        $cBucket = if ($idMeta.ContainsKey($cand)) { $idMeta[$cand].bucket } else { "" }
        $r = 0
        if ($cand -eq $target) {
            $r = 2
        } elseif ($tSrc -ne "" -and $cSrc -ne "" -and $tSrc -eq $cSrc) {
            $r = 2
        } elseif ($WeakRule -eq "samebucket" -and $tBucket -ne "" -and $cBucket -eq $tBucket) {
            # 跨事故但同故障域（同 bucket）：相关但不可复用 → 1。
            $r = 1
        } elseif ($weak.ContainsKey($ci)) {
            $tail = ($cand -split '_')[-1]
            if ($weak[$ci].ContainsKey($tail)) { $r = 1 }
        }
        $rel[$cand] = $r
        $dist[$r]++
    }
    if (-not $rel.Contains($target)) {
        if (-not $AllowMissingTarget) { throw "case[$ci] target not in packet: $target" }
        $rel[$target] = 2
        $dist[2]++
        Write-Warning "case[$ci] target not in packet, rel=2 recorded but retrieval miss stays: $target"
    }
    $cases[$ci] | Add-Member -NotePropertyName rel -NotePropertyValue $rel -Force
    $meta = [ordered]@{ by = $Judge; date = $now }
    $cases[$ci] | Add-Member -NotePropertyName judge -NotePropertyValue $meta -Force
}

$json = [ordered]@{ cases = $cases } | ConvertTo-Json -Depth 12
[IO.File]::WriteAllText($Out, $json, (New-Object Text.UTF8Encoding $false))
Write-Output ("apply-adjudication: judge={0} rel2={1} rel1={2} rel0={3} -> {4}" -f $Judge, $dist[2], $dist[1], $dist[0], $Out)
