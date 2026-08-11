//! Symlink round-trip test — the only content type with zero coverage anywhere.
//!
//! The engine has elaborate symlink code (create_symlink, elevation relaunch, target
//! validation) and none of it was tested. This test exercises the zgx symlink path:
//! create a real symlink with a known target, compress the folder, extract, verify the
//! link was recreated AND points to the right target.
//!
//! The test IS the capability probe: it tries to create the source symlink, and if the
//! OS returns a privilege error (no admin / Developer Mode off), it skip-with-messages.
//! Never a false fail on a box lacking the privilege. NOT #[ignore]'d.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[test]
fn zgx_symlink_round_trip_preserves_target() {
    let exe = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/release/lrgex-compress.exe");
    assert!(exe.exists(), "Release exe not built. Run `cargo build --release`.");

    let tmp = tempfile::TempDir::new().unwrap();
    let src = tmp.path().join("src");
    fs::create_dir_all(&src).unwrap();

    // A regular file the symlink will point to.
    let target_file = src.join("real_target.txt");
    fs::write(&target_file, b"the real content").unwrap();

    // THE PROBE: try to create a symlink pointing at the target file.
    let link_path = src.join("a_link.txt");
    let target_str = "real_target.txt"; // relative target
    #[cfg(windows)]
    {
        use std::os::windows::fs::symlink_file;
        match symlink_file(target_str, &link_path) {
            Ok(()) => { eprintln!("symlink created (privilege available)"); }
            Err(e) if e.raw_os_error() == Some(1314) => {
                // ERROR_PRIVILEGE_NOT_HELD — Windows needs admin or Developer Mode.
                eprintln!("SKIP zgx_symlink: symlink creation requires admin or Developer Mode (error 1314). \
                           Enable Developer Mode or run elevated to exercise this test.");
                return;
            }
            Err(e) => {
                // Some other error — surface it, don't silently skip.
                panic!("unexpected error creating source symlink: {e}");
            }
        }
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::symlink;
        symlink(target_str, &link_path).expect("create source symlink");
    }

    // We have a real symlink in the source tree. Compress + extract via the LRGEX exe.
    let archive = src.with_extension("zgx");
    let _ = fs::remove_file(&archive);
    let code = run_exe_blocking(&exe, &[src.to_string_lossy().to_string().as_str()], Duration::from_secs(120));
    assert_eq!(code, 0, "compress failed (exit {code})");
    assert!(archive.exists(), "no archive produced");

    // Extract to a sibling folder (archive.with_extension("") = src's parent / "src" stem
    // — but that's the SOURCE folder. Use -x to a fresh dest by extracting in a copy).
    // Simpler: extract the archive and check the link in the extracted output.
    let dest = archive.with_extension(""); // "src" folder next to the archive
    // The source folder is also named "src" — collision. Rename source first.
    let moved_src = tmp.path().join("src_orig");
    let _ = fs::remove_dir_all(&moved_src);
    fs::rename(&src, &moved_src).unwrap();
    let _ = fs::remove_dir_all(&dest);
    let code = run_exe_blocking(&exe, &["-x", &archive.to_string_lossy().to_string()], Duration::from_secs(120));
    assert_eq!(code, 0, "extract failed (exit {code})");
    assert!(dest.is_dir(), "no extracted folder");

    // THE ASSERTION: the symlink must exist in the extracted tree AND point at the target.
    let extracted_link = dest.join("a_link.txt");
    assert!(extracted_link.exists() || extracted_link.symlink_metadata().is_ok(),
        "symlink a_link.txt missing from extracted tree — symlink was not preserved");

    // Read the link target. The engine should have recreated it via create_symlink.
    let read_target = fs::read_link(&extracted_link)
        .expect("extracted a_link.txt is not a symlink — engine wrote it as a regular file or stub");
    let read_str = read_target.to_string_lossy().to_string();
    assert_eq!(read_str, target_str,
        "SYMLINK TARGET MISMATCH: source pointed at '{target_str}', extracted points at '{read_str}'");
    eprintln!("PASS zgx_symlink: link recreated, target '{target_str}' preserved");
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
