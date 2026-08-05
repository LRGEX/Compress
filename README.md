<div align="center">
  <img src="https://download.lrgex.com/Dark%20Full%20Logo.png" alt="LRGEX Logo" width="300">

  # Compress

  **Version 0.1.0**

</div>

## Description

Blazing-fast file and folder compression for Windows — WinRAR-style, right-click simple.
LRGEX Compress does one thing: right-click any file or folder, choose **LRGEX → Compress**,
and a `.zgx` archive appears next to the original. No settings, no compression-level
choices, no options to argue with — just the fastest proven compression, every time.

Archives use the `.zgx` format (internally `tar` + `zstd`). Extraction is the reverse:
right-click a `.zgx` → **LRGEX → Extract**, and the original tree comes back, empty
directories included.

## Features

- **Right-click cascade** — one `LRGEX` entry that opens a Compress / Extract submenu on
  hover (folders, files, and `.zgx` archives)
- **`.zgx` archive format** — internally `tar` + `zstd` (branded extension, like `.rar`)
- **WinRAR-style progress window** — colored bar, percent, MB/s, ETA, and a Cancel button
- **Byte-based progress** — 500 ms heartbeat, spinner, rate, and ETA; the UI never freezes
- **Genuinely fast** — multi-threaded zstd + parallel reads (all CPU cores), designed for
  multi-gigabyte folders
- **Atomic writes** — `name.zgx.part` is renamed to `name.zgx` only on full success, so an
  interrupted or crashed compress can never leave a half-written archive
- **Empty-directory preservation** — extraction restores the exact source tree, including
  empty folders
- **Cancel cleans up** — aborting a compress deletes the partial archive, every time
- **Skip-with-warning** — one locked file does not fail a multi-GB job; the archive still
  publishes and a `name.zgx.skipped.txt` sidecar lists anything that could not be read
- **Signed auto-updates** — Ed25519 signature verification on every update; a compromised
  server cannot push a malicious build
- **Per-user install** — no UAC prompt, no admin rights required
- **Self-contained exe** — ~14 MB, zero runtime dependencies, no WebView, no Node

## Installation

1. Download `LRGEX-Compress-v0.1-setup.exe`.
2. Double-click it — the app installs per-user to `%LOCALAPPDATA%\Programs\LRGEX Compress`
   with no UAC prompt.
3. The installer registers the right-click menu and the `.zgx` file association
   automatically.
4. Right-click any file or folder → **LRGEX → Compress**.

To uninstall: Add/Remove Programs → LRGEX Compress. The uninstaller removes every
registry entry it created — nothing is left behind.

## Usage

### Compress

1. In File Explorer, right-click any file or folder.
2. Select **LRGEX → Compress**.
3. A progress window opens showing percent, speed, and time remaining.
4. When it finishes, `<name>.zgx` appears next to the original.

### Extract

1. Right-click any `.zgx` archive.
2. Select **LRGEX → Extract**.
3. The archive is extracted to a folder named after it, in the same location.

### Cancel

Click **Cancel** during a compress to abort. The partial archive is deleted — no orphaned
half-files are left behind.

### Command line

```bash
lrgex-compress <folder-or-file>      # compress -> <name>.zgx
lrgex-compress -x <archive>.zgx      # extract  -> <name>\ folder
```

## Requirements

- Windows 10/11
- ~20 MB free disk space

## License

MIT License. See [LICENSE](LICENSE).

## Contributing

This project is developed internally at LRGEX. Source layout, build, and deployment
follow the LRGEX Rust Guidelines. To report issues or request features, open an issue on
the project repository.
