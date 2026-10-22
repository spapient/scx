# Seed 40 realistic sample memory cases and emit the matching eval CSV.
# NOTE: these are synthetic fixtures, not verified historical incidents.
# Usage: pwsh -NoProfile -File eval-real/seed-40.ps1
# Requires an empty -MemDir; refuses to append to an existing corpus.
param(
    [string]$MemDir = (Join-Path $PSScriptRoot "mem"),
    [string]$Csv = (Join-Path $PSScriptRoot "cases.csv"),
    [string]$Binary = (Join-Path $PSScriptRoot "..\rs\target\release\scx.exe"),
    [switch]$ReuseExisting
)

$ErrorActionPreference = "Stop"

function Write-Utf8([string]$Path, [string]$Text) {
    [IO.File]::WriteAllText($Path, $Text, (New-Object Text.UTF8Encoding $false))
}

function CsvField([string]$Value) {
    return '"' + $Value.Replace('"', '""') + '"'
}

if (-not (Test-Path -LiteralPath $Binary)) {
    throw "missing release binary: $Binary (run cargo build --release --locked in rs)"
}
if (-not (Test-Path -LiteralPath $MemDir)) {
    New-Item -ItemType Directory -Force -Path $MemDir | Out-Null
}
$existing = @(Get-ChildItem -LiteralPath $MemDir -Force -ErrorAction SilentlyContinue)
if (-not $ReuseExisting -and $existing.Count -gt 0) {
    throw "MemDir is not empty: $MemDir (refusing to mix a new corpus with existing cases)"
}

$idByQuery = @{}
if ($ReuseExisting) {
    $caseRoot = Join-Path $MemDir "cases"
    if (-not (Test-Path -LiteralPath $caseRoot)) {
        throw "ReuseExisting requested but no cases directory: $caseRoot"
    }
    foreach ($dir in Get-ChildItem -LiteralPath $caseRoot -Directory) {
        $caseJson = Get-Content -LiteralPath (Join-Path $dir.FullName "case.json") -Raw -Encoding UTF8 | ConvertFrom-Json
        $idByQuery[[string]$caseJson.query] = [string]$caseJson.id
    }
}

# layer is the intended eval layer; eval is the query used only for non-literal rows.
$cases = @(
    @{ q = "db pool exhausted conn"; layer = "literal"; eval = ""; text = "ERROR database connection pool exhausted conn_id=fkab`nat Pool.java:214"; fix = "increase pool to 64 and close leaked connections" },
    @{ q = "db write no space errno 28"; layer = "literal"; eval = ""; text = "ERROR db write errno=28 no space left on device`nat Disk.java:77"; fix = "free disk space and rotate WAL" },
    @{ q = "deadlock wait chain"; layer = "paraphrase"; eval = "transaction deadlock wait chain detected"; text = "ERROR deadlock detected wait chain transaction=8821`nat Lock.java:11"; fix = "retry transaction and fix lock ordering" },
    @{ q = "replica lag seconds"; layer = "paraphrase"; eval = "replication delay 420 seconds behind primary"; text = "WARN replica lag 420s behind primary`nat Repl.java:51"; fix = "throttle writes and check replica IO" },
    @{ q = "unique constraint violation"; layer = "literal"; eval = ""; text = "ERROR duplicate key value violates unique constraint users_email_key`nat Db.java:301"; fix = "deduplicate input and add upsert" },
    @{ q = "query timeout statement"; layer = "paraphrase"; eval = "database query timeout after 30 seconds"; text = "ERROR query timeout after 30s statement=select_orders`nat Jdbc.java:88"; fix = "add index on orders.created_at and retry" },
    @{ q = "disk full inode exhausted"; layer = "literal"; eval = ""; text = "ERROR disk full inode exhausted`nat Fs.java:9"; fix = "delete old logs and raise inode limit" },
    @{ q = "disk io read timeout"; layer = "paraphrase"; eval = "storage read timeout on device sda"; text = "ERROR io error read timeout dev=sda`nat Fs.java:4"; fix = "replace failing disk and reschedule job" },
    @{ q = "read only file system"; layer = "semantic"; eval = "文件系统只读 read-only 导致写入失败"; text = "ERROR read-only file system on /data`nat Mount.java:19"; fix = "remount rw and check fstab" },
    @{ q = "too many open files"; layer = "literal"; eval = ""; text = "ERROR EMFILE too many open files`nat Fd.java:6"; fix = "raise ulimit -n and close leaked handles" },
    @{ q = "nfs stale file handle"; layer = "paraphrase"; eval = "nfs mount reports stale file handle"; text = "ERROR stale file handle nfs mount /data`nat Nfs.java:33"; fix = "remount nfs and restart client" },
    @{ q = "out of memory heap"; layer = "semantic"; eval = "Java 进程内存耗尽 out of memory 崩溃"; text = "FATAL out of memory heap 4g`nat Mem.java:12"; fix = "reduce batch size and raise Xmx" },
    @{ q = "oom killer invoked"; layer = "literal"; eval = ""; text = "WARN oom-killer invoked pid=4421`nat Kernel.java:7"; fix = "set memory limits and add swap" },
    @{ q = "cpu throttled cgroup"; layer = "paraphrase"; eval = "container cpu throttling by cgroup quota"; text = "WARN cpu throttled cgroup cpu.max quota=20000`nat Cgroup.java:14"; fix = "raise cpu quota and reduce burst load" },
    @{ q = "gc pause too long"; layer = "semantic"; eval = "垃圾回收 gc 长时间停顿导致应用卡顿"; text = "WARN gc pause 2.4s full gc`nat Jvm.java:61"; fix = "tune heap and switch to G1" },
    @{ q = "connection reset peer"; layer = "literal"; eval = ""; text = "ERROR connection reset by peer errno=104`nat Net.java:3"; fix = "retry with backoff and check firewall" },
    @{ q = "dns lookup failed host"; layer = "paraphrase"; eval = "dns resolution failure for api.example.com host"; text = "ERROR dns lookup failed host=api.example.com`nat Net.java:8"; fix = "fix resolv.conf and verify DNS" },
    @{ q = "tls handshake timeout"; layer = "semantic"; eval = "证书过期导致 tls 握手失败"; text = "ERROR tls handshake timeout cert expired`nat Tls.java:5"; fix = "renew certificate and sync clock" },
    @{ q = "port already in use"; layer = "literal"; eval = ""; text = "ERROR port 8080 already in use`nat Sock.java:1"; fix = "kill stale process and change port" },
    @{ q = "connection refused upstream"; layer = "paraphrase"; eval = "upstream connection refused on 10.0.3.7 port 5432"; text = "ERROR connection refused upstream 10.0.3.7:5432`nat Proxy.java:22"; fix = "start upstream and check security group" },
    @{ q = "packet loss gateway"; layer = "semantic"; eval = "packet loss causing retransmissions"; text = "WARN packet loss 18% gateway 10.0.0.1`nat Netmon.java:40"; fix = "replace bad cable and check switch" },
    @{ q = "queue overflow backlog"; layer = "literal"; eval = ""; text = "ERROR queue overflow backlog limit 10000`nat Q.java:2"; fix = "add consumers and raise limit" },
    @{ q = "consumer lag increasing"; layer = "paraphrase"; eval = "kafka consumer group falling behind partition 3"; text = "WARN consumer lag increasing partition=3 lag=82000`nat Kafka.java:27"; fix = "scale consumers and rebalance" },
    @{ q = "thread pool rejected"; layer = "semantic"; eval = "thread pool 线程池饱和导致任务被拒绝"; text = "ERROR thread pool rejected execution`nat Exec.java:17"; fix = "increase pool and add backpressure" },
    @{ q = "redis maxmemory eviction"; layer = "literal"; eval = ""; text = "WARN redis maxmemory reached evicted keys`nat Redis.java:12"; fix = "raise maxmemory and set policy" },
    @{ q = "cargo build compile fail"; layer = "paraphrase"; eval = "cargo compilation failed for hello crate"; text = "error: could not compile hello`nat src/main.rs:14"; fix = "fix borrow checker error and rerun cargo build" },
    @{ q = "rust mismatched types"; layer = "semantic"; eval = "return value mismatched types"; text = "error[E0308]: mismatched types`nat src/main.rs:10"; fix = "match return type and rerun cargo check" },
    @{ q = "maven package not found"; layer = "literal"; eval = ""; text = "error: package foo does not exist`nat Build.java:214"; fix = "add dependency and refresh maven" },
    @{ q = "command not found kubectl"; layer = "paraphrase"; eval = "kubectl executable not found in shell path"; text = "bash: line 3: command not found: kubectl`nat deploy.sh"; fix = "install kubectl and fix PATH" },
    @{ q = "docker image pull unauthorized"; layer = "semantic"; eval = "镜像仓库 pull access denied 鉴权失败"; text = "ERROR pull access denied unauthorized`nat Docker.java:5"; fix = "docker login and update image tag" },
    @{ q = "helm release pending upgrade"; layer = "literal"; eval = ""; text = "ERROR release pending-upgrade timeout`nat Helm.java:8"; fix = "rollback release and retry upgrade" },
    @{ q = "pytest failed test_api"; layer = "paraphrase"; eval = "pytest test_api assertion failed with status 500"; text = "FAILED test_api - AssertionError: status 500`nat test_api.py:42"; fix = "fix null handling and rerun pytest" },
    @{ q = "jest test timeout"; layer = "semantic"; eval = "jest 前端测试用例执行超时"; text = "FAILED jest timeout 5000ms`nat api.test.js:19"; fix = "increase timeout and mock network" },
    @{ q = "flaky integration test"; layer = "literal"; eval = ""; text = "FAILED integration test order flaky`nat it_order.py:77"; fix = "isolate test data and retry" },
    @{ q = "jwt signature invalid"; layer = "paraphrase"; eval = "jwt token signature verification failed"; text = "ERROR jwt signature invalid`nat Auth.java:44"; fix = "rotate signing key and clear cache" },
    @{ q = "permission denied sudo"; layer = "semantic"; eval = "部署脚本缺少 sudo 执行权限"; text = "ERROR permission denied sudo`nat deploy.sh:9"; fix = "grant least privilege and rerun" },
    @{ q = "ssl certificate expired"; layer = "literal"; eval = ""; text = "ERROR ssl certificate expired`nat Nginx.java:3"; fix = "renew cert and reload nginx" },
    @{ q = "config key missing"; layer = "semantic"; eval = "服务启动缺少 config 配置项"; text = "ERROR missing required config key db.url`nat Config.java:12"; fix = "add key to env and restart" },
    @{ q = "timezone offset mismatch"; layer = "semantic"; eval = "服务器 timezone 时区与本地时间不一致"; text = "WARN timezone offset mismatch expected +08:00 got +00:00`nat Clock.java:6"; fix = "set TZ and restart service" },
    @{ q = "rate limit exceeded 429"; layer = "semantic"; eval = "接口调用 rate limit 限流需要退避重试"; text = "ERROR rate limit exceeded 429 retry_after=60`nat Client.java:91"; fix = "add backoff and raise quota" }
)

if ($cases.Count -ne 40) {
    throw "expected 40 seed cases, got $($cases.Count)"
}

$rows = New-Object System.Collections.ArrayList
$i = 0
foreach ($c in $cases) {
    $i++
    if ($ReuseExisting) {
        if (-not $idByQuery.ContainsKey([string]$c.q)) {
            throw "case $i not found in existing corpus: $($c.q)"
        }
        $caseId = $idByQuery[[string]$c.q]
    } else {
        $log = Join-Path $env:TEMP ("scx-seed-{0:D2}-{1}.log" -f $i, [guid]::NewGuid().ToString("N"))
        Write-Utf8 $log $c.text
        $output = (& $Binary --query $c.q --store 1 --fix $c.fix --mem-dir $MemDir $log 2>&1 | Out-String)
        if ($output -notmatch "scx: stored case=([^\s]+) added=yes") {
            if ($output -match "scx: stored case=([^\s]+) added=no") {
                throw "case $i deduplicated unexpectedly: $($Matches[1])"
            }
            throw "case $i store failed: $output"
        }
        $caseId = $Matches[1]
    }
    $hash16 = $caseId.Split('_')[-1]
    $evalQuery = if ($c.layer -eq "literal") { $c.q } else { $c.eval }
    $hint = ($c.text -split "`n")[0]
    $note = if ($c.layer -eq "literal") { "" } else { "target=$hash16 hint=[$hint] fix=[$($c.fix)]" }
    [void]$rows.Add([pscustomobject]@{ query = $evalQuery; layer = $c.layer; case_id = $caseId; note = $note })
    Write-Host ("{0:D2} {1,-13} {2}  {3}" -f $i, $c.layer, $caseId, $evalQuery)
}

$lines = @("query,layer,case_id,note")
foreach ($r in $rows) {
    $fields = @($r.query, $r.layer, $r.case_id, $r.note) | ForEach-Object { CsvField ([string]$_) }
    $lines += ($fields -join ",")
}
Write-Utf8 $Csv ($lines -join "`n")

$literal = @($rows | Where-Object { $_.layer -eq "literal" }).Count
$paraphrase = @($rows | Where-Object { $_.layer -eq "paraphrase" }).Count
$semantic = @($rows | Where-Object { $_.layer -eq "semantic" }).Count
Write-Host ""
Write-Host "wrote: $Csv"
Write-Host ("counts: total={0} literal={1} paraphrase={2} semantic={3}" -f $rows.Count, $literal, $paraphrase, $semantic)
