$ErrorActionPreference = 'Stop'

# 64-bit Windows required — this package has no 32-bit installer.
if (-not [Environment]::Is64BitOperatingSystem) {
    throw 'LRGEX Compress requires 64-bit Windows. This machine is 32-bit.'
}

$packageArgs = @{
  packageName    = 'lrgex-compress'
  fileType       = 'exe'
  url64bit       = 'https://download.lrgex.com/app/rst/lrgex-compress/LRGEX-Compress-v1.4.4-setup.exe'
  checksum64     = 'd5c70700ba435f35aa00db2bfe203eed517ebfca435faf523dce8be2a67062d2'
  checksumType64 = 'sha256'
  softwareName   = 'LRGEX Compress*'
  silentArgs     = '/VERYSILENT /NORESTART /NOCANCEL /SP- /ALLUSERS'
  validExitCodes = @(0)
}

Install-ChocolateyPackage @packageArgs
