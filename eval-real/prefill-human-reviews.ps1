# Prefill fei/hao review packets with AI suggestions for human review.
# The review_state column keeps these explicitly pending: close-v2-human.ps1
# refuses them until review-weak-human.ps1 marks every row human-reviewed.
param(
    [string]$Source = (Join-Path $PSScriptRoot "v2-judge-review-weak-codex.tsv"),
    [string]$FeiOut = (Join-Path $PSScriptRoot "v2-judge-review-weak-fei.tsv"),
    [string]$HaoOut = (Join-Path $PSScriptRoot "v2-judge-review-weak-hao.tsv"),
    [string]$SuggestedBy = "codex-ai-2026-10-23",
    [switch]$Force
)
$ErrorActionPreference = "Stop"

$sourceRows = @(Import-Csv -LiteralPath $Source -Delimiter "`t" -Encoding UTF8)
if ($sourceRows.Count -ne 312) { throw "source review must contain 312 rows, got $($sourceRows.Count)" }
$bad = @($sourceRows | Where-Object { ([string]$_.rel).Trim() -notin @("0", "1") })
if ($bad.Count -gt 0) { throw "source review has $($bad.Count) blank/invalid rel rows" }

function Write-Prefill([string]$Out, [string]$Reviewer) {
    if ((Test-Path -LiteralPath $Out) -and -not $Force) {
        $old = @(Import-Csv -LiteralPath $Out -Delimiter "`t" -Encoding UTF8)
        $done = @($old | Where-Object { [string]$_.review_state -eq "human-reviewed" })
        if ($done.Count -gt 0) {
            throw "$Out already contains $($done.Count) human-reviewed rows; use -Force only if you intend to discard them"
        }
    }
    $rows = foreach ($r in $sourceRows) {
        $copy = $r.PSObject.Copy()
        foreach ($name in @("review_state", "reviewed_by", "reviewed_at", "suggested_by")) {
            if (-not $copy.PSObject.Properties[$name]) {
                $copy | Add-Member -NotePropertyName $name -NotePropertyValue "" -Force
            }
        }
        $copy.review_state = "ai-suggested-pending-human"
        $copy.reviewed_by = ""
        $copy.reviewed_at = ""
        $copy.suggested_by = $SuggestedBy
        $copy.note = "AI 预填建议（$SuggestedBy）；待 $Reviewer 独立审核：Enter=采纳，1/0=改判。原注: $($r.note)"
        $copy
    }
    $rows | Export-Csv -LiteralPath $Out -Delimiter "`t" -NoTypeInformation -Encoding UTF8
    Write-Output ("prefilled {0} -> {1}; state=ai-suggested-pending-human" -f $Reviewer, $Out)
}

Write-Prefill $FeiOut "fei"
Write-Prefill $HaoOut "hao"
Write-Output "Human reviewers must run review-weak-human.ps1 -ReviewAll; close-v2-human.ps1 will reject pending rows."
