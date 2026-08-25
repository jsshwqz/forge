# Aion Forge 一键停止
Get-Process forge-server -ErrorAction SilentlyContinue | Stop-Process -Force
Write-Host "Aion Forge 服务已停止（PostgreSQL 容器保持运行，数据不丢失）。"
Start-Sleep -Seconds 2
