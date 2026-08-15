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

**Version 1.7.2**

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

## Benchmark

Independent comparison against 7-Zip, WinRAR, and Windows ZIP — fastest legitimate compression preset for each competitor, zero configuration for LRGEX.

**5 datasets · 4 compressors · 100 runs · 0 integrity failures**

### Compression speed (median, MB/s)

| Dataset | LRGEX | 7-Zip | WinRAR | Windows ZIP |
|---|---:|---:|---:|---:|
| Large files (3.6 GB) | **497** | 135 | 83 | 33 |
| Mixed content (1.4 GB) | **314** | 130 | 90 | 32 |
| Already compressed (1 GB) | **257** | 122 | 81 | 32 |
| Highly compressible (270 MB) | 100 | **275** | 133 | 68 |
| Many small files (10K) | 19 | **38** | 7 | 0.4 |

### Overall: LRGEX **235.6 MB/s** · 7-Zip **138 MB/s** · WinRAR **78.9 MB/s** · Windows ZIP **32.6 MB/s**

**LRGEX is 1.7× faster than 7-Zip overall** — fastest in 3 of 5 real-world categories.

### Where LRGEX loses (honest results)
- **7-Zip wins** on highly compressible data (275 vs 100 MB/s) and many-small-files (38 vs 19 MB/s)
- **7-Zip extracts faster** (1.6×–6.9× depending on dataset)
- **WinRAR compresses denser** on mixed content (ratio 1.163 vs 1.005) — at ~3.5× slower speed

<details>
<summary>Full methodology + per-dataset tables</summary>

- Each application/dataset combination ran **5 times**; **median** published
- All apps used their **fastest legitimate compression preset** — no Store/copy mode
- LRGEX: zero configuration (as shipped) · 7-Zip: `-mx=1` · WinRAR: `-m1 -r` · Windows ZIP: `Compress-Archive Fastest`
- Integrity verified: file count + size match on extraction, 100/100 PASS

**Benchmark performed on Windows** using LRGEX Compress, 7-Zip, WinRAR and Windows ZIP. Five runs per configuration; median results shown. No store/no-compression mode. All archives were extracted and verified.

#### Already Compressed (1 GB)

| Application | Mode | Archive Size | Comp Time (s) | Speed (MB/s) | Extract (s) | Ratio | Integrity |
|---|---|---:|---:|---:|---:|---:|---|
| LRGEX | Zero configuration | 1040 MB | 4.04 | 257.2 | 3.07 | 1 | PASS |
| 7-Zip | Fastest | 1040.1 MB | 8.52 | 122 | 1.08 | 1 | PASS |
| WinRAR | Fastest | 1040 MB | 12.79 | 81.3 | 1.06 | 1 | PASS |
| Windows ZIP | Fastest | 1040.3 MB | 32.18 | 32.3 | 1.86 | 1 | PASS |

#### Highly Compressible (270 MB)

| Application | Mode | Archive Size | Comp Time (s) | Speed (MB/s) | Extract (s) | Ratio | Integrity |
|---|---|---:|---:|---:|---:|---:|---|
| LRGEX | Zero configuration | 64.9 MB | 2.79 | 100.3 | 2.69 | 4.317 | PASS |
| 7-Zip | Fastest | 63.5 MB | 1.02 | 275.1 | 0.39 | 4.411 | PASS |
| WinRAR | Fastest | 64.1 MB | 2.11 | 132.6 | 0.95 | 4.368 | PASS |
| Windows ZIP | Fastest | 72.2 MB | 4.15 | 67.5 | 2.29 | 3.88 | PASS |

#### Large Files (3.6 GB)

| Application | Mode | Archive Size | Comp Time (s) | Speed (MB/s) | Extract (s) | Ratio | Integrity |
|---|---|---:|---:|---:|---:|---:|---|
| LRGEX | Zero configuration | 3672.1 MB | 7.38 | 497.4 | 5.48 | 1 | PASS |
| 7-Zip | Fastest | 3672.2 MB | 27.24 | 134.8 | 3.42 | 1 | PASS |
| WinRAR | Fastest | 3672 MB | 44.03 | 83.4 | 3.14 | 1 | PASS |
| Windows ZIP | Fastest | 3673.1 MB | 111.36 | 33 | 3.96 | 1 | PASS |

#### Many Small Files (10,000 files)

| Application | Mode | Archive Size | Comp Time (s) | Speed (MB/s) | Extract (s) | Ratio | Integrity |
|---|---|---:|---:|---:|---:|---:|---|
| LRGEX | Zero configuration | 54.3 MB | 2.79 | 19.4 | 14.34 | 0.994 | PASS |
| 7-Zip | Fastest | 54.1 MB | 1.41 | 38.4 | 7.48 | 0.998 | PASS |
| WinRAR | Fastest | 55.2 MB | 7.87 | 6.9 | 17.32 | 0.979 | PASS |
| Windows ZIP | Fastest | 55.5 MB | 153.86 | 0.4 | 127.86 | 0.974 | PASS |

#### Mixed Content (1.4 GB)

| Application | Mode | Archive Size | Comp Time (s) | Speed (MB/s) | Extract (s) | Ratio | Integrity |
|---|---|---:|---:|---:|---:|---:|---|
| LRGEX | Zero configuration | 1390.5 MB | 4.46 | 313.8 | 3.68 | 1.005 | PASS |
| 7-Zip | Fastest | 1391 MB | 10.75 | 130 | 1.43 | 1.005 | PASS |
| WinRAR | Fastest | 1202.4 MB | 15.56 | 89.9 | 1.46 | 1.163 | PASS |
| Windows ZIP | Fastest | 1392.6 MB | 44.44 | 31.5 | 3.22 | 1.004 | PASS |

</details>

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
