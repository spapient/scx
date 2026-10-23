# v2 事故级口径：expected = 同一公开复盘（同来源 URL）的全部 case。
# 与 v1 的 fei 裁决规则一致（同事故=2）；用于检验 MMR 多样性是否被
# “单目标 exact-target MRR”不公平惩罚。默认仍保留 exact-target baseline。
param(
    [string]$Baseline = (Join-Path $PSScriptRoot "v2-baseline.json"),
    [string]$MemDir = (Join-Path $PSScriptRoot "v2-mem"),
    [string]$Out = (Join-Path $PSScriptRoot "v2-baseline-multiexp.json")
)
$ErrorActionPreference = "Stop"

$meta = @{}
$bySrc = @{}
foreach ($d in Get-ChildItem -LiteralPath (Join-Path $MemDir "cases") -Directory) {
    $cj = Join-Path $d.FullName "case.json"
    if (-not (Test-Path -LiteralPath $cj)) { continue }
    $c = Get-Content -LiteralPath $cj -Raw -Encoding UTF8 | ConvertFrom-Json
    $src = ""
    foreach ($line in ([string]$c.text -split "`n")) {
        if ($line -match '来源:\s*(https?://\S+)') { $src = $matches[1]; break }
    }
    $meta[[string]$c.id] = $src
    if ($src -ne "") {
        if (-not $bySrc.ContainsKey($src)) { $bySrc[$src] = New-Object System.Collections.ArrayList }
        [void]$bySrc[$src].Add([string]$c.id)
    }
}

$base = Get-Content -LiteralPath $Baseline -Raw -Encoding UTF8 | ConvertFrom-Json
$cases = @($base.cases)
$now = [DateTime]::UtcNow.ToString("yyyy-MM-ddTHH:mm:ssZ")
for ($i = 0; $i -lt $cases.Count; $i++) {
    $target = [string]$cases[$i].expected[0]
    $src = if ($meta.ContainsKey($target)) { $meta[$target] } else { "" }
    if ($src -eq "" -or -not $bySrc.ContainsKey($src)) {
        throw "case[$i] target has no source: $target"
    }
    $ids = @($bySrc[$src] | Sort-Object)
    $rel = [ordered]@{}
    foreach ($id in $ids) { $rel[$id] = 2 }
    $cases[$i] | Add-Member -NotePropertyName expected -NotePropertyValue $ids -Force
    $cases[$i] | Add-Member -NotePropertyName rel -NotePropertyValue $rel -Force
    $cases[$i] | Add-Member -NotePropertyName judge -NotePropertyValue ([ordered]@{ by = "draft"; date = $now }) -Force
}
$json = [ordered]@{ cases = $cases } | ConvertTo-Json -Depth 12
[IO.File]::WriteAllText($Out, $json, (New-Object Text.UTF8Encoding $false))
Write-Output ("wrote {0} cases, expected sizes: {1} -> {2}" -f $cases.Count, (($cases | ForEach-Object { $_.expected.Count } | Sort-Object -Unique) -join ","), $Out)
