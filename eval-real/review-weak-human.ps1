# Interactive 1/0 review for the v2 same-bucket cross-incident packet.
# The reviewer sees only the query, candidate summary and fix. No target id,
# no A/B score, no other reviewer's label. Progress is saved after every row.
#
# Default mode resumes unanswered/pending rows. -ReviewAll walks all 312 rows
# and lets the reviewer accept the prefilled suggestion with Enter, or change it.
param(
    [Parameter(Mandatory = $true)][string]$In,
    [string]$Out = "",
    [string]$Reviewer = "",
    [switch]$ReviewAll,
    [switch]$Blind
)
$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($Out)) { $Out = $In }
$rows = @(Import-Csv -LiteralPath $In -Delimiter "`t" -Encoding UTF8)
if ($rows.Count -eq 0) { throw "empty review packet: $In" }

foreach ($r in $rows) {
    foreach ($name in @("review_state", "reviewed_by", "reviewed_at", "suggested_by")) {
        if (-not $r.PSObject.Properties[$name]) {
            $r | Add-Member -NotePropertyName $name -NotePropertyValue "" -Force
        }
    }
}

function Save-Review {
    $rows | Export-Csv -LiteralPath $Out -Delimiter "`t" -NoTypeInformation -Encoding UTF8
}

function Set-HumanAnswer($Row, [string]$Answer) {
    $Row.rel = $Answer
    $Row.review_state = "human-reviewed"
    $Row.reviewed_by = $Reviewer
    $Row.reviewed_at = [DateTime]::UtcNow.ToString("yyyy-MM-ddTHH:mm:ssZ")
}

$todo = if ($ReviewAll) {
    @($rows)
} else {
    @($rows | Where-Object { [string]$_.review_state -ne "human-reviewed" })
}
Write-Host ("review packet: {0} rows, to-review={1}, reviewer={2}, mode={3}" -f
    $rows.Count, $todo.Count, $Reviewer, $(if ($ReviewAll) { "all" } else { "resume" }))
Write-Host "1 = related failure mechanism but not reusable; 0 = unrelated; u = undo; q = save and quit"
if ($ReviewAll -and -not $Blind) { Write-Host "Enter = accept the prefilled suggestion (if present)" }
if ($Blind) { Write-Host "blind mode: suggestions are hidden; enter 1 or 0 for every row" }

$i = 0
while ($i -lt $rows.Count) {
    $r = $rows[$i]
    $cur = ([string]$r.rel).Trim()
    if (-not $ReviewAll -and [string]$r.review_state -eq "human-reviewed") { $i++; continue }

    Write-Host ""
    Write-Host ("[{0}/{1}] case={2} layer={3}" -f ($i + 1), $rows.Count, $r.case_index, $r.layer) -ForegroundColor Cyan
    Write-Host ("query: {0}" -f $r.query)
    Write-Host ("candidate: {0}" -f $r.summary)
    Write-Host ("fix: {0}" -f $r.fix)
    if ($cur -in @("0", "1") -and -not $Blind) {
        Write-Host ("prefilled suggestion: {0}" -f $cur) -ForegroundColor Yellow
        Write-Host "rel? [Enter=accept, 1/0=change, u=undo, q=save+quit] " -NoNewline
    } else {
        Write-Host "rel? [1=related/non-answer, 0=unrelated, u=undo, q=save+quit] " -NoNewline
    }
    $answer = (Read-Host).Trim().ToLowerInvariant()

    if ($answer -eq "q") {
        Save-Review
        Write-Host ("saved: {0}" -f $Out) -ForegroundColor Yellow
        exit 0
    }
    if ($answer -eq "u") {
        $j = $i - 1
        while ($j -ge 0 -and [string]$rows[$j].review_state -ne "human-reviewed") { $j-- }
        if ($j -ge 0) {
            $rows[$j].review_state = $(if ([string]$rows[$j].suggested_by) { "ai-suggested-pending-human" } else { "" })
            $rows[$j].reviewed_by = ""
            $rows[$j].reviewed_at = ""
            $i = $j
            Save-Review
            continue
        }
        Write-Host "nothing to undo" -ForegroundColor Yellow
        continue
    }
    if ($answer -eq "" -and $cur -in @("0", "1") -and -not $Blind) {
        Set-HumanAnswer $r $cur
        Save-Review
        $i++
        continue
    }
    if ($answer -notin @("0", "1")) {
        Write-Host "invalid answer: enter 1, 0, u, or q" -ForegroundColor Red
        continue
    }

    Set-HumanAnswer $r $answer
    Save-Review
    $i++
}

$dist0 = @($rows | Where-Object { ([string]$_.rel).Trim() -eq "0" }).Count
$dist1 = @($rows | Where-Object { ([string]$_.rel).Trim() -eq "1" }).Count
$pending = @($rows | Where-Object { [string]$_.review_state -ne "human-reviewed" }).Count
Write-Host ""
Write-Host ("done: rel1={0} rel0={1} pending_review={2} -> {3}" -f $dist1, $dist0, $pending, $Out) -ForegroundColor Green
if ($pending -gt 0) { exit 1 }
