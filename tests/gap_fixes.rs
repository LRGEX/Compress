//! Tests for the gap fixes: 7z symlink detection + RAR attrs restore.
//! Both need external tools (7-Zip / WinRAR) and auto-skip when absent.
//! 7z symlink test also needs admin/Developer Mode (skips cleanly w/o).

use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

fn run_exe(exe: &std::path::Path, args: &[&str], timeout: Duration) -> i32 {
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

#[test]
fn rar_attrs_round_trip() {
    let rar = std::path::Path::new(r"C:\Program Files\WinRAR\Rar.exe");
    if !rar.exists() { eprintln!("SKIP rar_attrs: WinRAR not found"); return; }
    use std::os::windows::fs::MetadataExt;

    let exe = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/release/lrgex-compress.exe");
    assert!(exe.exists(), "Release exe not built");
    let tmp = tempfile::TempDir::new().unwrap();
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();
    let probe = src.join("probe.txt");
    fs::write(&probe, b"rar attrs probe").unwrap();

    // Set ReadOnly attribute (0x1)
    use windows_sys::Win32::Storage::FileSystem::SetFileAttributesW;
    use std::os::windows::ffi::OsStrExt;
    let mut wide: Vec<u16> = probe.as_os_str().encode_wide().collect(); wide.push(0);
    unsafe { SetFileAttributesW(wide.as_ptr(), 0x1); } // FILE_ATTRIBUTE_READONLY

    let src_attrs = fs::metadata(&probe).unwrap().file_attributes();
    eprintln!("source attrs: 0x{src_attrs:x}");
    assert_ne!(src_attrs & 0x1, 0, "setup failed: ReadOnly not set");

    // Build .rar from inside src with relative path
    let archive = tmp.path().join("probe.rar");
    let out = Command::new(rar).args(["a", "-m1", &archive.to_string_lossy(), "probe.txt"])
        .current_dir(&src).output().expect("Rar.exe failed");
    assert!(out.status.success());

    let dest = archive.with_extension("");
    let _ = fs::remove_dir_all(&dest);
    let code = run_exe(&exe, &["-x", &archive.to_string_lossy()], Duration::from_secs(120));
    assert_eq!(code, 0, "rar extract failed");

    let extracted = dest.join("probe.txt");
    assert!(extracted.exists(), "probe.txt missing");
    let e_attrs = fs::metadata(&extracted).unwrap().file_attributes();
    eprintln!("extracted attrs: 0x{e_attrs:x}");
    assert_ne!(e_attrs & 0x1, 0, "RAR ATTRS MISMATCH: ReadOnly (0x1) not preserved");
    eprintln!("PASS rar_attrs: ReadOnly attribute preserved (0x{:x})", e_attrs & 0x1);
}

// ─── 7z symlink detection ─────────────────────────────────────────────────

#[test]
fn sevenz_symlink_round_trip() {
    let sevenz = std::path::Path::new(r"C:\Program Files\7-Zip\7z.exe");
    if !sevenz.exists() { eprintln!("SKIP sevenz_symlink: 7-Zip not found"); return; }

    let exe = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/release/lrgex-compress.exe");
    assert!(exe.exists(), "Release exe not built");
    let tmp = tempfile::TempDir::new().unwrap();
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();

    // Create a real file + a symlink pointing at it
    fs::write(src.join("real.txt"), b"the real content").unwrap();
    let link_path = src.join("a_link.txt");
    #[cfg(windows)]
    {
        use std::os::windows::fs::symlink_file;
        match symlink_file("real.txt", &link_path) {
            Ok(()) => { eprintln!("symlink created"); }
            Err(e) if e.raw_os_error() == Some(1314) => {
                eprintln!("SKIP sevenz_symlink: needs admin or Developer Mode (error 1314)");
                return;
            }
            Err(e) => panic!("unexpected error creating source symlink: {e}"),
        }
    }

    // Build .7z with 7-Zip (run from src, relative paths)
    let archive = tmp.path().join("probe.7z");
    let out = Command::new(sevenz).args(["a", "-t7z", "-mx=1", &archive.to_string_lossy()])
        .current_dir(&src).output().expect("7z failed");
    assert!(out.status.success(), "7z failed: {}", String::from_utf8_lossy(&out.stderr));

    // Extract via LRGEX (sibling folder named after stem)
    let dest = archive.with_extension("");
    // Move source aside to avoid collision
    let moved = tmp.path().join("src_orig");
    let _ = fs::remove_dir_all(&moved);
    fs::rename(&src, &moved).unwrap();
    let _ = fs::remove_dir_all(&dest);
    let code = run_exe(&exe, &["-x", &archive.to_string_lossy()], Duration::from_secs(120));
    assert_eq!(code, 0, "7z extract failed");

    // The symlink should exist in the extracted tree
    let extracted_link = dest.join("a_link.txt");
    assert!(extracted_link.exists() || extracted_link.symlink_metadata().is_ok(),
        "symlink a_link.txt missing from extracted 7z tree");

    // Verify it's actually a symlink (not a regular file)
    let target = fs::read_link(&extracted_link)
        .expect("extracted a_link.txt is NOT a symlink — 7z reparse point detection failed");
    assert_eq!(target.to_string_lossy(), "real.txt",
        "7z SYMLINK TARGET MISMATCH: expected 'real.txt', got '{}'", target.to_string_lossy());
    eprintln!("PASS sevenz_symlink: link recreated, target 'real.txt' preserved");
}
