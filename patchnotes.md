# Patch Notes - Version 0.1.0 - Current

## 🚀 New Features

- **Right-click compression and extraction**: one `LRGEX` Explorer entry that opens a hover submenu (Compress on folders/files, Extract on `.zgx` archives). Per-user, HKCU only, no UAC.
- **`.zgx` archive format**: branded extension (internally `tar` + `zstd`), like WinRAR's `.rar`. Double-clicking a `.zgx` extracts it.
- **WinRAR-style progress window**: colored progress bar, live percent, MB/s, and ETA. A 500 ms heartbeat + animated spinner keep it from ever looking frozen, even on a cold cache.
- **Byte-based progress**: percentage is always `bytes_done / bytes_total` (never file-count), so a single huge file does not sit at a misleading percent.
- **Multi-threaded compression**: zstd with `zstdmt` + parallel rayon reads across all CPU cores, designed for multi-gigabyte folders.
- **Atomic archive writes**: compress writes to `<name>.zgx.part` and atomically renames to `<name>.zgx` only on full success, so an interrupted or crashed compress can never leave a complete-looking partial archive.
- **Empty-directory preservation**: directory entries are archived before their contents, so extraction restores the exact source tree, including empty folders.
- **Cancel cleans up**: aborting a compress releases the `.part` file and deletes it — no orphaned half-files are left behind.
- **Skip-with-warning on locked files**: a single unreadable file does not fail a multi-GB job. The archive still publishes, and a `<name>.zgx.skipped.txt` sidecar lists anything that could not be read (auto-deleted on the next clean run).
- **Signed auto-updates**: Ed25519 signature verification on every update. The version check runs silently on launch; the download + verify + swap flow runs only after the current job finishes, so it can never interrupt or freeze a compress/extract in progress.
- **Per-user installer**: Inno Setup, `PrivilegesRequired=lowest` (no UAC prompt). Registers the right-click cascade and the `.zgx` file association, and removes every registry entry it created on uninstall.
- **Self-contained executable**: a single ~14 MB `.exe`, zero runtime dependencies (static CRT, software renderer) — runs anywhere, including VMs with no GPU.

## 🔧 Improvements & Changes

- N/A

## 🐞 Bug Fixes

- N/A

## ⚠️ Known Issues

- N/A

---

Developed by **LRGEX**
