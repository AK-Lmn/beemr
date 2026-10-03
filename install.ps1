# Install beemr on Windows (PowerShell):
#   irm https://raw.githubusercontent.com/osmanahmadxai/beemr/main/install.ps1 | iex
#
# Downloads the binary from GitHub Releases, verifies its SHA-256 checksum,
# adds it to your PATH, then runs `beemr setup` to name this device.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Repo = if ($env:BEEMR_REPO) { $env:BEEMR_REPO } else { 'osmanahmadxai/beemr' }
$InstallDir = if ($env:BEEMR_INSTALL_DIR) { $env:BEEMR_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'beemr' }
$Asset = 'beemr-windows-x86_64.exe'
$Base = "https://github.com/$Repo/releases/latest/download"

Write-Host 'This installer will:'
Write-Host '  - download beemr for Windows from GitHub and verify its checksum'
Write-Host "  - install it to $InstallDir and add that folder to your PATH"
Write-Host '  - ask you to name this device (nothing runs in the background)'
Write-Host "Uninstall any time: irm https://raw.githubusercontent.com/$Repo/main/uninstall.ps1 | iex"
Write-Host ''

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
$Exe = Join-Path $InstallDir 'beemr.exe'
$Tmp = Join-Path ([IO.Path]::GetTempPath()) "beemr-$([guid]::NewGuid()).exe"

Write-Host "Downloading beemr for Windows..."
Invoke-WebRequest "$Base/$Asset" -OutFile $Tmp -UseBasicParsing
$Sums = (Invoke-WebRequest "$Base/SHA256SUMS" -UseBasicParsing).Content
if ($Sums -is [byte[]]) { $Sums = [Text.Encoding]::UTF8.GetString($Sums) }
$Line = ($Sums -split "`n") | Where-Object { $_ -match " $([regex]::Escape($Asset))\s*$" } | Select-Object -First 1
$Expected = if ($Line) { ($Line -split ' ')[0].Trim() } else { '' }
$Actual = (Get-FileHash $Tmp -Algorithm SHA256).Hash.ToLower()
if (-not $Expected -or $Expected -ne $Actual) {
    Remove-Item $Tmp -ErrorAction SilentlyContinue
    throw 'Checksum mismatch; not installing.'
}

# Stop any running beemr (e.g. an older version's background service) so the file can be replaced.
Get-Process beemr -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Move-Item -Force $Tmp $Exe
Write-Host "Installed beemr to $Exe"

$UserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not $UserPath) { $UserPath = '' }
if (($UserPath -split ';') -notcontains $InstallDir) {
    [Environment]::SetEnvironmentVariable('Path', ($UserPath.TrimEnd(';') + ";$InstallDir"), 'User')
    $env:Path += ";$InstallDir"
    Write-Host "Added $InstallDir to your PATH (open a new terminal to use 'beemr' everywhere)."
}

Write-Host ''
& $Exe setup
