# 生成 fei 裁决草案（机器草案，须人工审改后才算独立裁决）：
# - 与目标同一来源（同一公开事故）的不同 case 候选 → rel=1（弱相关）
# - 目标本身 → rel=2
# - 其余 → rel=0
# 用 "公开复盘, 来源: <URL>" 文本行判断同源；来源清单见 docs/real-incident-sources.md。
param(
    [string]$Packet = (Join-Path $PSScriptRoot "judge-packet-real.tsv"),
    [string]$Csv = (Join-Path $PSScriptRoot "real-cases.csv"),
    [string]$MemDir = (Join-Path $PSScriptRoot "real-mem"),
    [string]$Out = (Join-Path $PSScriptRoot "judge-packet-real-fei.tsv")
)

$ErrorActionPreference = "Stop"

function Get-SrcById([string]$Root) {
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
            $map[$id] = $src
        } catch { }
    }
    $map
}

$rows = @(Import-Csv -LiteralPath $Csv -Encoding UTF8)
$pkt = @(Import-Csv -LiteralPath $Packet -Delimiter "`t" -Encoding UTF8)
$idToSrc = Get-SrcById $MemDir
$targetByCase = @{}
for ($i = 0; $i -lt $rows.Count; $i++) { $targetByCase[$i] = [string]$rows[$i].case_id }

$sb = New-Object System.Text.StringBuilder
[void]$sb.AppendLine("row`tcase_index`tlayer`tquery`tcandidate`tsummary`tfix`trel")
$rel1 = New-Object System.Collections.ArrayList
$dist = @{ 2 = 0; 1 = 0; 0 = 0 }

foreach ($p in $pkt) {
    $ci = [int]$p.case_index
    $cand = [string]$p.candidate
    $target = $targetByCase[$ci]
    $tSrc = if ($idToSrc.ContainsKey($target)) { $idToSrc[$target] } else { "" }
    $cSrc = if ($idToSrc.ContainsKey($cand)) { $idToSrc[$cand] } else { "" }
    $rel = "0"
    if ($cand -eq $target) {
        $rel = "2"
    } elseif ($tSrc -ne "" -and $cSrc -ne "" -and $tSrc -eq $cSrc) {
        $rel = "1"
        [void]$rel1.Add("case[$ci] $cand  (query=$($p.query))")
    }
    $dist[[int]$rel]++
    [void]$sb.AppendLine(
        ("{0}`t{1}`t{2}`t{3}`t{4}`t{5}`t{6}`t{7}" -f
            $p.row, $ci, $p.layer, $p.query, $cand, $p.summary, $p.fix, $rel)
    )
}

[IO.File]::WriteAllText($Out, $sb.ToString(), (New-Object Text.UTF8Encoding $false))
Write-Output ("draft-fei: wrote {0} rows rel2={1} rel1={2} rel0={3} -> {4}" -f $pkt.Count, $dist[2], $dist[1], $dist[0], $Out)
Write-Output "proposed weak-related (same public incident, different case):"
foreach ($r in $rel1) { Write-Output ("  " + $r) }
