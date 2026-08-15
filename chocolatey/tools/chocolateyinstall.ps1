$ErrorActionPreference = 'Stop'

# 64-bit Windows required — this package has no 32-bit installer.
if (-not [Environment]::Is64BitOperatingSystem) {
    throw 'LRGEX Compress requires 64-bit Windows. This machine is 32-bit.'
}

$packageArgs = @{
  packageName    = 'lrgex-compress'
  fileType       = 'exe'
  url64bit       = 'https://download.lrgex.com/app/rst/lrgex-compress/LRGEX-Compress-v1.7.2-setup.exe'
  checksum64     = 'a4b47a11830715459101690f98de65685741f3c81d08b4cbd9a612185544fbc1'
  checksumType64 = 'sha256'
  softwareName   = 'LRGEX Compress*'
  silentArgs     = '/VERYSILENT /NORESTART /NOCANCEL /SP- /ALLUSERS'
  validExitCodes = @(0, 3010)
}

Install-ChocolateyPackage @packageArgs
