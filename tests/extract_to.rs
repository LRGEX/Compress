// "Extract To..." (-o): user picks a destination folder; contents extract
// DIRECTLY into it (no subfolder). Tested via the LRGEX_EXTRACT_DEST env
// override so no GUI folder picker is needed in CI.

use std::time::Duration;

#[test]
fn extract_to_uses_picked_folder_directly() {
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

    assert!(picked.join("one.txt").exists(), "one.txt not in picked folder");
    assert!(picked.join("two.txt").exists(), "two.txt not in picked folder");
    assert_eq!(std::fs::read(picked.join("one.txt")).unwrap(), b"one");
    eprintln!("PASS extract_to: files land directly in the picked folder");
}
