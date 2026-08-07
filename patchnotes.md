# Patch Notes - Version 1.4.0 - Current

## 🛡️ Data Integrity — Full Metadata Preservation

LRGEX Compress now preserves your files **exactly** across a compress → extract round-trip.
Whatever goes into the archive comes out identical — no more "every extracted file shows today's date."

- **LastWriteTime (mtime)** preserved on every file and folder — sort-by-date, build systems, sync tools, and incremental backups all keep working correctly.
- **CreationTime (ctime)** preserved — the original creation date is restored, not lost.
- **Windows attributes** preserved — Hidden, Read-only, System, and Archive flags survive the round-trip. A hidden file extracts hidden; a read-only file extracts read-only.
- **Symbolic links / junctions** preserved — links recreate as links. On Windows, creating a link needs Administrator or Developer Mode; LRGEX asks **once** ("allow admin?") and, if yes, re-launches the extraction elevated to recreate every link exactly. If you decline, links degrade gracefully (never aborts the whole extraction).
- **Directory timestamps** restored in a final pass — so writing children into a folder doesn't overwrite the folder's original mtime.

### How it works

`.zgx` archives (tar + zstd) now carry real metadata via the standard tar `mtime` field
and PAX local extensions (`SCHILY.creationtime`, `LRGEX.fileattr`). `.zip` and `.rar`
extraction also restores the timestamps and attributes those archives already contain.

### Note on speed

Metadata preservation adds one `SetFileTime` + one `SetFileAttributes` call per file —
the cost of restoring everything faithfully. Extraction is slightly slower than 1.3.0
as a result, but the round-trip is now fully **lossless**.

---

# Patch Notes - Version 1.3.0

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
