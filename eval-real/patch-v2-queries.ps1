# 一次性修订 v2 的 23 条低词面重叠 eval query（只改 pb/pd/sf，不改入库文档）。
# 修订理由：原句含 "the rollback / the recovery" 等指代，脱离上下文无法检索；
# 改成仍为改写、但自带实体锚点的 standalone query。逐条断言命中一次。
$ErrorActionPreference = "Stop"
$path = Join-Path $PSScriptRoot "seed-v2.ps1"
$text = Get-Content -LiteralPath $path -Raw -Encoding UTF8
$pairs = [ordered]@{
    'pb="a partition between sites produced two writable database primaries"' = 'pb="network partition between sites triggered mysql primary failover and duplicate writes"'
    'pd="old manifests for an unsupported kubernetes release caused the outage"' = 'pd="stale end of life kubernetes manifests collided with the cluster upgrade"'
    'sf="集群升级到新版本后旧路由反射器命令失效，整站不可用"' = 'sf="K8s 版本升级触发 route reflector 配置冲突，整站服务中断"'
    'pb="an ordinary settings update exposed an undiscovered defect in the edge network"' = 'pb="a valid customer configuration change triggered a latent edge software bug"'
    'pd="reverting the customer setting stopped the global error spike"' = 'pd="rolling back the customer configuration restored most of the edge network"'
    'pd="the remediation separated name resolution from backbone reachability"' = 'pd="the remediation decoupled dns resolution from bgp backbone reachability"'
    'pb="the change interacted with latent defects and degraded several us regions"' = 'pb="a configuration change combined with latent software bugs degraded several us regions"'
    'pd="the rollback removed the trigger and traffic recovered"' = 'pd="reverting the configuration restored normal traffic behavior in the affected regions"'
    'pb="traffic from many regions was redirected to atlanta and overwhelmed it"' = 'pb="traffic from many regions was redirected to the atlanta backbone and overwhelmed it"'
    'sf="一行配置把全部路由泄漏进骨干网，流量涌向单点后压垮网络"' = 'sf="亚特兰大路由器一行配置把全部 BGP 路由泄漏进骨干网，压垮单点"'
    'pb="the prefix policy update caused routing failures across many data centers"' = 'pb="the bgp community standardization change disrupted advertised prefixes across many data centers"'
    'pd="reverting the service token code restored the affected products"' = 'pd="the service token release was rolled back and the control plane recovered"'
    'pb="valid dns queries received servfail from the 1.1.1.1 service"' = 'pb="an internal software error made the public dns resolver return servfail to valid queries"'
    'sf="公共 DNS 解析器内部软件错误，正常查询也返回 SERVFAIL"' = 'sf="1.1.1.1 公共 DNS 解析服务内部错误，有效查询返回 SERVFAIL"'
    'pb="the read only endpoint was marked unhealthy and the site lost reads"' = 'pb="a database configuration change broke routing health checks and the read only endpoint"'
    'pd="the rollback fixed the health check responses"' = 'pd="reverting the database configuration change restored read traffic and health checks"'
    'pd="the rollback removed the overload trigger"' = 'pd="reverting the metadata configuration change restored blob storage lookups"'
    'pb="the lb config change caused 22 minutes of increased failures"' = 'pb="the load balancer configuration rollout caused 22 minutes of increased error rates"'
    'pd="the rollback removed the faulty load balancer configuration"' = 'pd="reverting the load balancer configuration rollout restored normal error rates"'
    'pd="the rollback removed the defective dns configuration"' = 'pd="the dns configuration rollback removed the defective setting and restored resolution"'
    'pd="the rollback removed the invalid setting"' = 'pd="reverting the invalid storage configuration restored the storage service"'
    'sf="存储服务应用了错误配置，请求大面积失败"' = 'sf="Azure 存储配置错误导致存储服务请求大面积失败"'
    'pd="operators brought generators online and staged recovery"' = 'pd="operators brought generators online and used load shedding to recover the region"'
}
foreach ($k in $pairs.Keys) {
    $n = ([regex]::Matches($text, [regex]::Escape($k))).Count
    if ($n -ne 1) { throw "expected exactly one occurrence, got $n for ${k}" }
    $text = $text.Replace($k, $pairs[$k])
}
[IO.File]::WriteAllText($path, $text, (New-Object Text.UTF8Encoding $false))
Write-Output ("patched {0} eval queries -> {1}" -f $pairs.Count, $path)
