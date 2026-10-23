# Fill the v2 same-bucket cross-incident review with a deterministic,
# mechanism-level adjudication. This is an AI-assisted diagnostic baseline:
# it is reproducible and auditable, but it is NOT independent human evidence.
param(
    [string]$Weak = (Join-Path $PSScriptRoot "v2-judge-review-weak.tsv"),
    [string]$Baseline = (Join-Path $PSScriptRoot "v2-baseline.json"),
    [string]$MemDir = (Join-Path $PSScriptRoot "v2-mem"),
    [string]$Out = (Join-Path $PSScriptRoot "v2-judge-review-weak-codex.tsv"),
    [string]$PairsOut = (Join-Path $PSScriptRoot "v2-weak-pair-decisions.csv")
)
$ErrorActionPreference = "Stop"

# key = target incident index ":" candidate incident index.
# 1 = related failure mechanism, but not the answer; 0 = unrelated despite
# sharing a coarse scenario bucket.
$decisionText = @"
0:17|1|same database failover/replication and data-consistency class
1:2|0|operator deletion vs datacenter cooling
1:6|0|operator deletion vs key rotation
1:9|0|operator deletion vs control-plane release
1:26|1|same unintended-target deletion by an operator or cleanup script
2:1|0|datacenter cooling vs operator deletion
2:4|1|same datacenter power/cooling failure class
2:9|0|datacenter cooling vs control-plane release
2:13|0|datacenter cooling vs CPU saturation
2:22|0|datacenter cooling vs load-balancer config
2:26|0|datacenter cooling vs operator deletion
2:27|0|datacenter cooling vs service discovery
3:5|1|same storage capacity/config outage class
4:2|1|same datacenter power/cooling failure class
4:6|0|datacenter power vs key rotation
4:22|0|datacenter power vs load-balancer config
4:26|0|datacenter power vs operator deletion
5:23|1|same storage configuration outage class
5:27|0|storage config vs service discovery
6:1|0|key rotation vs operator deletion
6:4|0|key rotation vs datacenter power
6:8|1|same credential/key deployment failure class
6:9|0|key rotation vs control-plane release
6:10|0|key rotation vs CPU saturation
6:13|0|key rotation vs CPU saturation
6:24|0|key rotation vs dyno config propagation
6:26|0|key rotation vs operator deletion
7:18|1|same DNS resolution failure class
7:19|1|same DNS SERVFAIL/resolution failure class
7:25|1|same DNS resolution failure class
7:28|0|DNS failure vs generic latent-bug config change
8:6|1|same credential/key deployment failure class
8:13|0|credential config vs CPU saturation
8:22|0|credential config vs load-balancer config
8:26|0|credential config vs operator deletion
8:27|0|credential config vs service discovery
9:4|0|control-plane release vs datacenter power
9:6|0|control-plane release vs key rotation
9:22|0|control-plane release vs load-balancer config
9:27|0|control-plane release vs service discovery
10:4|0|CPU saturation vs datacenter power
10:6|0|CPU saturation vs key rotation
10:13|1|same rule-triggered CPU saturation / infinite-loop class
10:14|0|CPU saturation vs cache 530
10:22|0|CPU saturation vs load-balancer config
11:15|1|same BGP route announcement/withdrawal failure class
11:21|1|same BGP route announcement/withdrawal failure class
11:29|1|same BGP/route-reflector failure class
13:10|1|same rule-triggered CPU saturation / infinite-loop class
13:14|0|CPU saturation vs cache 530
13:27|0|CPU saturation vs service discovery
14:6|0|cache 530 vs key rotation
14:8|0|cache 530 vs credential config
14:9|0|cache 530 vs control-plane release
14:24|0|cache 530 vs dyno config propagation
15:7|1|same DNS resolution failure class
15:11|1|same BGP route announcement/withdrawal failure class
15:18|1|same DNS resolution failure class
15:19|1|same DNS resolution failure class
15:21|1|same BGP route announcement/withdrawal failure class
15:25|1|same DNS resolution failure class
16:0|1|same database failover/replication and data-consistency class
16:17|1|same database failover/replication and data-consistency class
17:0|1|same database failover/replication and data-consistency class
18:7|1|same DNS resolution failure class
18:15|1|same DNS resolution failure class
18:19|1|same DNS resolution failure class
18:21|1|same DNS/route reachability failure class
19:7|1|same DNS SERVFAIL/resolution failure class
19:18|1|same DNS resolution failure class
19:25|1|same DNS resolution failure class
19:28|0|DNS failure vs generic latent-bug config change
20:11|1|same BGP/network-routing congestion class
20:25|0|network congestion vs DNS config
20:28|1|same config-change-induced latent-bug failure class
21:15|1|same BGP route announcement/withdrawal failure class
21:25|0|BGP announcement withdrawal vs DNS config
21:28|0|BGP announcement withdrawal vs generic latent-bug config change
22:9|0|load-balancer config vs control-plane release
22:10|0|load-balancer config vs CPU saturation
22:14|0|load-balancer config vs cache 530
22:24|1|same configuration rollout/propagation failure class
23:5|1|same storage configuration outage class
24:4|0|dyno config propagation vs datacenter power
24:13|0|dyno config propagation vs CPU saturation
24:14|0|dyno config propagation vs cache 530
24:22|1|same configuration rollout/propagation failure class
25:18|1|same DNS resolution failure class
25:19|1|same DNS resolution failure class
25:20|0|DNS config vs network congestion
25:21|0|DNS config vs BGP announcement withdrawal
25:28|0|DNS config vs generic latent-bug config change
25:29|0|DNS config vs K8s route-reflector upgrade
26:1|1|same unintended-target deletion by an operator or cleanup script
26:2|0|operator deletion vs datacenter cooling
26:8|0|operator deletion vs credential config
26:10|0|operator deletion vs CPU saturation
26:14|0|operator deletion vs cache 530
27:9|0|service discovery vs control-plane release
27:13|0|service discovery vs CPU saturation
27:22|0|service discovery vs load-balancer config
27:24|0|service discovery vs dyno config propagation
28:19|1|same config-change-induced latent-bug failure class
28:20|1|same config-change-induced latent-bug failure class
28:21|0|generic latent-bug config change vs BGP announcement withdrawal
28:29|1|same config/upgrade-induced latent incompatibility class
29:11|1|same BGP/route-reflector failure class
29:25|0|K8s route-reflector upgrade vs DNS config
29:28|0|K8s route-reflector upgrade vs generic latent-bug config change
"@

$decision = @{}
$reason = @{}
foreach ($line in ($decisionText -split "`r?`n")) {
    if ([string]::IsNullOrWhiteSpace($line)) { continue }
    $parts = $line -split '\|', 3
    if ($parts.Count -ne 3) { throw "bad decision row: $line" }
    $key = $parts[0].Trim()
    $rel = [int]$parts[1]
    if ($rel -notin @(0, 1)) { throw "decision rel must be 0/1: $line" }
    if ($decision.ContainsKey($key)) { throw "duplicate decision key: $key" }
    $decision[$key] = $rel
    $reason[$key] = $parts[2].Trim()
}

# Resolve each memory case to its source incident index (URL-sorted, stable).
$info = @{}
$sources = New-Object 'System.Collections.Generic.HashSet[string]'
foreach ($d in Get-ChildItem -LiteralPath (Join-Path $MemDir "cases") -Directory) {
    $cj = Join-Path $d.FullName "case.json"
    if (-not (Test-Path -LiteralPath $cj)) { continue }
    $c = Get-Content -LiteralPath $cj -Raw -Encoding UTF8 | ConvertFrom-Json
    $src = ""
    foreach ($line in ([string]$c.text -split "`n")) {
        if ($line -match '来源:\s*(https?://\S+)') { $src = $matches[1]; break }
    }
    if ([string]::IsNullOrWhiteSpace($src)) { throw "case $($c.id) has no source URL" }
    [void]$sources.Add($src)
    $info[[string]$c.id] = @{ src = $src; bucket = [string]$c.bucket }
}
$srcIndex = @{}
$i = 0
foreach ($s in ($sources | Sort-Object)) { $srcIndex[$s] = $i; $i++ }

$base = Get-Content -LiteralPath $Baseline -Raw -Encoding UTF8 | ConvertFrom-Json
$targets = @($base.cases | ForEach-Object { [string]$_.expected[0] })
$rows = @(Import-Csv -LiteralPath $Weak -Delimiter "`t" -Encoding UTF8)
$used = @{}
$pairRows = @{}
$dist = @{ 0 = 0; 1 = 0 }

foreach ($r in $rows) {
    $ci = [int]$r.case_index
    $target = $targets[$ci]
    $cand = [string]$r.candidate
    if (-not $info.ContainsKey($cand)) {
        $matches = @($info.Keys | Where-Object { $_ -like "*_$cand" })
        if ($matches.Count -ne 1) { throw "cannot resolve candidate $cand for case $ci" }
        $cand = $matches[0]
    }
    $tk = [int]$srcIndex[$info[$target].src]
    $ck = [int]$srcIndex[$info[$cand].src]
    $key = "$tk`:$ck"
    if (-not $decision.ContainsKey($key)) { throw "missing decision for target/candidate incident $key (case $ci)" }
    $rel = [int]$decision[$key]
    $used[$key] = $true
    $dist[$rel]++
    $r.rel = [string]$rel
    $r.note = "codex-ai: $($reason[$key]); same-bucket cross-incident"
    $pairRows[$key] = [pscustomobject]@{
        target_incident = $tk
        candidate_incident = $ck
        rel = $rel
        reason = $reason[$key]
        target_query = [string]$r.query
        candidate_summary = [string]$r.summary
    }
}

$unused = @($decision.Keys | Where-Object { -not $used.ContainsKey($_) })
if ($unused.Count -gt 0) { throw "decision keys not present in review packet: $($unused -join ', ')" }

$dir = Split-Path -Parent $Out
if ($dir -and -not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Force $dir | Out-Null }
$rows | Export-Csv -LiteralPath $Out -Delimiter "`t" -NoTypeInformation -Encoding UTF8
$pairRows.Values | Sort-Object target_incident, candidate_incident |
    Export-Csv -LiteralPath $PairsOut -NoTypeInformation -Encoding UTF8

Write-Output ("codex-ai weak review: rows={0} rel1={1} rel0={2} pairs={3} -> {4}" -f
    $rows.Count, $dist[1], $dist[0], $pairRows.Count, $Out)
Write-Output ("pair decisions -> {0}" -f $PairsOut)
Write-Output "NOT independent human evidence; keep the fei/hao human packet for M4 closure."
