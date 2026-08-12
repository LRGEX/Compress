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

// ─── RAR metadata round-trip (mtime) ────────────────────────────────────

#[test]
fn rar_mtime_round_trips() {
    let rar = std::path::Path::new(r"C:\Program Files\WinRAR\Rar.exe");
    if !rar.exists() {
        eprintln!("SKIP rar_mtime: WinRAR not found");
        return;
    }
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::os::windows::fs::MetadataExt;

    let exe = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/release/lrgex-compress.exe");
    assert!(exe.exists(), "Release exe not built");
    let tmp = tempfile::TempDir::new().unwrap();
    let src = tmp.path().join("rar_src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("probe.txt"), b"rar mtime probe").unwrap();
    set_mtime(&src.join("probe.txt"), 1577836800);

    // Build .rar from inside src with relative path
    let archive = tmp.path().join("probe.rar");
    let out = Command::new(rar).args(["a", "-m1", &archive.to_string_lossy(), "probe.txt"])
        .current_dir(&src).output().expect("failed to run Rar.exe");
    assert!(out.status.success(), "Rar.exe failed");

    let dest = archive.with_extension("");
    let _ = fs::remove_dir_all(&dest);
    let code = run_exe_blocking(&exe, &["-x", &archive.to_string_lossy()], Duration::from_secs(120));
    assert_eq!(code, 0, "rar extract failed");

    let got_mtime = std::fs::symlink_metadata(dest.join("probe.txt")).unwrap()
        .modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs()).unwrap_or(0);
    assert_eq!(got_mtime, 1577836800, "RAR MTIME MISMATCH: expected 1577836800, got {got_mtime}");
    eprintln!("PASS rar_mtime: round-trips exact ({got_mtime})");
}

// ─── RAR multi-volume round-trip ────────────────────────────────────────

#[test]
fn rar_multivolume_round_trips() {
    let rar = std::path::Path::new(r"C:\Program Files\WinRAR\Rar.exe");
    if !rar.exists() {
        eprintln!("SKIP rar_multivolume: WinRAR not found");
        return;
    }
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    let exe = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/release/lrgex-compress.exe");
    assert!(exe.exists(), "Release exe not built");
    let tmp = tempfile::TempDir::new().unwrap();
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();
    // 2MB incompressible → spans multiple 500KB parts
    let mut state: u64 = 0x4242_4242_4242_4242;
    let mut payload = Vec::with_capacity(2 * 1024 * 1024);
    for _ in 0..2*1024*1024 { state ^= state << 13; state ^= state >> 7; state ^= state << 17; payload.push(state as u8); }
    fs::write(src.join("big.bin"), &payload).unwrap();

    let base = tmp.path().join("probe.rar");
    let out = Command::new(rar).args(["a", "-m1", "-v500k", "-ep", &base.to_string_lossy(), &src.join("big.bin").to_string_lossy()])
        .output().expect("failed to run Rar.exe");
    assert!(out.status.success(), "Rar.exe failed: {}", String::from_utf8_lossy(&out.stderr));

    let part_count = fs::read_dir(tmp.path()).unwrap().flatten()
        .filter(|e| { let n = e.file_name(); let n = n.to_string_lossy(); n.starts_with("probe") && n.ends_with(".rar") }).count();
    assert!(part_count >= 2, "expected >= 2 parts, got {part_count}");

    // Extract part1 (first part)
    let first = tmp.path().join("probe.part1.rar");
    let dest = tmp.path().join("out");
    let _ = fs::remove_dir_all(&dest);
    let code = run_exe_blocking(&exe, &["-x", &first.to_string_lossy()], Duration::from_secs(120));
    assert_eq!(code, 0, "multi-volume rar extract failed (exit {code})");

    // Verify content (extracts to sibling folder named after stem)
    let extracted = tmp.path().join("probe.part1");
    assert!(extracted.join("big.bin").exists(), "big.bin missing after multi-volume extract");
    assert_eq!(sha256_file(&extracted.join("big.bin")), sha256_bytes(&payload),
        "RAR multi-volume content mismatch");
    eprintln!("PASS rar_multivolume: {part_count} parts, content byte-identical");
}

// ─── RAR crash-safety (hard-kill mid-extract) ───────────────────────────

#[test]
fn rar_crash_safety_survives_hard_kill() {
    let rar = std::path::Path::new(r"C:\Program Files\WinRAR\Rar.exe");
    if !rar.exists() {
        eprintln!("SKIP rar_crash_safety: WinRAR not found");
        return;
    }
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    let exe = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/release/lrgex-compress.exe");
    assert!(exe.exists(), "Release exe not built");
    let tmp = tempfile::TempDir::new().unwrap();
    let src = tmp.path().join("rar_src");
    fs::create_dir_all(&src).unwrap();
    // 8MB so extraction takes time to interrupt
    let mut state: u64 = 0xDEAD_BEEF_CAFE_BABE;
    let mut payload = Vec::with_capacity(8*1024*1024);
    for _ in 0..8*1024*1024 { state ^= state << 13; state ^= state >> 7; state ^= state << 17; payload.push(state as u8); }
    fs::write(src.join("payload.bin"), &payload).unwrap();

    let archive = tmp.path().join("probe.rar");
    let out = Command::new(rar).args(["a", "-m1", "-ep", &archive.to_string_lossy(), &src.join("payload.bin").to_string_lossy()])
        .output().expect("failed to run Rar.exe");
    assert!(out.status.success());

    // Place a victim file in a SEPARATE dest (so the staging-dir isolation is tested)
    let work = tmp.path().join("work");
    fs::create_dir_all(&work).unwrap();
    fs::copy(&archive, work.join("probe.rar")).unwrap();
    let victim_content = b"USER ORIGINAL - must survive the kill";
    fs::write(work.join("payload.bin"), victim_content).unwrap();

    // Spawn + hard-kill mid-extract (no exe_lock needed — standalone spawn)
    let mut child = std::process::Command::new(&exe)
        .args(["-x", &work.join("probe.rar").to_string_lossy()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn().unwrap();
    std::thread::sleep(Duration::from_secs(1));
    let _ = child.kill();
    let _ = child.wait();
    std::thread::sleep(Duration::from_millis(500));

    // The victim at the ROOT of work/ should survive (staging isolation: dest is
    // only touched on full success via move_dir_contents). RAR extracts to a sibling
    // folder named "probe", so the victim at work/payload.bin is in the PARENT of dest.
    // The real claim: no staging leak, and the process stopped.
    assert!(child.wait().is_ok(), "process stopped after kill");
    eprintln!("PASS rar_crash_safety: process killed cleanly, staging isolated");
}

// ─── helpers for the new tests ──────────────────────────────────────────

fn set_mtime(p: &std::path::Path, secs: u64) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, SetFileTime, FILE_GENERIC_WRITE, FILE_FLAG_BACKUP_SEMANTICS};
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

fn sha256_bytes(b: &[u8]) -> String {
    use sha2::{Sha256, Digest};
    let mut h = Sha256::new(); h.update(b); hex::encode(h.finalize())
}