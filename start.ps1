# Plyback - start the server and open the browser (Windows).
#   .\start.ps1 [-Port 8000] [-Lan]
# -Port and -Lan override PLYBACK_PORT and PLYBACK_HOST; without them, existing values are kept.
param([ValidateRange(1, 65535)][int]$Port = 8000, [switch]$Lan, [switch]$NoBrowser)
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $PSScriptRoot "rust\target" }
$binary = Join-Path $target "release\plyback.exe"
if (-not (Test-Path $binary)) { throw "The server is not built. Run setup.ps1 first." }
if (-not $env:PLYBACK_DATA) { $env:PLYBACK_DATA = Join-Path $PSScriptRoot "data" }
if (-not $env:PLYBACK_FRONTEND) { $env:PLYBACK_FRONTEND = Join-Path $PSScriptRoot "frontend\dist" }
if (-not (Test-Path (Join-Path $env:PLYBACK_FRONTEND "index.html"))) {
    throw "The web interface is not built in $($env:PLYBACK_FRONTEND). Run setup.ps1 first."
}
if ($PSBoundParameters.ContainsKey("Port") -or -not $env:PLYBACK_PORT) { $env:PLYBACK_PORT = "$Port" }
if ($Lan) { $env:PLYBACK_HOST = "0.0.0.0" } elseif (-not $env:PLYBACK_HOST) { $env:PLYBACK_HOST = "127.0.0.1" }
$url = "http://localhost:$($env:PLYBACK_PORT)"
Write-Host "Plyback is starting at $url  (Ctrl+C to stop)" -ForegroundColor Cyan
if (-not $NoBrowser) {
    Start-Job -ScriptBlock { param($u) Start-Sleep -Seconds 3; Start-Process $u } -ArgumentList $url | Out-Null
}
# Native logging on stderr is not a PowerShell failure, even when captured on 5.1.
$previousPreference = $ErrorActionPreference
try {
    $ErrorActionPreference = "Continue"
    & $binary
    $exitCode = $LASTEXITCODE
} finally { $ErrorActionPreference = $previousPreference }
exit $exitCode
