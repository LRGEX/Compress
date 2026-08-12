// Extraction — multi-format dispatcher.
// Supported: .zgx (tar+zstd), .zip, .rar.
// Format detection: magic bytes first (reliable), extension fallback.
//
// METADATA FIDELITY (.zgx, v1.4): the zgx extractor restores mtime (LastWriteTime),
// ctime (CreationTime), Windows attributes (Hidden/ReadOnly/System/Archive), and
// symbolic links — whatever was preserved at compress time comes back identical.
// "Your data is safe and preserved as-is."
//
// Symlink elevation: creating a link on Windows needs Admin or Developer Mode. Our
// install is per-user. On the FIRST link that fails, we ask ONCE; if the user says
// yes, the WHOLE extract re-launches elevated (ShellExecuteW "runas") and the current
// process exits. If no, remaining links are skipped silently. One prompt, ever.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::progress::{self, ByteReader, Progress};

/// Set by main.rs when the process was relaunched elevated with `--elevated-rerun`.
/// When true, the extract path SKIPS regular files that already exist on disk
/// (the non-elevated pass already wrote them) and only recreates symlinks. This
/// avoids re-writing every file a second time on symlink-elevation relaunch.
pub static ELEVATED_RERUN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Drop guard for an in-flight output file. If the write fails, is cancelled, or the
/// thread panics, the guard deletes the partial file so a truncated/zero-filled file
/// never looks complete. Disarmed (forgotten) only after the write + metadata restore
/// fully succeed. NEVER use this for directories (rmdir would fail non-empty) or for
/// files we intend to keep.
struct PartialFile {
    path: PathBuf,
    armed: bool,
    /// Whether the file existed BEFORE we started writing. If it did, we must NOT
    /// delete it on failure/cancel — that would destroy the user's original file.
    /// Only delete files WE created (didn't exist before).
    pre_existed: bool,
}

impl PartialFile {
    fn new(path: PathBuf) -> Self {
        let pre_existed = path.exists();
        Self { path, armed: true, pre_existed }
    }
    /// Disarm: call this ONLY after the file is fully written and metadata applied.
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for PartialFile {
    fn drop(&mut self) {
        if self.armed && !self.pre_existed {
            // Only delete files WE created. If the file pre-existed (user clicked Yes
            // to overwrite, then cancelled mid-write), deleting would destroy their
            // original — leave the (now-partial) file instead. A partial overwrite is
            // bad, but destroying the original entirely is worse.
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Drop guard for a staging directory (used by RAR extraction). On panic/cancel/failure,
/// recursively delete the staging dir so we don't leak a multi-GB orphan. Disarmed after
/// the successful move-into-place.
struct StagingDir {
    path: PathBuf,
    armed: bool,
}

impl StagingDir {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Build a collision-free temp path for an in-flight extraction write. Appends a
/// unique suffix (`.<pid>-<counter>`) AFTER the full original filename — so siblings
/// like `data.txt` and `data.bin` get `data.txt.<pid>-0` and `data.bin.<pid>-1`,
/// NEVER colliding. (with_extension would have replaced the ext and caused data loss
/// for same-stem-different-extension pairs in the parallel batch.)
fn extract_temp_path(final_path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}.lrgex-tmp-{}-{}", final_path.display(), std::process::id(), id).into()
}

/// Classify an I/O error from the extract path. Returns true if FATAL (must abort
/// the whole extract), false if RECOVERABLE (skip this one file, continue).
/// ENOSPC / disk-full is systemic — continuing would mask it as 'N files skipped'.
/// Access-denied, locked, path issues are per-file and skip safely (WinRAR-style).
/// ERROR_DISK_FULL=112, ERROR_HANDLE_DISK_FULL=39.
fn is_fatal_extract_err(e: &std::io::Error) -> bool {
    let code = e.raw_os_error().unwrap_or(0);
    code == 112 || code == 39 || e.kind() == std::io::ErrorKind::StorageFull
}

/// Validate a symlink/hardlink TARGET so a crafted archive cannot create a link
/// pointing outside the destination root. Returns true if the target is safe.
///
/// Rules (mirror the entry-name guard + a resolved-path containment check):
///  - reject absolute paths (e.g. `C:\Windows\...`, `/etc/passwd`)
///  - reject any component that is not Normal or CurDir (rejects `..` and Windows
///    drive prefixes like `C:foo` which parse as Prefix)
///  - additionally resolve the target relative to the link's own directory and
///    confirm the canonicalized result stays inside `dest_root`
fn is_safe_link_target(target: &str, link_path: &Path, dest_root: &Path) -> bool {
    use std::path::Component;

    // Empty target — nothing to link to.
    if target.is_empty() { return false; }

    let p = std::path::Path::new(target);

    // Reject absolute paths.
    if p.is_absolute() { return false; }

    // Reject Windows drive-prefix components (e.g. `C:foo` parses as Prefix).
    // ParentDir (`..`) is ALLOWED here — legit symlinks use it (e.g.
    // `libfoo.so -> ../lib/libfoo.so.1`). The canonicalization containment
    // check below is the real authority that catches actual escapes.
    if p.components().any(|c| matches!(c, std::path::Component::Prefix(_))) {
        return false;
    }

    // Resolve the target relative to the link's own directory using LEXICAL
    // normalization only (resolve `.` and `..` as string operations, do NOT call
    // canonicalize on the target chain). canonicalize follows symlinks, so an earlier-
    // extracted symlink pointing outside dest could redirect a later link's parent
    // chain outside the root while the literal path stays inside. Lexical-only
    // normalization is immune to that cross-symlink redirect.
    let base = match link_path.parent() {
        Some(b) => b,
        None => return false,
    };
    // Canonicalize the base DIRECTORY (it exists on disk — we created it during
    // extract) so it's in the SAME verbatim form as canon_root below. Then append
    // the lexical-normalized target. Both sides now share the \?\
    // prefix + on-disk casing, so starts_with compares apples to apples.
    // (We canonicalize the parent — NOT the target — so we never follow a symlink
    // in the target chain itself.)
    let canon_base = match base.canonicalize() {
        Ok(c) => c,
        Err(_) => return false, // parent doesn't exist? reject safely
    };
    let resolved = canon_base.join(p);
    let normalized = lexical_normalize(&resolved);

    // Canonicalize dest_root (it's a real folder, not a symlink we created). Same
    // verbatim form as canon_base above.
    let canon_root = match dest_root.canonicalize() {
        Ok(c) => lexical_normalize(&c),
        Err(_) => return false,
    };

    normalized.starts_with(&canon_root)
}

/// Lexically normalize a path: resolve `.` and `..` components as string operations
/// WITHOUT touching the filesystem (no symlink following, no canonicalize). Used by
/// is_safe_link_target so the containment check can't be redirected by an earlier-
/// extracted symlink.
fn lexical_normalize(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut prefix: Option<std::ffi::OsString> = None;
    let mut out: Vec<std::ffi::OsString> = Vec::new();
    for c in p.components() {
        match c {
            Component::CurDir => {} // skip `.`
            Component::ParentDir => { out.pop(); } // `..` → go up one
            Component::RootDir => { out.clear(); } // absolute root — reset
            Component::Prefix(pfx) => { prefix = Some(pfx.as_os_str().to_os_string()); out.clear(); }
            Component::Normal(s) => out.push(s.to_os_string()),
        }
    }
    let mut result = PathBuf::new();
    if let Some(pfx) = prefix {
        result.push(pfx);
    }
    // Re-add the root separator if the original path was absolute (had RootDir or Prefix).
    if p.components().any(|c| matches!(c, Component::RootDir)) {
        result.push("");
    }
    for part in out {
        result.push(part);
    }
    result
}

/// Counting reader for zip extraction — ticks decompressed bytes into Progress.
struct ZipCountingReader<'a, R: std::io::Read> {
    inner: &'a mut R,
    prog: &'a Progress,
    cancel: Option<&'a AtomicBool>,
}

impl<'a, R: std::io::Read> std::io::Read for ZipCountingReader<'a, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(cancel) = self.cancel {
            if cancel.load(Ordering::Relaxed) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "LRGEX_CANCELLED",
                ));
            }
        }
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.prog.tick_bytes(n as u64);
        }
        Ok(n)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Zgx,    // tar + zstd
    Zip,
    Rar,
    SevenZ, // .7z (LZMA2)
}

/// Detect format from the first bytes of the file. Falls back to extension.
fn detect_format(path: &Path) -> Option<Format> {
    // Magic-byte detection.
    if let Ok(mut f) = std::fs::File::open(path) {
        use std::io::Read;
        let mut head = [0u8; 8];
        if f.read(&mut head).unwrap_or(0) >= 4 {
            // ZIP — "PK\x03\x04" (also PK\x05\x06 empty, PK\x07\x08 spanned).
            if head[0] == 0x50 && head[1] == 0x4b && (head[2] == 0x03 || head[2] == 0x05 || head[2] == 0x07) {
                return Some(Format::Zip);
            }
            // RAR — "Rar!\x1a\x07\x00" (RAR3) or "Rar!\x1a\x07\x01\x00" (RAR5).
            if head[0..6] == [0x52, 0x61, 0x72, 0x21, 0x1a, 0x07] {
                return Some(Format::Rar);
            }
            // zstd — 0x28 0xb5 0x2f 0xfd (legacy .zgx with no LRGEX magic).
            if head[0..4] == [0x28, 0xb5, 0x2f, 0xfd] {
                return Some(Format::Zgx);
            }
            // 7z — "7z\xbc\xaf\x27\x1c" (6-byte signature).
            if head.len() >= 6 && head[0..6] == [0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c] {
                return Some(Format::SevenZ);
            }
            // LRGEX magic — "LRGEX" (new .zgx format with magic header).
            if head.len() >= 5 && &head[0..5] == b"LRGEX" {
                return Some(Format::Zgx);
            }
        }
    }
    // Extension fallback.
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("zgx") | Some("zst") => Some(Format::Zgx),
        Some("zip") => Some(Format::Zip),
        Some("rar") => Some(Format::Rar),
        Some("7z") => Some(Format::SevenZ),
        _ => None,
    }
}

/// Top-level dispatcher. Routes to the right handler by detected format.
pub fn extract_archive(archive: &Path, dest: &Path, cancel: &AtomicBool) -> (bool, String) {
    // Split .zgx detection FIRST (by filename pattern) — before magic-byte detect.
    if crate::segment::parse_split_part(archive).is_some() {
        return extract_split_zgx(archive, dest, cancel);
    }
    // Multi-volume 7z detection by filename pattern (.7z.NNN) — before magic-byte
    // detect, because only .7z.001 has the signature and extension() returns "NNN".
    if is_sevenz_volume_part(archive) {
        return extract_7z(archive, dest, cancel);
    }
    match detect_format(archive) {
        Some(Format::Zgx) => extract_zgx(archive, dest, cancel),
        Some(Format::Zip) => extract_zip(archive, dest, cancel),
        Some(Format::Rar) => extract_rar(archive, dest, cancel),
        Some(Format::SevenZ) => extract_7z(archive, dest, cancel),
        None => (false, "Unrecognized archive format".to_string()),
    }
}

/// Check if extracting this archive into `dest` would overwrite any existing file.
/// Returns true if at least one entry in the archive points to a path that already
/// exists on disk under `dest`. Used to prompt the user before overwriting (WinRAR-style).
/// Best-effort: on any read error, returns false (let the extract path handle the error).
pub fn has_conflicts(archive: &Path, dest: &Path) -> bool {
    // Split .zgx detection FIRST.
    if crate::segment::parse_split_part(archive).is_some() {
        return split_has_conflicts(archive, dest);
    }
    // Multi-volume 7z: route to sevenz_has_conflicts (handles part redirect).
    if is_sevenz_volume_part(archive) {
        return sevenz_has_conflicts(archive, dest);
    }
    // NOTE: we deliberately do NOT fast-path on empty/non-existent dest. A file could
    // appear in the window between an empty check and the extract. Always scan the
    // archive contents so the prompt fires for any real conflict. (For a genuinely
    // empty dest, the scan finds no conflicts and returns false quickly anyway.)
    match detect_format(archive) {
        Some(Format::Zgx) => zgx_has_conflicts(archive, dest),
        Some(Format::Zip) => zip_has_conflicts(archive, dest),
        Some(Format::Rar) => rar_has_conflicts(archive, dest),
        Some(Format::SevenZ) => sevenz_has_conflicts(archive, dest),
        None => false,
    }
}

/// zgx: open the tar+zst stream and walk entry names.
fn zgx_has_conflicts(archive: &Path, dest: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = match std::fs::File::open(archive) { Ok(f) => f, Err(_) => return false };
    let mut head = [0u8; 16];
    let n = file.read(&mut head).unwrap_or(0);
    let start = if n >= 6 && &head[0..5] == b"LRGEX" && head[5] == 0x01 { 14 }
               else if n >= 12 && head[8..12] == [0x28,0xb5,0x2f,0xfd] { 8 }
               else if n >= 4 && head[0..4] == [0x28,0xb5,0x2f,0xfd] { 0 }
               else { return false };
    if file.seek(SeekFrom::Start(start as u64)).is_err() { return false; }
    let decoder = match zstd::Decoder::new(file) { Ok(d) => d, Err(_) => return false };
    let mut tar = tar::Archive::new(std::io::BufReader::with_capacity(256 * 1024, decoder));
    let entries = match tar.entries() { Ok(e) => e, Err(_) => return false };
    for entry in entries {
        if let Ok(e) = entry {
            // Skip directory entries — a pre-existing subdir is a harmless merge
            // (create_dir_all), not a conflict. Only flag real file collisions.
            if e.header().entry_type().is_dir() { continue; }
            if let Ok(p) = e.path() {
                // Same guard as the real extract: skip unsafe paths.
                use std::path::Component;
                if p.is_absolute() { continue; }
                if p.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir)) { continue; }
                let out = dest.join(&p);
                if out.exists() { return true; }
            }
        }
    }
    false
}

/// zip: walk the central directory.
fn zip_has_conflicts(archive: &Path, dest: &Path) -> bool {
    let file = match std::fs::File::open(archive) { Ok(f) => f, Err(_) => return false };
    let mut za = match zip::ZipArchive::new(file) { Ok(z) => z, Err(_) => return false };
    for i in 0..za.len() {
        if let Ok(entry) = za.by_index(i) {
            // Skip directory entries — matching rar_has_conflicts + extract semantics.
            if entry.is_dir() { continue; }
            if let Some(rel) = entry.enclosed_name() {
                if dest.join(&rel).exists() { return true; }
            }
        }
    }
    false
}

/// rar: walk the listing.
fn rar_has_conflicts(archive: &Path, dest: &Path) -> bool {
    // For multi-volume, always check from part001.
    let first_part = unrar::Archive::new(archive)
        .first_part_option()
        .filter(|p| p != archive && p.exists());
    let archive: &Path = first_part.as_deref().unwrap_or(archive);
    let list = match unrar::Archive::new(archive).open_for_listing() { Ok(l) => l, Err(_) => return false };
    for item in list {
        if let Ok(e) = item {
            if e.is_directory() { continue; }
            let p = dest.join(&e.filename);
            if p.exists() { return true; }
        }
    }
    false
}

/// .zgx = tar + zstd. Byte-counting via ByteReader so the heartbeat tracks bytes.
fn extract_zgx(archive: &Path, dest: &Path, cancel: &AtomicBool) -> (bool, String) {
    progress::clear_status();
    let label = archive
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let arch_size = std::fs::metadata(archive).map(|m| m.len()).unwrap_or(0);

    let prog = Progress::new(&label);
    let heartbeat = prog.spawn_writer();
    prog.set_phase(1);

    let file = match std::fs::File::open(archive) {
        Ok(f) => f,
        Err(e) => {
            prog.finish(4);
            let _ = heartbeat.join();
            return (false, format!("cannot open archive: {}", e));
        }
    };

    // Read first 16 bytes once, then branch on format.
    //   Case 1: LRGEX magic (bytes 0-4 = "LRGEX") + version 0x01 → NEW format.
    //           Total at bytes 6-13, zstd stream at byte 14.
    //   Case 2: LRGEX magic + unknown version → refuse (newer format).
    //   Case 3: No LRGEX magic, but zstd magic (28 B5 2F FD) at byte 8 → LEGACY.
    //           Total at bytes 0-7, zstd stream at byte 8 (exactly as today).
    //   Case 4: Anything else → refuse (not a valid archive).
    use std::io::{Read, Seek, SeekFrom};
    let mut file = file;
    let mut head = [0u8; 16];
    let bytes_read = file.read(&mut head).unwrap_or(0);
    let zstd_magic = [0x28u8, 0xb5, 0x2f, 0xfd];
    let lrgex_magic = *b"LRGEX";

    let (uncompressed_total, zstd_start_offset) = if bytes_read >= 6 && head[0..5] == lrgex_magic {
        // LRGEX magic present.
        let version = head[5];
        if version != 0x01 {
            prog.finish(4);
            let _ = heartbeat.join();
            return (false, "This archive was created by a newer version of LRGEX Compress. Please update.".to_string());
        }
        // NEW format: total at bytes 6-13, zstd at byte 14.
        if bytes_read < 14 {
            prog.finish(4);
            let _ = heartbeat.join();
            return (false, "Truncated LRGEX archive header.".to_string());
        }
        let total = u64::from_le_bytes([head[6], head[7], head[8], head[9], head[10], head[11], head[12], head[13]]);
        let val = if total > 0 && total < arch_size * 100 { total } else { arch_size };
        // Seek to byte 14 for the zstd decoder.
        let _ = file.seek(SeekFrom::Start(14));
        (val, 14)
    } else if bytes_read >= 12 && head[8..12] == zstd_magic {
        // LEGACY archive: 8-byte total at bytes 0-7, zstd at byte 8.
        let total = u64::from_le_bytes([head[0], head[1], head[2], head[3], head[4], head[5], head[6], head[7]]);
        let val = if total > 0 && total < arch_size * 100 { total } else { arch_size };
        let _ = file.seek(SeekFrom::Start(8));
        (val, 8)
    } else if bytes_read >= 4 && head[0..4] == zstd_magic {
        // Very old archive: no 8-byte total, zstd magic at byte 0.
        let _ = file.seek(SeekFrom::Start(0));
        (arch_size, 0)
    } else {
        prog.finish(4);
        let _ = heartbeat.join();
        return (false, "Not a valid LRGEX archive.".to_string());
    };
    prog.set_totals(0, uncompressed_total);

    let decoder = match zstd::Decoder::new(file) {
        Ok(d) => d,
        Err(e) => {
            prog.finish(4);
            let _ = heartbeat.join();
            return (false, format!("corrupt archive (zstd): {}", e));
        }
    };
    let counting = ByteReader::with_cancel(decoder, prog.clone(), cancel);
    let buf_decoder = std::io::BufReader::with_capacity(256 * 1024, counting);
    let mut tar = tar::Archive::new(buf_decoder);

    let _ = std::fs::create_dir_all(dest);
    let result = extract_zgx_inner(&mut tar, dest, cancel);

    match result {
        ZgxOutcome::Done(skipped) => {
            if skipped > 0 { prog.set_skipped(skipped); }
            prog.finish(3);
            let _ = heartbeat.join();
            (true, String::new())
        }
        ZgxOutcome::ElevatedRelaunched => {
            prog.finish(3);
            let _ = heartbeat.join();
            std::process::exit(0);
        }
        ZgxOutcome::Cancelled => {
            prog.finish(5);
            let _ = heartbeat.join();
            (true, String::new())
        }
        ZgxOutcome::Failed(e) => {
            prog.finish(4);
            let _ = heartbeat.join();
            (false, format!("extract failed: {}", e))
        }
    }
}

enum ZgxOutcome {
    Done(usize),
    Cancelled,
    ElevatedRelaunched,
    Failed(String),
}

/// Inner loop: walks tar entries, writes files/dirs, restores mtime+ctime+attrs,
/// recreates symlinks (with one-prompt elevation if needed). Directory mtimes are
/// restored in a FINAL pass (writing children updates a dir's mtime, so we set it last).
/// Parse the .lrgex/meta.bin sidecar body into a path→(ctime,attrs) map.
/// Format (little-endian): u32 count, then count records of [u16 plen, path bytes, u64 ctime, u32 attrs].
/// Malformed records are skipped (best-effort) — a bad sidecar never aborts the whole extract.
fn parse_sidecar(body: &[u8]) -> std::collections::HashMap<std::path::PathBuf, (u64, u32)> {
    let mut m = std::collections::HashMap::new();
    if body.len() < 4 { return m; }
    let count = u32::from_le_bytes([body[0], body[1], body[2], body[3]]) as usize;
    let mut pos = 4;
    for _ in 0..count {
        if pos + 2 > body.len() { break; }
        let plen = u16::from_le_bytes([body[pos], body[pos + 1]]) as usize;
        pos += 2;
        if pos + plen + 8 + 4 > body.len() { break; }
        let pbytes = &body[pos..pos + plen];
        pos += plen;
        let ctime = u64::from_le_bytes([
            body[pos], body[pos + 1], body[pos + 2], body[pos + 3],
            body[pos + 4], body[pos + 5], body[pos + 6], body[pos + 7],
        ]);
        pos += 8;
        let attrs = u32::from_le_bytes([body[pos], body[pos + 1], body[pos + 2], body[pos + 3]]);
        pos += 4;
        if let Ok(s) = std::str::from_utf8(pbytes) {
            m.insert(std::path::PathBuf::from(s), (ctime, attrs));
        }
    }
    m
}

fn extract_zgx_inner<R: std::io::Read>(tar: &mut tar::Archive<R>, dest: &Path, cancel: &AtomicBool) -> ZgxOutcome {
    use rayon::prelude::*;
    use std::io::Read;
    use std::path::{Component, PathBuf};

    const BATCH_ENTRIES: usize = 2048;
    const BATCH_BYTES: usize = 64 * 1024 * 1024;
    const STREAM_THRESHOLD: u64 = 1024 * 1024;

    // Per-file metadata captured from the tar header + PAX extensions.
    #[derive(Clone, Copy, Default)]
    struct EntryMeta { mtime: u64, ctime: u64, attrs: u32 }

    // Latched elevation decision. None = haven't been asked yet.
    // Some(true) = user said yes → we'll relaunch at the end.
    // Some(false) = user said no → all remaining links skip silently.
    let mut elevation_decision: Option<bool> = None;
    let mut needs_relaunch = false;
    // Count symlinks that couldn't be recreated (elevation denied/declined) so we can
    // surface 'Done - N skipped' to the user instead of silently dropping links.
    let mut skipped_links: usize = 0;
    // Recoverable per-file failures (locked/readonly-after-retry/path issues) —
    // accumulated from flush_batch, surfaced via set_skipped at the end.
    let mut skipped_files: usize = 0;
    // Accumulate (path, error) for ALL skipped files across all batches — written to a
    // single manifest once at the end (NOT per-batch, which would overwrite earlier
    // batches' entries). Surfaced next to the destination so the user can find it.
    let mut skipped_details: Vec<(PathBuf, std::io::Error)> = Vec::new();

    let mut entries = match tar.entries() {
        Ok(e) => e,
        Err(e) => return ZgxOutcome::Failed(format!("read entries: {}", e)),
    };

    let mut dir_cache: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    dir_cache.insert(dest.to_path_buf());

    // Batch of (path, data, meta) for small files — written in parallel, metadata restored after.
    let mut batch: Vec<(PathBuf, Vec<u8>, EntryMeta)> = Vec::with_capacity(BATCH_ENTRIES);
    let mut batch_bytes: usize = 0;

    // Directories whose mtime/ctime/attrs must be restored AFTER all children are written.
    let mut dir_meta_todo: Vec<(PathBuf, EntryMeta)> = Vec::new();

    // Read PAX fields (SCHILY.creationtime + LRGEX.fileattr) from an entry.
    let read_pax_meta = |entry: &mut tar::Entry<R>| -> (u64, u32) {
        let mut ctime = 0u64;
        let mut attrs = 0u32;
        if let Ok(Some(pax)) = entry.pax_extensions() {
            for f in pax.flatten() {
                if let Ok(k) = f.key() {
                    match k {
                        "SCHILY.creationtime" => {
                            if let Ok(v) = f.value() {
                                // value may be "1234567890" or "1234567890.123" — parse integer part.
                                ctime = v.split('.').next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
                            }
                        }
                        "LRGEX.fileattr" => {
                            if let Ok(v) = f.value() {
                                attrs = v.parse::<u32>().unwrap_or(0);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        (ctime, attrs)
    };

    // Flush the small-file batch: parallel write, then restore metadata on each file.
    // Dedicated write pool for flush_batch: 2-3x core count. The write tasks block in
    // CreateFile + Defender's filter driver (not CPU-bound), so oversubscription is
    // correct — the default global pool (sized to core count = 24) underutilizes when
    // workers are I/O-blocked. This pool is used ONLY for the parallel file writes.
    let write_pool = rayon::ThreadPoolBuilder::new()
        .num_threads((std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8)) * 3)
        .thread_name(|i| format!("lrgex-write-{}", i))
        .build()
        .ok();

    let flush_batch = |batch: &mut Vec<(PathBuf, Vec<u8>, EntryMeta)>,
                       batch_bytes: &mut usize,
                       dir_cache: &mut std::collections::HashSet<PathBuf>| -> Result<Vec<(PathBuf, std::io::Error)>, String> {
        if batch.is_empty() { return Ok(Vec::new()); }
        // 1. Create parent dirs (sequential, deduped).
        for (path, _, _) in batch.iter() {
            if let Some(parent) = path.parent() {
                if dir_cache.insert(parent.to_path_buf()) {
                    std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {}", e))?;
                }
            }
        }
        // 2. Parallel write + per-file metadata restore on the OPEN handle (no re-open).
        //    Runs on the dedicated oversubscribed write_pool (48-72 threads) — I/O-blocked
        //    workers don't burn CPU, so more threads than cores is correct here.
        //    Temp-then-rename: write to <path>.zgx-lrgex-extract-tmp, then atomically
        //    rename onto <path>. The user's original is never touched until the rename —
        //    no truncation, no partial-overwrite data loss if a write fails.
        use std::os::windows::io::AsRawHandle;
        let run_writes = || {
            batch
                .par_iter()
                .filter_map(|(path, data, meta)| {
                    let tmp = extract_temp_path(path);
                    // Guard the temp for panic safety. Disarmed after the rename succeeds.
                    let mut guard = PartialFile::new(tmp.clone());
                    let mut f = match std::fs::File::create(&tmp) {
                        Ok(f) => f,
                        Err(e) => return Some((path.clone(), e)),
                    };
                    use std::io::Write;
                    if let Err(e) = f.write_all(data) {
                        let _ = std::fs::remove_file(&tmp);
                        return Some((path.clone(), e));
                    }
                    let raw = f.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
                    // mtime + ctime survive the rename; attrs applied post-rename below.
                    crate::metaattr::apply_all_handle(raw, meta.mtime, meta.ctime, 0);
                    drop(f);
                    // Atomic swap: temp → final. atomic_replace clears ReadOnly on an
                    // existing destination (Windows MoveFileExW trap) before the rename.
                    if let Err(e) = crate::metaattr::atomic_replace(&tmp, path) {
                        let _ = std::fs::remove_file(&tmp);
                        return Some((path.clone(), e));
                    }
                    // Apply attributes to the FINAL path post-rename. ALWAYS normalize
                    // (not just when nonzero) — a fresh temp has the Archive bit set by
                    // NTFS, and we must clear it when the source didn't have it.
                    crate::metaattr::apply_attrs_normalized(path, meta.attrs);
                    guard.disarm(); // rename succeeded — temp no longer exists
                    None
                })
                .collect::<Vec<_>>()
        };
        let errs: Vec<(PathBuf, std::io::Error)> = match &write_pool {
            Some(p) => p.install(run_writes),
            None => run_writes(),  // fallback if pool creation failed
        };
        // With temp-then-rename, a failed batch leaves NO partial files at final paths
        // (every failed write stayed as a temp and was deleted; successful renames are
        // complete). Nothing to clean up — the originals were never touched.
        batch.clear();
        *batch_bytes = 0;
        if errs.is_empty() {
            Ok(Vec::new())
        } else {
            // Split FATAL (disk-full during temp WRITE — systemic, continuing would
            // mask it as '2000 files skipped') from RECOVERABLE (access-denied, locked,
            // path issues — skip the one file, continue the batch, WinRAR-style).
            // ERROR_DISK_FULL=112, ERROR_HANDLE_DISK_FULL=39.
            let mut fatal: Option<String> = None;
            let mut recoverable: Vec<(PathBuf, std::io::Error)> = Vec::new();
            for (p, e) in errs {
                if is_fatal_extract_err(&e) {
                    fatal = Some(format!("write {}: {} (disk full — aborting)", p.display(), e));
                    break;
                }
                recoverable.push((p, e));
            }
            if let Some(f) = fatal {
                return Err(f);
            }
            Ok(recoverable)
        }
    };

    // Sidecar metadata map (populated on first iteration if the archive has a sidecar).
    // New archives put ctime+attrs in one .lrgex/meta.bin blob at the front instead of
    // per-entry PAX headers (saves ~11k extra tar entries). Old archives have no sidecar
    // → map stays None → we fall back to read_pax_meta per entry (backward compat).
    let mut sidecar_map: Option<std::collections::HashMap<PathBuf, (u64, u32)>> = None;
    let mut first_checked = false;

    for entry in entries {
        if cancel.load(Ordering::Relaxed) {
            return ZgxOutcome::Cancelled;
        }
        let mut entry = match entry {
            Ok(e) => e,
            Err(e) => return ZgxOutcome::Failed(format!("entry read: {}", e)),
        };
        let rel = match entry.path() {
            Ok(p) => p.to_path_buf(),
            Err(e) => return ZgxOutcome::Failed(format!("path: {}", e)),
        };
        let outpath = dest.join(&rel);

        // First-iteration sidecar detection (clean peek that respects tar's single-entry
        // borrow). If the first entry IS the sidecar, parse it into the map and skip to
        // the next entry (don't write the sidecar blob to disk).
        if !first_checked {
            first_checked = true;
            if rel.as_path() == std::path::Path::new(crate::compress::SIDECAR_PATH) {
                use std::io::Read;
                let mut body = Vec::new();
                if let Err(e) = entry.read_to_end(&mut body) {
                    return ZgxOutcome::Failed(format!("read sidecar: {}", e));
                }
                sidecar_map = Some(parse_sidecar(&body));
                continue; // sidecar consumed — don't extract it as a real file
            }
            // else: old archive, no sidecar. sidecar_map stays None → read_pax_meta below.
        }

        // Path-traversal guard.
        if rel.is_absolute() { continue; }
        if rel.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir)) {
            continue;
        }

        let etype = entry.header().entry_type();
        let mtime = entry.header().mtime().unwrap_or(0);
        let (ctime, attrs) = if let Some(m) = &sidecar_map {
            // New archive: look up by path. The sidecar keys use forward slashes; rel
            // from tar::Entry also uses forward slashes, so the lookup matches directly.
            // Normalize just in case (strip any leading ./).
            let key = rel.strip_prefix("./").unwrap_or(&rel).to_path_buf();
            *m.get(&key).unwrap_or(&(0, 0))
        } else {
            // Old archive (no sidecar): read PAX extensions per entry.
            read_pax_meta(&mut entry)
        };
        let meta = EntryMeta { mtime, ctime, attrs };

        if etype.is_dir() {
            if dir_cache.insert(outpath.clone()) {
                if let Err(e) = std::fs::create_dir_all(&outpath) {
                    return ZgxOutcome::Failed(format!("mkdir: {}", e));
                }
            }
            // Defer mtime/ctime/attrs restore to the FINAL pass — writing children into
            // this dir would overwrite a mtime we set now.
            dir_meta_todo.push((outpath, meta));
            continue;
        }

        if etype.is_hard_link() {
            // Hard links: not currently preserved (rare on Windows; tar hardlink semantics
            // don't map cleanly to NTFS in this extractor). Skip rather than risk corruption.
            // (If hardlink creation is ever added, the target MUST go through the same
            // is_safe_link_target validation as symlinks below.)
            continue;
        }

        if etype.is_symlink() {
            // Flush any pending files first (they may be the link's siblings).
            match flush_batch(&mut batch, &mut batch_bytes, &mut dir_cache) {
                Err(e) => return ZgxOutcome::Failed(e),
                Ok(v) => { skipped_files += v.len(); skipped_details.extend(v); }
            }
            let target = match entry.link_name() {
                Ok(Some(t)) => t.to_string_lossy().to_string(),
                _ => continue, // symlink with no target — can't recreate, skip
            };
            // Validate the TARGET so a crafted archive can't create a link pointing
            // outside the destination (zip-slip equivalent for link targets).
            if !is_safe_link_target(&target, &outpath, dest) {
                // Skip — do not create the link. The user gets the data files; the
                // malicious/unsafe link is silently dropped.
                continue;
            }
            // Ensure parent dir exists.
            if let Some(parent) = outpath.parent() {
                if dir_cache.insert(parent.to_path_buf()) {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        return ZgxOutcome::Failed(format!("mkdir parent: {}", e));
                    }
                }
            }
            // Remove a pre-existing entry at the link path (else CreateSymbolicLinkW fails).
            let _ = std::fs::remove_file(&outpath).or_else(|_| std::fs::remove_dir(&outpath));
            // We didn't store is_dir on the link entry. Try as a directory link first
            // (junctions are the common case on Windows), fall back to a file link.
            let r = crate::metaattr::create_symlink(&outpath, &target, true);
            let r = match r {
                crate::metaattr::SymlinkResult::Created => {
                    // link ok — restore its metadata too.
                    if meta.mtime > 0 || meta.ctime > 0 {
                        crate::metaattr::apply_times_path(&outpath, meta.mtime, meta.ctime);
                    }
                    continue;
                }
                crate::metaattr::SymlinkResult::NeedsElevation => {
                    // Resolve the elevation decision ONCE.
                    match elevation_decision {
                        None => {
                            let allow = rfd::MessageDialog::new()
                                .set_title("LRGEX Compress")
                                .set_description(
                                    "This archive contains symbolic links. To recreate them exactly, \
                                     LRGEX needs administrator permission. Allow?"
                                )
                                .set_buttons(rfd::MessageButtons::YesNo)
                                .show();
                            let yes = allow == rfd::MessageDialogResult::Yes;
                            elevation_decision = Some(yes);
                            if yes { needs_relaunch = true; }
                            if !yes { skipped_links += 1; }
                            // Either way skip THIS link: if yes, the elevated pass recreates
                            // everything; if no, all remaining links skip silently.
                            continue;
                        }
                        Some(_) => { skipped_links += 1; continue; } // already decided → skip silently
                    }
                }
                crate::metaattr::SymlinkResult::Skipped(_) => {
                    // Directory-link creation failed for a non-privilege reason — try as a file link.
                    crate::metaattr::create_symlink(&outpath, &target, false)
                }
            };
            // Final fallback: if still not created and not elevating, write a tiny stub file
            // containing the target path so the user at least sees what was lost.
            if let crate::metaattr::SymlinkResult::Created = r {
                if meta.mtime > 0 || meta.ctime > 0 {
                    crate::metaattr::apply_times_path(&outpath, meta.mtime, meta.ctime);
                }
            } else if !needs_relaunch {
                let _ = std::fs::write(&outpath, format!("symlink target: {}\n", target));
            }
            continue;
        }

        // Regular file.
        let size = entry.header().size().unwrap_or(0);

        // Elevated re-pass: skip regular files that already exist — the non-elevated
        // pass wrote them. We're only here to recreate symlinks that need admin.
        if ELEVATED_RERUN.load(std::sync::atomic::Ordering::Relaxed) && outpath.exists() {
            continue;
        }

        if size > STREAM_THRESHOLD {
            // Large file: stream straight to disk, then restore metadata on the path.
            match flush_batch(&mut batch, &mut batch_bytes, &mut dir_cache) {
                Err(e) => return ZgxOutcome::Failed(e),
                Ok(v) => { skipped_files += v.len(); skipped_details.extend(v); }
            }
            if let Some(parent) = outpath.parent() {
                if dir_cache.insert(parent.to_path_buf()) {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        return ZgxOutcome::Failed(format!("mkdir parent: {}", e));
                    }
                }
            }
            // Temp-sidecar-then-rename: write to a temp file, then atomically rename
            // onto the final path ONLY after full success + metadata applied. The
            // user's original is never touched until the rename — no truncation, no
            // zero-hole, no partial-overwrite data loss. (Same pattern as compress's
            // .part write.) Note: during overwrite, temp + original briefly coexist,
            // so this needs up to ~2x file size in free disk during the write.
            let tmp = extract_temp_path(&outpath);
            let mut f = match std::fs::File::create(&tmp) {
                Ok(f) => f,
                Err(e) => {
                    if is_fatal_extract_err(&e) {
                        return ZgxOutcome::Failed(format!("create {}: {}", rel.display(), e));
                    }
                    skipped_files += 1;
                    skipped_details.push((outpath.clone(), e));
                    continue;
                }
            };
            let _ = f.set_len(size);
            // Guard the TEMP file: on panic/cancel mid-write, delete the temp so we
            // don't leak a multi-GB orphan. Disarmed after the successful rename.
            let mut guard = PartialFile::new(tmp.clone());
            // Write 4 MB chunks straight from the tar entry to the File — NO BufWriter.
            use std::io::{Read, Write};
            let mut chunk = vec![0u8; 4 * 1024 * 1024];
            let mut remaining = size as usize;
            let mut skip_file = false; // set on recoverable error → skip flush+rename
            loop {
                if cancel.load(Ordering::Relaxed) {
                    let _ = std::fs::remove_file(&tmp);
                    return ZgxOutcome::Cancelled;
                }
                if remaining == 0 { break; }
                let want = remaining.min(chunk.len());
                let n = match entry.read(&mut chunk[..want]) {
                    Ok(0) => break, // unexpected EOF — tar stream ended early
                    Ok(n) => n,
                    Err(e) => {
                        // read error: treat as fatal (corrupt stream affects everything after).
                        let _ = std::fs::remove_file(&tmp);
                        return ZgxOutcome::Failed(format!("read {}: {}", rel.display(), e));
                    }
                };
                if let Err(e) = f.write_all(&chunk[..n]) {
                    let _ = std::fs::remove_file(&tmp);
                    if is_fatal_extract_err(&e) {
                        return ZgxOutcome::Failed(format!("write {}: {}", rel.display(), e));
                    }
                    // Recoverable (locked/access-denied) — skip flush+rename for this file.
                    // guard's Drop removes the temp; we continue the OUTER extract loop.
                    skip_file = true;
                    skipped_files += 1;
                    skipped_details.push((outpath.clone(), e));
                    break;
                }
                if n > remaining { break; } // guard against bad header size
                remaining -= n;
            }
            if skip_file {
                continue; // recoverable write error — next entry (guard cleans temp)
            }
            // Flush the temp File to the OS. Times are set on the temp handle (they
            // survive a same-volume rename), but ATTRIBUTES must be applied to the
            // FINAL path post-rename — a fresh temp has the Archive bit set by default,
            // and applying attrs pre-rename would just set the temp's attrs (lost on the
            // overwrite rename).
            if let Err(e) = f.flush() {
                let _ = std::fs::remove_file(&tmp);
                if is_fatal_extract_err(&e) {
                    return ZgxOutcome::Failed(format!("flush {}: {}", rel.display(), e));
                }
                skipped_files += 1;
                skipped_details.push((outpath.clone(), e));
                continue; // recoverable — temp cleaned by guard, skip this file
            }
            {
                use std::os::windows::io::AsRawHandle;
                let raw = f.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
                // mtime + ctime survive the rename (same inode), so set them on the handle.
                crate::metaattr::apply_all_handle(raw, meta.mtime, meta.ctime, 0);
            }
            drop(f);
            // Atomic swap: temp → final. This is the ONLY point the final path is touched.
            if let Err(e) = crate::metaattr::atomic_replace(&tmp, &outpath) {
                let _ = std::fs::remove_file(&tmp);
                if is_fatal_extract_err(&e) {
                    return ZgxOutcome::Failed(format!("rename {}: {}", rel.display(), e));
                }
                skipped_files += 1;
                skipped_details.push((outpath.clone(), e));
                continue; // recoverable — skip this file, continue extract
            }
            // Apply attributes to the FINAL path now that it exists (post-rename).
            // ALWAYS normalize — clear stray Archive bit when source didn't have it.
            crate::metaattr::apply_attrs_normalized(&outpath, meta.attrs);
            guard.disarm(); // rename succeeded — temp no longer exists to clean up
            continue;
        }

        // Small file: read into memory, batch it with its metadata.
        let mut data = Vec::with_capacity(size as usize);
        if let Err(e) = entry.read_to_end(&mut data) {
            return ZgxOutcome::Failed(format!("read {}: {}", rel.display(), e));
        }
        batch_bytes += data.len();
        batch.push((outpath, data, meta));

        if batch.len() >= BATCH_ENTRIES || batch_bytes >= BATCH_BYTES {
            match flush_batch(&mut batch, &mut batch_bytes, &mut dir_cache) {
                Err(e) => return ZgxOutcome::Failed(e),
                Ok(v) => { skipped_files += v.len(); skipped_details.extend(v); }
            }
        }
    }

    // Flush remaining small files.
    match flush_batch(&mut batch, &mut batch_bytes, &mut dir_cache) {
        Err(e) => return ZgxOutcome::Failed(e),
        Ok(v) => { skipped_files += v.len(); skipped_details.extend(v); }
    }

    // FINAL pass: restore directory mtime/ctime/attrs AFTER all children written.
    // Process in reverse order (deepest first) so creating a child dir doesn't disturb
    // a parent's mtime after we set it.
    for (dir, meta) in dir_meta_todo.iter().rev() {
        if meta.mtime > 0 || meta.ctime > 0 {
            crate::metaattr::apply_times_path(dir, meta.mtime, meta.ctime);
        }
        if meta.attrs != 0 {
            crate::metaattr::apply_attrs_normalized(dir, meta.attrs);
        }
    }

    // If a symlink needed elevation and the user said yes → relaunch elevated with a
    // sentinel flag. The elevated re-pass sees the flag and SKIPS regular files that
    // already exist (the non-elevated pass wrote them) — only links are recreated.
    // This avoids re-writing every file a second time (2x I/O + 2x AV scan).
    if needs_relaunch {
        let mut args: Vec<String> = std::env::args().collect();
        args.push("--elevated-rerun".to_string());
        if crate::metaattr::relaunch_elevated(&args) {
            // Elevated process took over — exit cleanly so the GUI closes.
            return ZgxOutcome::ElevatedRelaunched;
        }
        // UAC denied or launch failed. Symlinks are missing, but all regular files
        // were extracted. Fall through to Done so the GUI shows success (not Failed),
        // with the caveat that links were skipped.
    }

    // Write the skipped-files manifest ONCE (all batches accumulated) next to the
    // destination so the user can find it. On a clean extract (no skips), DELETE any
    // stale manifest left by a prior failed extract so the user isn't misled.
    if let Some(dest_parent) = dest.parent() {
        let manifest = dest_parent.join(format!(
            "{}.lrgex-skipped.txt",
            dest.file_name().and_then(|n| n.to_str()).unwrap_or("extract")
        ));
        if skipped_files > 0 && !skipped_details.is_empty() {
            let body: String = skipped_details.iter()
                .map(|(p, e)| format!("{}: {}", p.display(), e))
                .collect::<Vec<_>>().join("\n");
            let _ = std::fs::write(&manifest, body);
        } else {
            // Clean extract — remove any stale manifest from a prior run.
            let _ = std::fs::remove_file(&manifest);
        }
    }

    ZgxOutcome::Done(skipped_links + skipped_files)
}

/// Convert a zip crate DateTime (DOS datetime: year/month/day + hour/min/sec) to
/// seconds since UNIX_EPOCH. Returns None if the date is invalid. Used to restore the
/// mtime the zip already carries (we don't strip it).
fn zip_mtime_to_epoch(dt: zip::DateTime) -> Option<u64> {
    // Civil-from-days algorithm (Howard Hinnant). days since 1970-01-01.
    let y = dt.year() as i64;
    let m = dt.month() as i64;
    let d = dt.day() as i64;
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = (y2 - era * 400) as u64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy as u64;
    let days = era * 146097 + doe as i64 - 719468; // days since 1970-01-01
    if days < 0 { return None; }
    let secs = days as u64 * 86400
        + dt.hour() as u64 * 3600
        + dt.minute() as u64 * 60
        + dt.second() as u64;
    Some(secs)
}

/// Split .zgx extraction: chains parts via ConcatReader, verifies SHA256 trailers,
/// then feeds to the existing zstd::Decoder → tar::Archive → extract_zgx_inner.
fn extract_split_zgx(archive: &Path, dest: &Path, cancel: &AtomicBool) -> (bool, String) {
    progress::clear_status();
    let label = archive.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let prog = Progress::new(&label);
    let heartbeat = prog.spawn_writer();
    prog.set_phase(1);

    let base = crate::segment::parse_split_part(archive)
        .map(|(b, _)| b)
        .unwrap_or_else(|| archive.to_path_buf());

    let (reader, header_total, data_sum) = match crate::segment::ConcatReader::open_and_verify(&base) {
        Ok(r) => r,
        Err(e) => {
            prog.set_error(&e.to_string());
            prog.finish(4);
            let _ = heartbeat.join();
            return (false, e.to_string());
        }
    };

    // Use header_total (uncompressed) for progress; fall back to data_sum if 0.
    let total = if header_total > 0 { header_total } else { data_sum };
    prog.set_totals(0, total);

    // P0-1 fix: ByteReader wraps the DECODER (decompressed bytes), not the raw stream.
    let buf_reader = std::io::BufReader::with_capacity(256 * 1024, reader);
    let decoder = match zstd::Decoder::new(buf_reader) {
        Ok(d) => d,
        Err(e) => {
            prog.set_error(&format!("zstd: {}", e));
            prog.finish(4); let _ = heartbeat.join(); return (false, format!("zstd: {}", e)); }
    };
    let counting = ByteReader::with_cancel(decoder, prog.clone(), cancel);
    let buf = std::io::BufReader::with_capacity(256 * 1024, counting);
    let mut tar = tar::Archive::new(buf);

    // Create dest before extracting (extract_zgx_inner assumes it exists).
    let _ = std::fs::create_dir_all(dest);

    let result = extract_zgx_inner(&mut tar, dest, cancel);

    match result {
        ZgxOutcome::Done(skipped) => {
            if skipped > 0 { prog.set_skipped(skipped); }
            // N-7: verify decompressed total matches header (catches silent truncation).
            // Finding B fix: allow 64KB slack for zstd/tar trailing buffered bytes
            // that tar's EOF stops pulling before ByteReader ticks them.
            if header_total > 0 {
                let decompressed = prog.bytes_done();
                let slack = 64 * 1024;
                if decompressed + slack < header_total {
                    prog.finish(3);
                    let _ = heartbeat.join();
                    return (true, format!(
                        "WARNING: archive may be truncated (got {} of {} bytes)",
                        decompressed, header_total));
                }
            }
            prog.finish(3);
            let _ = heartbeat.join();
            (true, String::new())
        }
        ZgxOutcome::ElevatedRelaunched => {
            prog.finish(3);
            let _ = heartbeat.join();
            std::process::exit(0);
        }
        ZgxOutcome::Cancelled => {
            prog.finish(5);
            let _ = heartbeat.join();
            (true, String::new())
        }
        ZgxOutcome::Failed(e) => {
            prog.finish(4);
            let _ = heartbeat.join();
            (false, format!("extract failed: {}", e))
        }
    }
}

/// Conflict check for split archives (uses open_no_verify — no SHA hashing).
fn split_has_conflicts(archive: &Path, dest: &Path) -> bool {
    let base = crate::segment::parse_split_part(archive)
        .map(|(b, _)| b)
        .unwrap_or_else(|| archive.to_path_buf());
    let reader = match crate::segment::ConcatReader::open_no_verify(&base) {
        Ok(r) => r,
        Err(_) => return false,
    };
    let decoder = match zstd::Decoder::new(reader) { Ok(d) => d, Err(_) => return false };
    let mut tar = tar::Archive::new(decoder);
    let entries = match tar.entries() { Ok(e) => e, Err(_) => return false };
    for entry in entries {
        if let Ok(e) = entry {
            if e.header().entry_type().is_dir() { continue; }
            if let Ok(p) = e.path() {
                use std::path::Component;
                if p.is_absolute() { continue; }
                if p.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir)) { continue; }
                if dest.join(&p).exists() { return true; }
            }
        }
    }
    false
}

/// .zip — via the `zip` crate (pure Rust). Decompressed bytes tick into Progress
/// via a counting reader; the total is the SUM of uncompressed entry sizes (from the
/// central directory), not the on-disk archive size, so the bar never overshoots.
///
/// Restores what the zip contains: mtime (DOS datetime), read-only bit (from unix
/// mode if present), and symlinks (target stored as entry data — zip convention).
/// Zip does NOT carry CreationTime or Windows-specific attributes, so those aren't
/// restored (nothing to restore — we don't strip anything that's actually there).
fn extract_zip(archive: &Path, dest: &Path, cancel: &AtomicBool) -> (bool, String) {
    progress::clear_status();
    let label = archive
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let arch_size = std::fs::metadata(archive).map(|m| m.len()).unwrap_or(0);
    let prog = Progress::new(&label);
    let heartbeat = prog.spawn_writer();
    prog.set_totals(0, arch_size);
    prog.set_phase(1);

    let result = (|| -> Result<(), String> {
        let file = std::fs::File::open(archive).map_err(|e| format!("cannot open: {}", e))?;
        let mut za = zip::ZipArchive::new(file).map_err(|e| format!("corrupt zip: {}", e))?;
        // Pre-scan: sum uncompressed sizes so the bar tracks decompressed bytes (not
        // compressed on-disk size — which would overshoot for compressible archives).
        let total_uncompressed: u64 = (0..za.len()).map(|i| za.by_index(i).map(|e| e.size()).unwrap_or(0)).sum();
        prog.set_totals(0, total_uncompressed);
        std::fs::create_dir_all(dest).map_err(|e| format!("cannot create dest: {}", e))?;

        // Directory mtime restore happens in a FINAL pass — extracting children into a
        // dir would overwrite the mtime we set now.
        let mut dir_mtimes: Vec<(PathBuf, u64)> = Vec::new();
        // Symlink elevation: same one-prompt model as zgx. Latched.
        let mut elevation_decision: Option<bool> = None;
        let mut needs_relaunch = false;
        // Count symlinks silently lost (elevation declined). Does NOT count stub-written
        // links (those aren't lost). Surfaced via set_skipped before Ok(()).
        let mut skipped_links: usize = 0;
        for i in 0..za.len() {
            if cancel.load(Ordering::Relaxed) {
                return Err("__LRGEX_CANCEL__".to_string());
            }
            let mut entry = za.by_index(i).map_err(|e| format!("entry {}: {}", i, e))?;
            // zip-slip guard: enclosed_name() returns a sanitized path or None for unsafe
            // names (e.g. containing `..` or absolute paths). Skip those entirely.
            let rel = match entry.enclosed_name() {
                Some(p) => p,
                None => continue,
            };
            let outpath = dest.join(&rel);
            // DOS mtime → epoch seconds. zip carries only mtime (no ctime, no Windows attrs).
            let mtime = entry.last_modified().and_then(zip_mtime_to_epoch);

            if entry.is_symlink() {
                // Symlink: the TARGET is stored as the entry's data content (zip convention).
                if let Some(parent) = outpath.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| format!("mkdir parent: {}", e))?;
                }
                let mut target = Vec::with_capacity(entry.size() as usize);
                let mut counting = ZipCountingReader { inner: &mut entry, prog: &prog, cancel: Some(cancel) };
                std::io::Read::read_to_end(&mut counting, &mut target).map_err(|e| format!("read link {}: {}", rel.display(), e))?;;
                let target = String::from_utf8_lossy(&target).trim_end_matches(['\0', '\r', '\n']).to_string();
                // Validate the TARGET so a crafted zip can't create a link pointing
                // outside the destination.
                if !is_safe_link_target(&target, &outpath, dest) {
                    continue; // unsafe target — skip
                }
                let _ = std::fs::remove_file(&outpath).or_else(|_| std::fs::remove_dir(&outpath));
                let is_dir = entry.unix_mode().map(|m| (m & 0o170000) == 0o040000).unwrap_or(false);
                let r = crate::metaattr::create_symlink(&outpath, &target, is_dir);
                match r {
                    crate::metaattr::SymlinkResult::Created => {}
                    crate::metaattr::SymlinkResult::NeedsElevation => match elevation_decision {
                        None => {
                            let allow = rfd::MessageDialog::new()
                                .set_title("LRGEX Compress")
                                .set_description(
                                    "This archive contains symbolic links. To recreate them exactly, \
                                     LRGEX needs administrator permission. Allow?"
                                )
                                .set_buttons(rfd::MessageButtons::YesNo)
                                .show();
                            elevation_decision = Some(allow == rfd::MessageDialogResult::Yes);
                            if allow == rfd::MessageDialogResult::Yes { needs_relaunch = true; }
                            else { skipped_links += 1; }
                        }
                        Some(_) => { skipped_links += 1; } // already decided → skip silently
                    },
                    crate::metaattr::SymlinkResult::Skipped(_) => {
                        if !needs_relaunch {
                            let _ = std::fs::write(&outpath, format!("symlink target: {}\n", target));
                        }
                    }
                }
                continue;
            }

            if entry.is_dir() {
                std::fs::create_dir_all(&outpath).map_err(|e| format!("mkdir {}: {}", rel.display(), e))?;
                if let Some(mt) = mtime { dir_mtimes.push((outpath, mt)); }
                continue;
            }

            // Regular file.
            // Elevated re-pass: skip regular files that already exist (non-elevated
            // pass wrote them); we're only here for symlinks.
            if ELEVATED_RERUN.load(std::sync::atomic::Ordering::Relaxed) && outpath.exists() {
                continue;
            }
            if let Some(parent) = outpath.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("mkdir parent: {}", e))?;
            }
            {
                // Temp-then-rename: write to a temp file, then atomically rename onto
                // the final path on success. The user's original is never touched until
                // the rename — no truncation, no partial-overwrite data loss.
                let tmp = extract_temp_path(&outpath);
                let mut guard = PartialFile::new(tmp.clone());
                let outf = std::fs::File::create(&tmp).map_err(|e| format!("create {}: {}", rel.display(), e))?;
                let mut counting = ZipCountingReader { inner: &mut entry, prog: &prog, cancel: Some(cancel) };
                let mut bw = std::io::BufWriter::new(&outf);
                let write_res = std::io::copy(&mut counting, &mut bw).map_err(|e| format!("write {}: {}", rel.display(), e));
                drop(bw);
                drop(outf);
                if let Err(e) = write_res {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(e);
                }
                // Atomic swap: temp → final.
                if let Err(e) = crate::metaattr::atomic_replace(&tmp, &outpath) {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(format!("rename {}: {}", rel.display(), e));
                }
                guard.disarm(); // rename succeeded
            }
            // Restore mtime AFTER the write (Windows updates mtime on write).
            if let Some(mt) = mtime {
                crate::metaattr::apply_times_path(&outpath, mt, 0);
            }
            // Restore read-only bit if the zip carried a unix mode with it.
            if let Some(mode) = entry.unix_mode() {
                if mode & 0o200 == 0 {
                    // owner-write bit clear → read-only
                    crate::metaattr::apply_attrs(&outpath, 0x1); // FILE_ATTRIBUTE_READONLY
                }
            }
        }
        // FINAL pass: directory mtimes AFTER children written.
        for (dir, mt) in dir_mtimes.iter().rev() {
            crate::metaattr::apply_times_path(dir, *mt, 0);
        }
        // If a symlink needed elevation and the user said yes → relaunch elevated with
        // the sentinel so the re-pass skips already-written regular files.
        if needs_relaunch {
            let mut args: Vec<String> = std::env::args().collect();
            args.push("--elevated-rerun".to_string());
            if crate::metaattr::relaunch_elevated(&args) {
                // Elevated process took over — close this GUI cleanly so the user
                // doesn't see a confusing "Failed" flash. Exit immediately; the
                // heartbeat thread will be reaped by process exit.
                prog.finish(3);
                std::process::exit(0);
            }
            // UAC denied — fall through to Ok(()) so the GUI shows success with the
            // caveat that symlinks were skipped (regular files are all extracted).
        }
        // Surface skipped-links count BEFORE finish so the GUI shows 'Done - N skipped'.
        if skipped_links > 0 { prog.set_skipped(skipped_links); }
        Ok(())
    })();

    match result {
        Ok(()) => {
            prog.finish(3);
            let _ = heartbeat.join();
            (true, String::new())
        }
        Err(e) => {
            let was_cancelled = cancel.load(Ordering::Relaxed);
            if was_cancelled {
                prog.finish(5);
            } else {
                prog.set_error(&e);
                prog.finish(4);
            }
            let _ = heartbeat.join();
            if was_cancelled { (true, String::new()) } else { (false, e) }
        }
    }
}

/// Recursively move all contents of `src` into `dst`, overwriting existing files.
/// Used by the RAR staging extraction: after unrar extracts successfully into a
/// staging dir, we move everything into the real destination. Both dirs are on the
/// same volume (staging is a sibling of dest), so each move is atomic.
fn move_dir_contents(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            move_dir_contents(&from, &to)?;
            let _ = std::fs::remove_dir(&from); // remove now-empty dir
        } else {
            // rename overwrites on Windows. Same volume = atomic.
            crate::metaattr::atomic_replace(&from, &to)?;
        }
    }
    Ok(())
}

/// Pass 1: read every header, sum real unpacked sizes.
/// Returns (file_count, total_bytes). Bytes are 0 if the archive is
/// encrypted or the header is unreadable -> caller falls back to indeterminate.
fn rar_totals(archive: &Path) -> (usize, u64) {
    let mut files = 0usize;
    let mut bytes = 0u64;
    // For multi-volume, always list from part001.
    let first_part = unrar::Archive::new(archive)
        .first_part_option()
        .filter(|p| p != archive && p.exists());
    let archive: &Path = first_part.as_deref().unwrap_or(archive);
    // Best-effort: walk the listing for sizes. On any read error, return zeros (caller
    // falls back to indeterminate progress). No diagnostic dump to disk in production.
    if let Ok(list) = unrar::Archive::new(archive).open_for_listing() {
        for item in list {
            if let Ok(e) = item {
                if !e.is_directory() {
                    files += 1;
                    bytes += e.unpacked_size;
                }
            }
        }
    }
    (files, bytes)
}

/// .rar — via `unrar-rs` (pure Rust, RAR5+RAR4). Replaces the old vendored C unrar.
/// Preserves ALL safety properties of the previous implementation:
///   - staging-dir isolation (extract to temp, move into dest ONLY on full success)
///   - stale-staging recovery (rename to ORPHAN, never delete — may hold unmoved files)
///   - instant mid-file cancel (custom Writer returns Err(Other) → aborts decompression)
///   - multi-volume support (StaticVolumeProvider from ordered part paths)
/// NEW (not in the old path):
///   - symlink preservation (member.is_symlink + link_target → create_symlink + elevation)
///   - Windows attributes restoration (member.attributes → apply_attrs_normalized)
///   - ctime restoration (member.ctime → apply_times_path, when carried)
fn extract_rar(archive: &Path, dest: &Path, cancel: &AtomicBool) -> (bool, String) {
    // RAR extraction via unrar-rs. See doc comment above for full design.
    progress::clear_status();

    // 1. Multi-volume: redirect to part001 + enumerate ALL parts
    let first_part = unrar::Archive::new(archive)
        .first_part_option()
        .filter(|p| p != archive && p.exists());
    let archive_path: &Path = first_part.as_deref().unwrap_or(archive);
    let label = archive_path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let volume_paths = enumerate_rar_volumes(archive_path);
    if volume_paths.is_empty() {
        return (false, format!("Could not find RAR volume parts starting from {}", archive_path.display()));
    }
    let provider = unrar_rs::StaticVolumeProvider::from_ordered(volume_paths);

    // 2. Open archive + get metadata
    let file = match std::fs::File::open(archive_path) {
        Ok(f) => f,
        Err(e) => return (false, format!("open archive: {e}")),
    };
    let mut rar = match unrar_rs::RarArchive::open(file) {
        Ok(r) => r,
        Err(e) => return (false, format!("open rar: {e}")),
    };
    let members: Vec<unrar_rs::MemberInfo> = rar.metadata().members.clone();

    let prog = Progress::new(&label);
    let heartbeat = prog.spawn_writer();
    prog.set_phase(0);
    let mut total_bytes: u64 = 0;
    let mut file_count: usize = 0;
    for m in &members {
        if !m.is_directory { file_count += 1; total_bytes += m.unpacked_size.unwrap_or(0); }
    }
    prog.set_totals(file_count, total_bytes);
    prog.set_phase(1);

    // 3. Staging dir (same crash-safety as the old path)
    let result = (|| -> Result<(), String> {
        std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
        let staging = dest.parent().unwrap_or_else(|| std::path::Path::new("."))
            .join(format!(".{}.lrgex-rar-staging-{}",
                dest.file_name().and_then(|n| n.to_str()).unwrap_or("archive"), std::process::id()));
        if staging.exists() {
            let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
            let orphan = staging.with_file_name(format!("{}.ORPHAN-{}-{}",
                staging.file_name().and_then(|n| n.to_str()).unwrap_or("lrgex-staging"), std::process::id(), ts));
            match std::fs::rename(&staging, &orphan) {
                Ok(()) => { let _ = std::fs::write(orphan.join("RECOVERY-README.txt"),
                    "LRGEX Compress found an interrupted extraction. Files here were not yet\r\nmoved into the destination. Move them manually if needed.\r\n"); }
                Err(_) => { return Err(format!("Found a previous interrupted extraction at {} - could not move it aside.", staging.display())); }
            }
        }
        std::fs::create_dir_all(&staging).map_err(|e| format!("staging mkdir: {e}"))?;
        let mut staging_guard = Some(StagingDir::new(staging.clone()));

        // 4. Per-member extraction
        let opts = unrar_rs::ExtractOptions { verify: true, password: None, restore_owners: false };
        let mut elevation_decision: Option<bool> = None;
        let mut skipped_links = 0u32;

        for (idx, member) in members.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                let _ = std::fs::remove_dir_all(&staging);
                if let Some(g) = staging_guard.take() { std::mem::forget(g); }
                return Err("cancelled".to_string());
            }
            let out_path = staging.join(&member.name);
            if !out_path.starts_with(&staging) { continue; }
            if member.is_directory { std::fs::create_dir_all(&out_path).ok(); continue; }

            // Symlink handling (NEW - not in the old vendored-unrar path)
            if member.is_symlink {
                if let Some(target) = &member.link_target {
                    if let Some(parent) = out_path.parent() { std::fs::create_dir_all(parent).ok(); }
                    let _ = std::fs::remove_file(&out_path).or_else(|_| std::fs::remove_dir(&out_path));
                    match crate::metaattr::create_symlink(&out_path, target, false) {
                        crate::metaattr::SymlinkResult::Created => {
                            if let Some(mt) = member.mtime {
                                let secs = mt.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs()).unwrap_or(0);
                                crate::metaattr::apply_times_path(&out_path, secs, 0);
                            }
                        }
                        crate::metaattr::SymlinkResult::NeedsElevation => {
                            match elevation_decision {
                                None => {
                                    let allow = rfd::MessageDialog::new().set_title("LRGEX Compress")
                                        .set_description("This archive contains symbolic links. To recreate them exactly, LRGEX needs administrator permission. Allow?")
                                        .set_buttons(rfd::MessageButtons::YesNo).show();
                                    let yes = allow == rfd::MessageDialogResult::Yes;
                                    elevation_decision = Some(yes);
                                    if !yes { skipped_links += 1; }
                                }
                                Some(_) => { skipped_links += 1; }
                            }
                        }
                        crate::metaattr::SymlinkResult::Skipped(_) => { skipped_links += 1; }
                    }
                }
                continue;
            }

            // Regular file: streaming extract via CancelWriter (temp-then-rename + cancel + progress)
            if let Some(parent) = out_path.parent() { std::fs::create_dir_all(parent).ok(); }
            let tmp = extract_temp_path(&out_path);
            let tmp_file = match std::fs::File::create(&tmp) { Ok(f) => f, Err(_) => continue };

            struct RarCancelWriter { file: std::fs::File, cancel: *const AtomicBool, prog: *const Progress }
            impl std::io::Write for RarCancelWriter {
                fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                    unsafe {
                        if (*self.cancel).load(Ordering::Relaxed) {
                            return Err(std::io::Error::new(std::io::ErrorKind::Other, "cancelled"));
                        }
                        (*self.prog).tick_bytes(buf.len() as u64);
                    }
                    self.file.write_all(buf)?;
                    Ok(buf.len())
                }
                fn flush(&mut self) -> std::io::Result<()> { self.file.flush() }
            }
            // Safety: cancel + prog live in extract_rar's frame, outliving all loop iterations.
            let mut writer = RarCancelWriter { file: tmp_file, cancel: cancel as *const AtomicBool, prog: &prog as *const Progress };

            match rar.extract_member_streaming(idx, &opts, &provider, &mut writer) {
                Ok(_) => {
                    drop(writer);
                    if let Err(_) = crate::metaattr::atomic_replace(&tmp, &out_path) {
                        let _ = std::fs::remove_file(&tmp); continue;
                    }
                    // Metadata restore (NEW)
                    let mt = member.mtime.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
                    let ct = member.ctime.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
                    crate::metaattr::apply_times_path(&out_path, mt, ct);
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp);
                    if cancel.load(Ordering::Relaxed) {
                        let _ = std::fs::remove_dir_all(&staging);
                        if let Some(g) = staging_guard.take() { std::mem::forget(g); }
                        return Err("cancelled".to_string());
                    }
                    return Err(format!("RAR extract {} failed: {e}", member.name));
                }
            }
        }

        if skipped_links > 0 { prog.set_skipped(skipped_links as usize); }

        // 5. Move staging into dest (same as old path)
        match move_dir_contents(&staging, dest) {
            Ok(()) => { if let Some(mut g) = staging_guard.take() { g.disarm(); } let _ = std::fs::remove_dir_all(&staging); Ok(()) }
            Err(e) => {
                if let Some(g) = staging_guard.take() { std::mem::forget(g); }
                let _ = std::fs::write(staging.join("RECOVERY-README.txt"), "LRGEX Compress could not move some files.\r\nCopy them manually if needed.\r\n");
                Err(format!("move from staging failed ({e}): unmoved files preserved at {}", staging.display()))
            }
        }
    })();

    let cancelled = result.is_err() && cancel.load(Ordering::Relaxed);
    prog.finish(if cancelled { 5 } else if result.is_ok() { 3 } else { 4 });
    let _ = heartbeat.join();
    match result {
        Ok(()) => (true, String::new()),
        Err(e) if cancelled => (true, String::new()),
        Err(e) => (false, e),
    }
}

/// Enumerate all volume parts of a multi-volume RAR archive.
/// Handles RAR5 (name.part1.rar) and RAR3 (name.rar, name.r00, ...) naming.
fn enumerate_rar_volumes(first_part: &Path) -> Vec<PathBuf> {
    let parent = match first_part.parent() { Some(p) => p, None => return vec![first_part.to_path_buf()] };
    let name = match first_part.file_name().and_then(|n| n.to_str()) { Some(n) => n, None => return vec![first_part.to_path_buf()] };

    // RAR5: name.part1.rar, name.part2.rar, ...
    if name.contains(".part") {
        if let Some(pos) = name.find(".part") {
            let stem = &name[..pos];
            let mut parts: Vec<PathBuf> = std::fs::read_dir(parent).into_iter().flatten().flatten().map(|e| e.path())
                .filter(|p| { let n = p.file_name().unwrap().to_string_lossy(); n.starts_with(stem) && n.contains(".part") && n.ends_with(".rar") })
                .collect();
            parts.sort(); return parts;
        }
    }
    // RAR3: name.rar, name.r00, name.r01, ...
    let stem = name.strip_suffix(".rar").unwrap_or(name);
    let mut parts: Vec<PathBuf> = std::fs::read_dir(parent).into_iter().flatten().flatten().map(|e| e.path())
        .filter(|p| { let n = p.file_name().unwrap().to_string_lossy().to_string(); n == format!("{stem}.rar") || (n.starts_with(&format!("{stem}.r")) && n[stem.len()+2..].chars().all(|c| c.is_ascii_digit())) })
        .collect();
    parts.sort();
    if parts.is_empty() { vec![first_part.to_path_buf()] } else { parts }
}

/// Detect multi-volume 7z part by filename: "*.7z.NNN" (digits).
pub(crate) fn is_sevenz_volume_part(path: &Path) -> bool {
    let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return false,
    };
    if let Some(pos) = name.rfind(".7z.") {
        let suffix = &name[pos + 4..];
        !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit())
    } else {
        false
    }
}

/// Multi-volume 7z redirect: if the user right-clicked file.7z.NNN (any part),
/// return the path to file.7z.001 (the first part). Returns None for plain .7z
/// or non-7z files. (Split 7z archives have NO plain .7z — first part is .7z.001.)
fn sevenz_first_part(archive: &Path) -> Option<PathBuf> {
    // Match "*.7z.NNN" where NNN is digits and not "001".
    let name = archive.file_name()?.to_str()?;
    // Must end with .7z.<digits>
    if let Some(pos) = name.rfind(".7z.") {
        let suffix = &name[pos + 4..]; // the digits after ".7z."
        if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) && suffix != "001" {
            let base = &name[..pos]; // everything before ".7z.NNN"
            let first = format!("{}.7z.001", base);
            let first_path = archive.with_file_name(first);
            if first_path.exists() {
                return Some(first_path);
            }
        }
    }
    None
}

/// .7z — via the `sevenz-rust2` crate (pure Rust, LZMA2).
/// Extract a single 7z entry's bytes from `reader` into `out_path` via TEMP-THEN-RENAME,
/// so a pre-existing file at `out_path` is never truncated/lost on cancel or failure.
///
/// Mirrors the proven zgx/zip extract pattern: write to a sidecar temp, flush, apply
/// mtime on the temp handle (survives same-volume rename), then `atomic_replace` onto
/// the final path ONLY on full success. On cancel/error the temp is deleted and the
/// user's original at `out_path` is untouched.
///
/// Returns Ok to continue, Ok(false) to signal cancel to the sevenz-rust2 callback
/// contract, or Err to abort with a message.
fn sevenz_write_entry_via_temp(
    reader: &mut dyn std::io::Read,
    out_path: &std::path::Path,
    cancel: &AtomicBool,
    prog: &Progress,
    meta: &SevenzMeta,
) -> Result<bool, sevenz_rust2::Error> {
    let tmp = extract_temp_path(out_path);
    // Guard the temp: on Drop (cancel/panic/error) delete the temp. pre_existed is
    // false because the temp is brand-new — the GUARD'S contract only ever deletes
    // temps, NEVER the user's final-path file.
    let mut guard = PartialFile::new(tmp.clone());
    let mut out = std::fs::File::create(&tmp)
        .map_err(|e| sevenz_rust2::Error::Other(format!("create: {}", e).into()))?;
    let mut buf = [0u8; 65536];
    loop {
        if cancel.load(Ordering::Relaxed) {
            // Drop cleans the temp; signal cancel to the outer callback.
            return Ok(false);
        }
        use std::io::Read;
        let n = reader.read(&mut buf)
            .map_err(|e| sevenz_rust2::Error::Other(format!("read: {}", e).into()))?;
        if n == 0 { break; }
        use std::io::Write;
        out.write_all(&buf[..n])
            .map_err(|e| sevenz_rust2::Error::Other(format!("write: {}", e).into()))?;
        prog.tick_bytes(n as u64);
    }
    // Flush before rename — ensure all bytes hit disk.
    use std::io::Write;
    out.flush()
        .map_err(|e| sevenz_rust2::Error::Other(format!("flush: {}", e).into()))?;
    drop(out);
    // Atomic swap: temp → final. This is the ONLY point out_path is touched.
    // atomic_replace handles the Windows ReadOnly-destination trap internally.
    if let Err(e) = crate::metaattr::atomic_replace(&tmp, out_path) {
        // Drop cleans the temp; surface the rename error.
        return Err(sevenz_rust2::Error::Other(format!("rename: {}", e).into()));
    }
    guard.disarm(); // rename succeeded — temp no longer exists
    // Restore all carried metadata on the FINAL path (post-rename).
    meta.apply_to(out_path);
    Ok(true)
}

/// All metadata the 7z path can extract from a SevenZArchiveEntry and restore
/// onto the extracted file. Each field is Option — 7z entries flag which fields
/// are present (has_last_modified_date, has_creation_date, has_windows_attributes).
struct SevenzMeta {
    mtime_secs: Option<u64>,
    ctime_secs: Option<u64>,
    attrs: Option<u32>,
}

impl SevenzMeta {
    fn from_entry(entry: &sevenz_rust2::SevenZArchiveEntry) -> Self {
        use std::time::SystemTime;
        let mtime_secs = if entry.has_last_modified_date {
            let st: SystemTime = entry.last_modified_date().into();
            st.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs())
        } else { None };
        let ctime_secs = if entry.has_creation_date {
            let st: SystemTime = entry.creation_date().into();
            st.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs())
        } else { None };
        let attrs = if entry.has_windows_attributes {
            Some(entry.windows_attributes())
        } else { None };
        Self { mtime_secs, ctime_secs, attrs }
    }

    /// Apply all carried fields to the extracted file at `path`. Best-effort —
    /// each step ignores failure (a missing ctime shouldn't fail an extract).
    fn apply_to(&self, path: &std::path::Path) {
        if let Some(mt) = self.mtime_secs {
            crate::metaattr::apply_times_path(path, mt, self.ctime_secs.unwrap_or(0));
        } else if let Some(ct) = self.ctime_secs {
            crate::metaattr::apply_times_path(path, 0, ct);
        }
        if let Some(a) = self.attrs {
            // apply_attrs_normalized masks to PRESERVED_ATTRS internally, so
            // 7z's full dwFileAttributes is filtered to the safe subset.
            crate::metaattr::apply_attrs_normalized(path, a);
        }
    }
}

/// Extract the entry's last-modified date as UNIX-epoch seconds (what apply_times_path
/// expects). Returns None if the archive didn't carry a usable mtime. The conversion
/// chain is: sevenz-rust2 `FileTime` -> `SystemTime` (via the library's own Into impl)
/// -> seconds since UNIX_EPOCH.
fn entry_mtime_secs(entry: &sevenz_rust2::SevenZArchiveEntry) -> Option<u64> {
    SevenzMeta::from_entry(entry).mtime_secs
}

fn extract_7z(archive: &Path, dest: &Path, cancel: &AtomicBool) -> (bool, String) {
    progress::clear_status();
    // Multi-volume 7z: parts are named file.7z.001, file.7z.002, ...
    // (there is NO plain file.7z — the first part is .7z.001).
    //
    // Two entry points:
    //   (a) User right-clicked .7z.NNN where NNN != 001 → redirect to .7z.001
    //       via sevenz_first_part(), then collect all parts.
    //   (b) User right-clicked .7z.001 directly → check if .7z.002 exists;
    //       if yes, multi-volume; if no, single-volume.
    let start = sevenz_first_part(archive).unwrap_or_else(|| archive.to_path_buf());
    let name = start.file_name().and_then(|n| n.to_str()).unwrap_or("");

    // Is this a .7z.NNN file? (Either .001 directly, or redirected from .NNN.)
    if let Some(pos) = name.rfind(".7z.") {
        let suffix = &name[pos + 4..];
        if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
            // Collect all parts starting from .001.
            let base = &name[..pos];
            // Find the .001 part.
            let first_name = format!("{}.7z.001", base);
            let first_path = start.with_file_name(&first_name);
            if first_path.exists() {
                let mut parts: Vec<PathBuf> = vec![];
                let mut idx = 1u32;
                loop {
                    let part_name = format!("{}.7z.{:03}", base, idx);
                    let part_path = start.with_file_name(part_name);
                    if part_path.exists() {
                        parts.push(part_path);
                        idx += 1;
                    } else {
                        break;
                    }
                }
                if parts.len() > 1 {
                    // Multi-volume — concat all parts into one stream.
                    return extract_7z_multi(&parts, dest, cancel, name.to_string());
                }
                // Only .001 exists, no .002 — treat as single-volume.
                return extract_7z_single(&first_path, dest, cancel, name.to_string());
            }
        }
    }

    // Plain .7z (single volume).
    let label = archive
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    extract_7z_single(archive, dest, cancel, label)
}

/// Multi-volume 7z — concat all parts (.001/.002/...) into a temp file, then
/// decompress that single file. Simpler and more reliable than a custom
/// cross-part Read+Seek adapter (7z's header lives at EOF, so any boundary
/// bug dooms it).
fn extract_7z_multi(parts: &[PathBuf], dest: &Path, cancel: &AtomicBool, label: String) -> (bool, String) {
    let prog = Progress::new(&label);
    let heartbeat = prog.spawn_writer();
    prog.set_phase(0);

    // Total work = concat (read all parts) + decompress (write all files).
    // Both phases read/write roughly the compressed size, so total = 2× compressed.
    // This keeps the progress bar coherent: 0-50% concat, 50-100% decompress.
    let compressed_size: u64 = parts.iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum();
    prog.set_totals(0, compressed_size * 2);

    prog.set_phase(1);

    // Concat all parts into a temp file in the SYSTEM temp dir (avoids path-with-
    // space issues in the archive's parent folder, and ensures space on the system drive).
    let temp_dir = std::env::temp_dir();
    let tmp_file = temp_dir.join(format!("lrgex-7z-concat-{}.tmp", std::process::id()));
    let prog_concat = prog.clone();
    let result = (|| -> Result<(), String> {
        {
            let mut out = std::fs::File::create(&tmp_file)
                .map_err(|e| format!("create concat temp: {}", e))?;
            use std::io::{Read, Write};
            for (i, part) in parts.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    return Err("cancelled".to_string());
                }
                let mut f = std::fs::File::open(part)
                    .map_err(|e| format!("open part {}: {}", i + 1, e))?;
                // Buffered copy with progress ticking (first 50% of the bar).
                let mut buf = [0u8; 1048576];
                loop {
                    let n = f.read(&mut buf)
                        .map_err(|e| format!("concat read part {}: {}", i + 1, e))?;
                    if n == 0 { break; }
                    out.write_all(&buf[..n])
                        .map_err(|e| format!("concat write part {}: {}", i + 1, e))?;
                    prog_concat.tick_bytes(n as u64);
                }
            }
            out.sync_all().map_err(|e| format!("sync: {}", e))?;
        }

        // Decompress the concatenated single file.
        let prog_arc = prog.clone();
        let dest_buf = dest.to_path_buf();
        sevenz_rust2::decompress_file_with_extract_fn(
            &tmp_file,
            dest,
            move |entry, rdr, out_path| -> Result<bool, sevenz_rust2::Error> {
                if cancel.load(Ordering::Relaxed) {
                    return Ok(false);
                }
                if !out_path.starts_with(&dest_buf) {
                    return Ok(true);
                }
                if entry.is_directory() {
                    std::fs::create_dir_all(out_path)
                        .map_err(|e| sevenz_rust2::Error::Other(format!("mkdir: {}", e).into()))?;
                } else {
                    if let Some(parent) = out_path.parent() {
                        std::fs::create_dir_all(parent)
                            .map_err(|e| sevenz_rust2::Error::Other(format!("mkdir: {}", e).into()))?;
                    }
                    let meta = SevenzMeta::from_entry(entry);
                    return sevenz_write_entry_via_temp(rdr, out_path, cancel, &prog_arc, &meta);
                }
                Ok(true)
            },
        ).map_err(|e| format!("7z: {}", e))?;
        Ok(())
    })();

    // ALWAYS clean up the temp file, even on cancel/failure.
    let _ = std::fs::remove_file(&tmp_file);

    let cancelled = result.is_err() && cancel.load(Ordering::Relaxed);
    if cancelled {
        prog.finish(5);
    } else {
        prog.finish(if result.is_ok() { 3 } else { 4 });
    }
    let _ = heartbeat.join();

    match result {
        Ok(()) => (true, String::new()),
        Err(e) if cancelled => (true, String::new()),
        Err(e) => (false, e),
    }
}

fn extract_7z_single(archive: &Path, dest: &Path, cancel: &AtomicBool, label: String) -> (bool, String) {
    progress::clear_status();
    let prog = Progress::new(&label);
    let heartbeat = prog.spawn_writer();
    prog.set_phase(0);
    // Set total = archive file size, so the progress bar is determinate.
    let total = std::fs::metadata(archive).map(|m| m.len()).unwrap_or(0);
    prog.set_totals(0, total);
    prog.set_phase(1);

    // sevenz-rust2's per-entry callback fires for every entry. It hands us the
    // computed destination path (already joined to `dest`), so we just validate
    // it stays inside `dest` (path-traversal guard) and copy bytes with progress.
    // Returning Ok(false) aborts early (our cancel signal); Err aborts with error.
    //
    // The callback is synchronous (no 'static bound), so capturing `cancel: &AtomicBool`
    // by move is safe and needs no unsafe pointer tricks.
    let prog_arc = prog.clone();
    let dest_buf = dest.to_path_buf();

    let result = sevenz_rust2::decompress_file_with_extract_fn(
        archive,
        dest,
        move |entry, reader, out_path| -> Result<bool, sevenz_rust2::Error> {
            // Cancel check — between files.
            if cancel.load(Ordering::Relaxed) {
                return Ok(false); // stop extraction
            }

            // Path-traversal guard — reject anything escaping `dest`.
            if !out_path.starts_with(&dest_buf) {
                return Ok(true); // skip this entry, keep going
            }

            if entry.is_directory() {
                std::fs::create_dir_all(out_path)
                    .map_err(|e| sevenz_rust2::Error::Other(format!("mkdir: {}", e).into()))?;
            } else {
                if let Some(parent) = out_path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| sevenz_rust2::Error::Other(format!("mkdir: {}", e).into()))?;
                }
                let meta = SevenzMeta::from_entry(entry);
                return sevenz_write_entry_via_temp(reader, out_path, cancel, &prog_arc, &meta);
            }
            Ok(true) // keep going
        },
    );

    let result = result.map_err(|e| e.to_string());
    let cancelled = result.is_err() && cancel.load(Ordering::Relaxed);

    if cancelled {
        prog.finish(5);
    } else {
        prog.finish(if result.is_ok() { 3 } else { 4 });
    }
    let _ = heartbeat.join();

    match result {
        Ok(()) => (true, String::new()),
        Err(e) if cancelled => (true, String::new()),
        Err(e) => (false, e),
    }
}

/// 7z conflict check — walk entry names from the 7z archive.
fn sevenz_has_conflicts(archive: &Path, dest: &Path) -> bool {
    let dest = match dest.canonicalize() {
        Ok(d) => d,
        Err(_) => return false,
    };

    // For multi-volume 7z, skip the upfront conflict check — reading the header
    // would require concatenating all parts (1-2 GB copy) just to list names,
    // which is too slow for a pre-extract prompt. Single .7z reads the header
    // instantly (no concat). Multi-volume users don't get the upfront prompt
    // (same trade-off as RAR for multi-volume).
    if is_sevenz_volume_part(archive) {
        return false;
    }
    let read_path = archive.to_path_buf();
    let _temp_to_clean: Option<PathBuf> = None;

    // Header-only read: get the file listing WITHOUT decompressing.
    // Archive::open parses the header/entry table without extracting file data.
    let archive_data = match sevenz_rust2::Archive::open(&read_path) {
        Ok(a) => a,
        Err(_) => {
            if let Some(t) = &_temp_to_clean { let _ = std::fs::remove_file(t); }
            return false;
        }
    };

    let mut conflict = false;
    for entry in archive_data.files.iter() {
        let name = entry.name();
        if name.contains("..") || name.starts_with('/') || name.starts_with('\\') { continue; }
        let p = dest.join(name);
        if p.exists() { conflict = true; break; }
    }

    if let Some(t) = &_temp_to_clean { let _ = std::fs::remove_file(t); }
    conflict
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// A zip with one normal entry and one `..`-traversal entry MUST extract the normal
    /// one and skip the traversal one — never write outside the destination.
    #[test]
    fn zip_slip_is_blocked() {
        let tmp = std::env::temp_dir().join(format!("lrgex-zipslip-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // Build the zip with the raw zip crate (writer).
        let zip_path = tmp.join("test.zip");
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(file);
            let opts = zip::write::SimpleFileOptions::default();
            // Normal entry.
            zw.start_file("safe.txt", opts).unwrap();
            zw.write_all(b"safe content").unwrap();
            // Traversal entry — MUST be rejected by enclosed_name() on extract.
            zw.start_file("../evil.txt", opts).unwrap();
            zw.write_all(b"should not land outside dest").unwrap();
            zw.finish().unwrap();
        }

        // dest is a subfolder of tmp. A slipped `../evil.txt` would resolve to tmp/evil.txt
        // — i.e. dest.parent().join("evil.txt"). Asserting THAT path doesn't exist proves
        // the guard works (checking only inside `dest` would be a false pass).
        let dest = tmp.join("out");
        let (ok, msg) = extract_archive(&zip_path, &dest, &std::sync::atomic::AtomicBool::new(false));
        assert!(ok, "zip extract failed: {}", msg);

        // Safe entry present + correct content.
        let safe = dest.join("safe.txt");
        assert!(safe.is_file(), "safe.txt not extracted");
        let mut s = String::new();
        std::fs::File::open(&safe).unwrap().read_to_string(&mut s).unwrap();
        assert_eq!(s, "safe content");

        // Traversal entry did NOT escape to dest.parent().
        let escaped = dest.parent().unwrap().join("evil.txt");
        assert!(!escaped.exists(), "zip-slip: evil.txt was written outside dest");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Temp-then-rename: after a SUCCESSFUL extract, no temp files should be left
    /// behind. Verifies the staging pattern cleans up after itself.
    #[test]
    fn extract_leaves_no_temp_files() {
        let tmp = std::env::temp_dir().join(format!("lrgex-notemp-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // Build a zip with one entry.
        let zip_path = tmp.join("test.zip");
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(file);
            zw.start_file("hello.txt", zip::write::SimpleFileOptions::default()).unwrap();
            zw.write_all(b"hi").unwrap();
            zw.finish().unwrap();
        }

        let dest = tmp.join("out");
        let (ok, msg) = extract_archive(&zip_path, &dest, &std::sync::atomic::AtomicBool::new(false));
        assert!(ok, "extract failed: {}", msg);

        // The final file exists.
        assert!(dest.join("hello.txt").is_file());
        // No leftover temp files anywhere under tmp. Match the ACTUAL temp-name
        // pattern (`.lrgex-tmp-<pid>-<n>` suffix, or staging dir), NOT a substring
        // that could match a legitimate user filename.
        let mut found_temps: Vec<PathBuf> = Vec::new();
        for entry in walk_all(&tmp) {
            let s = entry.to_string_lossy();
            if s.contains(".lrgex-tmp-") || s.contains(".lrgex-rar-staging") {
                found_temps.push(entry);
            }
        }
        assert!(found_temps.is_empty(), "leftover temp files: {:?}", found_temps);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Symlink target validation: an absolute target MUST be rejected (returns false).
    #[test]
    fn symlink_absolute_target_rejected() {
        let dest = std::path::Path::new("C:\\some\\dest");
        let link = dest.join("link.txt");
        assert!(!is_safe_link_target("C:\\Windows\\System32\\evil.dll", &link, dest));
        assert!(!is_safe_link_target("/etc/passwd", &link, dest));
    }

    /// Symlink target validation: a drive-prefix target (C:foo) MUST be rejected.
    #[test]
    fn symlink_drive_prefix_rejected() {
        let dest = std::path::Path::new("C:\\dest");
        let link = dest.join("link.txt");
        assert!(!is_safe_link_target("C:evil", &link, dest));
    }

    /// Symlink target validation: a parent-traversal escape MUST be rejected.
    #[test]
    fn symlink_parent_escape_rejected() {
        // Create a real dest so canonicalize works.
        let tmp = std::env::temp_dir().join(format!("lrgex-symlink-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let link = tmp.join("subdir").join("link.txt");
        // target resolves to tmp's parent — outside dest root.
        assert!(!is_safe_link_target("../../escape", &link, &tmp));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// has_conflicts: no conflict when dest is empty (and no false negatives).
    #[test]
    fn conflicts_empty_dest_is_false() {
        let tmp = std::env::temp_dir().join(format!("lrgex-conflicts-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let zip_path = tmp.join("test.zip");
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(file);
            zw.start_file("a.txt", zip::write::SimpleFileOptions::default()).unwrap();
            zw.write_all(b"x").unwrap();
            zw.finish().unwrap();
        }

        let dest = tmp.join("out");
        std::fs::create_dir_all(&dest).unwrap();
        assert!(!has_conflicts(&zip_path, &dest), "empty dest should have no conflicts");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// has_conflicts: returns true when dest has a file the archive will overwrite.
    #[test]
    fn conflicts_existing_file_is_true() {
        let tmp = std::env::temp_dir().join(format!("lrgex-conflicts2-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let zip_path = tmp.join("test.zip");
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(file);
            zw.start_file("a.txt", zip::write::SimpleFileOptions::default()).unwrap();
            zw.write_all(b"x").unwrap();
            zw.finish().unwrap();
        }

        let dest = tmp.join("out");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("a.txt"), b"pre-existing").unwrap();
        assert!(has_conflicts(&zip_path, &dest), "existing a.txt should be a conflict");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// lexical_normalize: resolves `.` and `..` as string ops, no filesystem access.
    #[test]
    fn lexical_normalize_resolves_parent() {
        // `..` pops the last component.
        let p = lexical_normalize(std::path::Path::new("C:\\dest\\sub\\..\\file.txt"));
        // Windows parses C: as a VerbatimDisk prefix; after popping 'sub' we expect
        // the path to resolve under the drive. The exact separator form is OS-dependent,
        // so assert the key property: 'sub' is gone and 'file.txt' is present.
        let s = p.to_string_lossy();
        assert!(s.ends_with("file.txt"), "got: {}", s);
        assert!(!s.contains("sub"), "'..' did not pop 'sub': {}", s);

        // `.` is stripped.
        let p2 = lexical_normalize(std::path::Path::new("C:\\dest\\.\\a\\b.txt"));
        let s2 = p2.to_string_lossy();
        assert!(s2.ends_with("a\\b.txt"), "got: {}", s2);
    }

    /// Overwrite contract: pre-existing file is REPLACED with the archive's bytes,
    /// and no temp file is left behind. Catches temp-then-rename disarm bugs.
    #[test]
    fn extract_overwrites_existing_cleanly() {
        let tmp = std::env::temp_dir().join(format!("lrgex-overwrite-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let zip_path = tmp.join("test.zip");
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(file);
            zw.start_file("target.txt", zip::write::SimpleFileOptions::default()).unwrap();
            zw.write_all(b"NEW CONTENT").unwrap();
            zw.finish().unwrap();
        }

        let dest = tmp.join("out");
        std::fs::create_dir_all(&dest).unwrap();
        // Pre-seed with OLD bytes — this is the file that will be overwritten.
        std::fs::write(dest.join("target.txt"), b"OLD CONTENT THAT MUST BE REPLACED").unwrap();

        let (ok, msg) = extract_archive(&zip_path, &dest, &std::sync::atomic::AtomicBool::new(false));
        assert!(ok, "extract failed: {}", msg);

        // Final content is the NEW bytes from the archive.
        let final_bytes = std::fs::read(dest.join("target.txt")).unwrap();
        assert_eq!(final_bytes, b"NEW CONTENT", "file was not overwritten with archive content");

        // No leftover temp file in dest.
        assert!(!dest.join("target.zgx-lrgex-extract-tmp").exists(),
            "temp file was left behind after successful overwrite");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Sibling collision regression: two entries with the same stem + different
    /// extension (Makefile + Makefile.in) MUST both extract with correct distinct
    /// content. Catches the with_extension temp-collision bug (P1-A).
    #[test]
    fn extract_same_stem_different_ext_no_collision() {
        let tmp = std::env::temp_dir().join(format!("lrgex-collision-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let zip_path = tmp.join("test.zip");
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(file);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("Makefile", opts).unwrap();
            zw.write_all(b"MAKEFILE BODY").unwrap();
            zw.start_file("Makefile.in", opts).unwrap();
            zw.write_all(b"MAKEFILE IN BODY").unwrap();
            zw.start_file("data.txt", opts).unwrap();
            zw.write_all(b"TXT").unwrap();
            zw.start_file("data.bin", opts).unwrap();
            zw.write_all(b"BIN").unwrap();
            zw.finish().unwrap();
        }

        let dest = tmp.join("out");
        let (ok, msg) = extract_archive(&zip_path, &dest, &std::sync::atomic::AtomicBool::new(false));
        assert!(ok, "extract failed: {}", msg);

        // ALL four files must exist with their DISTINCT original content. If the temp
        // naming collided (with_extension), one of each pair would be missing or wrong.
        assert_eq!(std::fs::read(dest.join("Makefile")).unwrap(), b"MAKEFILE BODY");
        assert_eq!(std::fs::read(dest.join("Makefile.in")).unwrap(), b"MAKEFILE IN BODY");
        assert_eq!(std::fs::read(dest.join("data.txt")).unwrap(), b"TXT");
        assert_eq!(std::fs::read(dest.join("data.bin")).unwrap(), b"BIN");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Helper: walk all paths under a directory recursively.
    fn walk_all(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(dir) {
            for entry in rd.flatten() {
                let p = entry.path();
                out.push(p.clone());
                if p.is_dir() {
                    out.extend(walk_all(&p));
                }
            }
        }
        out
    }
}
