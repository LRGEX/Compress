//! Black-box test harness. Spawns the REAL release exe. Zero engine coupling.
//! The GUI binary can't run concurrently — EXE_LOCK serializes every spawn.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

static EXE_LOCK: Mutex<()> = Mutex::new(());

pub fn exe() -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/release/lrgex-compress.exe");
    assert!(p.exists(), "Release exe not built. Run `cargo build --release`. Expected: {}", p.display());
    p
}

pub fn file_sha256(path: &Path) -> String {
    use sha2::{Sha256, Digest};
    let mut f = fs::File::open(path).unwrap();
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).unwrap();
        if n == 0 { break; }
        hasher.update(&buf[..n]);
    }
    hex::encode(hasher.finalize())
}

pub fn tree_fingerprint(root: &Path) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    walk(root, root, &mut |rel, path| {
        if path.is_file() {
            map.insert(rel.replace('\\', "/"), file_sha256(path));
        }
    });
    map
}

fn walk(root: &Path, cur: &Path, cb: &mut dyn FnMut(String, &Path)) {
    for entry in fs::read_dir(cur).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let rel = path.strip_prefix(root).unwrap().to_string_lossy().to_string();
        cb(rel, &path);
        if path.is_dir() {
            walk(root, &path, cb);
        }
    }
}

pub fn run(args: &[&str], timeout: Duration) -> i32 {
    let _guard = EXE_LOCK.lock().unwrap();
    let start = Instant::now();
    let mut cmd = Command::new(exe());
    cmd.args(args);
    let mut child = cmd.spawn().unwrap_or_else(|e| panic!("failed to spawn exe: {e}"));
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.code().unwrap_or(-1),
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    panic!("exe timed out after {:?}", timeout);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => panic!("wait failed: {e}"),
        }
    }
}

pub fn exe_lock() -> std::sync::MutexGuard<'static, ()> {
    EXE_LOCK.lock().unwrap()
}

pub fn compress(src: &Path) -> PathBuf {
    let dest = src.parent().unwrap().join(format!(
        "{}.zgx", src.file_name().unwrap().to_string_lossy()
    ));
    let _ = fs::remove_file(&dest);
    let code = run(&[src.to_str().unwrap()], Duration::from_secs(300));
    assert_eq!(code, 0, "compress failed (exit {code}) for {}", src.display());
    assert!(dest.exists(), "compress produced no archive at {}", dest.display());
    dest
}

pub fn extract(archive: &Path) -> PathBuf {
    let dest = archive.with_extension("");
    let _ = fs::remove_dir_all(&dest);
    let code = run(&["-x", archive.to_str().unwrap()], Duration::from_secs(300));
    assert_eq!(code, 0, "extract failed (exit {code}) for {}", archive.display());
    assert!(dest.is_dir(), "extract produced no folder at {}", dest.display());
    dest
}
