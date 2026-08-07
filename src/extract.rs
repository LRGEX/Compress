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

use crate::progress::{self, ByteReader, Progress};

/// Counting reader for zip extraction — ticks decompressed bytes into Progress.
struct ZipCountingReader<'a, R: std::io::Read> {
    inner: &'a mut R,
    prog: &'a Progress,
}

impl<'a, R: std::io::Read> std::io::Read for ZipCountingReader<'a, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.prog.tick_bytes(n as u64);
        }
        Ok(n)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Zgx, // tar + zstd
    Zip,
    Rar,
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
        _ => None,
    }
}

/// Top-level dispatcher. Routes to the right handler by detected format.
pub fn extract_archive(archive: &Path, dest: &Path) -> (bool, String) {
    match detect_format(archive) {
        Some(Format::Zgx) => extract_zgx(archive, dest),
        Some(Format::Zip) => extract_zip(archive, dest),
        Some(Format::Rar) => extract_rar(archive, dest),
        None => (false, "Unrecognized archive format".to_string()),
    }
}

/// .zgx = tar + zstd. Byte-counting via ByteReader so the heartbeat tracks bytes.
fn extract_zgx(archive: &Path, dest: &Path) -> (bool, String) {
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
    let counting = ByteReader::new(decoder, prog.clone());
    let buf_decoder = std::io::BufReader::with_capacity(256 * 1024, counting);
    let mut tar = tar::Archive::new(buf_decoder);

    let _ = std::fs::create_dir_all(dest);
    let result = extract_zgx_inner(&mut tar, dest);

    match result {
        ZgxOutcome::Done => {
            prog.finish(3);
            let _ = heartbeat.join();
            (true, String::new())
        }
        ZgxOutcome::ElevatedRelaunched => {
            // We re-launched as admin; the elevated process takes over and writes the
            // terminal phase. Exit now so the (non-elevated) GUI closes.
            let _ = heartbeat.join();
            std::process::exit(0);
        }
        ZgxOutcome::Failed(e) => {
            prog.finish(4);
            let _ = heartbeat.join();
            (false, format!("extract failed: {}", e))
        }
    }
}

enum ZgxOutcome {
    Done,
    ElevatedRelaunched, // re-launched as admin; current process must exit
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

fn extract_zgx_inner<R: std::io::Read>(tar: &mut tar::Archive<R>, dest: &Path) -> ZgxOutcome {
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
                       dir_cache: &mut std::collections::HashSet<PathBuf>| -> Result<(), String> {
        if batch.is_empty() { return Ok(()); }
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
        use std::os::windows::io::AsRawHandle;
        let run_writes = || {
            batch
                .par_iter()
                .filter_map(|(path, data, meta)| {
                    let mut f = match std::fs::File::create(path) {
                        Ok(f) => f,
                        Err(e) => return Some(e),
                    };
                    use std::io::Write;
                    if let Err(e) = f.write_all(data) { return Some(e); }
                    let raw = f.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
                    crate::metaattr::apply_all_handle(raw, meta.mtime, meta.ctime, meta.attrs & crate::metaattr::PRESERVED_ATTRS);
                    drop(f);
                    None
                })
                .collect::<Vec<_>>()
        };
        let errs: Vec<_> = match &write_pool {
            Some(p) => p.install(run_writes),
            None => run_writes(),  // fallback if pool creation failed
        };
        batch.clear();
        *batch_bytes = 0;
        match errs.into_iter().next() {
            Some(e) => Err(format!("write batch: {}", e)),
            None => Ok(()),
        }
    };

    // Sidecar metadata map (populated on first iteration if the archive has a sidecar).
    // New archives put ctime+attrs in one .lrgex/meta.bin blob at the front instead of
    // per-entry PAX headers (saves ~11k extra tar entries). Old archives have no sidecar
    // → map stays None → we fall back to read_pax_meta per entry (backward compat).
    let mut sidecar_map: Option<std::collections::HashMap<PathBuf, (u64, u32)>> = None;
    let mut first_checked = false;

    for entry in entries {
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
            continue;
        }

        if etype.is_symlink() {
            // Flush any pending files first (they may be the link's siblings).
            if let Err(e) = flush_batch(&mut batch, &mut batch_bytes, &mut dir_cache) {
                return ZgxOutcome::Failed(e);
            }
            let target = match entry.link_name() {
                Ok(Some(t)) => t.to_string_lossy().to_string(),
                _ => continue, // symlink with no target — can't recreate, skip
            };
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
                            // Either way skip THIS link: if yes, the elevated pass recreates
                            // everything; if no, all remaining links skip silently.
                            continue;
                        }
                        Some(_) => continue, // already decided → skip silently
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

        if size > STREAM_THRESHOLD {
            // Large file: stream straight to disk, then restore metadata on the path.
            if let Err(e) = flush_batch(&mut batch, &mut batch_bytes, &mut dir_cache) {
                return ZgxOutcome::Failed(e);
            }
            if let Some(parent) = outpath.parent() {
                if dir_cache.insert(parent.to_path_buf()) {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        return ZgxOutcome::Failed(format!("mkdir parent: {}", e));
                    }
                }
            }
            let mut f = match std::fs::File::create(&outpath) {
                Ok(f) => f,
                Err(e) => return ZgxOutcome::Failed(format!("create {}: {}", rel.display(), e)),
            };
            let _ = f.set_len(size);
            // Write 4 MB chunks straight from the tar entry to the File — NO BufWriter.
            // The File already hands bytes to the OS page cache; a BufWriter added a
            // redundant user-space copy (entry → 1MB buf → File → OS) and io::copy's
            // 8KB default did tons of tiny syscalls. A pinned 4MB buffer + File::write_all
            // is the minimal-copy path and was the main fix for the large-file extract gap.
            use std::io::{Read, Write};
            let mut chunk = vec![0u8; 4 * 1024 * 1024];
            let mut remaining = size as usize;
            loop {
                if remaining == 0 { break; }
                let want = remaining.min(chunk.len());
                let n = match entry.read(&mut chunk[..want]) {
                    Ok(0) => break, // unexpected EOF — tar stream ended early
                    Ok(n) => n,
                    Err(e) => return ZgxOutcome::Failed(format!("read {}: {}", rel.display(), e)),
                };
                if let Err(e) = f.write_all(&chunk[..n]) {
                    return ZgxOutcome::Failed(format!("write {}: {}", rel.display(), e));
                }
                if n > remaining { break; } // guard against bad header size
                remaining -= n;
            }
            // Flush the File to the OS, then restore mtime+ctime+attrs in ONE call on
            // the still-open handle — no re-open.
            if let Err(e) = f.flush() {
                return ZgxOutcome::Failed(format!("flush {}: {}", rel.display(), e));
            }
            {
                use std::os::windows::io::AsRawHandle;
                let raw = f.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
                crate::metaattr::apply_all_handle(raw, meta.mtime, meta.ctime, meta.attrs & crate::metaattr::PRESERVED_ATTRS);
            }
            drop(f);
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
            if let Err(e) = flush_batch(&mut batch, &mut batch_bytes, &mut dir_cache) {
                return ZgxOutcome::Failed(e);
            }
        }
    }

    // Flush remaining small files.
    if let Err(e) = flush_batch(&mut batch, &mut batch_bytes, &mut dir_cache) {
        return ZgxOutcome::Failed(e);
    }

    // FINAL pass: restore directory mtime/ctime/attrs AFTER all children written.
    // Process in reverse order (deepest first) so creating a child dir doesn't disturb
    // a parent's mtime after we set it.
    for (dir, meta) in dir_meta_todo.iter().rev() {
        if meta.mtime > 0 || meta.ctime > 0 {
            crate::metaattr::apply_times_path(dir, meta.mtime, meta.ctime);
        }
        if meta.attrs != 0 {
            crate::metaattr::apply_attrs(dir, meta.attrs);
        }
    }

    // If a symlink needed elevation and the user said yes → relaunch the WHOLE extract
    // elevated and exit. The elevated instance recreates everything including the links.
    if needs_relaunch {
        let args: Vec<String> = std::env::args().collect();
        let _ = crate::metaattr::relaunch_elevated(&args);
        return ZgxOutcome::ElevatedRelaunched;
    }

    ZgxOutcome::Done
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

/// .zip — via the `zip` crate (pure Rust). Decompressed bytes tick into Progress
/// via a counting reader; the total is the SUM of uncompressed entry sizes (from the
/// central directory), not the on-disk archive size, so the bar never overshoots.
///
/// Restores what the zip contains: mtime (DOS datetime), read-only bit (from unix
/// mode if present), and symlinks (target stored as entry data — zip convention).
/// Zip does NOT carry CreationTime or Windows-specific attributes, so those aren't
/// restored (nothing to restore — we don't strip anything that's actually there).
fn extract_zip(archive: &Path, dest: &Path) -> (bool, String) {
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

        for i in 0..za.len() {
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
                let mut counting = ZipCountingReader { inner: &mut entry, prog: &prog };
                std::io::Read::read_to_end(&mut counting, &mut target).map_err(|e| format!("read link {}: {}", rel.display(), e))?;
                let target = String::from_utf8_lossy(&target).trim_end_matches(['\0', '\r', '\n']).to_string();
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
                        }
                        Some(_) => {} // already decided → skip silently
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
            if let Some(parent) = outpath.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("mkdir parent: {}", e))?;
            }
            {
                // Write via a handle so we can set times on it before the writer drops.
                let outf = std::fs::File::create(&outpath).map_err(|e| format!("create {}: {}", rel.display(), e))?;
                let mut counting = ZipCountingReader { inner: &mut entry, prog: &prog };
                let mut bw = std::io::BufWriter::new(&outf);
                std::io::copy(&mut counting, &mut bw).map_err(|e| format!("write {}: {}", rel.display(), e))?;
                drop(bw);
                drop(outf);
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
        // If a symlink needed elevation and the user said yes → relaunch elevated.
        if needs_relaunch {
            let args: Vec<String> = std::env::args().collect();
            let _ = crate::metaattr::relaunch_elevated(&args);
            // Caller's heartbeat will be joined; we exit the run via the main thread.
            // Signal failure here so the GUI doesn't claim success on the non-elevated copy.
            return Err("__elevated_relaunch__".to_string());
        }
        Ok(())
    })();

    match result {
        Ok(()) => {
            prog.finish(3);
            let _ = heartbeat.join();
            (true, String::new())
        }
        Err(e) => {
            prog.finish(4);
            let _ = heartbeat.join();
            (false, e)
        }
    }
}

/// Pass 1: read every header, sum real unpacked sizes.
/// Returns (file_count, total_bytes). Bytes are 0 if the archive is
/// encrypted or the header is unreadable -> caller falls back to indeterminate.
fn rar_totals(archive: &Path) -> (usize, u64) {
    let mut files = 0usize;
    let mut bytes = 0u64;
    let mut errs = 0usize;
    let mut log = String::new();

    match unrar::Archive::new(archive).open_for_listing() {
        Ok(list) => {
            for item in list {
                match item {
                    Ok(e) => {
                        let dir = e.is_directory();
                        log.push_str(&format!("entry: dir={} size={} name={:?}\n", dir, e.unpacked_size, e.filename));
                        if !dir {
                            files += 1;
                            bytes += e.unpacked_size;
                        }
                    }
                    Err(e) => {
                        errs += 1;
                        log.push_str(&format!("ENTRY ERROR: {:?}\n", e));
                    }
                }
            }
        }
        Err(e) => {
            log.push_str(&format!("open_for_listing FAILED: {:?}\n", e));
            let _ = std::fs::write(std::env::temp_dir().join("lrgex-rar-diag.txt"), log);
            return (0, 0);
        }
    }

    log.push_str(&format!("RESULT: files={} bytes={} errs={}\n", files, bytes, errs));
    let _ = std::fs::write(std::env::temp_dir().join("lrgex-rar-diag.txt"), log);
    (files, bytes)
}

/// .rar — via the vendored `unrar` crate with UCM_PROCESSDATA counter patch.
/// Real sub-file progress: a 100ms sampler thread reads the global processed-bytes
/// counter and ticks deltas into Progress. Works for single-file and multi-file archives.
fn extract_rar(archive: &Path, dest: &Path) -> (bool, String) {
    progress::clear_status();
    let label = archive
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let prog = Progress::new(&label);
    let heartbeat = prog.spawn_writer();

    prog.set_phase(0);
    let (files, total) = rar_totals(archive);
    prog.set_totals(files, total); // total == 0 -> sweep (encrypted/unreadable headers)
    prog.set_phase(1);

    // Sub-file progress via the vendored UCM_PROCESSDATA counter.
    unrar::reset_processed();
    let sampler_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let sampler = {
        let prog = prog.clone();
        let stop = sampler_stop.clone();
        std::thread::spawn(move || {
            let mut last: u64 = 0;
            loop {
                let now = unrar::processed_bytes();
                if now > last {
                    prog.tick_bytes(now - last);
                    last = now;
                }
                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                    break; // one final sample above, then exit
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        })
    };

    let result = (|| -> Result<(), String> {
        std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
        let mut open = unrar::Archive::new(archive)
            .open_for_processing()
            .map_err(|e| e.to_string())?;
        loop {
            match open.read_header().map_err(|e| e.to_string())? {
                None => break,
                Some(with_header) => {
                    open = with_header
                        .extract_with_base(dest)
                        .map_err(|e| e.to_string())?;
                    // NO tick_bytes here — the sampler owns the counter now.
                }
            }
        }
        Ok(())
    })();

    sampler_stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = sampler.join();

    prog.finish(if result.is_ok() { 3 } else { 4 });
    let _ = heartbeat.join();

    match result {
        Ok(()) => (true, String::new()),
        Err(e) => (false, e),
    }
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
        let (ok, msg) = extract_archive(&zip_path, &dest);
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
}
