# v2 消融：复制同一份 v2-mem，只改 [params] 开关，跑同一 baseline。
# 目的：把 A 相对 B 的差异归因到 bucket / PRF / MMR / synonyms / fields / edge。
param(
    [string]$Baseline = (Join-Path $PSScriptRoot "v2-baseline-draft.json"),
    [string]$BaseMem = (Join-Path $PSScriptRoot "v2-mem"),
    [string]$OutDir = (Join-Path $PSScriptRoot "v2-ablate"),
    [string]$Binary = (Join-Path $PSScriptRoot "..\rs\target\release\scx.exe")
)
$ErrorActionPreference = "Stop"

$ws = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
$outAbs = [IO.Path]::GetFullPath($OutDir)
if (-not $outAbs.StartsWith($ws, [StringComparison]::OrdinalIgnoreCase)) {
    throw "refusing to write outside workspace: $outAbs"
}

function Set-MetaParam([string]$Path, [string]$Key, [double]$Value) {
    $lines = [Collections.Generic.List[string]](Get-Content -LiteralPath $Path -Encoding UTF8)
    $line = ("{0} = {1}" -f $Key, $Value)
    $pi = $lines.IndexOf("[params]")
    if ($pi -lt 0) { $lines.Add("[params]"); $lines.Add($line); }
    else {
        $replaced = $false
        for ($i = $pi + 1; $i -lt $lines.Count; $i++) {
            $t = $lines[$i].Trim()
            if ($t.StartsWith("[")) { break }
            if ($t -match ('^' + [regex]::Escape($Key) + '\s*=')) {
                $lines[$i] = $line
                $replaced = $true
                break
            }
        }
        if (-not $replaced) { $lines.Insert($pi + 1, $line) }
    }
    [IO.File]::WriteAllText($Path, (($lines -join "`n") + "`n"), (New-Object Text.UTF8Encoding $false))
}

if (-not (Test-Path -LiteralPath $Binary)) { throw "missing binary: $Binary" }
if (-not (Test-Path -LiteralPath $Baseline)) { throw "missing baseline: $Baseline" }
if (-not (Test-Path -LiteralPath $BaseMem)) { throw "missing mem dir: $BaseMem" }
if (-not (Test-Path -LiteralPath $outAbs)) { New-Item -ItemType Directory -Force -Path $outAbs | Out-Null }

$variants = [ordered]@{
    "full"                      = @{}
    "no-bucket"                 = @{ bucket_enable = 0.0 }
    "no-prf"                    = @{ prf_enable = 0.0 }
    "no-mmr"                    = @{ mmr_enable = 0.0 }
    "no-synonyms"               = @{ synonyms_enable = 0.0 }
    "no-edge"                   = @{ edge_enable = 0.0 }
    "no-fields"                 = @{ field_weight_error_code = 0.0; field_weight_file_loc = 0.0; field_weight_symbol = 0.0; field_weight_num_fp = 0.0 }
    "allow-other"               = @{ bucket_allow_other = 1.0 }
    "allow-other-no-mmr"        = @{ bucket_allow_other = 1.0; mmr_enable = 0.0 }
    "allow-other-no-mmr-no-syn" = @{ bucket_allow_other = 1.0; mmr_enable = 0.0; synonyms_enable = 0.0 }
    "mmr-lambda-0.2"            = @{ mmr_lambda = 0.2 }
    "mmr-lambda-0.1"            = @{ mmr_lambda = 0.1 }
    "allow-other-mmr-0.2"       = @{ bucket_allow_other = 1.0; mmr_lambda = 0.2 }
    "all-off"                   = @{ bucket_enable = 0.0; prf_enable = 0.0; mmr_enable = 0.0; synonyms_enable = 0.0; edge_enable = 0.0; field_weight_error_code = 0.0; field_weight_file_loc = 0.0; field_weight_symbol = 0.0; field_weight_num_fp = 0.0 }
}

foreach ($name in $variants.Keys) {
    $dst = Join-Path $outAbs $name
    if (Test-Path -LiteralPath $dst) {
        $dstAbs = (Resolve-Path -LiteralPath $dst).Path
        if (-not $dstAbs.StartsWith($ws, [StringComparison]::OrdinalIgnoreCase)) {
            throw "refusing to remove outside workspace: $dstAbs"
        }
        Remove-Item -LiteralPath $dstAbs -Recurse -Force
    }
    Copy-Item -LiteralPath $BaseMem -Destination $dst -Recurse -Force
    $meta = Join-Path $dst "meta.toml"
    foreach ($k in $variants[$name].Keys) { Set-MetaParam $meta $k $variants[$name][$k] }
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $out = (& $Binary --eval $Baseline --mem-dir $dst 2>&1 | Out-String)
    } finally { $ErrorActionPreference = $prev }
    $file = Join-Path $outAbs ("{0}.txt" -f $name)
    [IO.File]::WriteAllText($file, $out, (New-Object Text.UTF8Encoding $false))
    $summary = ($out -split "`n" | Where-Object { $_ -match '^layer ' }) -join "`n"
    Write-Output ("=== {0} ===`n{1}" -f $name, $summary)
}
