//! Secondary-format metadata round-trip tests — closes the per-format fidelity gaps.
//!
//! zgx metadata was proven in metadata_round_trip.rs. These cover the OTHER formats:
//!   - zip: mtime via DOS time field (zip carries no ctime / no Windows attrs)
//!   - RAR: mtime (restored by unrar's extract_with_base; engine move_dir_contents preserves it)
//!
//! Characterization-first: these tests assert what each format ACTUALLY preserves,
//! not what zgx preserves. If a format doesn't carry a field, we don't test it there.

mod common;

use common::harness;
use std::fs;
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, SetFileTime, FILE_GENERIC_WRITE, FILE_FLAG_BACKUP_SEMANTICS};

fn set_mtime(p: &Path, secs: u64) {
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
fn mtime_secs(p: &Path) -> u64 {
    let m = fs::symlink_metadata(p).unwrap();
    m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)
}
/// Read ctime via GetFileAttributesExW (Windows API) — more reliable than std's created().
/// Returns 0 on failure.
fn read_ctime_winapi(p: &Path) -> u64 {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetFileAttributesExW;
    use windows_sys::Win32::Storage::FileSystem::WIN32_FILE_ATTRIBUTE_DATA;
    let mut wide: Vec<u16> = p.as_os_str().encode_wide().collect(); wide.push(0);
    let mut data: WIN32_FILE_ATTRIBUTE_DATA = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetFileAttributesExW(wide.as_ptr(), 0, &mut data as *mut _ as *mut _) };
    if ok == 0 { return 0; }
    // ftCreationTime is 100ns intervals since 1601. Convert to unix secs.
    let lo = data.ftCreationTime.dwLowDateTime as u64;
    let hi = data.ftCreationTime.dwHighDateTime as u64;
    let intervals = (hi << 32) | lo;
    if intervals == 0 { return 0; }
    (intervals / 10_000_000).saturating_sub(11644473600)
}
fn run_exe_blocking(exe: &Path, args: &[String], timeout: Duration) -> i32 {
    let start = Instant::now();
    let mut child = Command::new(exe).args(args).spawn().unwrap();
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

// ─── ZIP metadata ──────────────────────────────────────────────────────────

#[test]
fn zip_mtime_round_trips() {
    // zip carries DOS mtime (2-second granularity). Set a known mtime on the source,
    // build a zip with the `zip` crate (which preserves mtime), extract via LRGEX,
    // verify mtime round-trips at DOS granularity (2-sec resolution).
    let exe = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/release/lrgex-compress.exe");
    assert!(exe.exists(), "Release exe not built");

    let tmp = TempDir::new().unwrap();
    let src_file = tmp.path().join("src_file.txt");
    fs::write(&src_file, b"zip mtime probe").unwrap();
    // 2020-01-01 00:00:00 UTC = 1577836800 (even second — DOS-safe)
    set_mtime(&src_file, 1577836800);
    assert_eq!(mtime_secs(&src_file), 1577836800, "setup failed");

    // Build a zip that carries a KNOWN mtime (2020-01-01 = 1577836800).
    // zip 8.6 DateTime has no from_time(); use from_date_and_time(2020,1,1,0,0,0).
    let archive = tmp.path().join("probe.zip");
    {
        let f = fs::File::create(&archive).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let dt = zip::DateTime::from_date_and_time(2020, 1, 1, 0, 0, 0)
            .expect("valid zip datetime");
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .last_modified_time(dt);
        z.start_file("src_file.txt", opts).unwrap();
        z.write_all(b"zip mtime probe").unwrap();
        z.finish().unwrap();
    }

    // Extract via LRGEX (extracts to sibling folder named "probe").
    let dest = archive.with_extension("");
    let _ = fs::remove_dir_all(&dest);
    let code = run_exe_blocking(&exe, &["-x".into(), archive.to_string_lossy().to_string()], Duration::from_secs(120));
    assert_eq!(code, 0, "zip extract failed (exit {code})");

    let out_file = dest.join("src_file.txt");
    assert!(out_file.exists(), "src_file.txt missing after zip extract");

    let got_mtime = mtime_secs(&out_file);
    // DOS time has 2-second granularity, so allow ±2s.
    let diff = (got_mtime as i64 - 1577836800).abs();
    assert!(diff <= 2, "ZIP MTIME MISMATCH: expected ~1577836800, got {got_mtime} (diff {diff}s)");
    // Defense (advisor): confirm ctime was NOT clobbered to 1970. apply_times_path(path, mt, 0)
    // passes ctime=0 which apply_times_handle treats as NULL (don't touch) — so ctime should
    // be roughly now(), NOT epoch 0. Read via the Windows API (std's created() can be flaky).
    let got_ctime = read_ctime_winapi(&out_file);
    if got_ctime > 0 {
        assert!(got_ctime > 1_000_000_000, "ZIP CTIME CLOBBERED: got {got_ctime} (looks like 1970) — apply_times_path ctime=0 path may be destroying creation time");
        eprintln!("PASS zip_mtime: mtime within DOS 2-sec granularity (got {got_mtime}), ctime preserved ({got_ctime})");
    } else {
        // ctime read failed (privilege/filesystem) — can't assert, but mtime is the real claim
        eprintln!("PASS zip_mtime: mtime within DOS 2-sec granularity (got {got_mtime}); ctime read returned 0 (couldn't verify, mtime is the documented zip claim)");
    }
}

// ─── RAR metadata ──────────────────────────────────────────────────────────

#[test]
fn rar_mtime_round_trips() {
    // RAR restores mtime via unrar's extract_with_base. Build a real .rar with WinRAR
    // (which carries mtime), extract via LRGEX, verify mtime round-trips.
    let rar = Path::new(r"C:\Program Files\WinRAR\Rar.exe");
    if !rar.exists() {
        eprintln!("SKIP rar_mtime: WinRAR Rar.exe not found");
        return;
    }
    let exe = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/release/lrgex-compress.exe");
    assert!(exe.exists(), "Release exe not built");

    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("rar_src");
    fs::create_dir_all(&src).unwrap();
    let probe = src.join("probe.txt");
    fs::write(&probe, b"rar mtime probe").unwrap();
    set_mtime(&probe, 1577836800);
    assert_eq!(mtime_secs(&probe), 1577836800, "setup failed");

    // Build the .rar. Run Rar from inside src with a relative path so the archive
    // stores the file at the root (not with the absolute path embedded).
    let archive = tmp.path().join("probe.rar");
    let out = Command::new(rar).args(["a", "-m1", &archive.to_string_lossy(), "probe.txt"])
        .current_dir(&src).output().expect("failed to run Rar.exe");
    assert!(out.status.success(), "Rar.exe failed: {}", String::from_utf8_lossy(&out.stderr));

    // Extract via LRGEX (sibling folder "probe").
    let dest = archive.with_extension("");
    let _ = fs::remove_dir_all(&dest);
    let code = run_exe_blocking(&exe, &["-x".into(), archive.to_string_lossy().to_string()], Duration::from_secs(120));
    assert_eq!(code, 0, "rar extract failed (exit {code})");

    let out_file = dest.join("probe.txt");
    assert!(out_file.exists(), "probe.txt missing after rar extract");

    let got_mtime = mtime_secs(&out_file);
    // RAR mtime granularity is 1 second.
    assert_eq!(got_mtime, 1577836800, "RAR MTIME MISMATCH: expected 1577836800, got {got_mtime}");
    eprintln!("PASS rar_mtime: round-trips exact (got {got_mtime})");
}
