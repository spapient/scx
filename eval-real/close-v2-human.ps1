# Close the v2 human loop: validate two independently filled 1/0 packets,
# merge them into the compact packet, import judge metadata, measure agreement,
# arbitrate disagreements, and run the final --gate.
param(
    [string]$FeiReview = (Join-Path $PSScriptRoot "v2-judge-review-weak-fei.tsv"),
    [string]$HaoReview = (Join-Path $PSScriptRoot "v2-judge-review-weak-hao.tsv"),
    [string]$ExpectedReview = (Join-Path $PSScriptRoot "v2-judge-review-weak.tsv"),
    [string]$Baseline = (Join-Path $PSScriptRoot "v2-baseline.json"),
    [string]$MemDir = (Join-Path $PSScriptRoot "v2-mem"),
    [string]$CompactDraft = (Join-Path $PSScriptRoot "v2-judge-compact-draft.tsv"),
    [string]$OutFei = (Join-Path $PSScriptRoot "v2-baseline-fei.json"),
    [string]$OutHao = (Join-Path $PSScriptRoot "v2-baseline-hao.json"),
    [string]$OutArb = (Join-Path $PSScriptRoot "v2-baseline-arb.json"),
    [string]$ArbLog = (Join-Path $PSScriptRoot "v2-arbitration-log.tsv"),
    [string]$AgreementLog = (Join-Path $PSScriptRoot "v2-judge-agreement.txt"),
    [string]$GateLog = (Join-Path $PSScriptRoot "v2-gate-human.txt"),
    [string]$By = "fei+hao"
)
$ErrorActionPreference = "Stop"
$Bin = Join-Path (Split-Path -Parent $PSScriptRoot) "rs\target\release\scx.exe"
$ScriptDir = $PSScriptRoot

function Get-Key($r) { "{0}|{1}" -f $r.case_index, $r.candidate }

function Test-Review([string]$Path, [string]$Name) {
    if (-not (Test-Path -LiteralPath $Path)) { throw "$Name review missing: $Path" }
    $rows = @(Import-Csv -LiteralPath $Path -Delimiter "`t" -Encoding UTF8)
    if ($rows.Count -ne 312) { throw "$Name review must contain 312 rows, got $($rows.Count)" }
    $bad = @($rows | Where-Object { ([string]$_.rel).Trim() -notin @("0", "1") })
    if ($bad.Count -gt 0) {
        throw ("$Name review has {0} blank/invalid rel rows; first case={1} candidate={2}" -f
            $bad.Count, $bad[0].case_index, $bad[0].candidate)
    }
    if ($rows[0].PSObject.Properties["review_state"]) {
        $pending = @($rows | Where-Object { [string]$_.review_state -ne "human-reviewed" })
        if ($pending.Count -gt 0) {
            throw ("$Name review has {0} rows still pending human review; first case={1}" -f
                $pending.Count, $pending[0].case_index)
        }
        $wrongBy = @($rows | Where-Object { [string]$_.reviewed_by -ne $Name })
        if ($wrongBy.Count -gt 0) {
            throw ("$Name review has {0} rows with reviewed_by != $Name" -f $wrongBy.Count)
        }
        $missingTime = @($rows | Where-Object { [string]::IsNullOrWhiteSpace([string]$_.reviewed_at) })
        if ($missingTime.Count -gt 0) {
            throw ("$Name review has {0} rows without reviewed_at" -f $missingTime.Count)
        }
    }
    $keys = @{}
    foreach ($r in $rows) {
        $k = Get-Key $r
        if ($keys.ContainsKey($k)) { throw "$Name review duplicate row: $k" }
        $keys[$k] = $true
    }
    return $rows
}

$expectedRows = @(Import-Csv -LiteralPath $ExpectedReview -Delimiter "`t" -Encoding UTF8)
if ($expectedRows.Count -ne 312) { throw "expected review packet must contain 312 rows, got $($expectedRows.Count)" }
$expectedKeys = @{}
foreach ($r in $expectedRows) { $expectedKeys[(Get-Key $r)] = $true }

$fei = @(Test-Review $FeiReview "fei")
$hao = @(Test-Review $HaoReview "hao")
foreach ($pair in @(@("fei", $fei), @("hao", $hao))) {
    $name = $pair[0]; $rows = $pair[1]
    $keys = @{}
    foreach ($r in $rows) { $keys[(Get-Key $r)] = $true }
    if ($keys.Count -ne $expectedKeys.Count -or @($keys.Keys | Where-Object { -not $expectedKeys.ContainsKey($_) }).Count -gt 0) {
        throw "$name review candidate set does not match the expected packet"
    }
}

Write-Output ("validated reviews: fei={0} hao={1} rows; both complete 1/0" -f $fei.Count, $hao.Count)
Write-Output "caution: fei and hao must have judged independently; this script cannot prove independence."

$feiCompact = Join-Path $ScriptDir "v2-judge-compact-fei-filled.tsv"
$haoCompact = Join-Path $ScriptDir "v2-judge-compact-hao-filled.tsv"
& powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $ScriptDir "merge-weak-review.ps1") `
    -Draft $CompactDraft -Weak $FeiReview -Out $feiCompact
if ($LASTEXITCODE -ne 0) { throw "fei merge failed" }
& powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $ScriptDir "merge-weak-review.ps1") `
    -Draft $CompactDraft -Weak $HaoReview -Out $haoCompact
if ($LASTEXITCODE -ne 0) { throw "hao merge failed" }

Copy-Item -LiteralPath $Baseline -Destination $OutFei -Force
Copy-Item -LiteralPath $Baseline -Destination $OutHao -Force
& powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $ScriptDir "..\scripts\judge-audit.ps1") `
    -Import $feiCompact -Baseline $OutFei -Judge fei
if ($LASTEXITCODE -ne 0) { throw "fei import failed" }
& powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $ScriptDir "..\scripts\judge-audit.ps1") `
    -Import $haoCompact -Baseline $OutHao -Judge hao
if ($LASTEXITCODE -ne 0) { throw "hao import failed" }

$agreement = & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $ScriptDir "..\scripts\judge-agreement.ps1") `
    -A $OutFei -B $OutHao
if ($LASTEXITCODE -ne 0) { throw "agreement failed" }
$agreement | Tee-Object -FilePath $AgreementLog

& powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $ScriptDir "arbitrate.ps1") `
    -Fei $OutFei -Hao $OutHao -Out $OutArb -Log $ArbLog -By $By
if ($LASTEXITCODE -ne 0) { throw "arbitration failed" }

$ErrorActionPreference = "Continue"
& $Bin --eval $OutArb --gate --mem-dir $MemDir 2>&1 | Tee-Object -FilePath $GateLog
$gateExit = $LASTEXITCODE
$ErrorActionPreference = "Stop"
Write-Output ("human gate exit={0}; log={1}" -f $gateExit, $GateLog)
exit $gateExit
