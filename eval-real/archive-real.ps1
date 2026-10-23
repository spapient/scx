# 归档真实语料全量证据：基线、裁决包、仲裁日志、语料库、文档、gate 原始输出。
param(
    [string]$Tag = "2026-10-23",
    [string]$Archive = (Join-Path $PSScriptRoot ("archive-real-" + $Tag))
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path -LiteralPath $PSScriptRoot).Path
$abs = [IO.Path]::GetFullPath($Archive)
if (-not $abs.StartsWith($root, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "archive must be inside eval-real: $abs"
}
New-Item -ItemType Directory -Force -Path $abs | Out-Null

$files = @(
    "real-cases.csv",
    "real-baseline.json",
    "real-baseline-fei.json",
    "real-baseline-hao.json",
    "real-baseline-arb.json",
    "real-baseline-draftvariant.json",
    "judge-packet-real.tsv",
    "judge-packet-real-fei.tsv",
    "judge-packet-real-hao.tsv",
    "judge-packet-real-fei-filled.tsv",
    "judge-packet-real-hao-filled.tsv",
    "judge-packet-real-arb-filled.tsv",
    "arbitration-log.tsv"
)
foreach ($f in $files) {
    $src = Join-Path $root $f
    if (-not (Test-Path -LiteralPath $src)) { throw "missing archive input: $src" }
    Copy-Item -LiteralPath $src -Destination (Join-Path $abs $f) -Force
}

$memSrc = Join-Path $root "real-mem"
if (Test-Path -LiteralPath $memSrc) {
    Copy-Item -LiteralPath $memSrc -Destination (Join-Path $abs "real-mem") -Recurse -Force
}

$docsDir = Join-Path $abs "docs"
New-Item -ItemType Directory -Force -Path $docsDir | Out-Null
Copy-Item -LiteralPath (Join-Path (Split-Path -Parent $root) "docs\real-incident-sources.md") -Destination $docsDir -Force
Copy-Item -LiteralPath (Join-Path (Split-Path -Parent $root) "docs\m4-real-corpus-report.md") -Destination $docsDir -Force

$bin = Join-Path (Split-Path -Parent $root) "rs\target\release\scx.exe"
$prev = $ErrorActionPreference
$ErrorActionPreference = "Continue"
try {
    $gate = (& $bin --eval (Join-Path $root "real-baseline-arb.json") --gate --mem-dir (Join-Path $root "real-mem") 2>&1 | Out-String)
    $code = $LASTEXITCODE
} finally {
    $ErrorActionPreference = $prev
}
[IO.File]::WriteAllText((Join-Path $abs "gate.txt"), $gate, (New-Object Text.UTF8Encoding $false))

$hashes = @()
foreach ($f in $files + @("gate.txt")) {
    $p = Join-Path $abs $f
    $h = (Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash
    $hashes += ("| {0} | {1} |" -f $f, $h)
}
$manifest = New-Object System.Collections.ArrayList
[void]$manifest.Add("# 真实语料归档 " + $Tag)
[void]$manifest.Add("")
[void]$manifest.Add("- 来源：``docs/real-incident-sources.md``（11 起公开复盘）")
[void]$manifest.Add("- 语料：40 case（literal 14 / paraphrase 13 / semantic 13）")
[void]$manifest.Add("- 裁决：fei rel2/1/0 = 147/0/197；hao = 147/151/46；分歧 151，一律采用 hao 判定")
[void]$manifest.Add("- 最终：``real-baseline-arb.json``，judge.by=fei+hao")
[void]$manifest.Add("- gate exit=" + $code)
[void]$manifest.Add("")
[void]$manifest.Add("## SHA256")
[void]$manifest.Add("")
[void]$manifest.Add("| file | sha256 |")
[void]$manifest.Add("|---|---|")
foreach ($h in $hashes) { [void]$manifest.Add($h) }
[IO.File]::WriteAllText((Join-Path $abs "MANIFEST.md"), ($manifest -join [Environment]::NewLine), (New-Object Text.UTF8Encoding $false))

Write-Output ("archived -> {0}" -f $abs)
Write-Output ("gate exit={0}" -f $code)
