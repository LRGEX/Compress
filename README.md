<div align="center">
  <img src="assets/logo.png" alt="Compress Logo" width="80">
  <br><br>
  
  <img src="https://download.lrgex.com/Dark%20Full%20Logo.png" alt="LRGEX Logo" width="300">

<div id="toc">
  <ul style="list-style: none">
    <summary>
      <h1>Compress</h1>
    </summary>
  </ul>
</div>

**Version 1.7.0**

Free, open source (GPL-3.0-or-later).

[<img src="https://slint.dev/logo/slint-logo-simple-light.png" width="100" alt="Made with Slint">](https://slint.dev)

</div>

---

## Extremely fast. No waiting.

LRGEX Compress is built around **Zstandard (zstd)** — a modern compression algorithm
optimized for speed. It uses all your CPU cores automatically, so big folders finish
in seconds instead of minutes.

For everyday backups and file sharing, seconds matter more than a slightly smaller
archive.

---

## Right-click. Done.

No app to open. No drag-and-drop. No compression levels to choose — LRGEX Compress always uses optimized defaults.

1. Right-click any file or folder
2. Click **LRGEX → Compress**
3. A progress window shows live speed and ETA
4. Done — `.zgx` archive appears next to your files

---

## Extraction

Right-click any `.zgx`, `.zip`, `.rar`, or `.7z` → **LRGEX → Extract** or **Extract Here**.

Extracts ZIP, RAR, 7z, and ZGX archives. You don't need WinRAR or 7-Zip installed.

---

## Features

- ✓ Zstandard compression — uses all CPU cores automatically
- ✓ Windows Explorer integration — right-click any file or folder
- ✓ Progress window with live speed and ETA
- ✓ Atomic archive creation — interrupted compress never leaves a broken file
- ✓ Split Compress — break large archives into parts (`.partNNN.zgx`)
- ✓ ZIP extraction
- ✓ RAR extraction
- ✓ 7z extraction (single + multi-volume)
- ✓ Empty folder preservation
- ✓ Overwrite confirmation before replacing existing files
- ✓ Instant cancel (compress + extract, all formats)
- ✓ Encrypted archive support — password prompt for RAR, 7z, and ZIP (CLI: `-p <password>`)
- ✓ Integrity checksum — `.zgx` archives verified on extract via XXHash64
- ✓ Clean archive names (`video.mp4` → `video.zgx`)
- ✓ Ed25519 cryptographically signed auto-updates
- ✓ GPL-3.0-or-later License
- ✓ No administrator required
- ✓ Command-line support

---

## What makes it different

- **Speed first.** Built on Zstandard, optimized for raw speed over slightly smaller archives. Big folders finish in seconds.
- **Atomic writes.** If your PC crashes or loses power mid-compress, you never get a
  broken half-written archive. The file only appears when it's 100% complete.
- **Empty folders preserved.** Some archivers drop empty directories silently.
  LRGEX Compress keeps your exact folder structure.
- **Metadata preserved exactly.** File contents, modification/creation timestamps
  (whole-second precision), Windows attributes (hidden, read-only, system), and
  symbolic links all round-trip identically. NTFS alternate data streams are not
  supported (rarely used; not planned).
- **Instant cancel.** Click Cancel during compress OR extract — it stops immediately
  (even mid-file for RAR/7z) and cleans up. No orphaned files, no waiting.
- **Resilient extraction.** If one file is locked or unreadable, extraction continues
  and the file is listed in a skipped-files report — not a total failure.
- **Signed auto-updates.** Every update is cryptographically verified using Ed25519
  before installation.
- **Per-user install.** No admin prompt. No UAC. Works on locked-down corporate machines.

---

## Installation

1. Download the latest `LRGEX-Compress-setup.exe` from [Releases](https://github.com/LRGEX/Compress/releases)
2. Double-click — installs with no admin prompt
3. Right-click any file or folder → **LRGEX → Compress**

Or install via Chocolatey:
```
choco install lrgex-compress
```

Or winget (pending):
```
winget install LRGEX.Compress
```

---

## Command line

```bash
lrgex-compress <folder-or-file>                 # compress -> <name>.zgx
lrgex-compress --split [--size <MB>] <folder>   # split compress -> .partNNN.zgx
lrgex-compress -x <archive>                     # extract  -> <name>\ folder
lrgex-compress -x -h <archive>                  # extract here (into the archive's folder)
lrgex-compress --help                           # show usage
```

## Requirements

- Windows 10/11
- ~20 MB free disk space

> **MSVCP140.dll missing?** Older versions (≤1.4.1) required the Visual C++ runtime.
> Install it from Microsoft: https://aka.ms/vc14/vc_redist.x64.exe
> (Version 1.4.4+ uses a static CRT — no runtime needed.)

## `.zgx` file format

A `.zgx` file is a `tar.zst` archive (tar stream compressed with zstd) with a small
LRGEX header for identification.

**New format (v1.4.1+):**
```
bytes 0-4    : ASCII "LRGEX" (magic header)
byte  5      : format version (0x01)
bytes 6-13   : uncompressed total size (u64 little-endian, for progress accuracy)
bytes 14+    : zstd-compressed tar stream
```

**Legacy format (pre-v1.4.1):**
```
bytes 0-7    : uncompressed total size (u64 little-endian)
bytes 8+     : zstd-compressed tar stream
```

The extractor detects both formats automatically. Legacy archives continue to extract
without any conversion.

---

## Dependencies

This project vendors one crate with a patch:

| Crate | Version | Patch | Why |
|---|---|---|---|
| `vendor/libz-ng-sys` | 1.1.29 | CMake static CRT defines (/MT + NDEBUG) | Self-contained exe (no VC++ redist needed) |

RAR extraction uses [`unrar-rs`](https://crates.io/crates/unrar-rs) 0.4.0 (pure Rust, no vendored C).

**Note:** `unrar-rs` is licensed GPL-3.0-or-later with the UnRAR source-code restriction.
Review compatibility with your distribution model.

The previous vendored C `unrar` crate (0.5.8 with UCM_PROCESSDATA patch) was fully removed.

---

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).

RAR extraction: [`unrar-rs`](https://crates.io/crates/unrar-rs) — GPL-3.0-or-later + UnRAR restriction.

> UnRAR source code may be used in any software to handle RAR archives without
> limitations free of charge, but cannot be used to develop RAR (WinRAR) compatible
> archiver and to re-create RAR compression algorithm, which is proprietary.
> Distribution of modified UnRAR source code in separate form or as a part of other
> software is permitted, provided that full text of this paragraph, starting from
> "UnRAR source code" words, is included in license, or in documentation if license
> is not available, and in source code comments of resulting package.

UI built with [Slint](https://slint.dev) (Slint Royalty-free License).
Third-party crates under their respective licenses (Apache-2.0 / BSD-3 / GPL-3.0).
