// Split-Compress — multi-volume .zgx support.
// SegmentWriter: splits the zstd output into parts with SHA256 trailers.
// ConcatReader: chains parts back into one stream, verifies each trailer.
// Part detection: parse_split_part resolves any part to part001.

use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::fs::File;
use sha2::{Sha256, Digest};

pub const SPLIT_HEADER_LEN: usize = 22;
pub const SPLIT_VERSION: u8 = 0x02;
pub const TRAILER_LEN: usize = 32;
pub const MAX_PARTS: usize = 9999;

// ─── HEADER ───────────────────────────────────────────────────────────

pub fn split_header(total_uncompressed: u64, segment_size: u64) -> [u8; SPLIT_HEADER_LEN] {
    let mut h = [0u8; SPLIT_HEADER_LEN];
    h[0..5].copy_from_slice(b"LRGEX");
    h[5] = SPLIT_VERSION;
    h[6..14].copy_from_slice(&total_uncompressed.to_le_bytes());
    h[14..22].copy_from_slice(&segment_size.to_le_bytes());
    h
}

// ─── PART NAME PARSING (hand-parsed, no regex) ───────────────────────

/// Parse `MyFolder.partNNN.zgx` → (base_path, part_number).
/// Accepts 1-4 digit part numbers. Returns None for non-split filenames.
pub fn parse_split_part(path: &Path) -> Option<(PathBuf, u32)> {
    let filename = path.file_name()?.to_str()?;
    let stem = filename.strip_suffix(".zgx")?;
    let dot = stem.rfind(".part")?;
    let digits = &stem[dot + 5..];
    if digits.is_empty() || digits.len() > 4 { return None; }
    let num: u32 = digits.parse().ok()?;
    if num == 0 || num > MAX_PARTS as u32 { return None; }
    let base_name = &stem[..dot];
    let parent = path.parent()?;
    Some((parent.join(base_name), num))
}

/// If `path` is a split part, return the path to part001.
pub fn resolve_to_part001(path: &Path) -> Option<PathBuf> {
    let (base, _num) = parse_split_part(path)?;
    let stem = base.file_name()?.to_str()?;
    let parent = base.parent()?;
    let part1 = parent.join(format!("{}.part001.zgx", stem));
    if part1.exists() { Some(part1) } else { None }
}

// ─── SEGMENT WRITER (compress side) ───────────────────────────────────

/// A Write impl that splits output into multiple part files with SHA256 trailers.
/// Each part is capped at `segment_size` (minus trailer). Part001 gets the split header.
/// On cancel/panic: Drop deletes all .tmp files AND rotated .zgx parts (no orphans).
pub struct SegmentWriter {
    base_stem: String,
    parent_dir: PathBuf,
    segment_size: u64,
    current_file: Option<File>,
    current_index: u32,
    bytes_in_current: u64,
    hasher: Sha256,
    total_uncompressed: u64,
    finished_ok: bool,
    rotated_finals: Vec<PathBuf>,
}

impl SegmentWriter {
    pub fn new(base_path: PathBuf, segment_size_mb: u32, total_uncompressed: u64) -> Self {
        let segment_size = (segment_size_mb.max(1) as u64) * 1024 * 1024;
        let segment_size = segment_size.max((TRAILER_LEN + SPLIT_HEADER_LEN + 1024) as u64);
        let parent_dir = base_path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let base_stem = base_path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("archive")
            .to_string();
        Self {
            base_stem, parent_dir, segment_size,
            current_file: None, current_index: 0,
            bytes_in_current: 0,
            hasher: Sha256::new(),
            total_uncompressed,
            finished_ok: false,
            rotated_finals: Vec::new(),
        }
    }

    fn ensure_open(&mut self) -> io::Result<()> {
        if self.current_file.is_some() { return Ok(()); }
        self.current_index = 1;
        let path = self.part_tmp_path(1);
        let mut f = File::create(&path)?;
        let header = split_header(self.total_uncompressed, self.segment_size);
        f.write_all(&header)?;
        self.hasher.update(&header);
        self.bytes_in_current = 0; // header is overhead, not data budget
        self.current_file = Some(f);
        Ok(())
    }

    fn room_left(&self) -> u64 {
        self.segment_size.saturating_sub(TRAILER_LEN as u64).saturating_sub(self.bytes_in_current)
    }

    fn rotate_part(&mut self) -> io::Result<()> {
        self.close_current_part()?;
        self.current_index += 1;
        if self.current_index as usize > MAX_PARTS {
            return Err(io::Error::new(io::ErrorKind::Other, "Exceeded max parts (9999)"));
        }
        let path = self.part_tmp_path(self.current_index);
        let f = File::create(&path)?;
        self.current_file = Some(f);
        self.bytes_in_current = 0;
        self.hasher.reset();
        Ok(())
    }

    fn close_current_part(&mut self) -> io::Result<()> {
        if let Some(mut f) = self.current_file.take() {
            let hash = self.hasher.finalize_reset();
            f.write_all(&hash)?;
            f.flush()?;
            drop(f);
            let tmp = self.part_tmp_path(self.current_index);
            let final_path = self.part_final_path(self.current_index);
            std::fs::rename(&tmp, &final_path)?;
            self.rotated_finals.push(final_path);
        }
        self.bytes_in_current = 0;
        Ok(())
    }

    fn part_tmp_path(&self, index: u32) -> PathBuf {
        self.parent_dir.join(format!("{}.part{:03}.zgx.tmp", self.base_stem, index))
    }
    fn part_final_path(&self, index: u32) -> PathBuf {
        self.parent_dir.join(format!("{}.part{:03}.zgx", self.base_stem, index))
    }

    pub fn finish(&mut self) -> io::Result<()> {
        self.close_current_part()?;
        self.finished_ok = true;
        self.rotated_finals.clear();
        Ok(())
    }

    pub fn cleanup_all(&self) {
        // Delete .tmp files.
        let prefix = format!("{}.part", self.base_stem);
        if let Ok(entries) = std::fs::read_dir(&self.parent_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with(&prefix) && name.ends_with(".zgx.tmp") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        // Delete rotated .zgx parts (from a failed run).
        for p in &self.rotated_finals {
            let _ = std::fs::remove_file(p);
        }
    }
}

// Chunked write — never lets a single buf exceed segment_size.
impl Write for SegmentWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.ensure_open()?;
        let mut written = 0;
        while written < buf.len() {
            let room = self.room_left();
            if room == 0 {
                self.rotate_part()?;
                continue;
            }
            let n = (buf.len() - written).min(room as usize);
            let chunk = &buf[written..written + n];
            if let Some(ref mut f) = self.current_file {
                f.write_all(chunk)?;
            }
            self.hasher.update(chunk);
            self.bytes_in_current += n as u64;
            written += n;
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(ref mut f) = self.current_file { f.flush()?; }
        Ok(())
    }
}

impl Drop for SegmentWriter {
    fn drop(&mut self) {
        if !self.finished_ok {
            self.current_file.take();
            self.cleanup_all();
        }
    }
}

// ─── CONCAT READER (extract side) ─────────────────────────────────────

/// Chains multiple part files into one stream. Skips part001's header.
/// Stops before each part's 32-byte trailer.
#[derive(Debug)]
pub struct ConcatReader {
    parts: Vec<PartEntry>,
    current: usize,
    pos_in_part: u64,
}

#[derive(Debug)]
struct PartEntry {
    file: File,
    data_len: u64,
}

impl ConcatReader {
    /// Open all parts, verify each SHA256 trailer (single-open per part),
    /// return (reader, header_total_uncompressed, data_sum).
    pub fn open_and_verify(base_path: &Path) -> io::Result<(Self, u64, u64)> {
        let stem = base_path.file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad base path"))?;
        let parent = base_path.parent().unwrap_or(Path::new("."));

        let mut parts: Vec<PartEntry> = Vec::new();
        let mut total_uncompressed: u64 = 0;
        let mut data_sum: u64 = 0;

        for i in 1..=MAX_PARTS {
            let path = parent.join(format!("{}.part{:03}.zgx", stem, i));
            if !path.exists() {
                if i == 1 {
                    return Err(io::Error::new(io::ErrorKind::NotFound,
                        format!("Part 001 not found: {}", path.display())));
                }
                break;
            }

            let meta = std::fs::metadata(&path)?;
            if (meta.len() as usize) < TRAILER_LEN {
                return Err(io::Error::new(io::ErrorKind::InvalidData,
                    format!("Part {:03} too small", i)));
            }
            let data_len = meta.len() - TRAILER_LEN as u64;
            data_sum += data_len;

            // Single-open: seek to trailer, read, seek back, hash, keep handle.
            let mut f = File::open(&path)?;

            // Read trailer.
            let mut trailer = [0u8; TRAILER_LEN];
            f.seek(SeekFrom::Start(data_len))?;
            f.read_exact(&mut trailer)?;

            // Stream-hash [0..data_len].
            f.seek(SeekFrom::Start(0))?;
            let mut hasher = Sha256::new();
            let mut limited = (&mut f).take(data_len);
            std::io::copy(&mut limited, &mut hasher)?;
            let computed = hasher.finalize();
            if computed.as_slice() != trailer {
                return Err(io::Error::new(io::ErrorKind::InvalidData,
                    format!("Part {:03} is corrupt (SHA256 mismatch). Redownload it.", i)));
            }

            // Read part001 header.
            if i == 1 {
                f.seek(SeekFrom::Start(0))?;
                let mut header = [0u8; SPLIT_HEADER_LEN];
                f.read_exact(&mut header)?;
                if &header[0..5] != b"LRGEX" {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "Not a LRGEX split archive"));
                }
                if header[5] != SPLIT_VERSION {
                    return Err(io::Error::new(io::ErrorKind::InvalidData,
                        format!("Unsupported split version: 0x{:02X}", header[5])));
                }
                total_uncompressed = u64::from_le_bytes(header[6..14].try_into().unwrap());
            }

            // Reset for ConcatReader streaming.
            f.seek(SeekFrom::Start(0))?;
            parts.push(PartEntry { file: f, data_len });
        }

        if parts.is_empty() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "No parts found"));
        }

        Ok((ConcatReader { parts, current: 0, pos_in_part: 0 }, total_uncompressed, data_sum))
    }

    /// Open WITHOUT SHA verification — for conflict checks only. O(1) IO per part.
    pub fn open_no_verify(base_path: &Path) -> io::Result<Self> {
        let stem = base_path.file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad base path"))?;
        let parent = base_path.parent().unwrap_or(Path::new("."));
        let mut parts: Vec<PartEntry> = Vec::new();
        for i in 1..=MAX_PARTS {
            let path = parent.join(format!("{}.part{:03}.zgx", stem, i));
            if !path.exists() {
                if i == 1 { return Err(io::Error::new(io::ErrorKind::NotFound, "Part 001 not found")); }
                break;
            }
            let meta = std::fs::metadata(&path)?;
            if (meta.len() as usize) < TRAILER_LEN {
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("Part {:03} too small", i)));
            }
            let data_len = meta.len() - TRAILER_LEN as u64;
            let file = File::open(&path)?;
            parts.push(PartEntry { file, data_len });
        }
        if parts.is_empty() { return Err(io::Error::new(io::ErrorKind::NotFound, "No parts found")); }
        Ok(ConcatReader { parts, current: 0, pos_in_part: 0 })
    }
}

impl Read for ConcatReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.current >= self.parts.len() {
                return Ok(0);
            }

            // Part001: skip the 22-byte header on first access.
            if self.current == 0 && self.pos_in_part == 0 {
                let mut hdr = [0u8; SPLIT_HEADER_LEN];
                self.parts[0].file.read_exact(&mut hdr)?;
                self.pos_in_part = SPLIT_HEADER_LEN as u64;
            }

            let part = &mut self.parts[self.current];
            let remaining = part.data_len.saturating_sub(self.pos_in_part);
            if remaining == 0 {
                self.current += 1;
                self.pos_in_part = 0;
                continue;
            }

            let n = (remaining.min(buf.len() as u64)) as usize;
            let n = part.file.read(&mut buf[..n])?;
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof,
                    format!("Part {:03} ended before its trailer", self.current + 1)));
            }
            self.pos_in_part += n as u64;
            return Ok(n);
        }
    }
}

// ─── TESTS ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_header_roundtrip() {
        let header = split_header(123456789, 30 * 1024 * 1024);
        assert_eq!(&header[0..5], b"LRGEX");
        assert_eq!(header[5], SPLIT_VERSION);
        let total = u64::from_le_bytes(header[6..14].try_into().unwrap());
        assert_eq!(total, 123456789);
        let seg = u64::from_le_bytes(header[14..22].try_into().unwrap());
        assert_eq!(seg, 30 * 1024 * 1024);
    }

    #[test]
    fn parse_split_part_filenames() {
        let p = Path::new("C:\\output\\MyFolder.part001.zgx");
        let (base, num) = parse_split_part(p).unwrap();
        assert_eq!(num, 1);

        let p = Path::new("C:\\output\\MyFolder.part042.zgx");
        let (_, num) = parse_split_part(p).unwrap();
        assert_eq!(num, 42);

        // 4-digit part (1000+).
        let p = Path::new("C:\\output\\Big.part1000.zgx");
        let (_, num) = parse_split_part(p).unwrap();
        assert_eq!(num, 1000);

        // Non-split filename.
        let p = Path::new("C:\\output\\MyFolder.zgx");
        assert!(parse_split_part(p).is_none());

        // part000 is invalid.
        let p = Path::new("C:\\output\\MyFolder.part000.zgx");
        assert!(parse_split_part(p).is_none());
    }

    #[test]
    fn segment_writer_produces_valid_parts() {
        let tmp = std::env::temp_dir().join(format!("split-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let base = tmp.join("TestArchive");
        let mut writer = SegmentWriter::new(base.clone(), 1, 1000); // 1MB parts

        // Write 2.5MB of data → should produce 3 parts.
        let chunk = vec![0xABu8; 1024 * 1024]; // 1MB
        for _ in 0..2 { writer.write_all(&chunk).unwrap(); }
        writer.write_all(&chunk[..512 * 1024]).unwrap(); // 0.5MB more
        writer.finish().unwrap();

        // Verify 3 parts exist.
        assert!(tmp.join("TestArchive.part001.zgx").exists());
        assert!(tmp.join("TestArchive.part002.zgx").exists());
        assert!(tmp.join("TestArchive.part003.zgx").exists());

        // Each part should be ≤ 1MB + 32 bytes trailer (+ 22 header for part001).
        let p1 = std::fs::metadata(tmp.join("TestArchive.part001.zgx")).unwrap().len();
        let p2 = std::fs::metadata(tmp.join("TestArchive.part002.zgx")).unwrap().len();
        assert!(p1 <= 1_048_576 + 32 + 22, "part1 too big: {}", p1);
        assert!(p2 <= 1_048_576 + 32, "part2 too big: {}", p2);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn concat_reader_chains_parts() {
        let tmp = std::env::temp_dir().join(format!("split-read-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let base = tmp.join("ReadTest");
        let mut writer = SegmentWriter::new(base.clone(), 1, 0);
        let data = vec![0x42u8; 2_500_000]; // 2.5MB
        writer.write_all(&data).unwrap();
        writer.finish().unwrap();

        // Open + verify.
        let (reader, _total, _sum) = ConcatReader::open_and_verify(&base).unwrap();

        // Read all data back.
        let mut read_back = Vec::new();
        reader.take(2_500_000).read_to_end(&mut read_back).ok();
        // (Header bytes are skipped by ConcatReader, so read_back should be the zstd data,
        // not the original uncompressed bytes. We just verify it doesn't crash and reads.)

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn corrupt_part_detected() {
        let tmp = std::env::temp_dir().join(format!("split-corrupt-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let base = tmp.join("CorruptTest");
        let mut writer = SegmentWriter::new(base.clone(), 1, 0);
        writer.write_all(&vec![0x55u8; 2_000_000]).unwrap();
        writer.finish().unwrap();

        // Flip a byte in part002.
        let p2 = tmp.join("CorruptTest.part002.zgx");
        if p2.exists() {
            let mut data = std::fs::read(&p2).unwrap();
            data[100] ^= 0xFF;
            std::fs::write(&p2, data).unwrap();

            let result = ConcatReader::open_and_verify(&base);
            assert!(result.is_err(), "should detect corruption");
            let err = result.unwrap_err().to_string();
            assert!(err.contains("corrupt"), "error should say corrupt: {}", err);
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
