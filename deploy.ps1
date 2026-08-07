# LRGEX Compress - build + sign installer + manifest + upload deploy script.
# Signs the INSTALLER (not the bare exe), so updates download and run the installer.
# Saves as UTF-8 (PowerShell-friendly).

$ErrorActionPreference = "Stop"

function Fail($step, $reason) {
    Write-Host ""
    Write-Host "BUILD FAILURE at $step" -ForegroundColor Red
    Write-Host "Reason: $reason" -ForegroundColor Red
    Write-Host "========================================" -ForegroundColor Red
    exit 1
}

$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
$UPLOAD_PATH = [string]([char]92) + [string]([char]92) + "192.168.1.4" + [string]([char]92) + "lrgex" + [string]([char]92) + "FileServer" + [string]([char]92) + "download" + [string]([char]92) + "data" + [string]([char]92) + "app" + [string]([char]92) + "rst" + [string]([char]92) + "lrgex-compress"
$DOWNLOAD_BASE = "https://download.lrgex.com/app/rst/lrgex-compress"
$ghRepo = "LRGEX/Compress"
$EXE_NAME = "lrgex-compress.exe"
$ISS_PATH = Join-Path $PSScriptRoot "installer\lrgex-compress.iss"
$ISCC = "C:\Program Files (x86)\Inno Setup 6\ISCC.exe"
$PROJECT_ROOT = $PSScriptRoot
Set-Location $PROJECT_ROOT

# ========== STEP 1: VERSION ==========
$cargoPath = Join-Path $PROJECT_ROOT "Cargo.toml"
$cargo = Get-Content $cargoPath -Raw
if ($cargo -notmatch '(?m)^version = "([^"]*)"') { Fail "Version" "Cannot find version in Cargo.toml" }
$version = $matches[1]
Write-Host "[1/7] Version: $version" -ForegroundColor Green

# Sync version into the Inno Setup .iss.
$iss = Get-Content $ISS_PATH -Raw
$issNew = $iss -replace '(?m)^#define MyAppVersion\s+"[^"]*"', "#define MyAppVersion `"$version`""
if ($iss -ne $issNew) { Set-Content $ISS_PATH $issNew -NoNewline }

# ========== STEP 2: BUILD (app + signer) ==========
Write-Host "[2/7] Building..." -ForegroundColor Yellow
$ErrorActionPreference = "Continue"
cargo build --release 2>&1 | Out-Null
$buildResult = $LASTEXITCODE
cargo build --bin sign --release 2>&1 | Out-Null
$signBuildResult = $LASTEXITCODE
$ErrorActionPreference = "Stop"
if ($buildResult -ne 0) { Fail "Build" "Main build failed (exit $buildResult)" }
if ($signBuildResult -ne 0) { Fail "Build" "Signer build failed (exit $signBuildResult)" }

$exePath = Join-Path $PROJECT_ROOT "target\release\$EXE_NAME"
$signerPath = Join-Path $PROJECT_ROOT "target\release\sign.exe"
if (-not (Test-Path $exePath)) { Fail "Build" "Exe not found: $exePath" }
if (-not (Test-Path $signerPath)) { Fail "Build" "Signer not found: $signerPath" }
Write-Host "      Build OK" -ForegroundColor Green

# ========== STEP 3: COMPILE INSTALLER ==========
Write-Host "[3/7] Compiling installer..." -ForegroundColor Yellow
if (-not (Test-Path $ISCC)) { Fail "Installer" "ISCC not found at $ISCC" }
$dispVer = "v$version"
$installerName = "LRGEX-Compress-$dispVer-setup"
& $ISCC "/Q" "/F$installerName" "$ISS_PATH"
if ($LASTEXITCODE -ne 0) { Fail "Installer" "ISCC failed (exit $LASTEXITCODE)" }
$installer = Join-Path $PROJECT_ROOT "target\installer\$installerName.exe"
if (-not (Test-Path $installer)) { Fail "Installer" "Expected installer not found at $installer" }
Write-Host "      Installer OK: $installer" -ForegroundColor Green

# ========== STEP 4: SIGN THE INSTALLER ==========
Write-Host "[4/7] Signing installer..." -ForegroundColor Yellow
$ErrorActionPreference = "Continue"
$signature = (& $signerPath $installer 2>&1 | Select-Object -Last 1).Trim()
$ErrorActionPreference = "Stop"
if ($signature.Length -ne 128) { Fail "Sign" "Invalid signature length: $($signature.Length) chars (expected 128). Output: $signature" }
Write-Host "      Signed OK ($($signature.Substring(0,16))...)" -ForegroundColor Green

# ========== STEP 5: MANIFEST ==========
Write-Host "[5/7] Creating manifest..." -ForegroundColor Yellow

# Finding #1: Sign the manifest itself (not just the binary) so a compromised server
# can't inject a fake version/URL. Uses a DETACHED signature (latest.json.sig) to avoid
# the chicken-and-egg problem of embedding a signature inside the JSON it signs.
# The client fetches both files, verifies the sig over the raw bytes, then parses.
$latestJson = @{
    version = $version
    notes = "Update available"
    pub_date = (Get-Date -Format "yyyy-MM-ddTHH:mm:ssZ")
    platforms = @{
        "windows-x86_64" = @{
            url = "$DOWNLOAD_BASE/$installerName.exe"
            signature = $signature
        }
    }
} | ConvertTo-Json -Depth 10

$manifestPath = Join-Path $PROJECT_ROOT "latest.json"
[System.IO.File]::WriteAllText($manifestPath, $latestJson, [System.Text.UTF8Encoding]::new($false))
if (-not (Test-Path $manifestPath)) { Fail "Manifest" "latest.json was not created" }

# Sign the manifest's raw bytes with the same Ed25519 key. Detached .sig file.
$manifestSigOutput = & $signerPath $manifestPath 2>&1
$manifestSig = ($manifestSigOutput | Select-Object -Last 1).Trim()
if ($manifestSig.Length -ne 128) {
    Fail "Manifest" "Invalid manifest signature length: $($manifestSig.Length) chars (expected 128)"
}
$manifestSigPath = Join-Path $PROJECT_ROOT "latest.json.sig"
[System.IO.File]::WriteAllText($manifestSigPath, $manifestSig, [System.Text.UTF8Encoding]::new($false))
Write-Host "      Manifest signed OK ($($manifestSig.Substring(0,16))...)" -ForegroundColor Green
Write-Host "      Manifest OK" -ForegroundColor Green

# ========== STEP 6: UPLOAD (installer + manifest) ==========
Write-Host "[6/7] Uploading..." -ForegroundColor Yellow
if (-not (Test-Path $UPLOAD_PATH)) {
    New-Item -ItemType Directory -Path $UPLOAD_PATH -Force | Out-Null
    if (-not (Test-Path $UPLOAD_PATH)) { Fail "Upload" "Cannot create or access server path: $UPLOAD_PATH" }
}

$localSize = (Get-Item $installer).Length
Copy-Item $installer (Join-Path $UPLOAD_PATH "$installerName.exe") -Force
if (-not (Test-Path (Join-Path $UPLOAD_PATH "$installerName.exe"))) { Fail "Upload" "Installer was not uploaded" }
$remoteSize = (Get-Item (Join-Path $UPLOAD_PATH "$installerName.exe")).Length
if ($remoteSize -ne $localSize) { Fail "Upload" "Installer size mismatch: local=$localSize remote=$remoteSize" }

Copy-Item $manifestPath (Join-Path $UPLOAD_PATH "latest.json") -Force
if (-not (Test-Path (Join-Path $UPLOAD_PATH "latest.json"))) { Fail "Upload" "latest.json was not uploaded" }
# Upload the detached manifest signature too.
Copy-Item $manifestSigPath (Join-Path $UPLOAD_PATH "latest.json.sig") -Force
if (-not (Test-Path (Join-Path $UPLOAD_PATH "latest.json.sig"))) { Fail "Upload" "latest.json.sig was not uploaded" }
Write-Host "      Upload OK ($localSize bytes)" -ForegroundColor Green

# ========== STEP 6b: VERIFY VIA PUBLIC URL ==========
Write-Host "Verifying via public URL..." -ForegroundColor Yellow
Start-Sleep -Seconds 2
try {
    $publicManifest = (Invoke-WebRequest "$DOWNLOAD_BASE/latest.json" -UseBasicParsing -TimeoutSec 15).Content
    if ($publicManifest -notmatch $version) { Fail "Verify" "Public URL does not contain version $version" }
    if ($publicManifest -notmatch $signature) { Fail "Verify" "Public URL does not contain correct signature" }
    # Verify the detached manifest signature is also served and matches what we signed.
    $publicSig = (Invoke-WebRequest "$DOWNLOAD_BASE/latest.json.sig" -UseBasicParsing -TimeoutSec 15).Content.Trim()
    if ($publicSig -ne $manifestSig) {
        Fail "Verify" "Public latest.json.sig does not match the signed manifest signature"
    }
    Write-Host "      PUBLIC URL VERIFIED: v$version with correct signature + manifest sig" -ForegroundColor Green
} catch {
    Fail "Verify" "Cannot fetch public URL $DOWNLOAD_BASE/latest.json (or .sig) : $_"
}

# ========== STEP 7: GITHUB RELEASE ==========
Write-Host "[7/8] GitHub Release..." -ForegroundColor Yellow
$tag = "v$version"
$releaseNotes = "LRGEX Compress $version"
$patchPath = Join-Path $PROJECT_ROOT "patchnotes.md"
if (Test-Path $patchPath) {
    $lines = Get-Content $patchPath -Encoding UTF8
    $capturing = $false
    $noteLines = @()
    foreach ($l in $lines) {
        if ($l -match "^## v$version " -or $l -match "^# Patch Notes - Version $version") { $capturing = $true; continue }
        if ($capturing -and $l -match "^# Patch Notes - Version ") { break }
        if ($capturing -and $l -match "^## v") { break }
        if ($capturing) { $noteLines += $l }
    }
    if ($noteLines.Count -gt 0) { $releaseNotes = ($noteLines | Where-Object { $_.Trim() }) -join "`n" }
}
$notesFile = Join-Path $env:TEMP "lrgex-compress-release-notes.txt"
[System.IO.File]::WriteAllText($notesFile, $releaseNotes, [System.Text.UTF8Encoding]::new($false))
$ErrorActionPreference = "Continue"
gh release create $tag $installer --title $tag --notes-file $notesFile 2>&1 | Out-Null
if ($LASTEXITCODE -eq 0) {
    $ErrorActionPreference = "Stop"
    Write-Host "      Release created: $tag" -ForegroundColor Green
} else {
    gh release edit $tag --notes-file $notesFile 2>&1 | Out-Null
    gh release upload $tag $installer --clobber 2>&1 | Out-Null
    $ErrorActionPreference = "Stop"
    if ($LASTEXITCODE -ne 0) { Fail "GitHub" "Cannot create or update release: exit $LASTEXITCODE" }
    Write-Host "      Release updated: $tag" -ForegroundColor Green
}

# Finding #4: Verify the GitHub release asset size-matches via the API.
# The green banner's contract requires EVERY gate to pass, including GitHub.
# Uses the GitHub API (not HTTP HEAD) to avoid CDN propagation lag.
$ghAssetSize = [int64](gh release view $tag --json assets -q ".assets[] | select(.name==\"$installerName.exe\") | .size" 2>$null)
if ($LASTEXITCODE -ne 0 -or $ghAssetSize -eq 0) {
    Fail "GitHub" "Cannot verify GitHub release asset via API (exit $LASTEXITCODE)"
}
if ($ghAssetSize -ne $localSize) {
    Fail "GitHub" "GitHub asset size mismatch: local=$localSize remote=$ghAssetSize"
}
Write-Host "      GitHub asset verified: $ghAssetSize bytes" -ForegroundColor Green
Remove-Item $notesFile -ErrorAction SilentlyContinue

# ========== STEP 8: DONE ==========
# The green banner ONLY prints after EVERY gate passed.
Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  Deployed v$version successfully!" -ForegroundColor Green
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  Server:    $DOWNLOAD_BASE/$installerName.exe" -ForegroundColor Gray
Write-Host "  Manifest:  $DOWNLOAD_BASE/latest.json" -ForegroundColor Gray
Write-Host "  Installer: $installer" -ForegroundColor Gray
