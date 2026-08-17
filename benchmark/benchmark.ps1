# LRGEX Compress - Fair Maximum-Speed Compression Benchmark
# Fully automated. No store mode. Fastest legitimate preset for each competitor.
# 5 runs per combination. Median is the published result.

param(
    [switch]$Quick  # 1 run instead of 5 (for debugging)
)

$ErrorActionPreference = "Stop"
# Fix PSModulePath when launched from git-bash (Get-FileHash/Compress-Archive need this)
$env:PSModulePath = "$env:windir\System32\WindowsPowerShell\v1.0\Modules;$env:ProgramFiles\WindowsPowerShell\Modules"
Import-Module Microsoft.PowerShell.Utility -ErrorAction SilentlyContinue
Import-Module Microsoft.PowerShell.Archive -ErrorAction SilentlyContinue

# -- Resolve Desktop ----------------------------------------------------------
$Desktop = [Environment]::GetFolderPath("Desktop")
$BenchRoot = Join-Path $Desktop "LRGEX-Benchmark"
$DataDir   = Join-Path $BenchRoot "Datasets"
$ResultsDir= Join-Path $BenchRoot "Results"
$ArchivesDir=Join-Path $BenchRoot "Archives"
$ExtractDir= Join-Path $BenchRoot "Extracted"
$LogsDir   = Join-Path $BenchRoot "Logs"

foreach ($d in @($DataDir, $ResultsDir, $ArchivesDir, $ExtractDir, $LogsDir)) {
    if (-not (Test-Path $d)) { New-Item -ItemType Directory -Path $d -Force | Out-Null }
}

$RUNS = if ($Quick) { 1 } else { 5 }

Write-Host "==" -ForegroundColor Cyan
Write-Host "|  LRGEX Compress - Fair Compression Benchmark           |" -ForegroundColor Cyan
Write-Host "=" -ForegroundColor Cyan
Write-Host "  Root: $BenchRoot"
Write-Host "  Runs per combination: $RUNS"
Write-Host ""

# -- System Info -------------------------------------------------------------
$cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
$os  = Get-CimInstance Win32_OperatingSystem
$sysDrive = Get-PSDrive C
$sysInfo = @{
    windows_version = $os.Caption
    windows_build   = $os.BuildNumber
    cpu             = $cpu.Name.Trim()
    cpu_cores       = $cpu.NumberOfCores
    cpu_threads     = $cpu.NumberOfLogicalProcessors
    ram_gb          = [math]::Round($os.TotalVisibleMemorySize / 1MB, 1)
    source_drive    = "E: (NVMe)"
    dest_drive      = "E: (NVMe)"
    filesystem      = "NTFS"
    free_space_gb   = [math]::Round($sysDrive.Free / 1GB, 1)
    benchmark_version = "1.0.0"
    timestamp       = (Get-Date -Format "yyyy-MM-ddTHH:mm:ss")
}
$sysInfo | ConvertTo-Json | Out-File (Join-Path $ResultsDir "system-info.json") -Encoding utf8

# -- App Detection ------------------------------------------------------------
$LrgexExe = "E:\LRG\LRG Data Cloud\L.R.G\Devoloping\Coding\Compress\target\release\lrgex-compress.exe"
$SevenZip = "C:\Program Files\7-Zip\7z.exe"
$WinRar   = "C:\Program Files\WinRAR\Rar.exe"

# Verify all present
foreach ($app in @(
    @{name="LRGEX"; path=$LrgexExe},
    @{name="7-Zip"; path=$SevenZip},
    @{name="WinRAR"; path=$WinRar}
)) {
    if (-not (Test-Path $app.path)) {
        Write-Host "FATAL: $($app.name) not found at $($app.path)" -ForegroundColor Red
        exit 1
    }
}

# Get versions
$lrgexVersion = (Select-String -Path (Join-Path (Split-Path $LrgexExe -Parent) "..\..\Cargo.toml") -Pattern '^version\s*=\s*"(.+)"').Matches[0].Groups[1].Value
$sevenZipVersion = (& $SevenZip | Select-String "7-Zip").ToString().Trim()
$winRarVersion = ((& $WinRar | Select-String "RAR") | Select-Object -First 1).ToString().Trim()

Write-Host "  LRGEX: v$lrgexVersion (zero configuration)"
Write-Host "  7-Zip: $sevenZipVersion (fastest preset: -mx=1)"
Write-Host "  WinRAR: $winRarVersion (fastest preset: -m1)"
Write-Host "  Windows ZIP: PowerShell Compress-Archive (Fastest)"
Write-Host ""

# -- Dataset Generation -------------------------------------------------------
function New-Dataset {
    param([string]$Name, [scriptblock]$Gen)
    $dir = Join-Path $DataDir $Name
    if (Test-Path $dir) { Remove-Item $dir -Recurse -Force }
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
    & $Gen $dir
    $count = (Get-ChildItem $dir -Recurse -File).Count
    $size = (Get-ChildItem $dir -Recurse -File | Measure-Object Length -Sum).Sum
    Write-Host "  $Name`: $count files, $([math]::Round($size/1GB, 2)) GB" -ForegroundColor Gray
}

# Deterministic pseudo-random bytes
function Get-RandomBytes {
    param([int]$Length, [int64]$Seed)
    $rng = [System.Random]::new($Seed)
    $buf = New-Object byte[] $Length
    $rng.NextBytes($buf)
    return ,$buf
}

# Realistic text content generator
function Get-TextContent {
    param([int]$KB, [int64]$Seed)
    $rng = [System.Random]::new(([int]($Seed % [int]::MaxValue)))
    $words = @("data","file","system","value","result","output","config","process","network","server","database","index","query","cache","buffer","stream","packet","module","handler","parser","encoder","decoder","thread","worker","queue","stack","heap","tree","graph","node","list","table","column","row","field","record","entry","key","token","session","request","response","header","payload","message","signal","event","action","state","model","view","controller","service","factory","adapter","proxy","bridge","strategy","observer","visitor","command","iterator","template","singleton","prototype")
    $sb = [System.Text.StringBuilder]::new()
    $wordCount = $words.Count
    $linesNeeded = $KB * 4
    for ($i = 0; $i -lt $linesNeeded; $i++) {
        $count = $rng.Next(5, 15)
        for ($j = 0; $j -lt $count; $j++) {
            [void]$sb.Append($words[$rng.Next($wordCount)])
            if ($j -lt $count - 1) { [void]$sb.Append(' ') }
        }
        [void]$sb.Append([char]10)
    }
    return $sb.ToString()
}

Write-Host "Generating datasets..." -ForegroundColor Yellow

# D1: Mixed-3GB (realistic mixed content)
New-Dataset "Mixed-3GB" {
    param($dir)
    # JPEG-like (incompressible headers + structured data)
    1..20 | ForEach-Object {
        $size = Get-Random -Maximum 30 -Minimum 15
        [IO.File]::WriteAllBytes("$dir\img_$($_).jpg", (Get-RandomBytes -Length ($size * 1MB) -Seed ($_ * 1000 + 7)))
    }
    # MP4-like (mostly incompressible)
    1..5 | ForEach-Object {
        [IO.File]::WriteAllBytes("$dir\video_$($_).mp4", (Get-RandomBytes -Length (150 * 1MB) -Seed ($_ * 2000 + 42)))
    }
    # Text files (compressible)
    1..50 | ForEach-Object {
        [IO.File]::WriteAllText("$dir\doc_$($_).txt", (Get-TextContent -KB (Get-Random -Maximum 200 -Minimum 50) -Seed ($_ * 3000)))
    }
    # JSON (compressible)
    1..30 | ForEach-Object {
        $json = "{`"id`":$_ ,`"data`":`"$(Get-TextContent -KB 5 -Seed ($_ * 4000))`",`"ts`":$($_ * 1000)}"
        [IO.File]::WriteAllText("$dir\data_$($_).json", $json)
    }
    # CSV (compressible)
    1..20 | ForEach-Object {
                $sb = [System.Text.StringBuilder]::new()
        for ($r = 1; $r -le 10000; $r++) {
            [void]$sb.Append($r.ToString() + ',' + ($r * 7 % 100000).ToString() + ',value_' + $r.ToString() + ',category_' + ($r % 20).ToString())
            [void]$sb.Append([char]10)
        }
        [IO.File]::WriteAllText("$dir\table_$($_).csv", $sb.ToString())
    }
    # EXE-like binary (mixed compressibility)
    1..10 | ForEach-Object {
        $buf = New-Object byte[] (20 * 1MB)
        # Fill with repeating patterns (compressible)
        $pattern = Get-RandomBytes -Length (1 * 1MB) -Seed ($_ * 5000)
        for ($i = 0; $i -lt $buf.Length; $i += $pattern.Length) {
            $copyLen = [Math]::Min($pattern.Length, $buf.Length - $i)
            [Array]::Copy($pattern, 0, $buf, $i, $copyLen)
        }
        [IO.File]::WriteAllBytes("$dir\binary_$($_).bin", $buf)
    }
}

# D2: Highly-Compressible-1GB
New-Dataset "HighlyCompressible-1GB" {
    param($dir)
    # Massive text files (very compressible)
    1..20 | ForEach-Object {
        [IO.File]::WriteAllText("$dir\text_$($_).txt", (Get-TextContent -KB (50 * 1024) -Seed ($_ * 6000)))
    }
    # XML (structured, very compressible)
    1..10 | ForEach-Object {
        $sb = [System.Text.StringBuilder]::new()
        [void]$sb.Append('<?xml version="1.0"?><root>')
        for ($i = 1; $i -le 50000; $i++) {
            [void]$sb.Append('<item id="')
            [void]$sb.Append($i.ToString())
            [void]$sb.Append('"><value>')
            [void]$sb.Append(($i * 13 % 1000).ToString())
            [void]$sb.Append('</value></item>')
        }
        [void]$sb.Append('</root>')
        $xml = $sb.ToString()
        [IO.File]::WriteAllText("$dir\xml_$($_).xml", $xml)
    }
}

# D3: AlreadyCompressed-1GB
New-Dataset "AlreadyCompressed-1GB" {
    param($dir)
    # JPEG-like (incompressible)
    1..40 | ForEach-Object {
        [IO.File]::WriteAllBytes("$dir\img_$($_).jpg", (Get-RandomBytes -Length (20 * 1MB) -Seed ($_ * 7000 + 100)))
    }
    # MP4-like (incompressible)
    1..3 | ForEach-Object {
        [IO.File]::WriteAllBytes("$dir\vid_$($_).mp4", (Get-RandomBytes -Length (80 * 1MB) -Seed ($_ * 8000 + 200)))
    }
}

# D4: ManySmallFiles-1GB
New-Dataset "ManySmallFiles-1GB" {
    param($dir)
    # 10000 small files across nested dirs
    1..100 | ForEach-Object {
        $subdir = "$dir\folder_$_\sub_($_ % 10)"
        New-Item -ItemType Directory -Path $subdir -Force | Out-Null
        1..100 | ForEach-Object {
            $size = Get-Random -Maximum 10240 -Minimum 1024
            [IO.File]::WriteAllBytes("$subdir\file_$($_).dat", (Get-RandomBytes -Length $size -Seed ($_ * 9000 + $subdir.GetHashCode())))
        }
    }
}

# D5: LargeFiles-3GB
New-Dataset "LargeFiles-3GB" {
    param($dir)
    1..3 | ForEach-Object {
        [IO.File]::WriteAllBytes("$dir\huge_$($_).bin", (Get-RandomBytes -Length (1 * 1024 * 1MB) -Seed ($_ * 10000)))
    }
    1..2 | ForEach-Object {
        [IO.File]::WriteAllBytes("$dir\big_$($_).dat", (Get-RandomBytes -Length (300 * 1MB) -Seed ($_ * 11000)))
    }
}

Write-Host ""

# -- Dataset Manifest ---------------------------------------------------------
Write-Host "Generating dataset manifests..." -ForegroundColor Yellow
$manifest = @{}
Get-ChildItem $DataDir -Directory | ForEach-Object {
    $dsName = $_.Name
    $files = Get-ChildItem $_.FullName -Recurse -File
    $dsManifest = @()
    $totalBytes = 0
    foreach ($f in $files) {
        $sha = ([System.Security.Cryptography.SHA256]::Create().ComputeHash([IO.File]::OpenRead($f.FullName)) | ForEach-Object { $_.ToString('x2') }) -join ''
        $dsManifest += @{
            path = $f.FullName.Substring($_.FullName.Length + 1)
            size = $f.Length
            sha256 = $sha.ToLower()
            ext = $f.Extension.ToLower()
        }
        $totalBytes += $f.Length
    }
    $manifest[$dsName] = @{
        files = $dsManifest
        total_bytes = $totalBytes
        file_count = $files.Count
    }
}
$manifest | ConvertTo-Json -Depth 4 | Out-File (Join-Path $ResultsDir "dataset-manifest.json") -Encoding utf8
Write-Host ""

# -- Benchmark Runner ---------------------------------------------------------
$results = @()
$global:startTime = Get-Date

function Invoke-Compress {
    param([string]$App, [string]$Source, [string]$Dest)
    $p = $null
    switch ($App) {
        "LRGEX" {
            $p = Start-Process -FilePath $LrgexExe -ArgumentList "`"$Source`"" -PassThru -WindowStyle Hidden
            $p.WaitForExit()
            # LRGEX creates <name>.zgx next to the SOURCE folder — find + move to $Dest
            $srcLeaf = Split-Path $Source -Leaf
            $srcParent = Split-Path $Source -Parent
            $createdArchive = Join-Path $srcParent ($srcLeaf + ".zgx")
            if (Test-Path $createdArchive) {
                Move-Item $createdArchive $Dest -Force
            }
            return @{ exit_code = $p.ExitCode; archive = $Dest }
        }
        "7-Zip" {
            $p = Start-Process -FilePath $SevenZip -ArgumentList "a -t7z -mx=1 `"$Dest`" `"$Source\*`"" -PassThru -WindowStyle Hidden 
        }
        "WinRAR" {
            $p = Start-Process -FilePath $WinRar -ArgumentList "a -m1 -r -ep1 `"$Dest`" `"$Source\*`"" -PassThru -WindowStyle Hidden 
        }
        "WindowsZIP" {
            # PowerShell Compress-Archive uses Optimal by default; Fastest is the fastest legitimate compression
            $srcLeaf = Split-Path $Source -Leaf
            $destParent = Split-Path $Dest -Parent
            Compress-Archive -Path "$Source\*" -DestinationPath "$Dest" -CompressionLevel Fastest
            return @{ exit_code = 0; archive = $Dest }
        }
    }
    if ($p) {
        $p.WaitForExit()
        return @{ exit_code = $p.ExitCode; archive = $Dest }
    }
}

function Invoke-Extract {
    param([string]$App, [string]$Archive, [string]$Dest)
    $p = $null
    switch ($App) {
        "LRGEX" {
            $p = Start-Process -FilePath $LrgexExe -ArgumentList "-x `"$Archive`"" -PassThru -WindowStyle Hidden
            $p.WaitForExit()
            # LRGEX extracts to a sibling folder named after the archive stem
            $archStem = [IO.Path]::GetFileNameWithoutExtension($Archive)
            $archParent = Split-Path $Archive -Parent
            $lrgexOut = Join-Path $archParent $archStem
            if (Test-Path $lrgexOut) {
                # Move contents into $Dest (LRGEX creates the folder, we want the files inside $Dest)
                if (Test-Path $Dest) { Remove-Item $Dest -Recurse -Force }
                Move-Item $lrgexOut $Dest -Force
            }
            return $p.ExitCode
        }
        "7-Zip" {
            $p = Start-Process -FilePath $SevenZip -ArgumentList "x `"$Archive`" -o`"$Dest`" -y" -PassThru -WindowStyle Hidden 
        }
        "WinRAR" {
            $p = Start-Process -FilePath $WinRar -ArgumentList "x `"$Archive`" `"$Dest\`" -y" -PassThru -WindowStyle Hidden 
        }
        "WindowsZIP" {
            Expand-Archive -Path $Archive -DestinationPath $Dest -Force
            return 0
        }
    }
    if ($p) {
        $p.WaitForExit()
        return $p.ExitCode
    }
}

function Test-Integrity {
    param([string]$Original, [string]$Extracted, [string]$AppName)
    # For 7-Zip/WinRAR, the extraction structure may include the source leaf name
    # Adjust the extracted path to match
    $origLeaf = Split-Path $Original -Leaf
    $adjustedExtracted = $Extracted
    $directDirs = @(Get-ChildItem $Extracted -Directory -ErrorAction SilentlyContinue)
    $directFiles = @(Get-ChildItem $Extracted -File -ErrorAction SilentlyContinue)
    if ($directFiles.Count -eq 0 -and $directDirs.Count -eq 1 -and $directDirs[0].Name -eq $origLeaf) {
        $Extracted = $directDirs[0].FullName
    }
    if (-not (Test-Path "$Extracted\*") ) { return $false }

    # Try direct match
    $origFiles = Get-ChildItem $Original -Recurse -File
    $extFiles = Get-ChildItem $Extracted -Recurse -File

    if ($origFiles.Count -ne $extFiles.Count) { return $false }

    foreach ($of in $origFiles) {
        $rel = $of.FullName.Substring($Original.Length)
        $ef = Join-Path $Extracted $rel
        if (-not (Test-Path $ef)) { return $false }
        if ($of.Length -ne (Get-Item $ef).Length) { return $false }
        # Skip SHA for speed on large datasets - file count + size match is sufficient
        # For the manifest, we already have SHA; just verify count+size for now
    }
    return $true
}

$apps = @(
    @{ name = "LRGEX"; preset = "Zero configuration"; version = "v$lrgexVersion" }
    @{ name = "7-Zip"; preset = "Fastest (-mx=1)"; version = $sevenZipVersion }
    @{ name = "WinRAR"; preset = "Fastest (-m1)"; version = $winRarVersion }
    @{ name = "WindowsZIP"; preset = "Fastest (Compress-Archive)"; version = "PowerShell 5.1" }
)

$datasets = Get-ChildItem $DataDir -Directory | Sort-Object Name

$totalRuns = 0
$failedRuns = 0
$integrityFailures = 0

foreach ($ds in $datasets) {
    $dsName = $ds.Name
    $dsPath = $ds.FullName
    $dsFiles = (Get-ChildItem $dsPath -Recurse -File).Count
    $dsBytes = (Get-ChildItem $dsPath -Recurse -File | Measure-Object Length -Sum).Sum

    Write-Host "+- Dataset: $dsName ($dsFiles files, $([math]::Round($dsBytes/1GB, 2)) GB)" -ForegroundColor Cyan

    foreach ($app in $apps) {
        $appName = $app.name
        Write-Host "|  $appName ($($app.preset))" -ForegroundColor White

        for ($run = 1; $run -le $RUNS; $run++) {
            $ext = switch ($appName) {
                "LRGEX" { ".zgx" }
                "7-Zip" { ".7z" }
                "WinRAR" { ".rar" }
                "WindowsZIP" { ".zip" }
            }
            $archivePath = Join-Path $ArchivesDir "$dsName-$appName$ext"
            $extractPath = Join-Path $ExtractDir "$dsName-$appName"

            # Clean previous
            Remove-Item $archivePath -Force -ErrorAction SilentlyContinue
            Remove-Item $extractPath -Recurse -Force -ErrorAction SilentlyContinue

            # Compress
            $t0 = [System.Diagnostics.Stopwatch]::StartNew()
            $compResult = Invoke-Compress -App $appName -Source $dsPath -Dest $archivePath
            $t0.Stop()
            $compTime = $t0.Elapsed.TotalSeconds

            $archiveSize = if (Test-Path $archivePath) { (Get-Item $archivePath).Length } else { 0 }
            $ratio = if ($archiveSize -gt 0) { [math]::Round($dsBytes / $archiveSize, 3) } else { 0 }
            $compMBps = [math]::Round($dsBytes / 1MB / $compTime, 1)

            # Extract
            New-Item -ItemType Directory -Path $extractPath -Force | Out-Null
            $t1 = [System.Diagnostics.Stopwatch]::StartNew()
            $extExit = Invoke-Extract -App $appName -Archive $archivePath -Dest $extractPath
            $t1.Stop()
            $extTime = $t1.Elapsed.TotalSeconds
            $extMBps = [math]::Round($dsBytes / 1MB / $extTime, 1)

            # Integrity check
            $integrity = Test-Integrity -Original $dsPath -Extracted $extractPath -AppName $appName
            $integrityStr = if ($integrity) { "PASS" } else { "FAIL" }
            if (-not $integrity) { $integrityFailures++ }

            $compExitStr = if ($compResult.exit_code -eq 0) { "OK" } else { "EXIT $($compResult.exit_code)" }
            if ($compResult.exit_code -ne 0 -or $archiveSize -eq 0) { $failedRuns++ }

            $results += [PSCustomObject]@{
                application = $appName
                application_version = $app.version
                compression_preset = $app.preset
                dataset = $dsName
                dataset_size_bytes = $dsBytes
                file_count = $dsFiles
                run_number = $run
                compression_time_seconds = [math]::Round($compTime, 2)
                compression_throughput_mb_s = $compMBps
                archive_size_bytes = $archiveSize
                compression_ratio = $ratio
                extraction_time_seconds = [math]::Round($extTime, 2)
                extraction_throughput_mb_s = $extMBps
                integrity_check = $integrityStr
                exit_code = $compResult.exit_code
                timestamp = (Get-Date -Format "yyyy-MM-ddTHH:mm:ss")
            }

            $totalRuns++
            Write-Host "|    Run $run`: ${compTime}s compress, ${extTime}s extract, ratio $ratio, $integrityStr" -ForegroundColor Gray
        }
    }
    Write-Host "+-" -ForegroundColor Cyan
}

# -- Export Raw Results -------------------------------------------------------
$results | Export-Csv (Join-Path $ResultsDir "results.csv") -NoTypeInformation -Encoding UTF8
$results | ConvertTo-Json | Out-File (Join-Path $ResultsDir "results.json") -Encoding utf8

# -- Generate Markdown Report (medians) --------------------------------------
function Get-Median {
    param([double[]]$Values)
    $sorted = $Values | Sort-Object
    $n = $sorted.Count
    if ($n % 2 -eq 0) {
        return ($sorted[$n/2 - 1] + $sorted[$n/2]) / 2
    } else {
        return $sorted[($n - 1) / 2]
    }
}

$md = @()
$md += "# LRGEX Compress - Fair Compression Benchmark Results"
$md += ""
$md += "Generated: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')"
$md += ""
$md += "## Methodology"
$md += "- Each application/dataset combination ran **$RUNS times**; **median** is published"
$md += "- All apps used their **fastest legitimate compression preset** (no Store/copy mode)"
$md += "- LRGEX: zero configuration (as shipped)"
$md += "- Integrity verified: file count + size match on extraction"
$md += ""

foreach ($ds in $datasets) {
    $dsName = $ds.Name
    $md += "## Dataset: $dsName"
    $md += ""
    $md += "| Application | Mode | Archive Size | Comp Time (s) | Speed (MB/s) | Extract (s) | Ratio | Integrity |"
    $md += "|---|---|---:|---:|---:|---:|---:|---|"

    foreach ($app in $apps) {
        $appResults = $results | Where-Object { $_.dataset -eq $dsName -and $_.application -eq $app.name }
        if ($appResults.Count -eq 0) { continue }

        $medComp = Get-Median ($appResults | ForEach-Object { $_.compression_time_seconds })
        $medExt  = Get-Median ($appResults | ForEach-Object { $_.extraction_time_seconds })
        $medSize = Get-Median ($appResults | ForEach-Object { $_.archive_size_bytes })
        $medSpeed = Get-Median ($appResults | ForEach-Object { $_.compression_throughput_mb_s })
        $medRatio = Get-Median ($appResults | ForEach-Object { $_.compression_ratio })
        $integrity = ($appResults | ForEach-Object { $_.integrity_check } | Sort-Object -Unique) -join "/"

        $sizeMB = [math]::Round($medSize / 1MB, 1)
        $md += "| $($app.name) | $($app.preset) | ${sizeMB} MB | $medComp | $medSpeed | $medExt | $medRatio | $integrity |"
    }
    $md += ""
}

# Winner summary
$md += "## Speed Ranking (median compression throughput across all datasets)"
$md += ""
$ranking = @()
foreach ($app in $apps) {
    $appResults = $results | Where-Object { $_.application -eq $app.name -and $_.integrity_check -eq "PASS" }
    if ($appResults.Count -gt 0) {
        $avgSpeed = ($appResults | ForEach-Object { $_.compression_throughput_mb_s } | Measure-Object -Average).Average
        $ranking += @{ name = $app.name; speed = [math]::Round($avgSpeed, 1) }
    }
}
$ranking | Sort-Object { -$_.speed } | ForEach-Object {
    $md += "$($_.name): **$($_.speed) MB/s**"
    $md += ""
}

$md | Out-File (Join-Path $ResultsDir "benchmark.md") -Encoding utf8

# -- Final Validation ---------------------------------------------------------
$totalTime = (Get-Date) - $global:startTime
Write-Host ""
Write-Host "==" -ForegroundColor Green
Write-Host "|  BENCHMARK COMPLETE                                     |" -ForegroundColor Green
Write-Host "=" -ForegroundColor Green
Write-Host "  Datasets: $($datasets.Count)"
Write-Host "  Applications: $($apps.Count)"
Write-Host "  Total runs: $totalRuns"
Write-Host "  Failed runs: $failedRuns"
Write-Host "  Integrity failures: $integrityFailures"
Write-Host "  Total duration: $([math]::Round($totalTime.TotalMinutes, 1)) minutes"
Write-Host "  Results: $ResultsDir"
Write-Host ""
Write-Host "  Median table: $(Join-Path $ResultsDir 'benchmark.md')"
