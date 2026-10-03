# Uninstall beemr installed with install.ps1 (Windows PowerShell):
#   irm https://raw.githubusercontent.com/osmanahmadxai/beemr/main/uninstall.ps1 | iex
#
# Removes the beemr program, its PATH entry, and any background service left
# by versions before 0.3. Your identity and contacts are kept unless BEEMR_PURGE=1 is set.
$ErrorActionPreference = 'Stop'

$InstallDir = if ($env:BEEMR_INSTALL_DIR) { $env:BEEMR_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'beemr' }
$Exe = Join-Path $InstallDir 'beemr.exe'

if (Test-Path $Exe) {
    try { & $Exe daemon uninstall | Out-Null; Write-Host 'Removed any background service left by older versions.' } catch { }
}
Get-Process beemr -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Remove-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'beemr' -ErrorAction SilentlyContinue

if (Test-Path $InstallDir) {
    Remove-Item -Recurse -Force $InstallDir
    Write-Host "Removed $InstallDir"
}

$UserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($UserPath) {
    $Kept = ($UserPath -split ';') | Where-Object { $_ -and $_ -ne $InstallDir }
    [Environment]::SetEnvironmentVariable('Path', ($Kept -join ';'), 'User')
    Write-Host 'Removed beemr from your PATH.'
}

$Data = Join-Path $env:APPDATA 'beemr'
if ($env:BEEMR_PURGE -eq '1') {
    if (Test-Path $Data) { Remove-Item -Recurse -Force $Data; Write-Host "Deleted your beemr identity and contacts ($Data)." }
} else {
    Write-Host 'Your identity and contacts were kept. To delete them too, set BEEMR_PURGE=1 and run this again.'
}
