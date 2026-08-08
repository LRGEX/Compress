# Cancel test - verifies clicking Cancel mid-compress leaves NO .zgx or .part orphan.
# Creates a large incompressible workload so you have plenty of time to click Cancel.
$exe = "C:\Program Files\LRGEX Compress\lrgex-compress.exe"
$src = Join-Path $env:TEMP "lrgex-cancel-$(Get-Random)"
$parent = $env:TEMP

New-Item -ItemType Directory -Path $src -Force | Out-Null
Write-Host "Creating workload (200 x 50MB = 10GB of incompressible data)..." -ForegroundColor Cyan
Write-Host "This gives you ~60+ seconds to click Cancel." -ForegroundColor Cyan
1..200 | ForEach-Object {
    $f = Join-Path $src "block$_.bin"
    $b = New-Object byte[] (50MB); (New-Object Random).NextBytes($b)
    [System.IO.File]::WriteAllBytes($f, $b)
}
$folderName = Split-Path $src -Leaf
$expectedZgx = Join-Path $parent "$folderName.zgx"
$expectedPart = "$expectedZgx.part"

Write-Host "`nLAUNCHING COMPRESS - CLICK CANCEL WHEN THE WINDOW APPEARS!" -ForegroundColor Yellow
Start-Sleep 2
$p = Start-Process -FilePath $exe -ArgumentList $src -PassThru
Write-Host "Waiting up to 120s for you to click Cancel..." -ForegroundColor Yellow
if (-not $p.WaitForExit(120000)) {
    try { $p.Kill() } catch {}
    Write-Host "Timed out - killing." -ForegroundColor Yellow
}
Start-Sleep 2

$zgxExists = Test-Path $expectedZgx
$partExists = Test-Path $expectedPart
Write-Host "`n==================================================" -ForegroundColor Cyan
if (-not $zgxExists -and -not $partExists) {
    Write-Host "PASS - no .zgx or .part orphan after cancel" -ForegroundColor Green
} else {
    Write-Host "FAIL - orphan left:" -ForegroundColor Red
    if ($zgxExists) { Write-Host "  .zgx exists at $expectedZgx" -ForegroundColor Red }
    if ($partExists) { Write-Host "  .part exists at $expectedPart" -ForegroundColor Red }
}
Write-Host "==================================================" -ForegroundColor Cyan

Write-Host "`nCleaning up 10GB test data (may take a moment)..." -ForegroundColor Cyan
try { Remove-Item $src -Recurse -Force -EA SilentlyContinue } catch {}
if ($zgxExists) { Remove-Item $expectedZgx -Force -EA SilentlyContinue }
if ($partExists) { Remove-Item $expectedPart -Force -EA SilentlyContinue }
Write-Host "Done."
