$ErrorActionPreference = 'Stop'

$packageName = 'lrgex-compress'
$softwareName = 'LRGEX Compress*'

# Find the recorded uninstaller via the registry (Inno Setup records it automatically).
# Works for both per-user (HKCU) and machine-wide (HKLM) installs.
$uninstaller = $null

# Check HKLM first (machine-wide / choco install)
$machineKey = Get-ItemProperty -Path "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\LRGEX Compress_is1" -ErrorAction SilentlyContinue
if ($machineKey -and $machineKey.UninstallString) {
    $uninstaller = $machineKey.UninstallString
}

# Check HKCU (per-user / portable install)
if (-not $uninstaller) {
    $userKey = Get-ItemProperty -Path "HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\LRGEX Compress_is1" -ErrorAction SilentlyContinue
    if ($userKey -and $userKey.UninstallString) {
        $uninstaller = $userKey.UninstallString
    }
}

if ($uninstaller) {
    # Inno Setup's uninstaller handles ALL registry cleanup (uninsdeletekey)
    # and file removal. We just call it silently.
    Uninstall-ChocolateyPackage `
        -PackageName $packageName `
        -FileType exe `
        -SilentArgs '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART' `
        -File $uninstaller
} else {
    Write-Warning "LRGEX Compress uninstaller not found in registry. Manual cleanup may be needed."
}
