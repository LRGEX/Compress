//! RAR round-trip test. Auto-RUNS when WinRAR is present (no --ignored needed).
//! When WinRAR is absent it no-ops as a PASS with a skip message — note this is a
//! FALSE GREEN on toolless boxes (run with --nocapture to see "SKIP rar_round_trip".
//! The win over #[ignore] is that the test runs automatically on any WinRAR-equipped
//! box; the cost is the false-green-on-absence. WinRAR-equipped boxes get real coverage.

use std::time::{Duration, Instant};

#[test]
fn rar_round_trip_is_byte_identical() {
    let rar = std::path::Path::new(r"C:\Program Files\WinRAR\Rar.exe");
    if !rar.exists() {
        eprintln!("SKIP rar_round_trip: WinRAR Rar.exe not found — install WinRAR to run this test");
        return;
    }
    use std::fs;
    use std::io::Read;
    use std::path::PathBuf;
    use std::process::Command;
    use sha2::{Sha256, Digest};

    let exe = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/release/lrgex-compress.exe");
    assert!(exe.exists(), "Release exe not built. Run `cargo build --release`.");

    let tmp = tempfile::TempDir::new().unwrap();
    let src = tmp.path().join("rar_src");
    fs::create_dir_all(&src).unwrap();
    // 8MB pseudo-random incompressible data.
    let mut state: u64 = 0xDEAD_BEEF_CAFE_BABE;
    let mut content = Vec::with_capacity(8 * 1024 * 1024);
    for _ in 0..8 * 1024 * 1024 {
        state ^= state << 13; state ^= state >> 7; state ^= state << 17;
        content.push(state as u8);
    }
    fs::write(src.join("payload.bin"), &content).unwrap();

    let src_hash = sha256_file(&src.join("payload.bin"));

    // Build the RAR archive with WinRAR.
    let archive = tmp.path().join("probe.rar");
    let out = Command::new(rar).args(["a", "-m1", "-ep",
        &archive.to_string_lossy(),
        &src.join("payload.bin").to_string_lossy()])
        .output().expect("failed to run Rar.exe");
    assert!(out.status.success(), "Rar.exe failed: {}", String::from_utf8_lossy(&out.stderr));

    // Extract via the LRGEX exe. RAR extracts to a sibling folder named after the archive stem.
    let dest = archive.with_extension("");
    let _ = fs::remove_dir_all(&dest);
    let code = run_exe_blocking(&exe, &["-x", &archive.to_string_lossy()], Duration::from_secs(120));
    assert_eq!(code, 0, "extract failed (exit {code})");

    let extracted = dest.join("payload.bin");
    assert!(extracted.exists(), "payload.bin not extracted");
    assert_eq!(src_hash, sha256_file(&extracted), "RAR round-trip content mismatch");
    eprintln!("PASS rar_round_trip: content byte-identical");
}

fn sha256_file(p: &std::path::Path) -> String {
    use sha2::{Sha256, Digest};
    let mut h = Sha256::new();
    let mut f = std::fs::File::open(p).unwrap();
    let mut buf = [0u8; 64 * 1024];
    loop { let n = std::io::Read::read(&mut f, &mut buf).unwrap(); if n == 0 { break; } h.update(&buf[..n]); }
    hex::encode(h.finalize())
}

fn run_exe_blocking(exe: &std::path::Path, args: &[&str], timeout: Duration) -> i32 {
    let start = Instant::now();
    let mut child = std::process::Command::new(exe).args(args).spawn().unwrap();
    loop {
        match child.try_wait() {
            Ok(Some(s)) => return s.code().unwrap_or(-1),
            Ok(None) => {
                if start.elapsed() > timeout { let _ = child.kill(); panic!("exe timeout"); }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(_) => panic!("wait failed"),
        }
    }
}
