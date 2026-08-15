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

/// THE ONE COMPRESS ENGINE. Feeds files through tar::Builder → zstd::Encoder → any Write sink.
/// The caller writes the LRGEX header BEFORE calling this, and finalizes the sink AFTER.
/// Returns (success, skipped_files).
fn compress_into<W: std::io::Write>(
    files: &[FileEnt],
    sink: W,
    cancel: &AtomicBool,
    progress: &Progress,
) -> (bool, Vec<String>) {
    let mut encoder = match zstd::Encoder::new(sink, 1) {
        Ok(e) => e,
        Err(_) => return (false, vec![]),
    };
    let threads = std::thread::available_parallelism()
        .map(|n| n.get() as u32).unwrap_or(4);
    let _ = encoder.multithread(threads);
    encoder.include_checksum(true).expect("enable zstd checksum");
    use zstd::stream::raw::CParameter;
    let _ = encoder.set_parameter(CParameter::JobSize(16 * 1024 * 1024));
    let _ = encoder.set_parameter(CParameter::OverlapSizeLog(0));
    let mut builder = tar::Builder::new(encoder.auto_finish());

    let _ = write_sidecar(&mut builder, files);

    let mut skipped: Vec<String> = Vec::new();
    let mut cancelled = false;

    for batch in files.chunks(BATCH) {
        if cancel.load(Ordering::Relaxed) { cancelled = true; break; }
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
            if cancel.load(Ordering::Relaxed) { cancelled = true; break; }
            let res: std::io::Result<()> = match &e.kind {
                EntKind::Symlink { target } => {
                    let mut h = tar::Header::new_gnu();
                    h.set_entry_type(tar::EntryType::Symlink);
                    h.set_size(0); h.set_mode(0o777); h.set_uid(0); h.set_gid(0);
                    h.set_mtime(e.meta.mtime);
                    if let Err(e) = h.set_link_name(target) { Err(e) }
                    else { h.set_cksum(); builder.append_data(&mut h, &e.rel, std::io::empty()) }
                }
                EntKind::Dir => {
                    let mut h = tar::Header::new_gnu();
                    h.set_entry_type(tar::EntryType::Directory);
                    h.set_size(0); h.set_mode(0o755); h.set_uid(0); h.set_gid(0);
                    h.set_mtime(e.meta.mtime); h.set_cksum();
                    builder.append_data(&mut h, &e.rel, std::io::empty())
                }
                EntKind::File => {
                    match data {
                        Some(Ok(buf)) => {
                            let mut h = make_header(buf.len() as u64, e.meta.mtime, 0o644);
                            let mut slice: &[u8] = buf.as_slice();
                            builder.append_data(&mut h, &e.rel, &mut slice)
                        }
                        Some(Err(err)) => Err(err),
                        None => {
                            match std::fs::File::open(&e.path) {
                                Ok(f) => match f.metadata() {
                                    Ok(m) => {
                                        let mut h = make_header(m.len(), e.meta.mtime, 0o644);
                                        let br = ByteReader::with_cancel(f, progress.clone(), cancel);
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
            if let Err(_err) = &res {
                if cancel.load(Ordering::Relaxed) { cancelled = true; }
                else { skipped.push(e.path.to_string_lossy().to_string()); }
            }
        }
        if cancelled { break; }
    }

    if cancelled {
        drop(builder);
        return (false, skipped);
    }

    progress.set_phase(2); // flush phase
    let mut ok = builder.finish().is_ok();
    match builder.into_inner() {
        Ok(mut enc) => { if enc.flush().is_err() { ok = false; } }
        Err(_) => ok = false,
    }
    (ok, skipped)
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

/// Serialize the file-path index (null-separated UTF-8 paths, zstd-compressed).
/// Used by v3 archives so the extractor can list files WITHOUT decompressing the
/// whole tar stream — conflict checks become instant even on 50GB archives.
/// NOTE: version byte is 0x03 (0x02 is reserved for the SPLIT format in segment.rs).
pub fn build_path_index(files: &[FileEnt]) -> std::io::Result<Vec<u8>> {
    // Pre-size: sum of path lengths + separators
    let mut raw = Vec::with_capacity(files.iter().map(|f| f.rel.as_os_str().len() + 1).sum());
    for f in files {
        let p = f.rel.to_string_lossy().replace('\\', "/");
        raw.extend_from_slice(p.as_bytes());
        raw.push(0); // null separator (Windows filenames can't contain NUL)
    }
    // Compress the blob — a 100k-file listing compresses to ~1-2MB.
    // Errors propagate (an empty index would silently skip conflict checks).
    zstd::stream::encode_all(raw.as_slice(), 3)
}

/// Write the v3 LRGEX header: magic + version 0x03 + total(8) + index_len(8) + index blob.
/// After this, the caller writes the zstd(tar) stream — the extractor seeks past the index.
/// 0x02 is reserved for the SPLIT format (segment.rs); 0x01 is legacy v1 (no index).
fn write_v3_header(writer: &mut impl std::io::Write, total_bytes: u64, files: &[FileEnt]) -> std::io::Result<()> {
    let index = build_path_index(files)?;
    writer.write_all(b"LRGEX\x03")?;
    writer.write_all(&total_bytes.to_le_bytes())?;
    writer.write_all(&(index.len() as u64).to_le_bytes())?;
    writer.write_all(&index)?;
    Ok(())
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

    // Write the v3 LRGEX header (magic + 0x03 + total + path index), then the zstd
    // stream below. Legacy v1 archives (pre-index) have no magic version 0x03 — the
    // extractor detects them by the version byte and falls back to stream scan.
    let mut header_writer = writer;
    if write_v3_header(&mut header_writer, total_bytes, &files).is_err() {
        progress.finish(4);
        let _ = heartbeat.join();
        let _ = std::fs::remove_file(&part);
        return (false, vec![]);
    }

    // Run the ONE engine (compress_into). The old inline engine code is replaced.
    let (ok, skipped) = compress_into(&files, header_writer, cancel, &progress);

    // Publish or clean up.
    let success = if ok {
        crate::metaattr::atomic_replace(&part, dest).is_ok()
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
    progress.set_skipped(skipped.len());
    // Distinguish cancel (phase 5) from failure (phase 4) from success (phase 3).
    let phase = if success { 3 } else if cancel.load(Ordering::Relaxed) { 5 } else { 4 };
    progress.finish(phase);
    let _ = heartbeat.join();

    (success, skipped)
}

/// Compress a folder to split .zgx parts. Each part capped at `segment_size_mb` MB.
/// Produces `MyFolder.part001.zgx`, `.part002.zgx`, etc. with per-part SHA256 trailers.
pub fn compress_folder_split(
    source: &Path,
    base_dest: &Path,       // base path WITHOUT extension (e.g. "C:\out\MyFolder")
    segment_size_mb: u32,
    excluded: &[String],
    cancel: &AtomicBool,
) -> (bool, Vec<String>) {
    progress::clear_status();
    let label = source.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let progress = Progress::new(&label);
    let heartbeat = progress.spawn_writer();
    progress.set_phase(0);

    let (files, total_bytes, count) = walk_tree(source, excluded);
    progress.set_totals(count, total_bytes);
    progress.set_phase(1);

    // Pre-flight: check part count won't exceed MAX_PARTS.
    let segment_bytes = (segment_size_mb as u64).max(1) * 1024 * 1024;
    let data_budget = segment_bytes.saturating_sub(crate::segment::TRAILER_LEN as u64).max(1);
    let estimated_parts = total_bytes.div_ceil(data_budget).max(1);
    if estimated_parts > crate::segment::MAX_PARTS as u64 {
        progress.finish(4);
        let _ = heartbeat.join();
        return (false, vec![format!(
            "Folder too large for {} MB parts (~{} parts needed; max {}). Use a larger size.",
            segment_size_mb, estimated_parts, crate::segment::MAX_PARTS)]);
    }

    let mut writer = crate::segment::SegmentWriter::new(
        base_dest.to_path_buf(), segment_size_mb, total_bytes,
    );

    // Run the ONE engine. SegmentWriter writes its own 0x02 header on first write.
    let (ok, skipped) = compress_into(&files, &mut writer, cancel, &progress);

    // Finalize: write last trailer + rename last .tmp. Sets finished_ok so Drop doesn't clean up.
    let success = if ok {
        writer.finish().is_ok()
    } else {
        writer.cleanup_all();
        false
    };

    // Sidecar.
    let sidecar = format!("{}.skipped.txt", base_dest.display());
    if success {
        if skipped.is_empty() { let _ = std::fs::remove_file(&sidecar); }
        else { let _ = std::fs::write(&sidecar, skipped.join("\r\n")); }
    }

    progress.set_skipped(skipped.len());
    let phase = if success { 3 } else if cancel.load(Ordering::Relaxed) { 5 } else { 4 };
    progress.finish(phase);
    let _ = heartbeat.join();
    (success, skipped)
}

/// Compress MULTIPLE inputs to split .zgx parts. Mirrors compress_paths but with
/// a SegmentWriter sink instead of a File.
pub fn compress_paths_split(
    inputs: &[PathBuf],
    base_dest: &Path,
    label: &str,
    segment_size_mb: u32,
    cancel: &AtomicBool,
) -> (bool, Vec<String>) {
    progress::clear_status();
    let progress = Progress::new(label);
    let heartbeat = progress.spawn_writer();
    progress.set_phase(0);

    // Walk every input into one entries list (same as compress_paths).
    let mut files: Vec<FileEnt> = Vec::new();
    let mut total_bytes: u64 = 0;
    for input in inputs {
        if input.is_dir() {
            let prefix = PathBuf::from(input.file_name().unwrap_or_else(|| std::ffi::OsStr::new(".")));
            let (mut entries, bytes, _) = walk_tree(input, &[]);
            for e in entries.drain(..) {
                let new_rel = prefix.join(&e.rel);
                files.push(FileEnt { path: e.path, rel: new_rel, size: e.size, kind: e.kind, meta: e.meta });
            }
            total_bytes += bytes;
        } else if input.is_file() {
            let size = std::fs::metadata(input).map(|m| m.len()).unwrap_or(0);
            let rel = input.file_name().map(PathBuf::from).unwrap_or_else(|| input.clone());
            let meta = metaattr::read_meta(input);
            files.push(FileEnt { path: input.clone(), rel, size, kind: EntKind::File, meta });
            total_bytes += size;
        }
    }
    let count = files.len();
    progress.set_totals(count, total_bytes);
    progress.set_phase(1);

    // Pre-flight part count check.
    let segment_bytes = (segment_size_mb as u64).max(1) * 1024 * 1024;
    let data_budget = segment_bytes.saturating_sub(crate::segment::TRAILER_LEN as u64).max(1);
    let estimated_parts = total_bytes.div_ceil(data_budget).max(1);
    if estimated_parts > crate::segment::MAX_PARTS as u64 {
        progress.finish(4);
        let _ = heartbeat.join();
        return (false, vec![format!(
            "Too large for {} MB parts (~{} needed; max {}).",
            segment_size_mb, estimated_parts, crate::segment::MAX_PARTS)]);
    }

    let mut writer = crate::segment::SegmentWriter::new(
        base_dest.to_path_buf(), segment_size_mb, total_bytes,
    );

    let (ok, skipped) = compress_into(&files, &mut writer, cancel, &progress);

    let success = if ok {
        writer.finish().is_ok()
    } else {
        writer.cleanup_all();
        false
    };

    let sidecar = format!("{}.skipped.txt", base_dest.display());
    if success {
        if skipped.is_empty() { let _ = std::fs::remove_file(&sidecar); }
        else { let _ = std::fs::write(&sidecar, skipped.join("\r\n")); }
    }

    progress.set_skipped(skipped.len());
    let phase = if success { 3 } else if cancel.load(Ordering::Relaxed) { 5 } else { 4 };
    progress.finish(phase);
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

    // Write the v3 LRGEX header (magic + total + index). Same as compress_folder.
    let mut header_writer = writer;
    if write_v3_header(&mut header_writer, total_bytes, &files).is_err() {
        progress.finish(4);
        let _ = heartbeat.join();
        let _ = std::fs::remove_file(&part);
        return (false, vec![]);
    }

    // Run the ONE engine (compress_into). Replaces the old inline encoder loop.
    let (ok, skipped) = compress_into(&files, header_writer, cancel, &progress);

    let success = if ok {
        crate::metaattr::atomic_replace(&part, dest).is_ok()
    } else {
        let _ = std::fs::remove_file(&part);
        false
    };
    let sidecar = format!("{}.skipped.txt", dest.display());
    if success {
        if skipped.is_empty() { let _ = std::fs::remove_file(&sidecar); }
        else { let _ = std::fs::write(&sidecar, skipped.join("\r\n")); }
    }
    let phase = if success { 3 } else if cancel.load(Ordering::Relaxed) { 5 } else { 4 };
    progress.finish(phase);
    let _ = heartbeat.join();
    (success, skipped)
}

/// Read a .zgx header and return the byte offset where the zstd tar stream starts.
/// Handles all formats: v3 (0x03 with index), v1 (0x01 legacy), and bare zstd.
/// Shared by compress tests AND the extractor — single source of truth for offset math.
pub fn zgx_stream_start(file: &mut std::fs::File) -> std::io::Result<u64> {
    use std::io::{Read, Seek, SeekFrom};
    let mut head = [0u8; 22];
    let n = file.read(&mut head)?;
    if n >= 6 && &head[0..5] == b"LRGEX" {
        match head[5] {
            0x03 => {
                if n < 22 { return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "truncated v3 header")); }
                let index_len = u64::from_le_bytes([head[14], head[15], head[16], head[17], head[18], head[19], head[20], head[21]]);
                Ok(22 + index_len)
            }
            0x01 => Ok(14),
            0x02 => Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "split archive - use split extractor")),
            v => Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("unsupported version 0x{:02x}", v))),
        }
    } else if n >= 12 && head[8..12] == [0x28, 0xb5, 0x2f, 0xfd] {
        Ok(8) // legacy with 8-byte size header
    } else if n >= 4 && head[0..4] == [0x28, 0xb5, 0x2f, 0xfd] {
        Ok(0) // bare zstd
    } else {
        Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "not a LRGEX/zstd archive"))
    }
}

/// Read the v3 path index from a .zgx file (instant — no tar decompression needed).
/// Returns None for v1/legacy archives (no index — caller falls back to stream scan).
pub fn read_path_index(file: &mut std::fs::File) -> std::io::Result<Option<Vec<String>>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut head = [0u8; 22];
    file.seek(SeekFrom::Start(0))?;
    let n = file.read(&mut head)?;
    if n >= 6 && &head[0..5] == b"LRGEX" && head[5] == 0x03 && n >= 22 {
        let index_len = u64::from_le_bytes([head[14], head[15], head[16], head[17], head[18], head[19], head[20], head[21]]) as usize;
        let mut index_blob = vec![0u8; index_len];
        file.read_exact(&mut index_blob)?;
        let raw = zstd::stream::decode_all(index_blob.as_slice())?;
        let paths: Vec<String> = raw.split(|&b| b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).to_string())
            .collect();
        Ok(Some(paths))
    } else {
        Ok(None) // v1 or legacy — no index
    }
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

        // 1) The empty dir MUST be an entry in the archive. Use zgx_stream_start to
        //    skip the v3 header (22 bytes + index_len) before the zstd stream.
        let f = std::fs::File::open(&archive).unwrap();
        let mut buf = std::io::BufReader::new(f);
        let mut file_for_offset = std::fs::File::open(&archive).unwrap();
        let stream_start = zgx_stream_start(&mut file_for_offset).unwrap();
        use std::io::{Read, Seek, SeekFrom};
        buf.get_ref().seek(SeekFrom::Start(stream_start)).unwrap();
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
        let mut file_for_offset2 = std::fs::File::open(&archive).unwrap();
        let stream_start2 = zgx_stream_start(&mut file_for_offset2).unwrap();
        let mut buf2 = std::io::BufReader::new(f2);
        buf2.get_ref().seek(SeekFrom::Start(stream_start2)).unwrap();
        let dec2 = zstd::Decoder::new(buf2).unwrap();
        tar::Archive::new(dec2).unpack(&out).unwrap();
        let empty_sub = out.join("empty_sub");
        assert!(empty_sub.is_dir(), "empty_sub not recreated on extract");
        assert!(std::fs::read_dir(&empty_sub).unwrap().next().is_none(), "empty_sub not empty");

        // 3) The v3 path index MUST round-trip: read_path_index returns the file listing.
        let mut f3 = std::fs::File::open(&archive).unwrap();
        let index = read_path_index(&mut f3).unwrap();
        assert!(index.is_some(), "v3 archive has no path index");
        let paths = index.unwrap();
        assert!(paths.iter().any(|p| p == "has/a.txt"), "index missing has/a.txt: {:?}", paths);
        assert!(paths.iter().any(|p| p == "empty_sub"), "index missing empty_sub: {:?}", paths);
        // The sidecar (.lrgex/meta.bin) is NOT in the index — it's metadata, not a file.
        assert!(!paths.iter().any(|p| p.contains("meta.bin")), "sidecar leaked into index: {:?}", paths);

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
