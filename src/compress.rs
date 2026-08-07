// Compression (tar + zstd) — adapted from LRGEX Restore sync.rs.
// Hardcoded engine: zstd level 1, rayon parallel reads (all cores, batch 2048),
// ByteReader for files >8 MB. Empty directories ARE archived (preserved on extract).
//
// METADATA FIDELITY (v1.4): every entry preserves its real mtime (tar native field),
// CreationTime + Windows attributes (PAX local extensions), and symbolic links (native
// tar Symlink entry with the target as link name). Round-trip is lossless: whatever
// goes in comes out identical.
//
// ATOMIC WRITE: compress to <dest>.part, rename to <dest> only on full success —
// so an interrupted / panicked / OOM-killed / force-closed compress can NEVER leave a
// complete-looking archive behind. Worst case is an obviously-incomplete `.part`.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;

use crate::metaattr::{self, MetaSnapshot};
use crate::progress::{self, ByteReader, Progress};

// Files up to this size are preloaded into RAM via parallel rayon reads.
// Larger files stream through ByteReader (avoids OOM on multi-GB files).
// Note: for pre-compressed data (e.g. FitGirl .bin repacks), zstd level 1 is inherently
// slow — the encoder processes every byte even if the data is incompressible. This is
// a zstd/algorithm limitation, not a code bug. Parallel reads don't help on a single
// NVMe drive (sequential read is already near-peak).
const BIG_FILE: u64 = 64 * 1024 * 1024; // 64 MB — balances RAM vs parallelism
const BATCH: usize = 2048; // parallel-read batch size

/// Kind of entry being archived. Symlinks carry their target so we can recreate the
/// link on extract (no file body is stored — links have no content of their own).
pub enum EntKind {
    File,
    Dir,
    Symlink { target: String },
}

pub struct FileEnt {
    pub path: PathBuf,
    pub rel: PathBuf,
    pub size: u64,
    pub kind: EntKind,
    pub meta: MetaSnapshot, // mtime / ctime / Windows attributes
}

/// Single recursive walk. Returns (entries, total_bytes, count). Archives files AND
/// directories (so empty folders are preserved) AND symlinks. Uses DirEntry::metadata()
/// which on Windows is served from the enumeration cache.
pub fn walk_tree(base: &Path, excluded: &[String]) -> (Vec<FileEnt>, u64, usize) {
    let mut out = Vec::with_capacity(4096);
    let mut total = 0u64;
    walk_inner(base, base, excluded, &mut out, &mut total);
    let count = out.len();
    (out, total, count)
}

fn walk_inner(base: &Path, current: &Path, excluded: &[String], out: &mut Vec<FileEnt>, total: &mut u64) {
    let entries = match std::fs::read_dir(current) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_s = name.to_string_lossy();
        if excluded.iter().any(|e| e.as_str() == name_s.as_ref()) {
            continue;
        }

        let path = entry.path();
        let rel = path.strip_prefix(base).unwrap_or(&path).to_path_buf();

        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };

        // Metadata: prefer DirEntry::metadata() — on Windows it's served from the
        // read_dir enumeration cache (WIN32_FIND_DATA) with NO per-file CreateFile.
        // symlink_metadata (read_meta, path-based) re-opens the file; we only need that
        // for symlinks (std re-queries reparse points, correctly giving the LINK's own
        // attrs rather than the target's). We fetch once and reuse for both metadata
        // AND file size (the old code called entry.metadata() twice).
        let dir_meta = if ft.is_symlink() {
            None   // symlinks: use path-based read_meta below (needs the link's own attrs)
        } else {
            entry.metadata().ok()
        };
        let meta = match &dir_meta {
            Some(m) => metaattr::read_meta_from(m),
            None => metaattr::read_meta(&path),
        };

        if ft.is_symlink() {
            // Preserve the link: store the target path. No file body — a symlink has no
            // content of its own. If we can't read the target, skip (can't preserve it).
            let target = match std::fs::read_link(&path) {
                Ok(t) => t.to_string_lossy().to_string(),
                Err(_) => continue,
            };
            out.push(FileEnt {
                path: path.clone(),
                rel,
                size: 0,
                kind: EntKind::Symlink { target },
                meta,
            });
            continue;
        }

        if ft.is_dir() {
            // Archive the directory entry itself (preserves EMPTY folders), then recurse.
            out.push(FileEnt {
                path: path.clone(),
                rel: rel.clone(),
                size: 0,
                kind: EntKind::Dir,
                meta,
            });
            walk_inner(base, &path, excluded, out, total);
        } else {
            let size = dir_meta.as_ref().map(|m| m.len()).unwrap_or(0);
            *total += size;
            out.push(FileEnt {
                path,
                rel,
                size,
                kind: EntKind::File,
                meta,
            });
        }
    }
}

/// Deterministic tar header for a REGULAR file. uid/gid zeroed, mtime = real source
/// mtime (preserved across the round-trip). Mode is the caller's choice (0o644 files,
/// 0o755 dirs).
fn make_header(size: u64, mtime: u64, mode: u32) -> tar::Header {
    let mut h = tar::Header::new_gnu();
    h.set_entry_type(tar::EntryType::Regular);
    h.set_size(size);
    h.set_mode(mode);
    h.set_uid(0);
    h.set_gid(0);
    h.set_mtime(mtime);
    h.set_cksum();
    h
}

/// Emit PAX local extensions carrying CreationTime + Windows attributes for the NEXT
/// entry. Only emitted when there's actually something to carry (skipped entirely for
/// all-zero metadata, so ordinary files pay no PAX overhead when they have nothing to
/// restore). Keys: `SCHILY.creationtime` (seconds, ASCII decimal) and `LRGEX.fileattr`
/// (raw u32 dwFileAttributes, ASCII decimal — masked at restore time to the
/// user-meaningful subset).
fn emit_pax_extensions(builder: &mut tar::Builder<impl Write>, meta: MetaSnapshot) -> std::io::Result<()> {
    let mut fields: Vec<(&str, Vec<u8>)> = Vec::new();
    if meta.ctime > 0 {
        fields.push(("SCHILY.creationtime", meta.ctime.to_string().into_bytes()));
    }
    let restorable = meta.restorable_attrs();
    if restorable != 0 {
        fields.push(("LRGEX.fileattr", restorable.to_string().into_bytes()));
    }
    if fields.is_empty() {
        return Ok(());
    }
    // append_pax_extensions takes (&str, &[u8]) pairs — borrow our Vec<u8> values.
    let refs: Vec<(&str, &[u8])> = fields.iter().map(|(k, v)| (*k, v.as_slice())).collect();
    builder.append_pax_extensions(refs)
}

/// Magic path of the metadata sidecar entry inside the .zgx tar stream.
/// The sidecar is the FIRST entry; it holds packed (path, ctime, attrs) records so
/// each real file/dir entry no longer needs its own PAX header (which doubled the tar
/// entry count). Keyed by path (not ordinal) so skipped entries simply never match.
pub const SIDECAR_PATH: &str = ".lrgex/meta.bin";

/// Build the sidecar body: a packed binary blob. Format (little-endian):
///   u32 record_count
///   then record_count records, each:
///     u16 path_len, path_bytes (UTF-8, forward slashes), u64 ctime_secs, u32 attrs
/// mtime stays in each entry's native tar header field — not duplicated here.
/// attrs are stored RAW (masked to PRESERVED_ATTRS at restore time).
fn build_sidecar(files: &[FileEnt]) -> Vec<u8> {
    // Upper-bound size: 4 + N*(2 + max_path + 8 + 4). Reuse forward-slash path bytes.
    let mut buf: Vec<u8> = Vec::with_capacity(4 + files.len() * 48);
    // Reserve the count slot; backfill after the loop with the ACTUAL number of records
    // written (paths >64KB are skipped, so files.len() would over-count and corrupt the
    // sidecar for the reader).
    let count_pos = buf.len();
    buf.extend_from_slice(&0u32.to_le_bytes());
    let mut written: u32 = 0;
    for e in files {
        let p = e.rel.to_string_lossy().replace('\\', "/");
        let pb = p.as_bytes();
        // Skip entries we can't represent (path too long for u16) — they won't round-trip
        // metadata anyway, and we must not corrupt the sidecar. They simply get no record.
        let plen = if pb.len() > 0xFFFF { continue } else { pb.len() as u16 };
        buf.extend_from_slice(&plen.to_le_bytes());
        buf.extend_from_slice(pb);
        buf.extend_from_slice(&e.meta.ctime.to_le_bytes());
        buf.extend_from_slice(&e.meta.attrs.to_le_bytes());
        written += 1;
    }
    buf[count_pos..count_pos + 4].copy_from_slice(&written.to_le_bytes());
    buf
}

/// Write the sidecar as the FIRST entry in the archive (a regular file at SIDECAR_PATH).
/// Must be called BEFORE any real entries are appended.
fn write_sidecar(builder: &mut tar::Builder<impl Write>, files: &[FileEnt]) -> std::io::Result<()> {
    let body = build_sidecar(files);
    let mut h = tar::Header::new_gnu();
    h.set_entry_type(tar::EntryType::Regular);
    h.set_size(body.len() as u64);
    h.set_mode(0o644);
    h.set_uid(0);
    h.set_gid(0);
    h.set_mtime(0); // the sidecar itself has no meaningful time
    h.set_cksum();
    builder.append_data(&mut h, SIDECAR_PATH, &mut body.as_slice())
}

fn read_whole(path: &Path, size: u64) -> std::io::Result<Vec<u8>> {
    let mut f = std::fs::File::open(path)?;
    let mut buf = Vec::with_capacity(size as usize + 64);
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

/// Compress a source directory (or single file) to a .tar.zst (branded .zgx). Writes to
/// `<dest>.part` and atomically renames to `<dest>` only on full success.
pub fn compress_folder(
    source: &Path,
    dest: &Path,
    excluded: &[String],
    cancel: &AtomicBool,
) -> (bool, Vec<String>) {
    progress::clear_status(); // start clean
    let label = source
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let progress = Progress::new(&label);
    let heartbeat = progress.spawn_writer();
    progress.set_phase(0); // walk

    let (files, total_bytes, count) = if source.is_file() {
        // Single file: archive just this file, stored at the archive root by its name.
        let size = std::fs::metadata(source).map(|m| m.len()).unwrap_or(0);
        let rel = source
            .file_name()
            .map(PathBuf::from)
            .unwrap_or_else(|| source.to_path_buf());
        let meta = metaattr::read_meta(source);
        (
            vec![FileEnt { path: source.to_path_buf(), rel, size, kind: EntKind::File, meta }],
            size,
            1,
        )
    } else {
        walk_tree(source, excluded)
    };
    progress.set_totals(count, total_bytes);
    progress.set_phase(1); // compress

    // Write to a .part file; rename to dest only after a fully successful flush.
    let part: PathBuf = format!("{}.part", dest.display()).into();
    let file = match std::fs::File::create(&part) {
        Ok(f) => f,
        Err(_) => {
            progress.finish(4);
            let _ = heartbeat.join();
            let _ = std::fs::remove_file(&part);
            return (false, vec![]);
        }
    };
    let writer = file;

    // Write the 6-byte LRGEX magic header ("LRGEX" + version 0x01), then the 8-byte
    // uncompressed total, then the zstd stream. Legacy archives (pre-magic) have no
    // magic — the extractor detects them by zstd magic at byte 8.
    let mut header_writer = writer;
    let _ = header_writer.write_all(b"LRGEX\x01");
    let _ = header_writer.write_all(&total_bytes.to_le_bytes());

    let mut encoder = match zstd::Encoder::new(header_writer, 1) {
        Ok(e) => e,
        Err(_) => {
            progress.finish(4);
            let _ = heartbeat.join();
            let _ = std::fs::remove_file(&part);
            return (false, vec![]);
        }
    };
    // Multi-threaded compression — biggest win (~N x on N cores). Requires the
    // "zstdmt" feature AND this call; a comment alone does nothing.
    let threads = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(4);
    let _ = encoder.multithread(threads);
    let _ = encoder.include_checksum(false);
    // Tune multithread job sizing. At level 1 the default zstdmt job size lands around
    // 512KB-2MB — with 24 workers that means tons of tiny jobs and thread-handoff
    // overhead. Bump job size to 16MB so each worker does real work, and disable overlap
    // (OverlapSizeLog(0)) — overlap helps ratio at higher levels but at L1 we want
    // raw throughput.
    use zstd::stream::raw::CParameter;
    let _ = encoder.set_parameter(CParameter::JobSize(16 * 1024 * 1024));
    let _ = encoder.set_parameter(CParameter::OverlapSizeLog(0));
    let mut builder = tar::Builder::new(encoder.auto_finish());

    // Write the metadata sidecar as the FIRST entry (path-keyed packed blob of
    // ctime + attrs for every planned entry). Replaces 11k+ per-entry PAX headers.
    // See build_sidecar / SIDECAR_PATH. mtime stays in each entry's native tar field.
    let _ = write_sidecar(&mut builder, &files);

    let mut skipped: Vec<String> = Vec::new();
    let mut cancelled = false;

    for batch in files.chunks(BATCH) {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }
        let loaded: Vec<Option<std::io::Result<Vec<u8>>>> = batch
            .par_iter()
            .map(|e| {
                match e.kind {
                    EntKind::File if e.size <= BIG_FILE => {
                        let r = read_whole(&e.path, e.size);
                        progress.tick_bytes(e.size); // bytes tick during the parallel read
                        Some(r)
                    }
                    _ => None, // dir / symlink (no body) or large file (stream later)
                }
            })
            .collect();

        for (e, data) in batch.iter().zip(loaded.into_iter()) {
            if cancel.load(Ordering::Relaxed) {
                cancelled = true;
                break;
            }
            let res: std::io::Result<()> = match &e.kind {
                EntKind::Symlink { target } => {
                    // Symbolic link: tar native Symlink entry, link name = target. No body.
                    // If set_link_name fails (e.g. target path too long for tar), surface it
                    // as an error so the entry is logged to `skipped` — never silently drop
                    // a link, that would break the "preserved as-is" promise.
                    // (ctime + attrs come from the sidecar at the front of the archive — no PAX here.)
                    let mut h = tar::Header::new_gnu();
                    h.set_entry_type(tar::EntryType::Symlink);
                    h.set_size(0);
                    h.set_mode(0o777);
                    h.set_uid(0);
                    h.set_gid(0);
                    h.set_mtime(e.meta.mtime);
                    if let Err(e) = h.set_link_name(target) {
                        Err(e) // target unrepresentable → entry goes to skipped list
                    } else {
                        h.set_cksum();
                        builder.append_data(&mut h, &e.rel, std::io::empty())
                    }
                }
                EntKind::Dir => {
                    // Directory entry: header only, no content (preserves empty folders).
                    // (ctime + attrs come from the sidecar — no PAX here.)
                    let mut h = tar::Header::new_gnu();
                    h.set_entry_type(tar::EntryType::Directory);
                    h.set_size(0);
                    h.set_mode(0o755);
                    h.set_uid(0);
                    h.set_gid(0);
                    h.set_mtime(e.meta.mtime);
                    h.set_cksum();
                    builder.append_data(&mut h, &e.rel, std::io::empty())
                }
                EntKind::File => {
                    // (ctime + attrs come from the sidecar — no PAX here.)
                    match data {
                        Some(Ok(buf)) => {
                            let mut h = make_header(buf.len() as u64, e.meta.mtime, 0o644);
                            let mut slice: &[u8] = buf.as_slice();
                            builder.append_data(&mut h, &e.rel, &mut slice)
                        }
                        Some(Err(err)) => Err(err),
                        None => {
                            // Large file: stream through ByteReader so bytes tick during compress.
                            match std::fs::File::open(&e.path) {
                                Ok(f) => match f.metadata() {
                                    Ok(m) => {
                                        let mut h = make_header(m.len(), e.meta.mtime, 0o644);
                                        // BufReader between ByteReader and tar: tar reads in 512-byte
                                        // blocks, which would be billions of tiny read() syscalls on
                                        // a multi-GB file. BufReader batches them into 4 MB reads.
                                        let br = ByteReader::new(f, progress.clone());
                                        let mut buf = std::io::BufReader::with_capacity(4 * 1024 * 1024, br);
                                        builder.append_data(&mut h, &e.rel, &mut buf)
                                    }
                                    Err(err) => Err(err),
                                },
                                Err(err) => Err(err),
                            }
                        }
                    }
                }
            };

            if let Err(_) = res {
                skipped.push(e.path.to_string_lossy().to_string());
            }
        }
        if cancelled {
            break;
        }
    }

    if cancelled {
        // User cancelled mid-compress: release the .part handle, then delete the partial.
        drop(builder);
        let _ = std::fs::remove_file(&part);
        progress.finish(5); // cancelled
        let _ = heartbeat.join();
        return (false, skipped);
    }

    progress.set_phase(2); // flush
    let mut ok = builder.finish().is_ok();
    match builder.into_inner() {
        Ok(mut enc) => {
            if enc.flush().is_err() {
                ok = false;
            }
        }
        Err(_) => ok = false,
    }

    // Publish the archive as long as the flush succeeded. Skipped files (locked /
    // unreadable) are a WARNING, not a total failure — never discard a multi-GB job
    // over one locked file (WinRAR-style). The skip count is surfaced to the GUI.
    progress.set_skipped(skipped.len());
    let success = if ok {
        std::fs::rename(&part, dest).is_ok()
    } else {
        let _ = std::fs::remove_file(&part);
        false
    };
    // Sidecar listing unreadable files. On a CLEAN success (0 skips) delete any stale
    // sidecar from a previous run so it never falsely signals "files missing".
    let sidecar = format!("{}.skipped.txt", dest.display());
    if success {
        if skipped.is_empty() {
            let _ = std::fs::remove_file(&sidecar);
        } else {
            let _ = std::fs::write(&sidecar, skipped.join("\r\n"));
        }
    }
    // finish() writes the terminal snapshot BEFORE stop; heartbeat.join() guarantees it
    // is on disk before we return.
    progress.finish(if success { 3 } else { 4 });
    let _ = heartbeat.join();

    (success, skipped)
}

/// Compress MULTIPLE inputs (multi-select) into ONE .zgx. Each input is walked and
/// appended to the same tar stream; entry paths are relative to the shared parent so
/// the archive opens with a clean tree. Empty dirs across inputs are preserved.
/// `label` is the display name shown in the progress window (e.g. parent folder name).
pub fn compress_paths(
    inputs: &[PathBuf],
    dest: &Path,
    label: &str,
    cancel: &AtomicBool,
) -> (bool, Vec<String>) {
    progress::clear_status();
    let progress = Progress::new(label);
    let heartbeat = progress.spawn_writer();
    progress.set_phase(0); // walk

    // Walk every input into one entries list. Each input's entries are prefixed with
    // the input's own leaf name so the archive has a clean per-input tree.
    let mut files: Vec<FileEnt> = Vec::new();
    let mut total_bytes: u64 = 0;
    for input in inputs {
        if input.is_dir() {
            let prefix = PathBuf::from(input.file_name().unwrap_or_else(|| std::ffi::OsStr::new(".")));
            let (mut entries, bytes, _) = walk_tree(input, &[]);
            for e in entries.drain(..) {
                // Prefix each entry's rel with the input folder's leaf name so the
                // archive opens to one root folder per input (clean tree).
                let new_rel = prefix.join(&e.rel);
                files.push(FileEnt { path: e.path, rel: new_rel, size: e.size, kind: e.kind, meta: e.meta });
            }
            total_bytes += bytes;
        } else if input.is_file() {
            let size = std::fs::metadata(input).map(|m| m.len()).unwrap_or(0);
            let rel = input
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| input.clone());
            let meta = metaattr::read_meta(input);
            files.push(FileEnt { path: input.clone(), rel, size, kind: EntKind::File, meta });
            total_bytes += size;
        }
        // Skip silently if the input vanished between select and launch.
    }
    let count = files.len();
    progress.set_totals(count, total_bytes);
    progress.set_phase(1); // compress

    let part: PathBuf = format!("{}.part", dest.display()).into();
    let file = match std::fs::File::create(&part) {
        Ok(f) => f,
        Err(_) => {
            progress.finish(4);
            let _ = heartbeat.join();
            let _ = std::fs::remove_file(&part);
            return (false, vec![]);
        }
    };
    let writer = file;

    // Write the 6-byte LRGEX magic header + 8-byte uncompressed total before the zstd stream.
    use std::io::Write;
    let mut header_writer = writer;
    let _ = header_writer.write_all(b"LRGEX\x01");
    let _ = header_writer.write_all(&total_bytes.to_le_bytes());

    let mut encoder = match zstd::Encoder::new(header_writer, 1) {
        Ok(e) => e,
        Err(_) => {
            progress.finish(4);
            let _ = heartbeat.join();
            let _ = std::fs::remove_file(&part);
            return (false, vec![]);
        }
    };
    let threads = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(4);
    let _ = encoder.multithread(threads);
    let _ = encoder.include_checksum(false);
    use zstd::stream::raw::CParameter;
    let _ = encoder.set_parameter(CParameter::JobSize(16 * 1024 * 1024));
    let _ = encoder.set_parameter(CParameter::OverlapSizeLog(0));
    let mut builder = tar::Builder::new(encoder.auto_finish());

    // Write the metadata sidecar as the FIRST entry. See build_sidecar / SIDECAR_PATH.
    let _ = write_sidecar(&mut builder, &files);

    let mut skipped: Vec<String> = Vec::new();
    let mut cancelled = false;

    for batch in files.chunks(BATCH) {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }
        let loaded: Vec<Option<std::io::Result<Vec<u8>>>> = batch
            .par_iter()
            .map(|e| {
                match e.kind {
                    EntKind::File if e.size <= BIG_FILE => {
                        let r = read_whole(&e.path, e.size);
                        progress.tick_bytes(e.size);
                        Some(r)
                    }
                    _ => None,
                }
            })
            .collect();

        for (e, data) in batch.iter().zip(loaded.into_iter()) {
            if cancel.load(Ordering::Relaxed) {
                cancelled = true;
                break;
            }
            let res: std::io::Result<()> = match &e.kind {
                EntKind::Symlink { target } => {
                    // (ctime + attrs come from the sidecar — no PAX here.)
                    let mut h = tar::Header::new_gnu();
                    h.set_entry_type(tar::EntryType::Symlink);
                    h.set_size(0);
                    h.set_mode(0o777);
                    h.set_uid(0);
                    h.set_gid(0);
                    h.set_mtime(e.meta.mtime);
                    if let Err(e) = h.set_link_name(target) {
                        Err(e) // target unrepresentable → entry goes to skipped list
                    } else {
                        h.set_cksum();
                        builder.append_data(&mut h, &e.rel, std::io::empty())
                    }
                }
                EntKind::Dir => {
                    // (ctime + attrs come from the sidecar — no PAX here.)
                    let mut h = tar::Header::new_gnu();
                    h.set_entry_type(tar::EntryType::Directory);
                    h.set_size(0);
                    h.set_mode(0o755);
                    h.set_uid(0);
                    h.set_gid(0);
                    h.set_mtime(e.meta.mtime);
                    h.set_cksum();
                    let dir_name = format!("{}/", e.rel.to_string_lossy().replace('\\', "/"));
                    builder.append_data(&mut h, dir_name, std::io::empty())
                }
                EntKind::File => {
                    // (ctime + attrs come from the sidecar — no PAX here.)
                    match data {
                        Some(Ok(buf)) => {
                            let mut h = make_header(buf.len() as u64, e.meta.mtime, 0o644);
                            let mut slice: &[u8] = buf.as_slice();
                            builder.append_data(&mut h, &e.rel, &mut slice)
                        }
                        Some(Err(err)) => Err(err),
                        None => match std::fs::File::open(&e.path) {
                            Ok(f) => match f.metadata() {
                                Ok(m) => {
                                    let mut h = make_header(m.len(), e.meta.mtime, 0o644);
                                    let br = ByteReader::new(f, progress.clone());
                                    let mut buf = std::io::BufReader::with_capacity(4 * 1024 * 1024, br);
                                    builder.append_data(&mut h, &e.rel, &mut buf)
                                }
                                Err(err) => Err(err),
                            },
                            Err(err) => Err(err),
                        },
                    }
                }
            };
            if let Err(_) = res {
                skipped.push(e.path.to_string_lossy().to_string());
            }
        }
        if cancelled {
            break;
        }
    }

    if cancelled {
        drop(builder);
        let _ = std::fs::remove_file(&part);
        progress.finish(5);
        let _ = heartbeat.join();
        return (false, skipped);
    }

    progress.set_phase(2);
    let mut ok = builder.finish().is_ok();
    match builder.into_inner() {
        Ok(mut enc) => {
            if enc.flush().is_err() {
                ok = false;
            }
        }
        Err(_) => ok = false,
    }
    progress.set_skipped(skipped.len());
    let success = if ok {
        std::fs::rename(&part, dest).is_ok()
    } else {
        let _ = std::fs::remove_file(&part);
        false
    };
    let sidecar = format!("{}.skipped.txt", dest.display());
    if success {
        if skipped.is_empty() {
            let _ = std::fs::remove_file(&sidecar);
        } else {
            let _ = std::fs::write(&sidecar, skipped.join("\r\n"));
        }
    }
    progress.finish(if success { 3 } else { 4 });
    let _ = heartbeat.join();
    (success, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn empty_directories_are_preserved() {
        let tmp = std::env::temp_dir().join(format!("lrgex-emptydir-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let root = tmp.join("root");
        std::fs::create_dir_all(root.join("empty_sub")).unwrap();
        std::fs::create_dir_all(root.join("has")).unwrap();
        std::fs::write(root.join("has/a.txt"), b"hi").unwrap();

        let archive = tmp.join("out.zgx");
        let cancel = AtomicBool::new(false);
        let (ok, skipped) = compress_folder(&root, &archive, &[], &cancel);
        assert!(ok, "compress failed");
        assert!(skipped.is_empty(), "files skipped: {:?}", skipped);

        // 1) The empty dir MUST be an entry in the archive. Skip the 6-byte LRGEX magic
        //    + 8-byte uncompressed-total header (14 bytes total) before the zstd stream.
        let f = std::fs::File::open(&archive).unwrap();
        let mut buf = std::io::BufReader::new(f);
        let mut header = [0u8; 14];
        use std::io::Read;
        let _ = buf.read_exact(&mut header); // consume the 14-byte header
        let dec = zstd::Decoder::new(buf).unwrap();
        let mut tar = tar::Archive::new(dec);
        let names: Vec<String> = tar
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().display().to_string())
            .collect();
        assert!(names.iter().any(|p| p == "empty_sub"), "empty_sub entry missing; entries = {:?}", names);

        // 2) Extraction MUST recreate the empty dir (and it must be empty).
        let out = tmp.join("extracted");
        std::fs::create_dir_all(&out).unwrap();
        let f2 = std::fs::File::open(&archive).unwrap();
        let mut buf2 = std::io::BufReader::new(f2);
        let mut header2 = [0u8; 14];
        let _ = buf2.read_exact(&mut header2);
        let dec2 = zstd::Decoder::new(buf2).unwrap();
        tar::Archive::new(dec2).unpack(&out).unwrap();
        let empty_sub = out.join("empty_sub");
        assert!(empty_sub.is_dir(), "empty_sub not recreated on extract");
        assert!(std::fs::read_dir(&empty_sub).unwrap().next().is_none(), "empty_sub not empty");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
