// Extraction (zstd -> tar) — adapted from LRGEX Restore sync.rs decompression.
// Uses a ByteReader so compressed bytes tick into Progress (byte-based %).

use std::path::Path;

use crate::progress::{self, ByteReader, Progress};

/// Extract a .zgx (.tar.zst) archive into `dest`. Returns (success, message).
pub fn extract_archive(archive: &Path, dest: &Path) -> (bool, String) {
    progress::clear_status();
    let label = archive
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let arch_size = std::fs::metadata(archive).map(|m| m.len()).unwrap_or(0);

    let prog = Progress::new(&label);
    let heartbeat = prog.spawn_writer();
    prog.set_totals(0, arch_size); // file count unknown for extract
    prog.set_phase(1);

    let file = match std::fs::File::open(archive) {
        Ok(f) => f,
        Err(e) => {
            prog.finish(4);
            let _ = heartbeat.join();
            return (false, format!("cannot open archive: {}", e));
        }
    };
    let counting = ByteReader::new(file, prog.clone());

    let decoder = match zstd::Decoder::new(counting) {
        Ok(d) => d,
        Err(e) => {
            prog.finish(4);
            let _ = heartbeat.join();
            return (false, format!("corrupt archive (zstd): {}", e));
        }
    };
    let mut tar = tar::Archive::new(decoder);

    let _ = std::fs::create_dir_all(dest);
    match tar.unpack(dest) {
        Ok(_) => {
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
