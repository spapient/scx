# 双裁决人分歧仲裁：沿用上一轮决议口径——分歧项采用 hao 的独立判定，一致项保留；
# 每条分歧写入 TSV 留档。仲裁结果写 judge.by="fei+hao"。
param(
    [string]$Fei = (Join-Path $PSScriptRoot "real-baseline-fei.json"),
    [string]$Hao = (Join-Path $PSScriptRoot "real-baseline-hao.json"),
    [string]$Out = (Join-Path $PSScriptRoot "real-baseline-arb.json"),
    [string]$Log = (Join-Path $PSScriptRoot "arbitration-log.tsv"),
    # 真实双人裁决用 fei+hao；机器草案必须用 draft-rule，避免把规则物化冒充人工证据。
    [string]$By = "fei+hao"
)

$ErrorActionPreference = "Stop"

$a = (Get-Content -LiteralPath $Fei -Raw -Encoding UTF8 | ConvertFrom-Json).cases
$b = (Get-Content -LiteralPath $Hao -Raw -Encoding UTF8 | ConvertFrom-Json).cases
if (@($a).Count -ne @($b).Count) { throw "case count mismatch: fei=$(@($a).Count) hao=$(@($b).Count)" }

$now = [DateTime]::UtcNow.ToString("yyyy-MM-ddTHH:mm:ssZ")
$logLines = @("case_index`tquery`tcandidate`tfei`thao`tchosen`trule")
$nDiff = 0
$dist = @{ 2 = 0; 1 = 0; 0 = 0 }

for ($i = 0; $i -lt @($a).Count; $i++) {
    $ca = $a[$i]
    $cb = $b[$i]
    if ([string]$ca.query -ne [string]$cb.query) { throw "case[$i] query mismatch" }
    $ra = $ca.rel
    $rb = $cb.rel
    $keys = New-Object System.Collections.ArrayList
    foreach ($p in $ra.PSObject.Properties) { [void]$keys.Add($p.Name) }
    foreach ($p in $rb.PSObject.Properties) {
        if (-not $keys.Contains($p.Name)) { [void]$keys.Add($p.Name) }
    }
    $rel = [ordered]@{}
    foreach ($k in $keys) {
        $pa = $ra.PSObject.Properties[$k]
        $pb = $rb.PSObject.Properties[$k]
        $va = if ($pa) { [int]$pa.Value } else { 0 }
        $vb = if ($pb) { [int]$pb.Value } else { 0 }
        $chosen = if ($va -eq $vb) { $va } else { $vb }
        $rule = if ($va -eq $vb) { "agree" } else { "adopt-hao" }
        $rel[$k] = $chosen
        $dist[$chosen]++
        if ($va -ne $vb) {
            $nDiff++
            $logLines += ("{0}`t{1}`t{2}`t{3}`t{4}`t{5}`t{6}" -f $i, $ca.query, $k, $va, $vb, $chosen, $rule)
        }
    }
    $ca | Add-Member -NotePropertyName rel -NotePropertyValue $rel -Force
    $meta = [ordered]@{ by = $By; date = $now }
    $ca | Add-Member -NotePropertyName judge -NotePropertyValue $meta -Force
}

$json = [ordered]@{ cases = @($a) } | ConvertTo-Json -Depth 12
[IO.File]::WriteAllText($Out, $json, (New-Object Text.UTF8Encoding $false))
[IO.File]::WriteAllText($Log, ($logLines -join "`n"), (New-Object Text.UTF8Encoding $false))
Write-Output ("arbitrate: disagreements={0} rel2={1} rel1={2} rel0={3} -> {4}" -f $nDiff, $dist[2], $dist[1], $dist[0], $Out)
Write-Output ("arbitration log -> {0}" -f $Log)
