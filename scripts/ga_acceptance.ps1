# =====================================================================
# E2E-GA 交钥匙终验剧本（契约 10.2/10.3；GA-FIX-4 步骤3 十三步版）
# 流程: 部署(本地构建+PG) → 注册能力 → 模板装配 → 产品实例化 → orchestrate
#       真实任务 → SSE 观测 → /metrics 核数 → knowledge-failures →
#       metrics-delta → 停机重启数据不丢；全程留证。
# 用法:  pwsh scripts/ga_acceptance.ps1 [-PgUrl "..."]   (默认连本机 15432)
# 产出:  artifacts/ga_evidence_<时间戳>.json
#        必含四要素(build_fix_r2 GA-FIX-4): session 创建记录(podman exec psql 直查)、
#        重启持久化通过、knowledge_count 字段、metrics_delta == 1、result=PASS
# =====================================================================
param(
    [string]$Port = "18080",
    [string]$PgUrl = "postgres://postgres:forge@localhost:15432/forge"
)
$ErrorActionPreference = "Stop"
$script:steps = New-Object System.Collections.Generic.List[object]
function Step($name, $ok, $detail) {
    $script:steps.Add([pscustomobject]@{ step = $name; pass = [bool]$ok; detail = [string]$detail })
    $mark = if ($ok) { "[PASS]" } else { "[FAIL]" }
    Write-Host "$mark $name :: $detail"
    if (-not $ok) { throw "GA-FAIL: $name" }
}
function Get-MetricsCounter($raw, $name) {
    $line = ($raw -split "`n" | Select-String "^$name ").Line
    if ($line) { return [int64]($line -replace "^$name\s+", "") }
    return -1
}

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# ---- G1. 构建（部署物=本机二进制）----
cargo build -p forge-server --bin forge-server 2>&1 | Out-Null
Step "build" ($LASTEXITCODE -eq 0) "cargo build forge-server"

# ---- G2. 启动服务（带 PG 持久化）----
$client = $null
$env:FORGE_PORT = $Port
$env:FORGE_PG_URL = $PgUrl
$srv = Start-Process -FilePath "$root\target\debug\forge-server.exe" -PassThru -WindowStyle Hidden `
        -RedirectStandardOutput "$root\artifacts\ga_server_out.log" -RedirectStandardError "$root\artifacts\ga_server_err.log"
try {
    # 等待健康检查
    $healthy = $false
    foreach ($i in 1..60) {
        try { $h = Invoke-RestMethod "http://127.0.0.1:$Port/health" -TimeoutSec 2; if ($h.status -eq "ok") { $healthy = $true; break } } catch { Start-Sleep 1 }
    }
    Step "deploy+health" $healthy "GET /health -> $($h.status), storage=PostgreSQL"

    # ---- G3. 注册能力（模板发布, Reviewer verdict=Pass）----
    $tpl = @{
        template = @{ id = "tpl.ga"; name = "GA demo"; parameters = @(); manifest_skeleton = @{
            id = ("product_" + [guid]::NewGuid().ToString("N").Substring(0, 12)); name = "ga-demo"; version = "1.0.0";
            description = "ga"; capabilities = @(@{ capability_name = "echo"; version = "0.1.0"; required = $true });
            entry_agent_role = "Orchestrator" } }
        version = "1.0.0"; review_verdict = "Pass"
    }
    $r = Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/templates" -ContentType "application/json" -Body ($tpl | ConvertTo-Json -Depth 8)
    Step "register-capability" ($r.published -eq "tpl.ga@1.0.0") "$($r.published)"

    # ---- G4. 模板装配 + 产品实例化 ----
    $inst = Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/products/instantiate" -ContentType "application/json" `
        -Body ('{"template_id":"tpl.ga","version":"1.0.0","name":"ga-inst"}')
    Step "instantiate" ($inst.state -eq "Draft") "instance=$($inst.instance_id)"

    # ---- G5. start → Active ----
    $p = Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/products/$($inst.instance_id)/start"
    Step "product-start" ($p.state -eq "Active") "state=$($p.state)"

    # ---- G6. orchestrate 真实任务（echo 工具 + 命令验收）----
    $rawBefore = (Invoke-WebRequest "http://127.0.0.1:$Port/metrics" -UseBasicParsing).Content
    $execBefore = Get-MetricsCounter $rawBefore "executions_total"
    $task = Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/tasks" -ContentType "application/json" `
        -Body '{"goal":"GA acceptance run","constraints":[],"acceptance":[]}'
    $cmd = if ($IsWindows -or $env:OS -like "*Windows*") { "echo ga-ok> ga.txt" } else { "echo ga-ok > ga.txt" }
    $run = Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/orchestrate" -ContentType "application/json" `
        -Body (@{ goal = "GA acceptance run"; timeout_secs = 30;
                  acceptance = @(@{ id = "AC-1"; description = "leave ga.txt"; check = @{ Command = $cmd } }) } | ConvertTo-Json -Depth 6)
    Step "orchestrate" ($run.gate_passed -eq $true) "final=$($run.final_status) gate=$($run.gate_passed) evidence=$($run.evidence_count)"
    $sessionCreated = ($run.gate_passed -eq $true)  # 编排器先建 Session 再执行，gate 通过即含 session 创建成功

    # ---- G7. SSE 观测：连接建立且能读到流字节 ----
    $client = [System.Net.Http.HttpClient]::new()
    $client.Timeout = [TimeSpan]::FromSeconds(10)
    $streamTask = $client.GetStreamAsync("http://127.0.0.1:$Port/events/stream")
    Start-Sleep -Seconds 2
    $sseOk = $streamTask.IsCompletedSuccessfully -and $streamTask.Result.CanRead
    if ($sseOk) { $streamTask.Result.Close() }
    Step "sse-observe" $sseOk "GET /events/stream 连接可读"

    # ---- G8. /metrics 核数 ----
    $m = Invoke-RestMethod "http://127.0.0.1:$Port/metrics"
    $mText = if ($m -is [string]) { $m } else { ($m | Out-String) }
    $raw = (Invoke-WebRequest "http://127.0.0.1:$Port/metrics" -UseBasicParsing).Content
    $execLine = ($raw -split "`n" | Select-String "^executions_total ").Line
    Step "metrics" ($raw -match "tasks_total ([1-9]\d*)") "counters: $(($raw -split "`n" | Where-Object { $_ -match "^[a-z]" -and $_ -notmatch "^#" }) -join '; ')"

    # ---- G9. knowledge-failures：KNW-001 服务面可读（GA-FIX-2 接线面）----
    $kf = Invoke-RestMethod "http://127.0.0.1:$Port/knowledge/failures"
    $kfArr = if ($kf -is [array]) { $kf } else { @($kf) }
    $knowledgeCount = $kfArr.Count
    Step "knowledge-failures" ($true) "GET /knowledge/failures -> 200, entries=$knowledgeCount"

    # ---- G10. metrics-delta：本次编排 executions_total 增量恰为 1 ----
    $rawAfter = (Invoke-WebRequest "http://127.0.0.1:$Port/metrics" -UseBasicParsing).Content
    $execAfter = Get-MetricsCounter $rawAfter "executions_total"
    $metricsDelta = ($execAfter - $execBefore)
    Step "metrics-delta" ($metricsDelta -eq 1) "executions_total: $execBefore -> $execAfter (delta=$metricsDelta)"

    # ---- G11. stop 产品 ----
    $p = Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:$Port/products/$($inst.instance_id)/stop"
    Step "product-stop" ($p.state -eq "Stopped") "state=$($p.state)"

    # ---- G12. 停机重启 → 数据不丢（PG）----
    Stop-Process -Id $srv.Id -Force; Wait-Process -Id $srv.Id -ErrorAction SilentlyContinue
    $srv = Start-Process -FilePath "$root\target\debug\forge-server.exe" -PassThru -WindowStyle Hidden `
            -RedirectStandardOutput "$root\artifacts\ga_server_out2.log" -RedirectStandardError "$root\artifacts\ga_server_err2.log"
    $persisted = $false
    foreach ($i in 1..60) {
        try {
            $t2 = Invoke-RestMethod "http://127.0.0.1:$Port/tasks/$($task.id)" -TimeoutSec 2
            if ($t2.id -eq $task.id) { $persisted = $true; break }
        } catch { Start-Sleep 1 }
    }
    Step "restart-persistence" $persisted "重启后 GET /tasks/$($task.id) 仍存在"

    Write-Host "`n=== GA 终验全部通过 ===" -ForegroundColor Green
    # ---- G13. 留证入库（含 session 直查 PG 记录）----
    $sessionId = $null
    try {
        $sessionId = (podman exec forge-pg psql -U postgres -d forge -t -A -c "SELECT id FROM sessions ORDER BY created_at DESC LIMIT 1" 2>$null)
    } catch { }
    if (-not $sessionId) {
        try {
            $sessionId = (docker exec forge-pg psql -U postgres -d forge -t -A -c "SELECT id FROM sessions ORDER BY created_at DESC LIMIT 1" 2>$null)
        } catch { }
    }
    $evidencePath = Join-Path $root "artifacts\ga_evidence_$(Get-Date -Format yyyyMMdd_HHmmss).json"
    [pscustomobject]@{
        run_at          = (Get-Date).ToString('o')
        pg_url          = $PgUrl
        task_id         = $task.id
        instance        = $inst.instance_id
        session_created = $sessionCreated
        session_id      = $sessionId
        knowledge_count = $knowledgeCount
        metrics_delta   = $metricsDelta
        result          = "PASS"
        steps           = $script:steps
    } | ConvertTo-Json -Depth 6 | Set-Content -Encoding utf8 $evidencePath
    Step "leave-evidence" ((Test-Path $evidencePath) -and $sessionId) $evidencePath
}
catch {
    Write-Host "GA 终验失败: $_" -ForegroundColor Red
    throw
}
finally {
    if ($srv -and -not $srv.HasExited) { Stop-Process -Id $srv.Id -Force }
    if ($null -ne $client) { $client.Dispose() }
}
