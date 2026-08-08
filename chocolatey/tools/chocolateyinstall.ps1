$ErrorActionPreference = 'Stop'

$packageArgs = @{
  packageName    = 'lrgex-compress'
  fileType       = 'exe'
  url64bit       = 'https://download.lrgex.com/app/rst/lrgex-compress/LRGEX-Compress-v1.4.2-setup.exe'
  checksum64     = 'abe58507bf67eeb8ffe835aff57e53470863f4c4a60a939c9bf05b50e8bba36c'
  checksumType64 = 'sha256'
  softwareName   = 'LRGEX Compress*'
  silentArgs     = '/VERYSILENT /NORESTART /NOCANCEL /SP- /ALLUSERS'
  validExitCodes = @(0)
}

Install-ChocolateyPackage @packageArgs
