//! 7z round-trip + metadata — closes the coverage gap for 7z single AND multi-volume.
//!
//! Previously 7z only had the overwrite+cancel kill test (multi) + shared-code argument (single).
//! No content round-trip, no metadata. This test builds REAL 7z archives with 7-Zip,
//! extracts via the LRGEX exe, and verifies content + mtime byte-for-byte.
//!
//! Auto-RUNS when 7-Zip is present (no --ignored needed). When 7-Zip is absent it no-ops
//! as a PASS with a skip message — note this is a FALSE GREEN on toolless boxes (run with
//! --nocapture to see "SKIP sevenz_*". The win over #[ignore] is that the tests run
//! automatically on any 7-Zip-equipped box; the cost is the false-green-on-absence.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn sevenz_single_round_trip_content_and_mtime() {
    let sevenz = std::path::Path::new(r"C:\Program Files\7-Zip\7z.exe");
    if !sevenz.exists() {
        eprintln!("SKIP sevenz_single: 7-Zip not found — install 7-Zip to run this test");
        return;
    }
    let exe = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/release/lrgex-compress.exe");
    assert!(exe.exists(), "Release exe not built. Run `cargo build --release`.");

    let tmp = tempfile::TempDir::new().unwrap();
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();

    // Distinctive content (incompressible-ish)
    let mut state: u64 = 0x1111_2222_3333_4444;
    let mut payload = Vec::with_capacity(256 * 1024);
    for _ in 0..256 * 1024 {
        state ^= state << 13; state ^= state >> 7; state ^= state << 17;
        payload.push(state as u8);
    }
    fs::write(src.join("payload.bin"), &payload).unwrap();
    fs::write(src.join("hello.txt"), b"seven and seven").unwrap();
    fs::create_dir_all(src.join("sub")).unwrap();
    fs::write(src.join("sub").join("nested.txt"), b"nested in 7z").unwrap();

    // Set a KNOWN mtime on payload.bin so we can verify it round-trips.
    set_mtime(&src.join("payload.bin"), 1577836800);
    let src_mtime = mtime_secs(&src.join("payload.bin"));
    assert_eq!(src_mtime, 1577836800, "setup failed: mtime didn't set");

    // Build the .7z with 7-Zip. Run 7z from INSIDE src with no path args so the
    // archive stores entries at the root (not embedded with the absolute source path).
    let archive = tmp.path().join("probe.7z");
    let out = Command::new(sevenz)
        .args(["a", "-t7z", "-mx=1", &archive.to_string_lossy()])
        .current_dir(&src)
        .output().expect("failed to run 7z");
    assert!(out.status.success(), "7z failed: {}", String::from_utf8_lossy(&out.stderr));

    // Extract via LRGEX. 7z extracts to sibling folder named after stem.
    let dest = archive.with_extension("");
    let _ = fs::remove_dir_all(&dest);
    let code = run_exe_blocking(&exe, &["-x", &archive.to_string_lossy()], Duration::from_secs(120));
    assert_eq!(code, 0, "7z extract failed");

    // Content checks (SHA256)
    assert_eq!(sha256_file(&dest.join("payload.bin")), sha256_bytes(&payload),
        "payload.bin content mismatch in 7z round-trip");
    assert_eq!(sha256_file(&dest.join("hello.txt")), sha256_bytes(b"seven and seven"),
        "hello.txt content mismatch");
    assert_eq!(sha256_file(&dest.join("sub").join("nested.txt")), sha256_bytes(b"nested in 7z"),
        "nested.txt content mismatch");

    // Metadata check: mtime MUST round-trip (the 7z mtime fix in extract.rs)
    let e_mtime = mtime_secs(&dest.join("payload.bin"));
    assert_eq!(e_mtime, src_mtime,
        "7z MTIME MISMATCH: source={src_mtime}, extracted={e_mtime}");
    eprintln!("PASS sevenz_single: content + mtime round-trip exact");
}

#[test]
fn sevenz_multi_volume_round_trip_content() {
    let sevenz = std::path::Path::new(r"C:\Program Files\7-Zip\7z.exe");
    if !sevenz.exists() {
        eprintln!("SKIP sevenz_multi: 7-Zip not found");
        return;
    }
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;
    use std::process::Command;

    let exe = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/release/lrgex-compress.exe");
    assert!(exe.exists(), "Release exe not built");

    let tmp = tempfile::TempDir::new().unwrap();
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();
    let mut state: u64 = 0x5555_6666_7777_8888;
    let mut payload = Vec::with_capacity(4 * 1024 * 1024);
    for _ in 0..4 * 1024 * 1024 {
        state ^= state << 13; state ^= state >> 7; state ^= state << 17;
        payload.push(state as u8);
    }
    fs::write(src.join("big.bin"), &payload).unwrap();

    // Build multi-volume 7z: 100KB parts. Run 7z from src with relative path.
    let base = tmp.path().join("probe.7z");
    let out = Command::new(sevenz)
        .args(["a", "-t7z", "-mx=1", "-v100k", &base.to_string_lossy(), "big.bin"])
        .current_dir(&src)
        .output().expect("failed to run 7z");
    assert!(out.status.success(), "7z multi failed: {}", String::from_utf8_lossy(&out.stderr));

    let first = tmp.path().join("probe.7z.001");
    assert!(first.exists(), "multi-volume first part not created");
    let part_count = fs::read_dir(tmp.path()).unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with("probe.7z.")).count();
    assert!(part_count >= 2, "expected >=2 parts, got {part_count}");

    // Extract part001 via LRGEX. Copy parts to a clean dir to avoid collision.
    let clean = tmp.path().join("clean");
    fs::create_dir_all(&clean).unwrap();
    for e in fs::read_dir(tmp.path()).unwrap().flatten() {
        let n = e.file_name();
        let ns = n.to_string_lossy();
        if ns.starts_with("probe.7z.") {
            fs::copy(e.path(), clean.join(n)).unwrap();
        }
    }
    let first_clean = clean.join("probe.7z.001");
    let dest_clean = clean.join("probe");
    let _ = fs::remove_dir_all(&dest_clean);
    let code = run_exe_blocking(&exe, &["-x", &first_clean.to_string_lossy()], Duration::from_secs(120));
    assert_eq!(code, 0, "7z multi extract failed");

    assert!(dest_clean.join("big.bin").exists(), "big.bin missing after multi extract");
    assert_eq!(sha256_file(&dest_clean.join("big.bin")), sha256_bytes(&payload),
        "7z multi-volume content mismatch");
    eprintln!("PASS sevenz_multi: content byte-identical across {part_count} parts");
}

// ─── helpers ──────────────────────────────────────────────────────────────

fn sha256_bytes(b: &[u8]) -> String {
    use sha2::{Sha256, Digest};
    let mut h = Sha256::new(); h.update(b); hex::encode(h.finalize())
}
fn sha256_file(p: &std::path::Path) -> String {
    use sha2::{Sha256, Digest};
    let mut h = Sha256::new();
    let mut f = std::fs::File::open(p).unwrap();
    let mut buf = [0u8; 64*1024];
    loop { let n = std::io::Read::read(&mut f, &mut buf).unwrap(); if n==0 {break;} h.update(&buf[..n]); }
    hex::encode(h.finalize())
}
fn mtime_secs(p: &std::path::Path) -> u64 {
    let m = std::fs::symlink_metadata(p).unwrap();
    m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs()).unwrap_or(0)
}
fn set_mtime(p: &std::path::Path, secs: u64) {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, SetFileTime, FILE_GENERIC_WRITE, FILE_FLAG_BACKUP_SEMANTICS};
    use std::os::windows::ffi::OsStrExt;
    let mut wide: Vec<u16> = p.as_os_str().encode_wide().collect(); wide.push(0);
    let intervals = (secs + 11644473600) * 10_000_000;
    let ft = FILETIME { dwLowDateTime: (intervals & 0xFFFFFFFF) as u32, dwHighDateTime: (intervals >> 32) as u32 };
    unsafe {
        let h = CreateFileW(wide.as_ptr(), FILE_GENERIC_WRITE, 0, std::ptr::null(), 3, FILE_FLAG_BACKUP_SEMANTICS, std::ptr::null_mut());
        if h as isize != -1 {
            let _ = SetFileTime(h, std::ptr::null(), std::ptr::null(), &ft);
            let _ = windows_sys::Win32::Foundation::CloseHandle(h);
        }
    }
}
fn run_exe_blocking(exe: &std::path::Path, args: &[&str], timeout: Duration) -> i32 {
    let start = Instant::now();
    let mut child = std::process::Command::new(exe).args(args).spawn().unwrap();
    loop {
        match child.try_wait() {
            Ok(Some(s)) => return s.code().unwrap_or(-1),
            Ok(None) => {
                if start.elapsed() > timeout { let _ = child.kill(); panic!("exe timeout"); }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(_) => panic!("wait failed"),
        }
    }
}
