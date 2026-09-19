$ErrorActionPreference = 'Stop'
# Fat-LTO rustc needs a bigger thread stack or it crashes (0xc0000409, flaky at 32MB) — see AGENTS.md rule 7.
$env:RUST_MIN_STACK = '67108864'
$log = 'E:\LRG\LRG Data Cloud\L.R.G\Devoloping\Coding\Compress\deploy-dev.log'
$src = 'E:\LRG\LRG Data Cloud\L.R.G\Devoloping\Coding\Compress\target\release\lrgex-compress.exe'
$dst = 'C:\Program Files\LRGEX Compress\lrgex-compress.exe'
try {
    $p = Get-Process lrgex-compress -ErrorAction SilentlyContinue
    if ($p) { $p | Stop-Process -Force; Start-Sleep -Milliseconds 500 }
    Copy-Item $src $dst -Force
    $h1 = (certutil -hashfile $src SHA256 | Select-Object -Skip 1 -First 1).Trim()
    $h2 = (certutil -hashfile $dst SHA256 | Select-Object -Skip 1 -First 1).Trim()
    if ($h1 -eq $h2) { "DEPLOY_OK $h1" | Out-File $log -Encoding ascii } else { "DEPLOY_MISMATCH src=$h1 dst=$h2" | Out-File $log -Encoding ascii }
} catch {
    "DEPLOY_FAILED: $($_.Exception.Message)" | Out-File $log -Encoding ascii
}
