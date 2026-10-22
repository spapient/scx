# Held-out CSV -> baseline.json
# 标签口径：expected target 由公开事故来源机械确定，target rel=2，其余候选未标注=0。
# 这不是人工盲审；judge.by 明确写 mechanical-target-v1，便于报告区分证据等级。
param(
    [string]$Csv = "eval-real\holdout-cases.csv",
    [string]$Out = "eval-real\holdout-baseline.json",
    [string]$JudgeBy = "mechanical-target-v1",
    [string]$JudgeDate = "2026-10-23T00:00:00Z"
)
$ErrorActionPreference = "Stop"

$rows = @(Import-Csv -LiteralPath $Csv -Encoding UTF8)
if ($rows.Count -eq 0) { throw "CSV has no data rows: $Csv" }

$cases = New-Object System.Collections.ArrayList
$counts = @{ literal = 0; paraphrase = 0; semantic = 0 }
$i = 1
foreach ($r in $rows) {
    $i++
    $q = ([string]$r.query).Trim()
    $layer = ([string]$r.layer).Trim().ToLower()
    $id = ([string]$r.case_id).Trim()
    if ([string]::IsNullOrWhiteSpace($q)) { throw "row ${i}: empty query" }
    if ($layer -notin @("literal", "paraphrase", "semantic")) { throw "row ${i}: bad layer $layer" }
    if ([string]::IsNullOrWhiteSpace($id)) { throw "row ${i}: empty case_id" }

    $obj = [ordered]@{
        query    = $q
        layer    = $layer
        expected = @($id)
        rel      = [ordered]@{ $id = 2 }
        judge    = [ordered]@{ by = $JudgeBy; date = $JudgeDate }
        note     = "target-only; " + [string]$r.note
    }
    [void]$cases.Add([pscustomobject]$obj)
    $counts[$layer]++
}

$dir = Split-Path -Parent $Out
if ($dir -and -not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
$json = [ordered]@{ cases = $cases } | ConvertTo-Json -Depth 10
[IO.File]::WriteAllText((Resolve-Path -LiteralPath $dir).Path + [IO.Path]::DirectorySeparatorChar + (Split-Path -Leaf $Out), $json, (New-Object Text.UTF8Encoding $false))
Write-Output ("wrote: {0}" -f $Out)
Write-Output ("cases={0} literal={1} paraphrase={2} semantic={3}" -f $cases.Count, $counts.literal, $counts.paraphrase, $counts.semantic)
