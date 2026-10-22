# scx 记忆知识库（RAG）端到端演示（Windows PowerShell）
# 一键跑通：store → recall（literal/变体/实体）→ rate 反馈 → eval 对拍 → l1-hist → rebuild。
# 库用临时目录（$TEMP\scx-demo-mem），可反复重跑；演示 case 同时是人工标注的现成素材。
# 用法：powershell -ExecutionPolicy Bypass -File scripts/demo.ps1
$ErrorActionPreference = "Stop"
# 无 BOM 写入（Windows PowerShell 5.1 的 Out-File utf8 会写 BOM，污染 scx 解析）。
function Write-Utf8([string]$path, [string]$text) {
    [IO.File]::WriteAllText($path, $text, (New-Object Text.UTF8Encoding $false))
}
$scx = Join-Path $PSScriptRoot "..\rs\target\release\scx.exe"
if (-not (Test-Path $scx)) {
    Write-Error "未找到 release 二进制：先运行 cd rs && cargo build --release --locked"
}
$mem = Join-Path $env:TEMP "scx-demo-mem"
if (Test-Path $mem) { Remove-Item -Recurse -Force $mem }
New-Item -ItemType Directory -Force $mem | Out-Null
$log = Join-Path $env:TEMP "scx-demo-log.txt"

# 16 个真实感问题 case（query 各不相同 → 指纹互不碰撞，不会去重）
$cases = @(
    @{ q = "db timeout conn_id";              t = "ERROR db timeout conn_id=fkab pool exhausted`nat Pool.java:214" },
    @{ q = "db write errno no space";         t = "ERROR db write errno=28 no space left`nat Disk.java:77" },
    @{ q = "cargo build compile fail";        t = "error: could not compile hello`nat Pool.java:214" },
    @{ q = "disk full inode";                 t = "ERROR disk full inode exhausted`nat Fs.java:9" },
    @{ q = "out of memory heap";              t = "FATAL out of memory heap 4g`nat Mem.java:12" },
    @{ q = "connection reset peer";           t = "ERROR connection reset by peer errno=104`nat Net.java:3" },
    @{ q = "dns lookup failed host";          t = "ERROR dns lookup failed host=api.example.com`nat Net.java:8" },
    @{ q = "tls handshake timeout";           t = "ERROR tls handshake timeout cert expired`nat Tls.java:5" },
    @{ q = "deadlock detected";               t = "ERROR deadlock detected wait chain`nat Lock.java:11" },
    @{ q = "queue overflow backlog";          t = "ERROR queue overflow backlog limit`nat Q.java:2" },
    @{ q = "command not found kubectl";       t = "bash: line 3: command not found: kubectl`nat deploy.sh" },
    @{ q = "port already in use";             t = "ERROR port 8080 already in use`nat Sock.java:1" },
    @{ q = "maven package not found";         t = "error: package foo does not exist`nat Build.java:214" },
    @{ q = "pytest failed test_api";          t = "FAILED test_api - AssertionError: status 500`nat test_api.py:42" },
    @{ q = "rust mismatched types";           t = "error[E0308]: mismatched types`nat main.rs:10" },
    @{ q = "disk io read timeout";            t = "ERROR io error read timeout dev=sda`nat Fs.java:4" }
)

Write-Host "== 1) store：16 个证据块沉淀为记忆条目 ==" -ForegroundColor Cyan
foreach ($c in $cases) {
    Write-Utf8 $log $c.t
    & $scx --query $c.q --store 1 --mem-dir $mem $log | Out-Null
}
Write-Host "   库已建：$mem"

# 返回完整 case id（--rate 需要完整 id）；eval 用 hash8 后缀。
function CaseId([string]$q) {
    foreach ($d in Get-ChildItem "$mem\cases" -Directory) {
        $cj = Get-Content (Join-Path $d.FullName "case.json") -Raw -ErrorAction SilentlyContinue
        if ($cj -match [regex]::Escape("`"query`":`"$q`"")) { return $d.Name }
    }
    return ""
}
function Hash8([string]$q) { return (CaseId $q).Split('_')[-1] }

Write-Host ""
Write-Host "== 2) recall：literal 同 query（L1 直答目标）==" -ForegroundColor Cyan
& $scx --query "db timeout conn_id" --recall-only --recall 2 --mem-dir $mem

Write-Host ""
Write-Host "== 3) recall：paraphrase 变体 + 要点摘要（--recall-detail）==" -ForegroundColor Cyan
& $scx --query "数据库连接池超时 连接池" --recall-only --recall 2 --recall-detail --mem-dir $mem

Write-Host ""
Write-Host "== 4) recall：字段实体（file_loc Pool.java:214 跨桶兄弟由实体边兜底）==" -ForegroundColor Cyan
& $scx --query "连接池 timeout at Pool.java:214" --recall-only --recall 3 --mem-dir $mem

Write-Host ""
Write-Host "== 5) rate 反馈：good 标注（状态机 → solved + 词级调权 + L1 基线自适应）==" -ForegroundColor Cyan
$id0 = CaseId "db timeout conn_id"
& $scx --rate good $id0 --mem-dir $mem

Write-Host ""
Write-Host "== 6) eval：冻结标注集 A/B 对拍 + L1 命中率 ==" -ForegroundColor Cyan
$baseline = Join-Path $env:TEMP "scx-demo-baseline.json"
$id0 = Hash8 "db timeout conn_id"
$id3 = Hash8 "disk full inode"
$js = @"
{"cases":[
 {"query":"db timeout conn_id","layer":"literal","expected":["$id0"],"rel":{"$id0":2}},
 {"query":"数据库连接池超时 连接池","layer":"paraphrase","expected":["$id0"]},
 {"query":"数据库连接池耗尽","layer":"semantic","expected":["$id0"]},
 {"query":"disk full inode","layer":"literal","expected":["$id3"],"rel":{"$id3":2}},
 {"query":"存储空间不足","layer":"semantic","expected":["$id3"]}
]}
"@
Write-Utf8 $baseline $js
& $scx --eval $baseline --mem-dir $mem

Write-Host ""
Write-Host "== 7) l1-hist：运行期 top1.sim 命中分布 ==" -ForegroundColor Cyan
& $scx --l1-hist --mem-dir $mem

Write-Host ""
Write-Host "== 8) rebuild：倒排索引全量重建（df 快照 + qmap 合并）==" -ForegroundColor Cyan
& $scx --rebuild --mem-dir $mem

Write-Host ""
Write-Host "演示完成。库仍在 $mem（可删可留）；标注模板见 docs/eval-baseline-template.md。"
