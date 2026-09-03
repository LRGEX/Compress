$ErrorActionPreference = 'Stop'

# 64-bit Windows required — this package has no 32-bit installer.
if (-not [Environment]::Is64BitOperatingSystem) {
    throw 'LRGEX Compress requires 64-bit Windows. This machine is 32-bit.'
}

$packageArgs = @{
  packageName    = 'lrgex-compress'
  fileType       = 'exe'
  url64bit       = 'https://download.lrgex.com/app/rst/lrgex-compress/LRGEX-Compress-v1.8.0-setup.exe'
  checksum64     = '10ef23875797f1312d5ae63c40619a5a7ab32e8523ca4c070f0fe4ac7a3e5f9b'
  checksumType64 = 'sha256'
  softwareName   = 'LRGEX Compress*'
  silentArgs     = '/VERYSILENT /NORESTART /NOCANCEL /SP- /ALLUSERS'
  validExitCodes = @(0, 3010)
}

Install-ChocolateyPackage @packageArgs
