# 真实历史事故语料：从公开复盘派生 40 条 case 入库，并输出匹配的 eval CSV。
# 红线：不编造事故与原始日志；每条 text 标注 "公开复盘, 来源: <URL>"。
# 来源清单见 docs/real-incident-sources.md。
#
# 用法（Windows PowerShell 5.1）：
#   powershell -NoProfile -ExecutionPolicy Bypass -File eval-real\seed-real.ps1
#   powershell -NoProfile -ExecutionPolicy Bypass -File eval-real\seed-real.ps1 -MemDir eval-real\real-mem -Csv eval-real\real-cases.csv
param(
    [string]$MemDir = (Join-Path $PSScriptRoot "real-mem"),
    [string]$Csv = (Join-Path $PSScriptRoot "real-cases.csv"),
    [string]$Binary = (Join-Path $PSScriptRoot "..\rs\target\release\scx.exe"),
    [switch]$ReuseExisting,
    [switch]$Force
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
    if (-not $Force) {
        throw "MemDir is not empty: $MemDir (refusing to mix a new corpus with existing cases; use -Force to clear a workspace-local dir)"
    }
    $wsRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
    $memAbs = (Resolve-Path -LiteralPath $MemDir).Path
    if (-not $memAbs.StartsWith($wsRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "refusing to clear non-workspace dir: $memAbs"
    }
    Remove-Item -LiteralPath $memAbs -Recurse -Force
    New-Item -ItemType Directory -Force -Path $memAbs | Out-Null
    Write-Output "cleared (workspace-local): $memAbs"
}

$src = @{
    cloudflare = "https://blog.cloudflare.com/details-of-the-cloudflare-outage-on-july-2-2019/"
    gitlab     = "https://about.gitlab.com/blog/2017/02/10/postmortem-of-database-outage-of-january-31/"
    github     = "https://github.blog/news-insights/company-news/oct21-post-incident-analysis/"
    reddit     = "https://www.reddit.com/r/RedditEng/comments/11xx5o0/you_broke_reddit_the_piday_outage/"
    fastly     = "https://www.fastly.com/blog/summary-of-june-8-outage"
    atlassian  = "https://www.atlassian.com/blog/company-news/post-incident-review-april-2022-outage"
    facebook   = "https://engineering.fb.com/2021/10/05/networking-traffic/outage-details/"
    slack      = "https://slack.engineering/what-happened-during-slacks-dnssec-rollout/"
    aws        = "https://aws.amazon.com/message/65648/"
    gcp        = "https://status.cloud.google.com/incident/cloud-networking/19008"
    azure      = "https://azure.status.microsoft.com/en-us/status/history/"
}

# 每条：q=入库 query；layer=literal/paraphrase/semantic；eval=非 literal 的 eval query（literal 留空取 q）；
# t1=text 首行（症状摘要）；rc=根因一行；fix=修复结论；src=来源 URL。
$cases = @(
    # 1 Cloudflare —— literal / literal2 / paraphrase / semantic
    @{ q = "waf regex catastrophic backtracking cpu"; layer = "literal"; eval = ""; t1 = "WAF 正则回溯导致边缘节点 CPU 100% 返回 502"; rc = "WAF 新规则正则回溯，全球边缘节点 CPU 打满约 27 分钟"; fix = "回滚该 WAF 正则规则，加回溯上限并分阶段灰度发布"; src = $src.cloudflare },
    @{ q = "edge 502 bad gateway spike"; layer = "literal"; eval = ""; t1 = "边缘节点 502 Bad Gateway 请求激增"; rc = "同一 Cloudflare 事故的边缘 502 表现"; fix = "回滚触发规则并监控边缘 502 率"; src = $src.cloudflare },
    @{ q = "waf regex catastrophic backtracking"; layer = "paraphrase"; eval = "waf regex caused catastrophic backtracking that pegged every edge cpu"; t1 = "WAF 正则回溯导致边缘节点 CPU 100% 返回 502"; rc = "WAF 新规则正则回溯，全球边缘节点 CPU 打满约 27 分钟"; fix = "回滚该 WAF 正则规则，加回溯上限并分阶段灰度发布"; src = $src.cloudflare },
    @{ q = "edge cpu exhaustion 502"; layer = "semantic"; eval = "网站全部返回 502 因为边缘节点 edge CPU 被打满"; t1 = "边缘节点 CPU 100%，全球 502 错误"; rc = "WAF 新规则正则回溯，全球边缘节点 CPU 打满约 27 分钟"; fix = "回滚该 WAF 正则规则，加回溯上限并分阶段灰度发布"; src = $src.cloudflare },

    # 2 GitLab —— literal / paraphrase / semantic / paraphrase2
    @{ q = "database replication failure data loss"; layer = "literal"; eval = ""; t1 = "数据库复制故障叠加主库目录误删导致 6 小时数据丢失"; rc = "复制故障期间误删主库数据目录，约 6 小时数据丢失"; fix = "从备份恢复，修复复制与备份流程并加安全操作护栏"; src = $src.gitlab },
    @{ q = "db replica lag primary directory removed"; layer = "paraphrase"; eval = "primary database directory was accidentally removed while fixing replica lag"; t1 = "数据库复制故障叠加主库目录误删导致 6 小时数据丢失"; rc = "复制故障期间误删主库数据目录，约 6 小时数据丢失"; fix = "从备份恢复，修复复制与备份流程并加安全操作护栏"; src = $src.gitlab },
    @{ q = "database recovery data loss"; layer = "semantic"; eval = "误删数据库主目录导致 database data 丢失 6 小时"; t1 = "误删数据库主目录，6 小时数据丢失"; rc = "复制故障期间误删主库数据目录，约 6 小时数据丢失"; fix = "从备份恢复，修复复制与备份流程并加安全操作护栏"; src = $src.gitlab },
    @{ q = "restoring from backup after accidental deletion"; layer = "paraphrase"; eval = "recovery from backup after accidental primary deletion took about six hours"; t1 = "误删主库后从备份恢复耗时约 6 小时"; rc = "复制故障期间误删主库数据目录，约 6 小时数据丢失"; fix = "从备份恢复，修复复制与备份流程并加安全操作护栏"; src = $src.gitlab },

    # 3 GitHub —— literal / literal2 / paraphrase / semantic
    @{ q = "mysql network partition split brain"; layer = "literal"; eval = ""; t1 = "网络分区导致 MySQL 主从裂脑与元数据不一致"; rc = "网络分区触发 MySQL 主库切换与裂脑写入，约 24 小时降级"; fix = "恢复拓扑，加主从选举护栏与分区检测"; src = $src.github },
    @{ q = "primary failover during network partition"; layer = "literal"; eval = ""; t1 = "网络分区期间数据库主库故障切换"; rc = "网络分区触发 MySQL 主库切换与裂脑写入，约 24 小时降级"; fix = "恢复拓扑，加主从选举护栏与分区检测"; src = $src.github },
    @{ q = "db split brain inconsistent metadata"; layer = "paraphrase"; eval = "network partition caused database split brain and inconsistent metadata"; t1 = "网络分区导致 MySQL 主从裂脑与元数据不一致"; rc = "网络分区触发 MySQL 主库切换与裂脑写入，约 24 小时降级"; fix = "恢复拓扑，加主从选举护栏与分区检测"; src = $src.github },
    @{ q = "database failover inconsistency"; layer = "semantic"; eval = "网络分区引发 database failover 主备裂脑、数据不一致"; t1 = "数据库主备裂脑，元数据不一致"; rc = "网络分区触发 MySQL 主库切换与裂脑写入，约 24 小时降级"; fix = "恢复拓扑，加主从选举护栏与分区检测"; src = $src.github },

    # 4 Reddit —— literal / paraphrase / semantic
    @{ q = "kubernetes upgrade route reflector breakage"; layer = "literal"; eval = ""; t1 = "K8s 1.23 到 1.24 升级触发 route reflector 标签冲突致站点中断"; rc = "K8s 升级与 route reflector 标签重命名冲突，致全站中断"; fix = "回滚升级，对齐 route reflector 配置并清理旧版命令"; src = $src.reddit },
    @{ q = "k8s upgrade breaks route reflectors"; layer = "paraphrase"; eval = "cluster upgrade to 1.24 broke route reflector label configuration"; t1 = "K8s 1.23 到 1.24 升级触发 route reflector 标签冲突致站点中断"; rc = "K8s 升级与 route reflector 标签重命名冲突，致全站中断"; fix = "回滚升级，对齐 route reflector 配置并清理旧版命令"; src = $src.reddit },
    @{ q = "kubernetes cluster upgrade outage"; layer = "semantic"; eval = "Kubernetes 集群升级后 route reflector 配置冲突导致服务中断"; t1 = "Kubernetes 集群升级后 route reflector 配置冲突，服务中断"; rc = "K8s 升级与 route reflector 标签重命名冲突，致全站中断"; fix = "回滚升级，对齐 route reflector 配置并清理旧版命令"; src = $src.reddit },

    # 5 Fastly —— literal / paraphrase / semantic
    @{ q = "config change triggers software bug outage"; layer = "literal"; eval = ""; t1 = "合法客户配置变更触发未发现软件缺陷，85% 网络报错"; rc = "合法配置变更触发未发现缺陷，约 50 分钟 85% 网络报错"; fix = "回滚配置并修复缺陷，补回归测试"; src = $src.fastly },
    @{ q = "valid config change triggers latent bug"; layer = "paraphrase"; eval = "a valid customer configuration change activated a latent software bug and took the network down"; t1 = "合法客户配置变更触发未发现软件缺陷，85% 网络报错"; rc = "合法配置变更触发未发现缺陷，约 50 分钟 85% 网络报错"; fix = "回滚配置并修复缺陷，补回归测试"; src = $src.fastly },
    @{ q = "cdn config change outage"; layer = "semantic"; eval = "客户改了一个合法 config 就触发未发现缺陷导致全球 CDN 故障"; t1 = "合法配置变更触发未发现缺陷，全球 CDN 故障"; rc = "合法配置变更触发未发现缺陷，约 50 分钟 85% 网络报错"; fix = "回滚配置并修复缺陷，补回归测试"; src = $src.fastly },

    # 6 Atlassian —— literal / paraphrase / semantic
    @{ q = "script wrong mode deletes customer data"; layer = "literal"; eval = ""; t1 = "维护脚本以错误执行模式与错误 ID 列表误删客户数据"; rc = "脚本用错执行模式与 ID 列表，误删客户站点数据"; fix = "引入软删除策略，复核脚本与 runbook 并恢复数据"; src = $src.atlassian },
    @{ q = "maintenance script deleted sites"; layer = "paraphrase"; eval = "maintenance script executed with wrong mode and wrong id list erased customer sites"; t1 = "维护脚本以错误执行模式与错误 ID 列表误删客户数据"; rc = "脚本用错执行模式与 ID 列表，误删客户站点数据"; fix = "引入软删除策略，复核脚本与 runbook 并恢复数据"; src = $src.atlassian },
    @{ q = "script accidental deletion"; layer = "semantic"; eval = "运维 script 用错执行模式和 ID 列表，误删 deletion 客户数据"; t1 = "运维脚本用错执行模式和 ID 列表，误删客户数据"; rc = "脚本用错执行模式与 ID 列表，误删客户站点数据"; fix = "引入软删除策略，复核脚本与 runbook 并恢复数据"; src = $src.atlassian },

    # 7 Facebook —— literal / paraphrase / semantic / paraphrase2
    @{ q = "bgp routes withdrawn dns unavailable"; layer = "literal"; eval = ""; t1 = "骨干维护命令导致 BGP 路由撤回、DNS 不可用"; rc = "骨干维护命令使 BGP 路由撤回、DNS 不可用约 6 小时"; fix = "恢复骨干路由，解耦 DNS 与 BGP 依赖并加防护"; src = $src.facebook },
    @{ q = "backbone command withdrew bgp routes"; layer = "paraphrase"; eval = "a maintenance command on the backbone withdrew all bgp routes and took dns offline"; t1 = "骨干维护命令导致 BGP 路由撤回、DNS 不可用"; rc = "骨干维护命令使 BGP 路由撤回、DNS 不可用约 6 小时"; fix = "恢复骨干路由，解耦 DNS 与 BGP 依赖并加防护"; src = $src.facebook },
    @{ q = "bgp withdrawal dns outage"; layer = "semantic"; eval = "骨干网维护命令导致 BGP 路由全部撤回，域名 DNS 解析随之不可用"; t1 = "BGP 路由全部撤回，域名解析不可用"; rc = "骨干维护命令使 BGP 路由撤回、DNS 不可用约 6 小时"; fix = "恢复骨干路由，解耦 DNS 与 BGP 依赖并加防护"; src = $src.facebook },
    @{ q = "backbone maintenance took down dns and bgp"; layer = "paraphrase"; eval = "a backbone maintenance window took down both dns and bgp at the same time"; t1 = "骨干维护同时打挂 DNS 与 BGP"; rc = "骨干维护命令使 BGP 路由撤回、DNS 不可用约 6 小时"; fix = "恢复骨干路由，解耦 DNS 与 BGP 依赖并加防护"; src = $src.facebook },

    # 8 Slack —— literal / paraphrase / semantic / semantic2
    @{ q = "dnssec rollout servfail wildcard bug"; layer = "literal"; eval = ""; t1 = "DNSSEC 上线触发 Route53 通配符缺陷与 NSEC 错误返回 SERVFAIL"; rc = "DNSSEC 上线触发 Route53 通配符缺陷与 NSEC 错误，SERVFAIL 约 24 小时"; fix = "回退签名，改 ALIAS 记录并修复 NSEC"; src = $src.slack },
    @{ q = "dnssec signing caused servfail"; layer = "paraphrase"; eval = "enabling dnssec signing on the domain triggered a provider wildcard bug and nsec error causing servfail"; t1 = "DNSSEC 上线触发 Route53 通配符缺陷与 NSEC 错误返回 SERVFAIL"; rc = "DNSSEC 上线触发 Route53 通配符缺陷与 NSEC 错误，SERVFAIL 约 24 小时"; fix = "回退签名，改 ALIAS 记录并修复 NSEC"; src = $src.slack },
    @{ q = "dnssec enablement outage"; layer = "semantic"; eval = "上线 DNSSEC 后域名解析返回 SERVFAIL 持续近一天"; t1 = "DNSSEC 上线后域名解析返回 SERVFAIL"; rc = "DNSSEC 上线触发 Route53 通配符缺陷与 NSEC 错误，SERVFAIL 约 24 小时"; fix = "回退签名，改 ALIAS 记录并修复 NSEC"; src = $src.slack },
    @{ q = "dns signing rollback failed"; layer = "semantic"; eval = "DNS signing 签名 rollback 回滚时又遇到 NSEC 错误导致解析失败"; t1 = "DNS 签名回滚时遇到 NSEC 错误，解析持续失败"; rc = "DNSSEC 上线触发 Route53 通配符缺陷与 NSEC 错误，SERVFAIL 约 24 小时"; fix = "回退签名，改 ALIAS 记录并修复 NSEC"; src = $src.slack },

    # 9 AWS —— literal / literal2 / paraphrase / semantic
    @{ q = "ebs re-mirroring storm capacity shortage"; layer = "literal"; eval = ""; t1 = "网络事件引发 EBS re-mirroring 风暴与容量耗尽"; rc = "us-east-1 网络事件引发 EBS re-mirroring 风暴与容量耗尽"; fix = "扩容并修复控制面，错峰 re-mirroring"; src = $src.aws },
    @{ q = "ebs volume stuck re-mirroring state"; layer = "literal"; eval = ""; t1 = "EBS 卷卡在 re-mirroring 状态无法恢复"; rc = "us-east-1 网络事件引发 EBS re-mirroring 风暴与容量耗尽"; fix = "扩容并修复控制面，错峰 re-mirroring"; src = $src.aws },
    @{ q = "ebs volume re-mirroring overload"; layer = "paraphrase"; eval = "a network event caused an ebs volume re-mirroring storm that exhausted capacity in one zone"; t1 = "网络事件引发 EBS re-mirroring 风暴与容量耗尽"; rc = "us-east-1 网络事件引发 EBS re-mirroring 风暴与容量耗尽"; fix = "扩容并修复控制面，错峰 re-mirroring"; src = $src.aws },
    @{ q = "cloud volume recovery storm"; layer = "semantic"; eval = "网络事件引发 cloud 云盘 volume 大规模重新镜像，整个可用区容量打满"; t1 = "云盘大规模重新镜像，可用区容量打满"; rc = "us-east-1 网络事件引发 EBS re-mirroring 风暴与容量耗尽"; fix = "扩容并修复控制面，错峰 re-mirroring"; src = $src.aws },

    # 10 GCP —— literal / paraphrase / semantic
    @{ q = "network configuration change congestion outage"; layer = "literal"; eval = ""; t1 = "配置变更叠加软件缺陷导致多区域网络拥塞约 4 小时"; rc = "配置变更叠加软件缺陷导致多区域网络拥塞约 4 小时"; fix = "回滚配置并修复缺陷"; src = $src.gcp },
    @{ q = "configuration change caused network congestion"; layer = "paraphrase"; eval = "a configuration change plus software bugs caused multi-region network congestion for hours"; t1 = "配置变更叠加软件缺陷导致多区域网络拥塞约 4 小时"; rc = "配置变更叠加软件缺陷导致多区域网络拥塞约 4 小时"; fix = "回滚配置并修复缺陷"; src = $src.gcp },
    @{ q = "cloud network congestion multi region"; layer = "semantic"; eval = "一次 config 变更叠加缺陷导致多个 region network congestion 网络拥塞数小时"; t1 = "配置变更叠加缺陷，多区域网络拥塞"; rc = "配置变更叠加软件缺陷导致多区域网络拥塞约 4 小时"; fix = "回滚配置并修复缺陷"; src = $src.gcp },

    # 11 Azure AD —— literal / paraphrase / semantic / semantic2
    @{ q = "signing key rotation token validation failure"; layer = "literal"; eval = ""; t1 = "签名密钥轮换错误移除关键密钥导致令牌校验失败"; rc = "密钥轮换错误移除关键密钥，令牌校验失败约 14 小时"; fix = "回滚元数据，修复轮换逻辑并加速缓存恢复"; src = $src.azure },
    @{ q = "key rotation removed critical signing key"; layer = "paraphrase"; eval = "routine signing key rotation mistakenly removed a critical key and broke token validation"; t1 = "签名密钥轮换错误移除关键密钥导致令牌校验失败"; rc = "密钥轮换错误移除关键密钥，令牌校验失败约 14 小时"; fix = "回滚元数据，修复轮换逻辑并加速缓存恢复"; src = $src.azure },
    @{ q = "identity key rotation outage"; layer = "semantic"; eval = "身份认证签名 key rotation 轮换出错，登录令牌校验大面积失败"; t1 = "签名密钥轮换出错，令牌校验大面积失败"; rc = "密钥轮换错误移除关键密钥，令牌校验失败约 14 小时"; fix = "回滚元数据，修复轮换逻辑并加速缓存恢复"; src = $src.azure },
    @{ q = "single sign on login failure"; layer = "semantic"; eval = "企业单点登录 single sign on 大面积失败，用户无法登录办公套件"; t1 = "企业单点登录大面积失败，用户无法登录"; rc = "密钥轮换错误移除关键密钥，令牌校验失败约 14 小时"; fix = "回滚元数据，修复轮换逻辑并加速缓存恢复"; src = $src.azure }
)

$expected = 40
if ($cases.Count -ne $expected) {
    throw "expected $expected real cases, got $($cases.Count)"
}

$rows = New-Object System.Collections.ArrayList
$i = 0
foreach ($c in $cases) {
    $i++
    $log = Join-Path $env:TEMP ("scx-real-{0:D2}-{1}.log" -f $i, [guid]::NewGuid().ToString("N"))
    $text = ("{0}`n公开复盘, 来源: {1}`n根因: {2}" -f $c.t1, $c.src, $c.rc)
    Write-Utf8 $log $text
    # Windows PowerShell 5.1 下 native stderr 经 2>&1 会变成 ErrorRecord，
    # 配合脚本顶部 ErrorActionPreference=Stop 会在首行 stderr 处终止；局部降为 Continue 再合并。
    $prevEap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $output = (& $Binary --query $c.q --store 1 --fix $c.fix --mem-dir $MemDir $log 2>&1 | Out-String)
    } finally {
        $ErrorActionPreference = $prevEap
    }
    if ($output -notmatch "scx: stored case=([^\s]+) added=yes") {
        if ($output -match "scx: stored case=([^\s]+) added=no") {
            throw "case $i deduplicated unexpectedly: $($Matches[1]) query=$($c.q)"
        }
        throw "case $i store failed: $output"
    }
    $caseId = $Matches[1]
    $hash16 = $caseId.Split('_')[-1]
    $evalQuery = if ($c.layer -eq "literal") { $c.q } else { $c.eval }
    $note = if ($c.layer -eq "literal") { "" } else { "target=$hash16 hint=[$($c.t1)] fix=[$($c.fix)]" }
    [void]$rows.Add([pscustomobject]@{ query = $evalQuery; layer = $c.layer; case_id = $caseId; note = $note })
    Write-Host ("{0:D2} {1,-11} {2}  {3}" -f $i, $c.layer, $caseId, $evalQuery)
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
Write-Host "next: powershell -NoProfile -ExecutionPolicy Bypass -File scripts\real-eval-build.ps1 -Csv $Csv -Out eval-real\real-baseline.json"
