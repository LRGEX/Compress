// "Extract To..." (-o): user picks a destination folder; the archive extracts
// into an archive-named SUBFOLDER of it (Desktop Folder.zgx → <picked>\Desktop Folder\).
// Tested via the LRGEX_EXTRACT_DEST env override so no GUI folder picker is needed in CI.

use std::time::Duration;

/// GUI instances must not run concurrently (single-instance coordinator batches
/// simultaneous launches into one multi-select job — see src/multiselect.rs) —
/// serialize these tests.
static EXE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn extract_to_uses_picked_folder_directly() {
    let _lock = EXE_LOCK.lock().unwrap();
    let exe = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/release/lrgex-compress.exe");
    assert!(exe.exists(), "Release exe not built. Run `cargo build --release`.");

    let tmp = tempfile::TempDir::new().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("one.txt"), b"one").unwrap();
    std::fs::write(src.join("two.txt"), b"two").unwrap();

    // Compress
    let out = std::process::Command::new(&exe)
        .arg(src.to_string_lossy().as_ref())
        .output().expect("spawn compress");
    assert!(out.status.success(), "compress exited {}", out.status);
    let archive = tmp.path().join("src.zgx");
    assert!(archive.exists(), "archive not created");

    // Extract To: a chosen folder — contents land DIRECTLY inside it.
    let picked = tmp.path().join("chosen-dest");
    std::fs::create_dir_all(&picked).unwrap();
    let mut child = std::process::Command::new(&exe)
        .env("LRGEX_EXTRACT_DEST", picked.to_string_lossy().as_ref())
        .args(["-x", "-o", archive.to_string_lossy().as_ref()])
        .spawn().expect("spawn extract");
    let status_path = std::env::temp_dir()
        .join(format!("lrgex-compress-status-{}.json", child.id()));
    let _ = std::fs::remove_file(&status_path);

    let start = std::time::Instant::now();
    loop {
        if let Ok(c) = std::fs::read_to_string(&status_path) {
            if c.contains("\"phase\":3") || c.contains("\"phase\":4") || c.contains("\"phase\":5") {
                std::thread::sleep(Duration::from_millis(800));
                break;
            }
        }
        if start.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            panic!("no terminal status within 60s");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_file(&status_path);

    let sub = picked.join("src"); // archive-named subfolder
    assert!(sub.join("one.txt").exists(), "one.txt not in archive-named subfolder");
    assert!(sub.join("two.txt").exists(), "two.txt not in archive-named subfolder");
    assert_eq!(std::fs::read(sub.join("one.txt")).unwrap(), b"one");
    eprintln!("PASS extract_to: files land directly in the picked folder");
}

#[test]
fn extract_to_writes_nothing_outside_destination() {
    let _lock = EXE_LOCK.lock().unwrap();
    // THE LAW: Extract To writes ZERO bytes outside the chosen destination —
    // no staging dirs, no temps, nothing in dest's parent (the drive-root bug
    // wrote a 9GB staging folder to the DESKTOP when the user picked D:\).
    let exe = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/release/lrgex-compress.exe");
    assert!(exe.exists(), "Release exe not built. Run `cargo build --release`.");

    let tmp = tempfile::TempDir::new().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("data.bin"), b"payload").unwrap();

    let out = std::process::Command::new(&exe)
        .arg(src.to_string_lossy().as_ref())
        .output().expect("spawn compress");
    assert!(out.status.success(), "compress exited {}", out.status);
    let archive = tmp.path().join("src.zgx");
    assert!(archive.exists(), "archive not created");

    // dest inside a PARENT we can inspect afterwards.
    let parent = tmp.path().join("outer");
    let dest = parent.join("picked");
    std::fs::create_dir_all(&dest).unwrap();
    let before: Vec<std::path::PathBuf> = std::fs::read_dir(&parent).unwrap()
        .flatten().map(|e| e.path()).collect();
    assert_eq!(before.len(), 1, "precondition: parent holds only dest");

    let mut child = std::process::Command::new(&exe)
        .env("LRGEX_EXTRACT_DEST", dest.to_string_lossy().as_ref())
        .args(["-x", "-o", archive.to_string_lossy().as_ref()])
        .spawn().expect("spawn extract");
    let status_path = std::env::temp_dir()
        .join(format!("lrgex-compress-status-{}.json", child.id()));
    let _ = std::fs::remove_file(&status_path);

    let start = std::time::Instant::now();
    loop {
        if let Ok(c) = std::fs::read_to_string(&status_path) {
            if c.contains("\"phase\":3") || c.contains("\"phase\":4") || c.contains("\"phase\":5") {
                std::thread::sleep(Duration::from_millis(800));
                break;
            }
        }
        if start.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            panic!("no terminal status within 60s");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_file(&status_path);

    assert!(dest.join("src").join("data.bin").exists(), "extraction missing in archive-named subfolder");
    // THE LAW, part 2: dest itself must hold ONLY the extracted content — no lingering
    // staging dir inside it either.
    let dest_entries: Vec<String> = std::fs::read_dir(&dest).unwrap()
        .flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
    // dest holds the archive-named subfolder only ("src") — no staging/temp litter.
    assert_eq!(dest_entries, vec!["src".to_string()],
        "leftover staging/temp inside dest: {:?}", dest_entries);
    // THE LAW, part 1: the parent must contain ONLY dest — no staging, no temps, no litter.
    let after: Vec<std::path::PathBuf> = std::fs::read_dir(&parent).unwrap()
        .flatten().map(|e| e.path()).collect();
    assert_eq!(after.len(), 1,
        "artifacts written OUTSIDE the destination: {:?}", after);
    eprintln!("PASS extract_to_law: zero bytes outside the picked destination");
}
