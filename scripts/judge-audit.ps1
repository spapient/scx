# 盲测候选审计：为 baseline 的每条 query 生成 top-k 候选包（隐藏目标 id 与 A/B 分数），
# 供人工逐候选打 rel（2=强相关 / 1=弱相关 / 0=无关 / s=跳过），产出弱相关/负例证据；
# 用 -Import 把填好的包合并回 baseline（rel 字典 + judge 元数据）。
#
# 用法：
#   1) 生成盲测包：pwsh -NoProfile -File scripts\judge-audit.ps1 -Baseline eval-real\baseline.json -MemDir eval-real\mem -Out eval-real\judge-packet.tsv
#   2) 人工填 rel 列（2/1/0/s，s 或缺省=跳过）
#   3) 回填：pwsh -NoProfile -File scripts\judge-audit.ps1 -Import eval-real\judge-packet.tsv -Baseline eval-real\baseline.json -Judge <姓名>
#
# 盲测纪律：包内不出现 target id、不出现 A/B 分数；只有 query + 候选摘要 + fix。
# 第二裁决人：另一人用同一 -Baseline 的 judge:null 副本独立生成/填写，再用 judge-agreement.ps1 比对。
param(
    [string]$Baseline = "eval-real\baseline.json",
    [string]$MemDir = "eval-real\mem",
    [string]$Out = "eval-real\judge-packet.tsv",
    [int]$TopK = 10,
    [string]$Import = "",
    [string]$Judge = ""
)

$ErrorActionPreference = "Stop"
$Bin = Join-Path (Split-Path -Parent $PSScriptRoot) "rs\target\release\scx.exe"

function Get-Baseline([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { throw "missing baseline: $Path" }
    (Get-Content -LiteralPath $Path -Raw -Encoding UTF8 | ConvertFrom-Json).cases
}

function Write-Baseline([string]$Path, $Cases) {
    $json = [ordered]@{ cases = $Cases } | ConvertTo-Json -Depth 10
    [IO.File]::WriteAllText($Path, $json, (New-Object Text.UTF8Encoding $false))
}

function Get-Candidates([string]$Query) {
    $out = & $Bin --query $Query --recall-only --recall $TopK --recall-detail --mem-dir $MemDir 2>$null
    $rows = @()
    for ($i = 0; $i -lt $out.Count; $i++) {
        $m = [regex]::Match($out[$i], 'case=([0-9a-f]{8,})')
        if ($m.Success) {
            $hash16 = $m.Groups[1].Value
            $summary = ""
            $fix = ""
            if ($i + 1 -lt $out.Count -and $out[$i + 1] -match '^\s+summary: (.*)') {
                $summary = $matches[1]
            }
            if ($i + 2 -lt $out.Count -and $out[$i + 2] -match '^\s+fix: (.*)') {
                $fix = $matches[1] -replace '\s+\(candidate, sim=.*\)$', ''
            }
            $full = if ($script:IdMap.ContainsKey($hash16)) { $script:IdMap[$hash16] } else { $hash16 }
            $rows += [pscustomobject]@{ candidate = $full; summary = $summary; fix = $fix }
        }
    }
    $rows
}

function Get-IdMap([string]$Root) {
    $map = @{}
    $casesDir = Join-Path $Root "cases"
    if (-not (Test-Path -LiteralPath $casesDir)) { return $map }
    foreach ($d in Get-ChildItem -LiteralPath $casesDir -Directory) {
        $cj = Join-Path $d.FullName "case.json"
        if (-not (Test-Path -LiteralPath $cj)) { continue }
        try {
            $id = (Get-Content -LiteralPath $cj -Raw | ConvertFrom-Json).id
            $tail = $id -split '_' | Select-Object -Last 1
            if ($tail) { $map[[string]$tail] = [string]$id }
        } catch { }
    }
    $map
}

if ($Import) {
    if (-not (Test-Path -LiteralPath $Import)) { throw "missing packet: $Import" }
    if ([string]::IsNullOrWhiteSpace($Judge)) { throw "-Import 需要 -Judge <姓名>" }
    $packet = @(Import-Csv -LiteralPath $Import -Delimiter "`t" -Encoding UTF8)
    $cases = @(Get-Baseline $Baseline)
    $byCase = @{}
    foreach ($p in $packet) {
        $ci = [int]$p.case_index
        $rel = ([string]$p.rel).Trim().ToLower()
        if ($rel -in @("", "s")) { continue }
        if ($rel -notin @("0", "1", "2")) { throw "row $($p.row): rel must be 0/1/2/s, got '$($p.rel)'" }
        if (-not $byCase.ContainsKey($ci)) { $byCase[$ci] = [ordered]@{} }
        $byCase[$ci][[string]$p.candidate] = [int]$rel
    }
    $dist = @{ 2 = 0; 1 = 0; 0 = 0 }
    for ($i = 0; $i -lt $cases.Count; $i++) {
        if (-not $byCase.ContainsKey($i)) { continue }
        $rel = $byCase[$i]
        $target = [string]$cases[$i].expected[0]
        if (-not $rel.Contains($target)) {
            Write-Warning "case[$i] target $target 未在包中裁决，保留 judge:null"
            continue
        }
        $cases[$i] | Add-Member -NotePropertyName rel -NotePropertyValue $rel -Force
        $judge = [ordered]@{ by = $Judge; date = [DateTime]::UtcNow.ToString("yyyy-MM-ddTHH:mm:ssZ") }
        $cases[$i] | Add-Member -NotePropertyName judge -NotePropertyValue $judge -Force
        foreach ($k in @($rel.Keys)) { $dist[[int]$rel[$k]]++ }
    }
    Write-Baseline $Baseline $cases
    Write-Output ("judge-audit: import judge={0} rel2={1} rel1={2} rel0={3} -> {4}" -f $Judge, $dist[2], $dist[1], $dist[0], $Baseline)
    exit 0
}

# 生成盲测包
$cases = @(Get-Baseline $Baseline)
if ($cases.Count -eq 0) { throw "baseline has no cases: $Baseline" }
$script:IdMap = Get-IdMap $MemDir
$sb = New-Object System.Text.StringBuilder
[void]$sb.AppendLine("row`tcase_index`tlayer`tquery`tcandidate`tsummary`tfix`trel")
$row = 1
for ($ci = 0; $ci -lt $cases.Count; $ci++) {
    $c = $cases[$ci]
    foreach ($cand in @(Get-Candidates ([string]$c.query))) {
        [void]$sb.AppendLine(
            ("{0}`t{1}`t{2}`t{3}`t{4}`t{5}`t{6}`t" -f
                $row, $ci, [string]$c.layer, ([string]$c.query).Replace("`t", " "),
                $cand.candidate, $cand.summary.Replace("`t", " "), $cand.fix.Replace("`t", " "))
        )
        $row++
    }
}
$dir = Split-Path -Parent $Out
if ($dir -and -not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Force $dir | Out-Null }
[IO.File]::WriteAllText($Out, $sb.ToString(), (New-Object Text.UTF8Encoding $false))
Write-Output ("judge-audit: wrote {0} rows to {1} (blind: no target id, no A/B score)" -f ($row - 1), $Out)
Write-Output "fill rel column (2/1/0/s), then: pwsh -NoProfile -File scripts\judge-audit.ps1 -Import $Out -Baseline $Baseline -Judge <姓名>"
