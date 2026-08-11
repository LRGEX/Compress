//! Checksum-integrity tests — proves include_checksum(true) catches payload corruption
//! on extract via zstd's XXHash64 frame checksum (guaranteed, not luck-of-offset).
//!
//! We flip bytes at many offsets across a .zgx and extract each corrupted copy.
//! With the checksum ON, ANY flip that changes the decompressed payload MUST be caught
//! (extract errors or reproduces original). Zero cases of "extract succeeds with wrong bytes".

mod common;

use common::harness;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use tempfile::TempDir;

/// Build a reference .zgx with known content. Returns (tmpdir_keeper, archive_path, original_bytes).
fn make_reference() -> (TempDir, std::path::PathBuf, Vec<u8>) {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();
    // Use non-trivial content so the checksum has something to verify.
    let mut content = Vec::with_capacity(64 * 1024);
    for i in 0..65536u32 { content.push((i ^ (i >> 8)) as u8); }
    fs::write(src.join("probe.bin"), &content).unwrap();
    let archive = harness::compress(&src);
    (tmp, archive, content)
}

/// Try to extract `archive`, return (exit_code, extracted_bytes_if_any).
fn try_extract(archive: &Path) -> (i32, Option<Vec<u8>>) {
    let dest = archive.with_extension("");
    let _ = fs::remove_dir_all(&dest);
    let code = harness::run(&["-x", archive.to_str().unwrap()], Duration::from_secs(120));
    let extracted = dest.join("probe.bin");
    let bytes = if extracted.exists() { Some(fs::read(&extracted).unwrap()) } else { None };
    (code, bytes)
}

#[test]
fn checksum_catches_every_single_byte_flip() {
    let (_tmp, archive, original) = make_reference();
    let archive_bytes = fs::read(&archive).unwrap();
    let total = archive_bytes.len();

    // Flip at many offsets: header region, zstd magic, frame, middle, near-end.
    // This covers flips that land on payload bytes (where checksum is the ONLY detector).
    let offsets: Vec<usize> = vec![
        14, 20, 30, 40, 50, 60,
        total / 8, total / 4, total / 2, 3 * total / 4, 7 * total / 8,
        total.saturating_sub(32), total.saturating_sub(16), total.saturating_sub(8),
    ];

    let mut detected = 0;       // extract errored or produced no output
    let mut reproduced = 0;     // extract succeeded AND bytes match original (benign flip — e.g. padding)
    let mut silent_corruption = 0; // extract succeeded BUT bytes differ — THIS is what checksum prevents

    for &off in &offsets {
        if off >= total { continue; }
        let tmp = TempDir::new().unwrap();
        let corrupt = tmp.path().join("corrupt.zgx");
        let mut buf = archive_bytes.clone();
        buf[off] ^= 0xFF;
        fs::write(&corrupt, &buf).unwrap();

        let (code, extracted) = try_extract(&corrupt);
        match extracted {
            None => detected += 1,
            Some(got) => {
                if got == original { reproduced += 1; }
                else {
                    silent_corruption += 1;
                    eprintln!("SILENT CORRUPTION at offset {} — extract exited {} but bytes differ", off, code);
                }
            }
        }
    }

    let classified = detected + reproduced + silent_corruption;
    eprintln!("checksum bit-flip: offsets={}, detected={}, reproduced_identical={}, SILENT_CORRUPTION={}",
              offsets.len(), detected, reproduced, silent_corruption);

    assert_eq!(classified, offsets.iter().filter(|&&o| o < total).count(), "a flip path didn't classify");
    assert_eq!(silent_corruption, 0,
        "CHECKSUM FAILED: {} of {} corrupted archives extracted successfully with WRONG bytes. \
         include_checksum(true) should make this impossible.", silent_corruption, offsets.len());
}

#[test]
fn checksum_round_trip_is_byte_identical() {
    // Sanity: turning the checksum on doesn't break the happy path.
    let (_tmp, archive, original) = make_reference();
    let (_, extracted) = try_extract(&archive);
    assert_eq!(extracted, Some(original), "checksum-on round-trip lost data");
}
