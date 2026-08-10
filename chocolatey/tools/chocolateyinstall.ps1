$ErrorActionPreference = 'Stop'

# 64-bit Windows required — this package has no 32-bit installer.
if (-not [Environment]::Is64BitOperatingSystem) {
    throw 'LRGEX Compress requires 64-bit Windows. This machine is 32-bit.'
}

$packageArgs = @{
  packageName    = 'lrgex-compress'
  fileType       = 'exe'
  url64bit       = 'https://download.lrgex.com/app/rst/lrgex-compress/LRGEX-Compress-v1.5.3-setup.exe'
  checksum64     = '69cc1861d052fb7bd727b8cb1062b587bfb9466b0e2e7f83cc69a991c52500de'
  checksumType64 = 'sha256'
  softwareName   = 'LRGEX Compress*'
  silentArgs     = '/VERYSILENT /NORESTART /NOCANCEL /SP- /ALLUSERS'
  validExitCodes = @(0, 3010)
}

Install-ChocolateyPackage @packageArgs
