// Extraction — multi-format dispatcher.
// Supported: .zgx (tar+zstd), .zip, .rar.
// Format detection: magic bytes first (reliable), extension fallback.

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
            // zstd — 0x28 0xb5 0x2f 0xfd.
            if head[0..4] == [0x28, 0xb5, 0x2f, 0xfd] {
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

    // Read the 8-byte uncompressed total header written during compression.
    // Detect old archives (no header) by checking if first 4 bytes are zstd magic (28 b5 2f fd).
    // If so, skip the header read and use compressed size as fallback.
    use std::io::{Read, Seek, SeekFrom};
    let mut file = file;
    let mut magic = [0u8; 4];
    let uncompressed_total = if file.read_exact(&mut magic).is_ok() {
        if magic == [0x28, 0xb5, 0x2f, 0xfd] {
            // Old archive — first 4 bytes are zstd magic, no header.
            // Seek back to start so zstd decoder can read from the beginning.
            let _ = file.seek(SeekFrom::Start(0));
            arch_size
        } else {
            // New archive — first 8 bytes are the uncompressed total.
            // Read the remaining 4 bytes of the header.
            let mut rest = [0u8; 4];
            if file.read_exact(&mut rest).is_ok() {
                let mut header = [0u8; 8];
                header[..4].copy_from_slice(&magic);
                header[4..].copy_from_slice(&rest);
                let val = u64::from_le_bytes(header);
                if val > 0 && val < arch_size * 100 { val } else { arch_size }
            } else {
                let _ = file.seek(SeekFrom::Start(0));
                arch_size
            }
        }
    } else {
        arch_size
    };
    prog.set_totals(0, uncompressed_total);

    // Count UNCOMPRESSED bytes on the decoder output side — matches the uncompressed total.
    let decoder = match zstd::Decoder::new(file) {
        Ok(d) => d,
        Err(e) => {
            prog.finish(4);
            let _ = heartbeat.join();
            return (false, format!("corrupt archive (zstd): {}", e));
        }
    };
    // Wrap decoder output in ByteReader to tick uncompressed bytes — matches total.
    let counting = ByteReader::new(decoder, prog.clone());
    // BufReader between decoder and tar — tar reads in 512-byte chunks;
    // 256KB is plenty, 4MB just evicts cache.
    let buf_decoder = std::io::BufReader::with_capacity(256 * 1024, counting);
    let mut tar = tar::Archive::new(buf_decoder);

    let _ = std::fs::create_dir_all(dest);
    // Batch-parallel extraction: accumulate entries sequentially (tar is a stream),
    // flush in batches via rayon. Large files stream directly. No channel/Mutex.
    let result = (|| -> Result<(), String> {
        use std::io::Read;
        use rayon::prelude::*;

        const BATCH_ENTRIES: usize = 2048;
        const BATCH_BYTES: usize = 64 * 1024 * 1024;
        const STREAM_THRESHOLD: u64 = 1024 * 1024;

        let entries = tar.entries().map_err(|e| format!("read entries: {}", e))?;
        let mut dir_cache: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        dir_cache.insert(dest.to_path_buf());

        let mut batch: Vec<(PathBuf, Vec<u8>)> = Vec::with_capacity(BATCH_ENTRIES);
        let mut batch_bytes: usize = 0;

        // Helper: create dirs sequentially, then parallel-write the batch.
        let flush_batch = |batch: &mut Vec<(PathBuf, Vec<u8>)>,
                          batch_bytes: &mut usize,
                          dir_cache: &mut std::collections::HashSet<PathBuf>| -> Result<(), String> {
            if batch.is_empty() { return Ok(()); }

            // 1. Directories created SEQUENTIALLY first, deduped.
            for (path, _) in batch.iter() {
                if let Some(parent) = path.parent() {
                    if dir_cache.insert(parent.to_path_buf()) {
                        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {}", e))?;
                    }
                }
            }

            // 2. Writes in parallel. No channel, no Mutex, no shared state.
            let errs: Vec<_> = batch
                .par_iter()
                .filter_map(|(path, data)| std::fs::write(path, data).err())
                .collect();

            batch.clear();
            *batch_bytes = 0;

            match errs.into_iter().next() {
                Some(e) => Err(format!("write batch: {}", e)),
                None => Ok(()),
            }
        };

        for entry in entries {
            let mut entry = entry.map_err(|e| format!("entry read: {}", e))?;
            let rel = entry.path().map_err(|e| format!("path: {}", e))?.to_path_buf();
            let outpath = dest.join(&rel);

            // Path-traversal guard
            if rel.is_absolute() { continue; }
            if rel.components().any(|c| !matches!(c, std::path::Component::Normal(_) | std::path::Component::CurDir)) {
                continue;
            }

            let etype = entry.header().entry_type();
            if etype.is_dir() {
                if dir_cache.insert(outpath.clone()) {
                    std::fs::create_dir_all(&outpath).map_err(|e| format!("mkdir: {}", e))?;
                }
                continue;
            }
            if etype.is_symlink() || etype.is_hard_link() { continue; }

            let size = entry.header().size().unwrap_or(0);

            // Large entries bypass the batch — stream straight to disk.
            if size > STREAM_THRESHOLD {
                flush_batch(&mut batch, &mut batch_bytes, &mut dir_cache)?;
                if let Some(parent) = outpath.parent() {
                    if dir_cache.insert(parent.to_path_buf()) {
                        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir parent: {}", e))?;
                    }
                }
                let f = std::fs::File::create(&outpath).map_err(|e| format!("create {}: {}", rel.display(), e))?;
                let _ = f.set_len(size);
                let mut bw = std::io::BufWriter::with_capacity(1024 * 1024, f);
                std::io::copy(&mut entry, &mut bw).map_err(|e| format!("write {}: {}", rel.display(), e))?;
                continue;
            }

            // Small entries: read into memory, accumulate into batch.
            let mut data = Vec::with_capacity(size as usize);
            entry.read_to_end(&mut data).map_err(|e| format!("read {}: {}", rel.display(), e))?;
            batch_bytes += data.len();
            batch.push((outpath, data));

            if batch.len() >= BATCH_ENTRIES || batch_bytes >= BATCH_BYTES {
                flush_batch(&mut batch, &mut batch_bytes, &mut dir_cache)?;
            }
        }
        // Flush remaining
        flush_batch(&mut batch, &mut batch_bytes, &mut dir_cache)?;
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
            (false, format!("extract failed: {}", e))
        }
    }
}

/// .zip — via the `zip` crate (pure Rust). Decompressed bytes tick into Progress
/// via a counting reader; the total is the SUM of uncompressed entry sizes (from the
/// central directory), not the on-disk archive size, so the bar never overshoots.
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
        for i in 0..za.len() {
            let mut entry = za.by_index(i).map_err(|e| format!("entry {}: {}", i, e))?;
            // zip-slip guard: enclosed_name() returns a sanitized path or None for unsafe
            // names (e.g. containing `..` or absolute paths). Skip those entirely.
            let rel = match entry.enclosed_name() {
                Some(p) => p,
                None => continue,
            };
            let outpath = dest.join(&rel);
            if entry.is_dir() {
                std::fs::create_dir_all(&outpath).map_err(|e| format!("mkdir {}: {}", rel.display(), e))?;
            } else {
                if let Some(parent) = outpath.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| format!("mkdir parent: {}", e))?;
                }
                let mut outf = std::fs::File::create(&outpath).map_err(|e| format!("create {}: {}", rel.display(), e))?;
                // Wrap the entry in a counting reader so decompressed bytes tick into Progress.
                let mut counting = ZipCountingReader { inner: &mut entry, prog: &prog };
                std::io::copy(&mut counting, &mut outf).map_err(|e| format!("write {}: {}", rel.display(), e))?;
            }
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
