# Plyback - one-time setup for Windows (PowerShell).
#   powershell -ExecutionPolicy Bypass -File setup.ps1
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

function Invoke-Checked {
    param([string]$Executable, [string[]]$Arguments)
    & $Executable @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Executable failed with exit code $LASTEXITCODE." }
}

function Find-Python {
    $candidates = @(@{ Exe = "py"; Args = @("-3") }, @{ Exe = "python"; Args = @() }, @{ Exe = "python3"; Args = @() })
    foreach ($c in $candidates) {
        if (-not (Get-Command $c.Exe -ErrorAction SilentlyContinue)) { continue }
        try {
            $v = & $c.Exe @($c.Args + @("-c", "import sys; print('%d.%d' % sys.version_info[:2])")) 2>$null
            if ($LASTEXITCODE -eq 0 -and $v -and [version]$v -ge [version]"3.10") { return $c }
        } catch {}
    }
    throw "Python 3.10+ is required. Install it from https://www.python.org/downloads/ and re-run setup."
}

$py = Find-Python
Write-Host "==> Using $($py.Exe) $($py.Args -join ' ')" -ForegroundColor Green
if (-not (Test-Path ".venv")) {
    Invoke-Checked -Executable $py.Exe -Arguments ($py.Args + @("-m", "venv", ".venv"))
}
$venvPy = Join-Path $PSScriptRoot ".venv\Scripts\python.exe"
Invoke-Checked -Executable $venvPy -Arguments @("-m", "pip", "install", "--upgrade", "pip", "--quiet")
Invoke-Checked -Executable $venvPy -Arguments @("-m", "pip", "install", "--require-hashes", "-r", "backend/requirements.txt", "--quiet")
Write-Host "==> Python dependencies installed" -ForegroundColor Green

Invoke-Checked -Executable $venvPy -Arguments @("scripts/install_stockfish.py")
Write-Host "==> Stockfish ready" -ForegroundColor Green

if (Get-Command npm -ErrorAction SilentlyContinue) {
    Invoke-Checked -Executable "node" -Arguments @("-e", "const [m,n]=process.versions.node.split('.').map(Number); process.exit(m<22||(m===22&&n<12)?1:0)")
    Push-Location frontend
    try {
        Invoke-Checked -Executable "npm" -Arguments @("ci", "--no-audit", "--no-fund")
        Invoke-Checked -Executable "npm" -Arguments @("run", "build")
    } finally { Pop-Location }
    Write-Host "==> Web interface built" -ForegroundColor Green
} elseif (Test-Path "frontend\dist\index.html") {
    Write-Host "==> Node.js not found; using the pre-built web interface" -ForegroundColor Yellow
} else {
    throw "Node.js 22.12+ is needed to build the web interface (https://nodejs.org)."
}

Write-Host ""
Write-Host "Setup complete. Start the app with:  .\start.ps1   (or double-click start.bat)" -ForegroundColor Cyan
