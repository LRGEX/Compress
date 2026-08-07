# Patch Notes - Version 1.4.1 - Current

## 🛡️ Data Integrity — Full Metadata Preservation

LRGEX Compress now preserves your files **exactly** across a compress → extract round-trip.
Whatever goes into the archive comes out identical — no more "every extracted file shows today's date."

- **LastWriteTime (mtime)** preserved on every file and folder — sort-by-date, build systems, sync tools, and incremental backups all keep working correctly.
- **CreationTime (ctime)** preserved — the original creation date is restored, not lost.
- **Windows attributes** preserved — Hidden, Read-only, System, and Archive flags survive the round-trip. A hidden file extracts hidden; a read-only file extracts read-only.
- **Symbolic links / junctions** preserved — links recreate as links. On Windows, creating a link needs Administrator or Developer Mode; LRGEX asks **once** ("allow admin?") and, if yes, re-launches the extraction elevated to recreate every link exactly. If you decline, links degrade gracefully (never aborts the whole extraction).
- **Directory timestamps** restored in a final pass — so writing children into a folder doesn't overwrite the folder's original mtime.

NTFS alternate data streams are not supported (rarely used; not planned).

## ⚡ Performance — Extract & Compress Wins

- **Metadata sidecar** (`.lrgex/meta.bin`): one small packed blob at the front of the
  archive replaces 11k+ per-entry metadata headers on big folders. Both compress and
  extract handle roughly half as many tar entries now. Old v1.4.0 archives still read
  correctly via the per-entry fallback.
- **One-syscall metadata restore** (`SetFileInformationByHandle`): mtime + ctime +
  attributes set in a single call on the already-open file handle — no per-file re-open.
- **Direct 4 MB chunked writes** on large-file extract (no redundant `BufWriter` copy).
- **Cache-served metadata on compress walk** (`DirEntry::metadata()` instead of a fresh
  `CreateFile` per file) — saves thousands of syscalls on big folders.
- **Removed the redundant 4 MB `BufWriter`** on compress output — zstd already emits
  good-sized blocks; the buffer was a pure memcpy of every compressed byte.
- **Tuned zstd multithreading** — `JobSize(16MB)` + `OverlapSizeLog(0)` at level 1,
  fewer thread handoffs.
- **Dedicated oversubscribed write pool** (3× core count) for extraction writes —
  correct sizing for workers that block in `CreateFile` and Defender scans.

## 🐞 Fixes

- **Multi-select coordinator hardened**: handles `WAIT_ABANDONED` (previous instance
  crashed holding the mutex) and stale coordinator-PID lockfiles — a second rapid launch
  can no longer silently no-op.
- Round-trip verified lossless: mtime/ctime/Hidden/ReadOnly/System, Unicode names,
  deeply nested paths, and a 3000-file stress batch (SHA256-checked).

## 🔒 Security Hardening

- **Signed manifest (detached `latest.json.sig`)**: the auto-update manifest is now
  cryptographically signed with Ed25519, preventing a compromised server from injecting
  a fake version or download URL.
- **Download size cap (100 MB)**: the updater rejects files exceeding 100 MB, preventing
  memory exhaustion from a poisoned manifest pointing at a huge file.
- **Update-channel logging**: failed update checks now write to `update.log` in the
  install folder, so silent update-channel failures are observable.
- **GitHub release asset verification**: `deploy.ps1` now size-verifies the GitHub
  release asset via the API, not just the exit code.
- **Path-length guard**: right-clicking a file with a path longer than 247 characters
  shows a clear error instead of a confusing "not found".
- **Slint attribution**: added "Made with Slint" badge (Slint Royalty-free License §2).

---

# Patch Notes - Version 1.4.0

## 🚀 New Features

- **Extract Here**: right-click `.zgx`/`.zip`/`.rar` → LRGEX → Extract Here (extracts to current folder, no subfolder)

## 🔧 Improvements & Changes

- Removed `AppliesTo` from installer (was hiding the right-click menu on all files)
- Added Extract Here to installer `.iss` for all three archive extensions

## 🐞 Bug Fixes

- Fixed: right-click menu disappeared on all files due to malformed AppliesTo value

## ⚠️ Known Issues

- N/A

---

# Patch Notes - Version 1.2.1

## 🚀 New Features

- N/A

## 🔧 Improvements & Changes

- N/A

## 🐞 Bug Fixes

- N/A

## ⚠️ Known Issues

- N/A

---

# Patch Notes - Version 1.2.0

## 🚀 New Features

- **Installer-based auto-update**: updates now download and run the full installer (not just the bare exe), ensuring registry entries, icons, and file associations are always updated correctly.

## 🔧 Improvements & Changes

- Installer filename now includes full version number (e.g. v1.2.0, not v1.2).
- `CloseApplications=force` and `RestartApplications=no` added to installer for silent updates.
- App silently exits when launched with no arguments (fixes post-update restart error).

## 🐞 Bug Fixes

- N/A

## ⚠️ Known Issues

- N/A

---

# Patch Notes - Version 1.1.0

## 🚀 New Features

- N/A

## 🔧 Improvements & Changes

- N/A

## 🐞 Bug Fixes

- N/A

## ⚠️ Known Issues

- N/A

---

# Patch Notes - Version 1.0.0

## 🚀 New Features

- **Multi-format extraction**: `.zip` and `.rar` extraction added alongside `.zgx`. Magic-byte detection (zstd/zip/rar) with extension fallback.
- **Real `.rar` progress**: vendored unrar 0.5.8 with UCM_PROCESSDATA counter patch for byte-level extraction progress.
- **Extract Here**: right-click `.zgx`/`.zip`/`.rar` → Extract Here dumps contents into the current folder.
- **Multi-select compress**: selecting multiple files/folders and right-clicking Compress produces ONE `.zgx` containing all items.
- **Signed auto-updates**: Ed25519 signature verification, deferred until after operation completes.
- **8-byte uncompressed total header** in `.zgx` format for accurate extraction progress.
- **Batch-parallel extraction**: rayon batch writer (2048 entries/64MB) with directory cache for many-small-files workloads.
- **Version badge** in lower right of progress window.
- **Shimmer animation** on progress bar fill.
- **Auto-close** 2s after successful operation.
- **Cancel kills process** immediately — no orphan threads.
- **Window icon** from product logo.
- **op-label/op-detail split**: action word in white bold, filename in silver.

## 🔧 Improvements & Changes

- ZIP backend switched to `deflate-flate2-zlib-ng` (C-accelerated, 4× faster decompression).
- `[profile.dev.package."*"] opt-level = 3` — dependencies compiled at -O3 even in debug builds.
- File preallocation (`set_len`) for entries >1MB during extraction.
- BufReader 256KB between zstd decoder and tar (reduced from 4MB to avoid cache eviction).
- Removed redundant BufReader before zstd decoder (it reads 128KB chunks natively).
- Folder name stripping: `x.md` folder → `x.zgx` (not `x.md.zgx`).
- Stable install path for right-click verbs (survives exe rebuilds).

## 🐞 Bug Fixes

- Empty directories now preserved through compress → extract round-trip.
- Atomic writes: `name.zgx.part` → `name.zgx` only on full success.
- Cancel during compress deletes the partial archive.
- Zip-slip protection (path traversal guard for both `.zip` and `.zgx` extraction).
- Failed extraction no longer hangs the UI (shows "Failed" instead of freezing).

## ⚠️ Known Issues

- Many-small-files extraction is limited by Windows Defender real-time scanning (~5-15 MB/s). This is a documented Windows limitation, not a code issue.

---

# Patch Notes - Version 0.1.0

## 🚀 New Features

- **Right-click compression and extraction**: one `LRGEX` Explorer entry that opens a hover submenu (Compress on folders/files, Extract on `.zgx` archives). Per-user, HKCU only, no UAC.
- **`.zgx` archive format**: Double-clicking a `.zgx` extracts it.
- **Progress window**: colored progress bar, live percent, MB/s, and ETA. A 500 ms heartbeat + animated spinner keep it from ever looking frozen, even on a cold cache.
- **Byte-based progress**: percentage is always `bytes_done / bytes_total` (never file-count), so a single huge file does not sit at a misleading percent.
- **Multi-threaded compression**: parallel reads across all CPU cores, designed for multi-gigabyte folders.
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
