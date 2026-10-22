# Confirmatory corpus 5: 30 public incidents not used by v1/v2/holdout/confirm2/confirm3/confirm4.
#
# Same five-sibling design as the preceding confirmation sets. Facts come from
# the public postmortem summaries in confirm5-incidents.json; Chinese fields are
# translations or paraphrases, not invented incidents.
param(
    [string]$MemDir = (Join-Path $PSScriptRoot "confirm5-mem"),
    [string]$Csv = (Join-Path $PSScriptRoot "confirm5-cases.csv"),
    [string]$IncidentsPath = (Join-Path $PSScriptRoot "confirm5-incidents.json"),
    [string]$Binary = (Join-Path $PSScriptRoot "..\rs\target\release\scx.exe"),
    [switch]$Force
)

$ErrorActionPreference = "Stop"

function Write-Utf8([string]$Path, [string]$Text) {
    [IO.File]::WriteAllText($Path, $Text, (New-Object Text.UTF8Encoding $false))
}
function CsvField([string]$Value) { return '"' + $Value.Replace('"', '""') + '"' }

if (-not (Test-Path -LiteralPath $Binary)) { throw "missing release binary: $Binary" }
if (-not (Test-Path -LiteralPath $IncidentsPath)) { throw "missing incidents json: $IncidentsPath" }
$raw = Get-Content -LiteralPath $IncidentsPath -Raw -Encoding UTF8 | ConvertFrom-Json
$items = @($raw)
if ($items.Count -ne 30) { throw "expected 30 incidents, got $($items.Count)" }
$required = @("id", "src", "sym", "key", "pa", "pb", "pc", "pd", "se", "sf", "rc", "fix")
$seenIds = @{}
$seenSrc = @{}
$seenQuery = @{}
foreach ($c in $items) {
    foreach ($f in $required) {
        if ([string]::IsNullOrWhiteSpace([string]$c.$f)) { throw "incident $($c.id): empty field $f" }
    }
    if ($seenIds.ContainsKey([string]$c.id)) { throw "duplicate incident id: $($c.id)" }
    $seenIds[[string]$c.id] = $true
    if ($seenSrc.ContainsKey([string]$c.src)) { throw "duplicate source URL: $($c.src)" }
    $seenSrc[[string]$c.src] = $true
    foreach ($f in @("sym", "key", "pa", "pb", "pc", "pd", "se", "sf")) {
        $q = ([string]$c.$f).Trim().ToLowerInvariant()
        if ($seenQuery.ContainsKey($q)) { throw "duplicate query $($c.id).$f == $($seenQuery[$q])" }
        $seenQuery[$q] = "$($c.id).$f"
    }
}

if (-not (Test-Path -LiteralPath $MemDir)) { New-Item -ItemType Directory -Force -Path $MemDir | Out-Null }
$existing = @(Get-ChildItem -LiteralPath $MemDir -Force -ErrorAction SilentlyContinue)
if ($existing.Count -gt 0) {
    if (-not $Force) { throw "MemDir is not empty: $MemDir (use -Force to clear workspace-local dir)" }
    $wsRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
    $memAbs = (Resolve-Path -LiteralPath $MemDir).Path
    if (-not $memAbs.StartsWith($wsRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "refusing to clear non-workspace dir: $memAbs"
    }
    Remove-Item -LiteralPath $memAbs -Recurse -Force
    New-Item -ItemType Directory -Force -Path $memAbs | Out-Null
}

$rows = New-Object System.Collections.ArrayList
$n = 0
function Store-Case([string]$q, [string]$evalQuery, [string]$layer, [string]$rc, [string]$fix, [string]$src) {
    $script:n++
    $log = Join-Path $env:TEMP ("scx-confirm5-{0:D3}-{1}.log" -f $script:n, [guid]::NewGuid().ToString("N"))
    $text = ("{0}`n公开复盘, 来源: {1}`n根因: {2}" -f $q, $src, $rc)
    Write-Utf8 $log $text
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try { $out = (& $Binary --query $q --store 1 --fix $fix --mem-dir $MemDir $log 2>&1 | Out-String) }
    finally { $ErrorActionPreference = $prev }
    if ($out -notmatch "stored case=([^\s]+) added=yes") { throw "store failed case $($script:n): $q`n$out" }
    $id = $Matches[1]
    $h = $id.Split('_')[-1]
    $note = "target=$h source=$src"
    [void]$rows.Add([pscustomobject]@{ query = $evalQuery; layer = $layer; case_id = $id; note = $note })
}

foreach ($c in $items) {
    Store-Case $c.sym $c.sym "literal" $c.rc $c.fix $c.src
    Store-Case $c.key $c.key "literal" $c.rc $c.fix $c.src
    Store-Case $c.pa  $c.pb  "paraphrase" $c.rc $c.fix $c.src
    Store-Case $c.pc  $c.pd  "paraphrase" $c.rc $c.fix $c.src
    Store-Case $c.se  $c.sf  "semantic" $c.rc $c.fix $c.src
}

$lines = @("query,layer,case_id,note")
foreach ($r in $rows) {
    $lines += ((@($r.query, $r.layer, $r.case_id, $r.note) | ForEach-Object { CsvField ([string]$_) }) -join ",")
}
Write-Utf8 $Csv ($lines -join "`n")

$l = @($rows | Where-Object { $_.layer -eq "literal" }).Count
$p = @($rows | Where-Object { $_.layer -eq "paraphrase" }).Count
$s = @($rows | Where-Object { $_.layer -eq "semantic" }).Count
Write-Host ("confirm5 corpus: cases={0} eval={1} literal={2} paraphrase={3} semantic={4}" -f $items.Count, $rows.Count, $l, $p, $s)
Write-Host "next: powershell -File scripts\holdout-build.ps1 -Csv eval-real\confirm5-cases.csv -Out eval-real\confirm5-baseline.json -JudgeBy mechanical-target-confirm5-v1 -JudgeDate 2026-10-23T00:00:00Z"
