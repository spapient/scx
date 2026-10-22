# 真实 M4 语料：CSV -> baseline.json
# 用法：
#   powershell -ExecutionPolicy Bypass -File scripts\real-eval-build.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\real-eval-build.ps1 -Csv eval-real\cases.csv -Out eval-real\baseline.json
#
# CSV 列：query,layer,case_id,note
# - layer = literal / paraphrase / semantic
# - case_id = 入库时 stderr 的 case=<id> 的 hash16 后缀（或完整 id）
# - literal 自动写 rel=2；paraphrase/semantic 写 judge:null，等 --judge 盲裁决。
param(
    [string]$Csv = "eval-real\cases.csv",
    [string]$Out = "eval-real\baseline.json"
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path $Csv)) {
    $dir = Split-Path -Parent $Csv
    if ($dir -and -not (Test-Path $dir)) {
        New-Item -ItemType Directory -Force $dir | Out-Null
    }
    Set-Content -Path $Csv -Value "query,layer,case_id,note" -Encoding UTF8
    Write-Output "created template: $Csv"
    Write-Output "fill 30+ rows (literal/paraphrase/semantic each >=10), then rerun this script"
    exit 0
}

$rows = @(Import-Csv -Path $Csv -Encoding UTF8)
if ($rows.Count -eq 0) {
    throw "CSV has no data rows: $Csv"
}

$cases = New-Object System.Collections.ArrayList
$counts = @{ literal = 0; paraphrase = 0; semantic = 0 }
$rowNo = 1

foreach ($r in $rows) {
    $rowNo++
    $q = [string]$r.query
    $layer = ([string]$r.layer).Trim().ToLower()
    $id = ([string]$r.case_id).Trim()
    $note = [string]$r.note

    if ([string]::IsNullOrWhiteSpace($q)) {
        throw "row ${rowNo}: empty query"
    }
    if ($layer -notin @("literal", "paraphrase", "semantic")) {
        throw "row ${rowNo}: layer must be literal/paraphrase/semantic, got '$layer'"
    }
    if ([string]::IsNullOrWhiteSpace($id) -or $id -match "PASTE|TODO|HASH") {
        throw "row ${rowNo}: invalid case_id '$id' (paste the hash16 from scx: stored case=...)"
    }

    $obj = [ordered]@{
        query    = $q.Trim()
        layer    = $layer
        expected = @($id)
    }
    if ($layer -eq "literal") {
        $rel = @{}
        $rel[$id] = 2
        $obj["rel"] = $rel
    } else {
        $obj["judge"] = $null
        if ([string]::IsNullOrWhiteSpace($note)) {
            $note = "target=$id"
        }
    }
    if (-not [string]::IsNullOrWhiteSpace($note)) {
        $obj["note"] = $note.Trim()
    }

    [void]$cases.Add([pscustomobject]$obj)
    $counts[$layer]++
}

$dir = Split-Path -Parent $Out
if ($dir -and -not (Test-Path $dir)) {
    New-Item -ItemType Directory -Force $dir | Out-Null
}
$json = [ordered]@{ cases = $cases } | ConvertTo-Json -Depth 10
[IO.File]::WriteAllText($Out, $json, (New-Object Text.UTF8Encoding $false))

Write-Output "wrote: $Out"
Write-Output ("gate sample: total={0} literal={1} paraphrase={2} semantic={3}" -f $cases.Count, $counts.literal, $counts.paraphrase, $counts.semantic)
if ($cases.Count -lt 30 -or $counts.literal -lt 10 -or $counts.paraphrase -lt 10 -or $counts.semantic -lt 10) {
    Write-Warning "not gate-ready: need total>=30 and literal/paraphrase/semantic each >=10"
} else {
    Write-Output "gate-ready sample counts; next: --judge, then --eval --gate"
}
