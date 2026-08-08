# TODO for next Chocolatey version push:
#
# 1. Add `softwareName = 'LRGEX Compress*'` to $packageArgs in chocolateyinstall.ps1
#    (required for choco upgrade/uninstall detection)
#
# 2. Create chocolateyuninstall.ps1 using Get-UninstallRegistryKey for clean removal:
#    $packageName = 'lrgex-compress'
#    $uninstalled = $false
#    $key = Get-UninstallRegistryKey -SoftwareName 'LRGEX Compress*'
#    if ($key) {
#        $uninstallArgs = $key.UninstallString
#        Uninstall-ChocolateyPackage -PackageName $packageName `
#          -FileType exe -SilentArgs '/VERYSILENT /NORESTART' -File $uninstallArgs
#    }
#
# These were flagged by the Chocolatey advisor but can't be applied to 1.4.1
# (already pushed). Add on next version push.
