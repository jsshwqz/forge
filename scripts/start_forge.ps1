# Aion Forge 一键启动：确保PG容器 -> 启动server(持久化) -> 打开控制台
$ErrorActionPreference = "Continue"
$root = Split-Path -Parent $PSScriptRoot
$port = if ($env:FORGE_PORT) { $env:FORGE_PORT } else { "8080" }
$base = "http://127.0.0.1:$port"

# 0) 已在运行 => 直接打开控制台
try {
    $h = Invoke-RestMethod "$base/health" -TimeoutSec 2
    if ($h.status -eq "ok") { Start-Process "$base/"; Write-Host "Aion Forge 正在运行，已打开控制台。"; exit 0 }
} catch {}

# 1) 确保 Podman 虚拟机与 PG 容器运行（数据持久化依赖它）
$pgUp = (& podman inspect -f "{{.State.Running}}" forge-pg 2>$null) -eq "true"
if (-not $pgUp) {
    Write-Host "启动 Podman 虚拟机与 PostgreSQL 容器..."
    & podman machine start 2>&1 | Out-Null
    & podman start forge-pg 2>&1 | Out-Null
    Start-Sleep -Seconds 5
}

# 2) 确保二进制存在（首次双击会先编译一次）
$exe = Join-Path $root "target\debug\forge-server.exe"
if (-not (Test-Path $exe)) {
    Write-Host "首次启动，正在编译（约几分钟）..."
    $cargo = "C:\Users\Administrator\.cargo\bin\cargo.exe"
    if (-not (Test-Path $cargo)) { $cargo = "cargo" }
    Push-Location $root
    & $cargo build -p forge-server --bin forge-server
    Pop-Location
}

# 3) 启动服务（PostgreSQL 持久化 + 加载仓库根 .env 的 LLM 等配置）
New-Item -ItemType Directory -Force -Path (Join-Path $root "artifacts") | Out-Null
$envFile = Join-Path $root ".env"
if (Test-Path $envFile) {
    Get-Content $envFile | ForEach-Object {
        if ($_ -match '^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.+)\s*$') {
            [Environment]::SetEnvironmentVariable($matches[1], $matches[2].Trim('"'))
        }
    }
    Write-Host "已加载 .env 配置（含 LLM）"
}
$env:FORGE_PORT = $port
$env:FORGE_PG_URL = "postgres://postgres:forge@localhost:15432/forge"
$srv = Start-Process -FilePath $exe -WorkingDirectory $root -WindowStyle Hidden -PassThru `
        -RedirectStandardOutput (Join-Path $root "artifacts\server_run.log") `
        -RedirectStandardError  (Join-Path $root "artifacts\server_err.log")

# 4) 健康检查后打开控制台
$ok = $false
foreach ($i in 1..30) {
    try {
        $h = Invoke-RestMethod "$base/health" -TimeoutSec 2
        if ($h.status -eq "ok") { $ok = $true; break }
    } catch { Start-Sleep -Seconds 1 }
}
if ($ok) {
    Start-Process "$base/"
    Write-Host "Aion Forge 已启动: $base （PID $($srv.Id)，停止请双击桌面的『Aion Forge 停止』）"
} else {
    Stop-Process -Id $srv.Id -Force -ErrorAction SilentlyContinue
    Write-Host "启动失败，请查看 artifacts\server_err.log"
}
