//! RAR round-trip test. Marked #[ignore] because it requires WinRAR's Rar.exe to build
//! the test archive — not present on every dev/CI machine. Run explicitly with:
//!     cargo test --test rar_round_trip -- --ignored
//!
//! This is NOT a silent skip — `cargo test` reports it as `ignored`, visibly signaling
//! "RAR coverage exists but needs an external tool." That's the honest signal. A silent
//! early-return would falsely report PASS on WinRAR-less machines and hide the gap.

#[ignore = "requires WinRAR Rar.exe at C:\\Program Files\\WinRAR\\Rar.exe — run with --ignored"]
#[test]
fn rar_round_trip_is_byte_identical() {
    // Minimal harness inlined here (single test, no need for the shared module).
    use std::fs;
    use std::io::Read;
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::{Duration, Instant};
    use sha2::{Sha256, Digest};

    let rar = std::path::Path::new(r"C:\Program Files\WinRAR\Rar.exe");
    assert!(rar.exists(), "WinRAR Rar.exe not found — this test needs it to build a test archive");

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

    // SHA256 of source.
    let src_hash = {
        let mut h = Sha256::new();
        let mut f = fs::File::open(src.join("payload.bin")).unwrap();
        let mut buf = [0u8; 64 * 1024];
        loop { let n = f.read(&mut buf).unwrap(); if n == 0 { break; } h.update(&buf[..n]); }
        hex::encode(h.finalize())
    };

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
    let run = || -> i32 {
        let start = Instant::now();
        let mut child = Command::new(&exe).args(["-x", &archive.to_string_lossy()])
            .spawn().unwrap();
        loop {
            match child.try_wait() {
                Ok(Some(s)) => return s.code().unwrap_or(-1),
                Ok(None) => {
                    if start.elapsed() > Duration::from_secs(120) { let _ = child.kill(); panic!("timeout"); }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(_) => panic!("wait failed"),
            }
        }
    };
    let code = run();
    assert_eq!(code, 0, "extract failed (exit {code})");

    let extracted = dest.join("payload.bin");
    assert!(extracted.exists(), "payload.bin not extracted");

    let got_hash = {
        let mut h = Sha256::new();
        let mut f = fs::File::open(&extracted).unwrap();
        let mut buf = [0u8; 64 * 1024];
        loop { let n = f.read(&mut buf).unwrap(); if n == 0 { break; } h.update(&buf[..n]); }
        hex::encode(h.finalize())
    };

    assert_eq!(src_hash, got_hash, "RAR round-trip content mismatch — data changed across extract");
}
