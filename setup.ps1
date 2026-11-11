# Build the native server and interface. Python is not required.
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot
function Invoke-Checked {
    param([string]$Executable, [string[]]$Arguments)
    # Windows PowerShell 5.1 can wrap native stderr (including Cargo progress)
    # as error records when output is captured. Only the exit code signals failure.
    $previousPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = "Continue"
        & $Executable @Arguments
        $exitCode = $LASTEXITCODE
    } finally { $ErrorActionPreference = $previousPreference }
    if ($exitCode -ne 0) { throw "$Executable failed with exit code $exitCode." }
}
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw "Install Rust from https://rustup.rs first (including the Windows build tools)." }
if (-not (Get-Command node -ErrorAction SilentlyContinue)) { throw "Node.js 22.12+ is required to build the interface." }
Invoke-Checked "node" @("-e", "const [m,n]=process.versions.node.split('.').map(Number); process.exit(m<22||(m===22&&n<12)?1:0)")
Push-Location frontend
try {
    Invoke-Checked "npm" @("ci", "--no-audit", "--no-fund")
    Invoke-Checked "npm" @("run", "build")
} finally { Pop-Location }
Invoke-Checked "cargo" @("build", "--release", "--locked", "--manifest-path", "rust/Cargo.toml")
# Last: a failed engine download must not leave the app without its interface.
if (-not $env:PLYBACK_STOCKFISH) { Invoke-Checked "node" @("scripts/install_stockfish.mjs") }
Write-Host "Setup complete. Start with: .\start.ps1" -ForegroundColor Cyan
