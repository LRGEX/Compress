// Compression (tar + zstd) — adapted from LRGEX Restore sync.rs.
// Hardcoded engine: zstd level 1, rayon parallel reads (all cores, batch 2048),
// ByteReader for files >8 MB. Empty directories ARE archived (preserved on extract).
//
// ATOMIC WRITE: compress to <dest>.part, rename to <dest> only on full success —
// so an interrupted / panicked / OOM-killed / force-closed compress can NEVER leave a
// complete-looking archive behind. Worst case is an obviously-incomplete `.part`.

use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;

use crate::progress::{self, ByteReader, Progress};

const BIG_FILE: u64 = 8 * 1024 * 1024; // stream these instead of preloading
const BATCH: usize = 2048; // parallel-read batch size

pub struct FileEnt {
    pub path: PathBuf,
    pub rel: PathBuf,
    pub size: u64,
    pub is_dir: bool,
}

/// Single recursive walk. Returns (entries, total_bytes, count). Archives files AND
/// directories (so empty folders are preserved). Uses DirEntry::metadata() which on
/// Windows is served from the enumeration cache.
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

        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if ft.is_symlink() {
            continue;
        }

        let path = entry.path();
        if ft.is_dir() {
            // Archive the directory entry itself (preserves EMPTY folders), then recurse.
            let rel = path.strip_prefix(base).unwrap_or(&path).to_path_buf();
            out.push(FileEnt { path: path.clone(), rel, size: 0, is_dir: true });
            walk_inner(base, &path, excluded, out, total);
        } else {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            *total += size;
            let rel = path.strip_prefix(base).unwrap_or(&path).to_path_buf();
            out.push(FileEnt { path, rel, size, is_dir: false });
        }
    }
}

/// Deterministic tar header (mtime/uid/gid zeroed, mode 0o644).
fn make_header(size: u64) -> tar::Header {
    let mut h = tar::Header::new_gnu();
    h.set_entry_type(tar::EntryType::Regular);
    h.set_size(size);
    h.set_mode(0o644);
    h.set_uid(0);
    h.set_gid(0);
    h.set_mtime(0);
    h.set_cksum();
    h
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
        (vec![FileEnt { path: source.to_path_buf(), rel, size, is_dir: false }], size, 1)
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
    let writer = BufWriter::with_capacity(4 * 1024 * 1024, file);

    // Write 8-byte little-endian uncompressed total as a header BEFORE the zstd stream.
    // Extraction reads this to set accurate progress totals without a pre-scan.
    use std::io::Write;
    let mut header_writer = writer;
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
    let mut builder = tar::Builder::new(encoder.auto_finish());

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
                if e.is_dir || e.size > BIG_FILE {
                    None // directory (no content) or large file (stream later)
                } else {
                    let r = read_whole(&e.path, e.size);
                    progress.tick_bytes(e.size); // bytes tick during the parallel read
                    Some(r)
                }
            })
            .collect();

        for (e, data) in batch.iter().zip(loaded.into_iter()) {
            if cancel.load(Ordering::Relaxed) {
                cancelled = true;
                break;
            }
            let res: std::io::Result<()> = if e.is_dir {
                // Directory entry: header only, no content (preserves empty folders).
                let mut h = tar::Header::new_gnu();
                h.set_entry_type(tar::EntryType::Directory);
                h.set_size(0);
                h.set_mode(0o755);
                h.set_uid(0);
                h.set_gid(0);
                h.set_mtime(0);
                h.set_cksum();
                builder.append_data(&mut h, &e.rel, &mut std::io::empty())
            } else {
                match data {
                    Some(Ok(buf)) => {
                        let mut h = make_header(buf.len() as u64);
                        let mut slice: &[u8] = buf.as_slice();
                        builder.append_data(&mut h, &e.rel, &mut slice)
                    }
                    Some(Err(err)) => Err(err),
                    None => {
                        // Large file: stream through ByteReader so bytes tick during compress.
                        match std::fs::File::open(&e.path) {
                            Ok(f) => match f.metadata() {
                                Ok(m) => {
                                    let mut h = make_header(m.len());
                                    let mut br = ByteReader::new(f, progress.clone());
                                    builder.append_data(&mut h, &e.rel, &mut br)
                                }
                                Err(err) => Err(err),
                            },
                            Err(err) => Err(err),
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
                files.push(FileEnt { path: e.path, rel: new_rel, size: e.size, is_dir: e.is_dir });
            }
            total_bytes += bytes;
        } else if input.is_file() {
            let size = std::fs::metadata(input).map(|m| m.len()).unwrap_or(0);
            let rel = input
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| input.clone());
            files.push(FileEnt { path: input.clone(), rel, size, is_dir: false });
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
    let writer = std::io::BufWriter::with_capacity(4 * 1024 * 1024, file);

    // Write 8-byte little-endian uncompressed total as a header BEFORE the zstd stream.
    use std::io::Write;
    let mut header_writer = writer;
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
    let mut builder = tar::Builder::new(encoder.auto_finish());

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
                if e.is_dir || e.size > BIG_FILE {
                    None
                } else {
                    let r = read_whole(&e.path, e.size);
                    progress.tick_bytes(e.size);
                    Some(r)
                }
            })
            .collect();

        for (e, data) in batch.iter().zip(loaded.into_iter()) {
            if cancel.load(Ordering::Relaxed) {
                cancelled = true;
                break;
            }
            let res: std::io::Result<()> = if e.is_dir {
                let mut h = tar::Header::new_gnu();
                h.set_entry_type(tar::EntryType::Directory);
                h.set_size(0);
                h.set_mode(0o755);
                h.set_uid(0);
                h.set_gid(0);
                h.set_mtime(0);
                h.set_cksum();
                let dir_name = format!("{}/", e.rel.to_string_lossy().replace('\\', "/"));
                builder.append_data(&mut h, dir_name, &mut std::io::empty())
            } else {
                match data {
                    Some(Ok(buf)) => {
                        let mut h = make_header(buf.len() as u64);
                        let mut slice: &[u8] = buf.as_slice();
                        builder.append_data(&mut h, &e.rel, &mut slice)
                    }
                    Some(Err(err)) => Err(err),
                    None => match std::fs::File::open(&e.path) {
                        Ok(f) => match f.metadata() {
                            Ok(m) => {
                                let mut h = make_header(m.len());
                                let mut br = ByteReader::new(f, progress.clone());
                                builder.append_data(&mut h, &e.rel, &mut br)
                            }
                            Err(err) => Err(err),
                        },
                        Err(err) => Err(err),
                    },
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

        // 1) The empty dir MUST be an entry in the archive.
        let f = std::fs::File::open(&archive).unwrap();
        let dec = zstd::Decoder::new(f).unwrap();
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
        let dec2 = zstd::Decoder::new(f2).unwrap();
        tar::Archive::new(dec2).unpack(&out).unwrap();
        let empty_sub = out.join("empty_sub");
        assert!(empty_sub.is_dir(), "empty_sub not recreated on extract");
        assert!(std::fs::read_dir(&empty_sub).unwrap().next().is_none(), "empty_sub not empty");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
