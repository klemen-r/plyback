# Plyback - start the server and open the browser (Windows).
#   .\start.ps1 [-Port 8000] [-Lan]
param([ValidateRange(1, 65535)][int]$Port = 8000, [switch]$Lan, [switch]$NoBrowser)
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot
$binary = Join-Path $PSScriptRoot "rust\target\release\plyback.exe"
if (-not (Test-Path $binary)) { throw "Run setup.ps1 first." }
$env:PLYBACK_PORT = "$Port"
$env:PLYBACK_HOST = if ($Lan) { "0.0.0.0" } else { "127.0.0.1" }
$url = "http://localhost:$Port"
Write-Host "Plyback is starting at $url  (Ctrl+C to stop)" -ForegroundColor Cyan
if (-not $NoBrowser) {
    Start-Job -ScriptBlock { param($u) Start-Sleep -Seconds 3; Start-Process $u } -ArgumentList $url | Out-Null
}
if (-not $env:PLYBACK_DATA) { $env:PLYBACK_DATA = Join-Path $PSScriptRoot "data" }
if (-not $env:PLYBACK_FRONTEND) { $env:PLYBACK_FRONTEND = Join-Path $PSScriptRoot "frontend\dist" }
& $binary

exit $LASTEXITCODE
