<div align="center">
  <img src="https://download.lrgex.com/Dark%20Full%20Logo.png" alt="LRGEX Logo" width="300">
  <br><br>
  <img src="assets/logo.png" alt="Compress Logo" width="80">

<div id="toc">
  <ul style="list-style: none">
    <summary>
      <h1>Compress</h1>
    </summary>
  </ul>
</div>

**Version 1.3.0**

</div>

---

## Compress 145,000 files before WinRAR finishes 30%.

That's not marketing — that's a real test. Same folder, same machine, same files.
LRGEX Compress finished while WinRAR was still showing 29%.

**How?** Zstandard (zstd). It's a newer compression algorithm designed for speed.
WinRAR and 7-Zip use older algorithms (RAR, LZMA2) that compress smaller but take
5-10× longer. LRGEX Compress trades a slightly larger file size for dramatically
faster speed — and for most people, time matters more than a few extra megabytes.

| | LRGEX Compress | WinRAR | 7-Zip |
|---|---|---|---|
| **Speed** | Fastest | Moderate | Slow |
| **Algorithm** | zstd (modern) | RAR (proprietary) | LZMA2 |
| **File size** | Slightly larger | Smaller | Smallest |
| **UI** | Right-click, done | Full app | Full app |
| **Settings** | None | Many | Many |
| **Price** | Free | Trial nagware | Free |

---

## Right-click. Done.

No app to open. No drag-and-drop. No settings dialog with 15 compression levels.

1. Right-click any file or folder
2. Click **LRGEX → Compress**
3. A progress window shows live speed and ETA
4. Done — `.zgx` archive appears next to your files

That's it. No levels to choose. No options to argue with. The fastest proven settings,
every time.

---

## Extraction

Right-click any `.zgx`, `.zip`, or `.rar` → **LRGEX → Extract** or **Extract Here**.

Yes — it extracts ZIP and RAR too. You don't need WinRAR or 7-Zip installed.

---

## What makes it different

- **No settings.** No compression level slider. No dictionary size. No "solid archive"
  checkbox. Just compress and extract.
- **Atomic writes.** If your PC crashes or loses power mid-compress, you never get a
  broken half-written archive. The file only appears when it's 100% complete.
- **Empty folders preserved.** Some archivers drop empty directories silently.
  LRGEX Compress keeps your exact folder structure — including empty folders.
- **Cancel is clean.** Click Cancel, the partial archive is deleted. No orphaned files.
- **Signed auto-updates.** Every update is Ed25519-verified. A hacked server can't push
  malware — the app refuses anything without your valid signature.
- **Per-user install.** No admin prompt. No UAC. Works on locked-down corporate machines.

---

## Installation

1. Download `LRGEX-Compress-v1.3-setup.exe` from [Releases](https://github.com/LRGEX/Compress/releases)
2. Double-click — installs with no admin prompt
3. Right-click any file or folder → **LRGEX → Compress**

Or install via winget:
```
winget install LRGEX.Compress
```

To uninstall: Settings → Add/Remove Programs → LRGEX Compress. Removes everything.

---

## Command line

```bash
lrgex-compress <folder-or-file>      # compress -> <name>.zgx
lrgex-compress -x <archive>.zgx      # extract  -> <name>\ folder
lrgex-compress -x -h <archive>.zgx   # extract here (into current folder)
```

## Requirements

- Windows 10/11
- ~20 MB free disk space

## License

MIT License. See [LICENSE](LICENSE).
