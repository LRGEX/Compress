$ErrorActionPreference = 'Continue'
$exe = "C:\Program Files\LRGEX Compress\lrgex-compress.exe"
$work = Join-Path $env:TEMP "lrgex-fire-$(Get-Random)"
$pass = 0; $fail = 0; $results = @()

function T($n) { $script:current = $n }
function P($m="") { $script:pass++; $script:results += "PASS  $script:current $m"; Write-Host "  PASS  $script:current $m" -ForegroundColor Green }
function F($m="") { $script:fail++; $script:results += "FAIL  $script:current $m"; Write-Host "  FAIL  $script:current $m" -ForegroundColor Red }

function Sha256($path) {
    $h = [System.Security.Cryptography.SHA256]::Create()
    $fs = [System.IO.File]::OpenRead($path)
    try { $b = $h.ComputeHash($fs) } finally { $fs.Close() }
    return ([BitConverter]::ToString($b) -replace '-','').ToLower()
}

function RunExe($exeArgs, $timeoutSec = 60) {
    $p = Start-Process -FilePath $exe -ArgumentList $exeArgs -PassThru -WindowStyle Hidden
    if (-not $p.WaitForExit($timeoutSec * 1000)) { try { $p.Kill() } catch {}; return $false }
    return $true
}

function DoCompress($srcDir) {
    $name = (Get-Item $srcDir).Name; $parent = (Get-Item $srcDir).Parent.FullName
    $out = Join-Path $parent "$name.zgx"; if (Test-Path $out) { Remove-Item $out -Force }
    RunExe @($srcDir) 60 | Out-Null; return $out
}
function DoExtract($archive) {
    $name = [System.IO.Path]::GetFileNameWithoutExtension($archive); $parent = (Split-Path $archive -Parent)
    $out = Join-Path $parent $name; if (Test-Path $out) { Remove-Item $out -Recurse -Force }
    RunExe @("-x", $archive) 60 | Out-Null; return $out
}

[Console]::OutputEncoding = [Text.Encoding]::UTF8
New-Item -ItemType Directory -Force -Path $work | Out-Null
Write-Host "`n=== LRGEX Compress v1.4.5 FIRE TEST (automated) ===" -ForegroundColor Cyan
Write-Host "Work: $work`n"

T "01_Basic_round_trip"
$src = Join-Path $work "t1"; New-Item -ItemType Directory -Path $src -Force | Out-Null
"Hello LRGEX" | Set-Content (Join-Path $src "readme.txt")
$z = DoCompress $src; if (Test-Path $z) { P "archive created" } else { F "archive NOT created" }
$ext = DoExtract $z; $c = Get-Content (Join-Path $ext "readme.txt") -Raw -EA SilentlyContinue
if ($c -and $c.Trim() -eq "Hello LRGEX") { P "content matches" } else { F "content mismatch" }

T "02_Empty_folder"
$src = Join-Path $work "t2"; New-Item -ItemType Directory -Path (Join-Path $src "empty") -Force | Out-Null
"f" | Set-Content (Join-Path $src "f.txt"); $z = DoCompress $src; $ext = DoExtract $z
if (Test-Path (Join-Path $ext "empty") -PathType Container) { P "empty dir preserved" } else { F "empty dir MISSING" }

T "03_Deep_nesting"
$src = Join-Path $work "t3"; $d = $src
1..10 | ForEach-Object { $d = Join-Path $d "L$_"; New-Item -ItemType Directory -Force -Path $d | Out-Null }
"deep" | Set-Content (Join-Path $d "deep.txt"); $z = DoCompress $src; $ext = DoExtract $z
$dp = $ext; 1..10 | ForEach-Object { $dp = Join-Path $dp "L$_" }
if (Test-Path (Join-Path $dp "deep.txt")) { P "10-level deep file" } else { F "deep file MISSING" }

T "04_Large_file"
$src = Join-Path $work "t4"; New-Item -ItemType Directory -Path $src -Force | Out-Null
$big = Join-Path $src "big.bin"; $bytes = New-Object byte[] 5000000; (New-Object Random).NextBytes($bytes)
[System.IO.File]::WriteAllBytes($big, $bytes); $h1 = Sha256 $big
$z = DoCompress $src; $ext = DoExtract $z; $exFile = Join-Path $ext "big.bin"
if (Test-Path $exFile) { $h2 = Sha256 $exFile; if ($h1 -eq $h2) { P "5MB SHA256 match" } else { F "5MB hash mismatch" } } else { F "5MB file not extracted" }

T "05_Many_small_files"
$src = Join-Path $work "t5"; New-Item -ItemType Directory -Path $src -Force | Out-Null
$hashes = @{}
0..199 | ForEach-Object { $f = Join-Path $src "f$_.dat"; $b = New-Object byte[] (10KB + $_); (New-Object Random).NextBytes($b); [System.IO.File]::WriteAllBytes($f, $b); $hashes[$_] = Sha256 $f }
$z = DoCompress $src; $ext = DoExtract $z; $ok = $true
0..199 | ForEach-Object { $f = Join-Path $ext "f$_.dat"; if (-not (Test-Path $f) -or (Sha256 $f) -ne $hashes[$_]) { $ok = $false } }
if ($ok) { P "200 small files all match" } else { F "some small files mismatch" }

T "06_Same_stem_diff_ext"
$src = Join-Path $work "t6"; New-Item -ItemType Directory -Path $src -Force | Out-Null
"MF" | Set-Content (Join-Path $src "Makefile"); "MFI" | Set-Content (Join-Path $src "Makefile.in")
"DT" | Set-Content (Join-Path $src "data.txt"); "DB" | Set-Content (Join-Path $src "data.bin")
$z = DoCompress $src; $ext = DoExtract $z
$a = (Get-Content (Join-Path $ext "Makefile") -Raw -EA SilentlyContinue)
$b = (Get-Content (Join-Path $ext "Makefile.in") -Raw -EA SilentlyContinue)
$c = (Get-Content (Join-Path $ext "data.txt") -Raw -EA SilentlyContinue)
$d = (Get-Content (Join-Path $ext "data.bin") -Raw -EA SilentlyContinue)
if ($a -and $a.Trim() -eq "MF" -and $b.Trim() -eq "MFI" -and $c.Trim() -eq "DT" -and $d.Trim() -eq "DB") { P "all 4 distinct" }
else { F "collision" }

T "07_Unicode"
$src = Join-Path $work "t7"; New-Item -ItemType Directory -Path $src -Force | Out-Null
$cyrName = [char]0x0444 + [char]0x0430 + [char]0x0439 + [char]0x043B + ".txt"
"u" | Set-Content (Join-Path $src $cyrName)
$emojiName = "doc " + [char]0xD83C + [char]0xDF89 + ".md"
"e" | Set-Content (Join-Path $src $emojiName)
$z = DoCompress $src; $ext = DoExtract $z
if ((Test-Path (Join-Path $ext $cyrName)) -and (Test-Path (Join-Path $ext $emojiName))) { P "unicode names preserved" } else { F "unicode names lost" }

T "08_ReadOnly_preserved"
$src = Join-Path $work "t8"; New-Item -ItemType Directory -Path $src -Force | Out-Null
"ro" | Set-Content (Join-Path $src "ro.txt"); (Set-ItemProperty (Join-Path $src "ro.txt") -Name IsReadOnly -Value $true)
$z = DoCompress $src; $ext = DoExtract $z; $rf = Join-Path $ext "ro.txt"
if (Test-Path $rf) { if ((Get-Item $rf -Force).IsReadOnly) { P "ReadOnly preserved" } else { F "ReadOnly lost" }; Set-ItemProperty $rf -Name IsReadOnly -Value $false } else { F "ro.txt not extracted" }

T "09_Hidden_preserved"
$src = Join-Path $work "t9"; New-Item -ItemType Directory -Path $src -Force | Out-Null
"h" | Set-Content (Join-Path $src "h.txt"); (Get-Item (Join-Path $src "h.txt")).Attributes = 'Hidden'
$z = DoCompress $src; $ext = DoExtract $z; $hf = Join-Path $ext "h.txt"
if (Test-Path $hf) { if ((Get-Item $hf -Force).Attributes -band [System.IO.FileAttributes]::Hidden) { P "Hidden preserved" } else { F "Hidden lost" } } else { F "h.txt not extracted" }

T "10_ZIP_extract"
$zs = Join-Path $work "t10src"; New-Item -ItemType Directory -Path $zs -Force | Out-Null
"zzip" | Set-Content (Join-Path $zs "z.txt"); $zp = Join-Path $work "t10.zip"
Add-Type -AssemblyName System.IO.Compression.FileSystem; [System.IO.Compression.ZipFile]::CreateFromDirectory($zs, $zp)
$ext = DoExtract $zp; if (Test-Path (Join-Path $ext "z.txt")) { P "zip entry extracted" } else { F "zip entry MISSING" }

T "11_Magic_header"
$src = Join-Path $work "t11"; New-Item -ItemType Directory -Path $src -Force | Out-Null
"x" | Set-Content (Join-Path $src "f.txt"); $z = DoCompress $src; $b = [System.IO.File]::ReadAllBytes($z)
$magic = [System.Text.Encoding]::ASCII.GetString($b[0..4])
if ($magic -eq "LRGEX" -and $b[5] -eq 1) { P "magic=LRGEX v1" } else { F "magic='$magic' v=$($b[5])" }

T "12_No_orphans"
$src = Join-Path $work "t12"; New-Item -ItemType Directory -Path $src -Force | Out-Null
"a" | Set-Content (Join-Path $src "a.txt"); "b" | Set-Content (Join-Path $src "b.txt")
$z = DoCompress $src; $ext = DoExtract $z
$orphans = Get-ChildItem $work -Recurse -Force -EA SilentlyContinue | Where-Object { $_.Name -match 'lrgex-tmp|\.part$|staging' }
if (-not $orphans -or $orphans.Count -eq 0) { P "no orphans" } else { F "$($orphans.Count) orphans" }

T "13_mtime_preserved"
$src = Join-Path $work "t13"; New-Item -ItemType Directory -Path $src -Force | Out-Null
"m" | Set-Content (Join-Path $src "m.txt"); $ot = (Get-Item (Join-Path $src "m.txt")).LastWriteTime; Start-Sleep 2
$z = DoCompress $src; $ext = DoExtract $z; $nt = (Get-Item (Join-Path $ext "m.txt")).LastWriteTime
$diff = [Math]::Abs(($ot - $nt).TotalSeconds)
if ($diff -lt 2) { P "mtime within 2s" } else { F "mtime diff ${diff}s" }

T "14_Dir_structure"
$src = Join-Path $work "t14"; New-Item -ItemType Directory -Path (Join-Path $src "a\b\c") -Force | Out-Null
"3" | Set-Content (Join-Path $src "a\b\c\f3.txt"); $z = DoCompress $src; $ext = DoExtract $z
if (Test-Path (Join-Path $ext "a\b\c\f3.txt")) { P "structure mirrored" } else { F "structure broken" }

Write-Host "`n==================================================" -ForegroundColor Cyan
Write-Host "RESULT: $pass passed, $fail failed (of $($pass+$fail))" -ForegroundColor $(if($fail -eq 0){'Green'}else{'Red'})
Write-Host "==================================================`n" -ForegroundColor Cyan
if ($fail -gt 0) { Write-Host "FAILURES:" -ForegroundColor Red; $results | Where-Object { $_ -match '^FAIL' } | ForEach-Object { Write-Host "  $_" -ForegroundColor Red } }
try { Remove-Item $work -Recurse -Force -EA SilentlyContinue } catch {}
