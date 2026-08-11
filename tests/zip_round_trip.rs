//! Zip extraction round-trip — parity with the other 3 formats (zgx, 7z, RAR).
//!
//! zgx has 40+ proptest cases, 7z has the overwrite+cancel kill test, RAR has crash-safety.
//! Zip — one of four advertised formats — previously had NO round-trip. This closes that gap.

mod common;

use common::harness;
use std::fs;
use std::io::Write;
use std::path::Path;
use tempfile::TempDir;

/// Build a real .zip with mixed content using the `zip` crate (already a regular dep,
/// available to tests). Then extract via the LRGEX exe and verify byte-identical.
fn build_zip(archive_path: &Path) {
    let f = fs::File::create(archive_path).unwrap();
    let mut zip = zip::ZipWriter::new(f);

    // Regular small text file
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored);
    zip.start_file("hello.txt", opts).unwrap();
    zip.write_all(b"hello zip world").unwrap();

    // A binary file with incompressible-ish content
    zip.start_file("data.bin", opts).unwrap();
    let mut buf = Vec::with_capacity(64 * 1024);
    let mut state: u64 = 0xABCDEF0123456789;
    for _ in 0..64 * 1024 {
        state ^= state << 13; state ^= state >> 7; state ^= state << 17;
        buf.push(state as u8);
    }
    zip.write_all(&buf).unwrap();

    // A nested file (zip paths use forward slashes)
    zip.start_file("sub/nested.txt", opts).unwrap();
    zip.write_all(b"inside a subfolder").unwrap();

    zip.finish().unwrap();
}

#[test]
fn zip_round_trip_is_byte_identical() {
    let tmp = TempDir::new().unwrap();
    let archive = tmp.path().join("probe.zip");
    build_zip(&archive);

    // SHA256 the raw bytes we wrote, so we compare against exactly what's in the zip
    // (not what we'd reconstruct) — the zip crate stored them as-is via Stored method.
    let want_hello = sha256_of_bytes(b"hello zip world");
    let want_nested = sha256_of_bytes(b"inside a subfolder");
    let mut state: u64 = 0xABCDEF0123456789;
    let mut want_data = Vec::with_capacity(64 * 1024);
    for _ in 0..64 * 1024 {
        state ^= state << 13; state ^= state >> 7; state ^= state << 17;
        want_data.push(state as u8);
    }
    let want_data = sha256_of_bytes(&want_data);

    // Extract via the LRGEX exe (extracts to sibling folder named after stem).
    let dest = archive.with_extension("");
    let _ = fs::remove_dir_all(&dest);
    let code = harness::run(&["-x", &archive.to_string_lossy()], std::time::Duration::from_secs(120));
    assert_eq!(code, 0, "zip extract failed (exit {code})");
    assert!(dest.is_dir(), "extract produced no folder at {}", dest.display());

    // Verify each extracted file matches.
    let got_hello = harness::file_sha256(&dest.join("hello.txt"));
    assert_eq!(got_hello, want_hello, "hello.txt content mismatch after zip round-trip");

    let got_data = harness::file_sha256(&dest.join("data.bin"));
    assert_eq!(got_data, want_data, "data.bin content mismatch after zip round-trip");

    let got_nested = harness::file_sha256(&dest.join("sub").join("nested.txt"));
    assert_eq!(got_nested, want_nested, "sub/nested.txt content mismatch after zip round-trip");
}

fn sha256_of_bytes(b: &[u8]) -> String {
    use sha2::{Sha256, Digest};
    let mut h = Sha256::new();
    h.update(b);
    hex::encode(h.finalize())
}
