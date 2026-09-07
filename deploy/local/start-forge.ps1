# =====================================================================
# Aion Forge one-click start (local practical mode)
# Steps: Podman/PG up -> load forge.env -> start forge-server -> health -> open workbench
# Idempotent: if already running, just opens the workbench.
# =====================================================================
$ErrorActionPreference = "Continue"
$local = $PSScriptRoot
$root = Split-Path -Parent (Split-Path -Parent $local)   # aion-forge/
$port = 8787

# ---- 0. already running? -> open workbench ----
try {
    $h = Invoke-RestMethod "http://127.0.0.1:$port/health" -TimeoutSec 2
    if ($h.status -eq "ok") {
        Write-Host "[OK] Aion Forge already running -> opening workbench" -ForegroundColor Green
        Start-Process "http://127.0.0.1:$port/"
        exit 0
    }
} catch { }

# ---- 1. load config ----
Get-Content (Join-Path $local "forge.env") | Where-Object { $_ -match '^\s*FORGE_[A-Z_]+\s*=' } | ForEach-Object {
    $k, $v = $_ -split '=', 2
    Set-Item -Path "env:$($k.Trim())" -Value $v.Trim()
}
Write-Host "[1/4] config loaded (LLM: $env:FORGE_LLM_BASE_URL / model: $env:FORGE_TIER_HIGH_MODEL)"

# ---- 2. Podman machine + PG container ----
# machine 已运行时 start 会挂起——先查状态
 = (podman machine list --format '{{.Running}}' 2> | Select-Object -First 1)
if ( -ne 'true') { podman machine start 2>&1 | Out-Null }
podman start forge-pg 2>&1 | Out-Null
$pgOk = $false
foreach ($i in 1..30) {
    $null = podman exec forge-pg pg_isready -U postgres 2>$null
    if ($LASTEXITCODE -eq 0) { $pgOk = $true; break }
    Start-Sleep 2
}
if (-not $pgOk) { Write-Host "[FAIL] PostgreSQL did not start (forge-pg container)" -ForegroundColor Red; pause; exit 1 }
Write-Host "[2/4] PostgreSQL ready (15432)"

# ---- 3. forge-server ----
$serverExe = Join-Path $root "target\debug\forge-server.exe"
if (-not (Test-Path $serverExe)) {
    Write-Host "First run: compiling (2~5 minutes)..."
    cargo build -q -p forge-server --bin forge-server
}
if (-not (Test-Path $serverExe)) { Write-Host "[FAIL] build failed" -ForegroundColor Red; pause; exit 1 }
$stamp = Get-Date -Format yyyyMMdd_HHmmss
$srv = Start-Process -FilePath $serverExe -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput "$local\server_out_$stamp.log" -RedirectStandardError "$local\server_err_$stamp.log"
$ok = $false
foreach ($i in 1..30) {
    try { $h = Invoke-RestMethod "http://127.0.0.1:$port/health" -TimeoutSec 2; if ($h.status -eq "ok") { $ok = $true; break } } catch { Start-Sleep 1 }
}
if (-not $ok) {
    Write-Host "[FAIL] forge-server failed to start, see $local\server_err_$stamp.log" -ForegroundColor Red
    pause; exit 1
}
Write-Host "[3/4] forge-server running (http://127.0.0.1:$port)"
Write-Host "[4/4] opening workbench..."
Start-Process "http://127.0.0.1:$port/"
Write-Host ""
Write-Host "=== Aion Forge is READY: submit tasks in the browser workbench ===" -ForegroundColor Green
Write-Host "To stop: desktop [StopForge.bat] (data persists in PG volume)"
