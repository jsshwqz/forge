# =====================================================================
# PERF-001 压测脚本（RPS / p50 / p95 / 错误率）
# 补齐 BASELINE.md 的"压测四数"；此前文档引用本文件但从未落盘（G-V5 复核发现）。
#
# 用法:
#   pwsh deploy/bench/orch_load.ps1 -Concurrent 10 -DurationSec 20        # API 面 POST /tasks
#   pwsh deploy/bench/orch_load.ps1 -Path /orchestrate -Body '<json>' `
#       -Concurrent 1 -MaxRequests 40
#
# 注意: /orchestrate 受 TEN-003 配额门控（默认并发 4 / 日 100），压测该路径
#       必须低并发 + 限量，否则 429 属配额生效而非性能失败。
# 兼容: PowerShell 5.1 / 7+（批式 HttpClient + Task.WaitAll，不用 -Parallel）。
# =====================================================================
param(
    [int]$Concurrent = 10,
    [int]$DurationSec = 20,
    [int]$MaxRequests = 0,                                   # 0 = 按时长；>0 = 总请求数封顶
    [string]$Port = "18081",
    [string]$Path = "/tasks",
    [string]$Body = '{"goal":"load-probe","constraints":[],"acceptance":[]}',
    [string]$PgUrl = "postgres://postgres:forge@localhost:15432/forge",
    [switch]$KeepServer
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)

# ---- 启动被测服务（真实 PG 存储）----
$env:FORGE_PORT = $Port
$env:FORGE_PG_URL = $PgUrl
$srv = Start-Process -FilePath "$root\target\debug\forge-server.exe" -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput "$root\artifacts\load_srv_out.log" -RedirectStandardError "$root\artifacts\load_srv_err.log"
$healthy = $false
foreach ($i in 1..60) {
    try { $h = Invoke-RestMethod "http://127.0.0.1:$Port/health" -TimeoutSec 2; if ($h.status -eq "ok") { $healthy = $true; break } } catch { Start-Sleep 1 }
}
if (-not $healthy) { throw "server failed to start (see artifacts/load_srv_err.log)" }

try {
    # 显式绕过系统代理（本机代理端口可能未开）
    $handler = [System.Net.Http.HttpClientHandler]::new()
    $handler.UseProxy = $false
    $client = [System.Net.Http.HttpClient]::new($handler)
    $client.Timeout = [TimeSpan]::FromSeconds(30)
    $url = "http://127.0.0.1:$Port$Path"

    $lat = New-Object System.Collections.Generic.List[double]
    $ok = [int64]0; $fail = [int64]0
    $t0 = [System.Diagnostics.Stopwatch]::StartNew()
    $deadline = $t0.ElapsedMilliseconds + [int64]($DurationSec * 1000)

    while ($t0.ElapsedMilliseconds -lt $deadline -and ($MaxRequests -le 0 -or ($ok + $fail) -lt $MaxRequests)) {
        $n = [math]::Min($Concurrent, $(if ($MaxRequests -gt 0) { $MaxRequests - $ok - $fail } else { $Concurrent }))
        if ($n -le 0) { break }
        $tasks = @(); $sws = @()
        for ($i = 0; $i -lt $n; $i++) {
            $content = [System.Net.Http.StringContent]::new($Body, [System.Text.Encoding]::UTF8, "application/json")
            $sw = [System.Diagnostics.Stopwatch]::StartNew()
            $tasks += $client.PostAsync($url, $content)
            $sws += $sw
        }
        [void][System.Threading.Tasks.Task]::WaitAll($tasks, [TimeSpan]::FromSeconds(35))
        for ($i = 0; $i -lt $n; $i++) {
            $sws[$i].Stop()
            $lat.Add($sws[$i].Elapsed.TotalMilliseconds)
            try {
                if ($tasks[$i].Result.IsSuccessStatusCode) { $script:ok++ } else { $script:fail++ }
            } catch { $script:fail++ }
        }
    }
    $t0.Stop()
    $client.Dispose()

    $sorted = @($lat) | Sort-Object
    $total = $ok + $fail
    $elapsed = [math]::Round($t0.Elapsed.TotalSeconds, 2)
    function Pct($arr, $p) {
        if ($arr.Count -eq 0) { return 0 }
        $idx = [math]::Min([math]::Floor($p / 100 * ($arr.Count - 1)), $arr.Count - 1)
        return [math]::Round($arr[$idx], 2)
    }
    $out = [pscustomobject]@{
        path       = $Path
        concurrent = $Concurrent
        requests   = $total
        elapsed_s  = $elapsed
        rps        = if ($elapsed -gt 0) { [math]::Round($total / $elapsed, 1) } else { 0 }
        p50_ms     = Pct $sorted 50
        p95_ms     = Pct $sorted 95
        error_rate = if ($total -gt 0) { [math]::Round(100.0 * $fail / $total, 2) } else { 0 }
    }
    $out | ConvertTo-Json | Write-Host
    $out | ConvertTo-Json -Compress | Set-Content -Encoding utf8 "$root\artifacts\load_last.json"
}
finally {
    if (-not $KeepServer -and $srv -and -not $srv.HasExited) { Stop-Process -Id $srv.Id -Force }
}
