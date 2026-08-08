$ErrorActionPreference = 'Stop'

# 64-bit Windows required — this package has no 32-bit installer.
if (-not [Environment]::Is64BitOperatingSystem) {
    throw 'LRGEX Compress requires 64-bit Windows. This machine is 32-bit.'
}

$packageArgs = @{
  packageName    = 'lrgex-compress'
  fileType       = 'exe'
  url64bit       = 'https://download.lrgex.com/app/rst/lrgex-compress/LRGEX-Compress-v1.4.2-setup.exe'
  checksum64     = 'f57a0be6b8bebb04f9509716e94432c867ff74ccae53f29e3ed81d69bf67234b'
  checksumType64 = 'sha256'
  softwareName   = 'LRGEX Compress*'
  silentArgs     = '/VERYSILENT /NORESTART /NOCANCEL /SP- /ALLUSERS'
  validExitCodes = @(0)
}

Install-ChocolateyPackage @packageArgs
