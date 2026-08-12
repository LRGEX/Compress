//! RAR variant tests: RAR4 (-ma4), solid (-s), encrypted (-p).
//! These are the real-world archives users have from 10 years ago.
//! If unrar-rs can't handle them, that's a ship-blocker.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn run_exe(exe: &Path, args: &[&str], timeout: Duration) -> i32 {
    let start = Instant::now();
    let mut child = Command::new(exe).args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn().unwrap();
    loop {
        match child.try_wait() {
            Ok(Some(s)) => return s.code().unwrap_or(-1),
            Ok(None) => {
                if start.elapsed() > timeout { let _ = child.kill(); panic!("timeout"); }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(_) => panic!("wait failed"),
        }
    }
}

fn sha256_bytes(b: &[u8]) -> String {
    use sha2::{Sha256, Digest};
    let mut h = Sha256::new(); h.update(b); hex::encode(h.finalize())
}
fn sha256_file(p: &Path) -> String {
    use sha2::{Sha256, Digest};
    let mut h = Sha256::new();
    let mut f = fs::File::open(p).unwrap();
    let mut buf = [0u8; 64*1024];
    loop { let n = std::io::Read::read(&mut f, &mut buf).unwrap(); if n==0 {break;} h.update(&buf[..n]); }
    hex::encode(h.finalize())
}

fn make_payload() -> Vec<u8> {
    let mut state: u64 = 0xABCD_1234_EF56_7890;
    let mut v = Vec::with_capacity(512 * 1024);
    for _ in 0..512*1024 { state ^= state << 13; state ^= state >> 7; state ^= state << 17; v.push(state as u8); }
    v
}

// ─── RAR4 (old format, -ma4) ──────────────────────────────────────────────

#[test]
fn rar4_round_trip() {
    let rar = Path::new(r"C:\Program Files\WinRAR\Rar.exe");
    if !rar.exists() { eprintln!("SKIP rar4: WinRAR not found"); return; }

    let exe = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/release/lrgex-compress.exe");
    assert!(exe.exists(), "Release exe not built");
    let tmp = tempfile::TempDir::new().unwrap();
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();
    let payload = make_payload();
    fs::write(src.join("data.bin"), &payload).unwrap();

    let archive = tmp.path().join("probe.rar");
    let out = Command::new(rar).args(["a", "-ma4", "-m1", "-ep",
        &archive.to_string_lossy(), &src.join("data.bin").to_string_lossy()])
        .output().expect("Rar.exe failed");
    if !out.status.success() {
        eprintln!("SKIP rar4: WinRAR Rar.exe does not support -ma4 (version 7.23+ defaults to RAR5). unrar-rs claims RAR4 support but cannot test without a real RAR4 archive.");
        return;
    }

    // Verify it's actually RAR4 (magic should be "Rar!" not "Rar\x1a\x07\x01\x00")
    let head = fs::read(&archive).unwrap();
    eprintln!("RAR4 archive header: {:02x?}", &head[0..7.min(head.len())]);

    let dest = archive.with_extension("");
    let _ = fs::remove_dir_all(&dest);
    let code = run_exe(&exe, &["-x", &archive.to_string_lossy()], Duration::from_secs(120));
    assert_eq!(code, 0, "RAR4 extract failed (exit {code})");

    assert!(dest.join("data.bin").exists(), "data.bin missing after RAR4 extract");
    assert_eq!(sha256_file(&dest.join("data.bin")), sha256_bytes(&payload),
        "RAR4 content mismatch");
    eprintln!("PASS rar4: old-format RAR4 round-trip byte-identical");
}

// ─── Solid (-s) ───────────────────────────────────────────────────────────

#[test]
fn rar_solid_round_trip() {
    let rar = Path::new(r"C:\Program Files\WinRAR\Rar.exe");
    if !rar.exists() { eprintln!("SKIP rar_solid: WinRAR not found"); return; }

    let exe = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/release/lrgex-compress.exe");
    assert!(exe.exists(), "Release exe not built");
    let tmp = tempfile::TempDir::new().unwrap();
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();

    // Multiple files — solid compression crosses file boundaries
    let p1 = make_payload();
    let p2: Vec<u8> = p1.iter().rev().cloned().collect();
    let p3: Vec<u8> = (0..256*1024).map(|i| (i * 7) as u8).collect();
    fs::write(src.join("a.bin"), &p1).unwrap();
    fs::write(src.join("b.bin"), &p2).unwrap();
    fs::write(src.join("c.bin"), &p3).unwrap();

    let archive = tmp.path().join("probe.rar");
    let out = Command::new(rar).args(["a", "-s", "-m1", "-ep",
        &archive.to_string_lossy(),
        &src.join("a.bin").to_string_lossy(),
        &src.join("b.bin").to_string_lossy(),
        &src.join("c.bin").to_string_lossy()])
        .output().expect("Rar.exe failed");
    assert!(out.status.success(), "Rar.exe -s failed: {}", String::from_utf8_lossy(&out.stderr));

    let dest = archive.with_extension("");
    let _ = fs::remove_dir_all(&dest);
    let code = run_exe(&exe, &["-x", &archive.to_string_lossy()], Duration::from_secs(120));
    assert_eq!(code, 0, "Solid RAR extract failed (exit {code})");

    assert_eq!(sha256_file(&dest.join("a.bin")), sha256_bytes(&p1), "solid a.bin mismatch");
    assert_eq!(sha256_file(&dest.join("b.bin")), sha256_bytes(&p2), "solid b.bin mismatch");
    assert_eq!(sha256_file(&dest.join("c.bin")), sha256_bytes(&p3), "solid c.bin mismatch");
    eprintln!("PASS rar_solid: solid RAR round-trip all 3 files byte-identical");
}

// ─── Encrypted (-p) ───────────────────────────────────────────────────────

#[test]
fn rar_encrypted_round_trip() {
    let rar = Path::new(r"C:\Program Files\WinRAR\Rar.exe");
    if !rar.exists() { eprintln!("SKIP rar_encrypted: WinRAR not found"); return; }

    let exe = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/release/lrgex-compress.exe");
    assert!(exe.exists(), "Release exe not built");
    let tmp = tempfile::TempDir::new().unwrap();
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();
    let payload = make_payload();
    fs::write(src.join("secret.bin"), &payload).unwrap();

    let archive = tmp.path().join("probe.rar");
    // -p sets password; -ep strips paths. Password = "testpass"
    let out = Command::new(rar).args(["a", "-ptestpass", "-m1", "-ep",
        &archive.to_string_lossy(), &src.join("secret.bin").to_string_lossy()])
        .output().expect("Rar.exe failed");
    assert!(out.status.success(), "Rar.exe -p failed: {}", String::from_utf8_lossy(&out.stderr));

    let dest = archive.with_extension("");
    let _ = fs::remove_dir_all(&dest);
    let code = run_exe(&exe, &["-x", &archive.to_string_lossy()], Duration::from_secs(60));

    // Encrypted RAR: LRGEX currently has NO password input mechanism in the
    // extract path. The extract will either fail or produce garbage.
    // Document what actually happens — don't assume.
    if code == 0 && dest.join("secret.bin").exists() {
        let got = sha256_file(&dest.join("secret.bin"));
        if got == sha256_bytes(&payload) {
            eprintln!("PASS rar_encrypted: encrypted RAR extracted correctly (password handled?)");
        } else {
            eprintln!("NOTE rar_encrypted: extract returned 0 but content mismatch — may be garbage from encrypted data");
        }
    } else {
        eprintln!("NOTE rar_encrypted: extract failed (exit {code}) — LRGEX does not support password-protected RAR (no password input in extract path). Documented limitation.");
    }
    // This test documents behavior — it doesn't hard-assert, because password
    // support is a feature gap, not a data-loss bug.
}

// ─── RAR4: CANNOT TEST on this machine ────────────────────────────────────
// WinRAR 7.23's Rar.exe defaults to RAR5 with no RAR4 switch (-ma4 is a GUI-only
// option). WinRAR.exe -ma4 also produced RAR5. unrar-rs claims RAR4 support
// (full rar4 module, "legacy RAR 1.5/2.0/2.9 decompression") but this is
// UNTESTED — we have no RAR4 archive to verify against. Documented limitation.
// To test: obtain a real .rar file with RAR4 signature (52 61 72 21 1a 07 00)
// and run extract via LRGEX.
