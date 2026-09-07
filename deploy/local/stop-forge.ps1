# Stop Aion Forge (server process + PG container; data persists in PG volume)
Write-Host "Stopping forge-server..."
Get-Process forge-server -ErrorAction SilentlyContinue | Stop-Process -Force
Write-Host "Stopping PostgreSQL container..."
podman stop forge-pg 2>$null | Out-Null
Write-Host "Stopped. Double-click [StartForge.bat] to resume; no data loss."
