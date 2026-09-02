// Regression test: extraction failures must surface a reason (member file name +
// error) — never a reason-less "Failed". Root cause: extract_impls called
// prog.finish(4) without set_error, and run_one's safety net skipped backfilling
// because the status was already terminal. Fixed in main.rs + extract.rs
// (see doc/fix-later-error-reporting.md).
//
// Method: build a STORED (-m0) RAR so member data exists verbatim in the archive,
// flip one data byte (header CRCs stay valid, data CRC then mismatches), extract,
// and assert the on-disk status JSON ends with phase=4 AND a non-empty error_msg
// that names the corrupted member.

use std::time::Duration;

fn sha256_not_needed() {}

fn run_exe_blocking(exe: &std::path::Path, args: &[&str], timeout: Duration) -> i32 {
    let start = std::time::Instant::now();
    let mut child = std::process::Command::new(exe).args(args).spawn().expect("spawn exe");
    loop {
        match child.try_wait().expect("try_wait") {
            Some(status) => return status.code().unwrap_or(-1),
            None => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    return -1;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

/// Scan %TEMP% for lrgex-compress-status-*.json files and return the one with
/// the newest mtime (the test run's terminal snapshot).
fn latest_status_json() -> Option<(std::path::PathBuf, String)> {
    let dir = std::env::temp_dir();
    let mut best: Option<(std::path::PathBuf, std::time::SystemTime, String)> = None;
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("lrgex-compress-status-") && name.ends_with(".json") {
            let mtime = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            let content = std::fs::read_to_string(entry.path()).unwrap_or_default();
            if best.as_ref().map(|b| mtime > b.1).unwrap_or(true) {
                best = Some((entry.path(), mtime, content));
            }
        }
    }
    best.map(|(p, _, c)| (p, c))
}

#[test]
fn corrupt_rar_member_error_names_the_file() {
    let rar = std::path::Path::new(r"C:\Program Files\WinRAR\Rar.exe");
    if !rar.exists() {
        eprintln!("SKIP corrupt_rar_member: WinRAR Rar.exe not found — install WinRAR to run this test");
        return;
    }

    let exe = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/release/lrgex-compress.exe");
    assert!(exe.exists(), "Release exe not built. Run `cargo build --release`.");

    let tmp = tempfile::TempDir::new().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();

    // Incompressible-ish content, stored verbatim by -m0.
    let member_name = "victim.bin";
    let mut content = vec![0u8; 256 * 1024];
    let mut state: u64 = 0x1234_5678_9ABC_DEF0;
    for b in content.iter_mut() {
        state ^= state << 13; state ^= state >> 7; state ^= state << 17;
        *b = state as u8;
    }
    // Embed a unique marker deep inside so we can locate the data region.
    let marker = b"__CORRUPT_ME_MARKER__";
    content[100_000..100_000 + marker.len()].copy_from_slice(marker);
    std::fs::write(src.join(member_name), &content).unwrap();

    let archive = tmp.path().join("probe-corrupt.rar");
    let out = std::process::Command::new(rar)
        .args(["a", "-m0", "-ep",
            &archive.to_string_lossy(),
            &src.join(member_name).to_string_lossy()])
        .output()
        .expect("failed to run Rar.exe");
    assert!(out.status.success(), "Rar.exe failed: {}", String::from_utf8_lossy(&out.stderr));

    // Flip ONE data byte just after the marker (stored data → raw bytes in file).
    let mut raw = std::fs::read(&archive).unwrap();
    let pos = raw
        .windows(marker.len())
        .position(|w| w == marker)
        .expect("marker not found in stored archive");
    raw[pos + marker.len()] ^= 0xFF;
    std::fs::write(&archive, &raw).unwrap();

    let dest = archive.with_extension("");
    let _ = std::fs::remove_dir_all(&dest);

    // Failure state does NOT auto-close (user must read the error), so we poll the
    // live status JSON DURING the run — the moment the terminal snapshot appears
    // with the reason, we've proven the fix; then we kill the child.
    let start = std::time::Instant::now();
    let mut child = std::process::Command::new(&exe)
        .args(["-x", &archive.to_string_lossy()])
        .spawn()
        .expect("spawn exe");
    let status_path = std::env::temp_dir().join(format!("lrgex-compress-status-{}.json", child.id()));
    // Defensive: clear any stale file for this recycled pid.
    let _ = std::fs::remove_file(&status_path);

    let status = loop {
        if let Ok(content) = std::fs::read_to_string(&status_path) {
            if content.contains("\"phase\":4") && !content.contains("\"error_msg\":\"\"") {
                break content;
            }
            // Cancel (phase 5) or success (phase 3) would be wrong here.
            if content.contains("\"phase\":3") || content.contains("\"phase\":5") {
                let _ = child.kill();
                panic!("corrupt archive reported success/cancel — CRC check broken? status: {}", content);
            }
        }
        if start.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            panic!("no phase-4-with-reason status JSON within 60s — the reason-less \"Failed\" regression is back");
        }
        std::thread::sleep(Duration::from_millis(300));
    };
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_file(&status_path);

    eprintln!("status: {}", status);
    assert!(status.contains("\"phase\":4"), "expected phase 4 (error), got: {}", status);
    let err = status
        .split("\"error_msg\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or("")
        .to_string();
    assert!(
        !err.is_empty(),
        "error_msg is EMPTY — the reason-less \"Failed\" regression is back. status: {}",
        status
    );
    // The reason must name the corrupted member (WinRAR-parity expectation).
    assert!(
        err.contains(member_name),
        "error_msg does not name the corrupted member '{}': {}",
        member_name,
        err
    );

    // No corrupted output may survive: dest must not contain a silently-written file.
    let leaked = dest.join(member_name);
    assert!(
        !leaked.exists(),
        "corrupt member leaked to output — temp-then-rename staging violated"
    );
    eprintln!("PASS corrupt_rar_member: error names file, nothing leaked");
}

#[allow(dead_code)]
fn _keep() { let _ = run_exe_blocking; }
