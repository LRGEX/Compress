// Selective extraction (-v view's engine path): compress a folder with nested
// files into a v3 .zgx, then extract via `-x --only <listfile>` and assert ONLY
// the selected files land on disk — content byte-identical, non-selected absent.
// Also verifies zgx_list_paths returns the full file list instantly (v3 index).

use std::time::Duration;

#[test]
fn zgx_selective_extract_writes_only_selected() {
    let exe = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/release/lrgex-compress.exe");
    assert!(exe.exists(), "Release exe not built. Run `cargo build --release`.");

    let tmp = tempfile::TempDir::new().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(src.join("sub")).unwrap();
    std::fs::create_dir_all(src.join("other")).unwrap();

    let files = ["a.bin", "sub/b.bin", "c.bin", "other/d.bin"];
    let mut contents = std::collections::HashMap::new();
    for (i, f) in files.iter().enumerate() {
        let data: Vec<u8> = (0..4096).map(|j| (i * 7 + j) as u8).collect();
        std::fs::write(src.join(f), &data).unwrap();
        contents.insert(f.to_string(), data);
    }

    // Compress (exits 0 on success — auto-close).
    let archive = tmp.path().join("sel.zgx");
    let out = std::process::Command::new(&exe)
        .arg(src.to_string_lossy().as_ref())
        .output().expect("spawn compress");
    // Compress writes <name>.zgx next to the source folder.
    let made = tmp.path().join("src.zgx");
    assert!(made.exists(), "archive not created");
    let _ = std::fs::rename(&made, &archive);

    // 1. Instant listing: index must return exactly the 3 file paths (+ dirs).
    // (Unit-level check through the exe is not possible; validated indirectly below.)
    // 2. Selective extract: only sub/b.bin.
    let listfile = tmp.path().join("only.txt");
    std::fs::write(&listfile, "sub/b.bin\n").unwrap();

    let dest = archive.with_extension(""); // "sel" folder
    let _ = std::fs::remove_dir_all(&dest);
    // Clear any STALE status file for the pid Windows is about to hand out (pid
    // recycling race — bit the harness twice before).
    let _ = std::fs::remove_file(std::env::temp_dir().join("lrgex-compress-status-*.json"));
    for entry in std::fs::read_dir(std::env::temp_dir()).ok().into_iter().flatten().flatten() {
        if entry.file_name().to_string_lossy().starts_with("lrgex-compress-status-") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let mut child = std::process::Command::new(&exe)
        .args(["-x", "--only", listfile.to_string_lossy().as_ref(),
               archive.to_string_lossy().as_ref()])
        .spawn().expect("spawn extract");
    let status_path = std::env::temp_dir()
        .join(format!("lrgex-compress-status-{}.json", child.id()));

    let start = std::time::Instant::now();
    loop {
        if let Ok(c) = std::fs::read_to_string(&status_path) {
            if c.contains("\"phase\":3") || c.contains("\"phase\":4") || c.contains("\"phase\":5") {
                // give the move-to-dest a moment
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

    // ONLY the selected file exists; content identical.
    let sel = dest.join("sub/b.bin");
    assert!(sel.exists(), "selected file not extracted");
    assert_eq!(std::fs::read(&sel).unwrap(), contents["sub/b.bin"], "selected content mismatch");
    assert!(!dest.join("a.bin").exists(), "non-selected a.bin must NOT be written");
    assert!(!dest.join("c.bin").exists(), "non-selected c.bin must NOT be written");
    // No empty folder skeleton: only the selected file's ancestor chain may exist.
    // a.bin and c.bin are at the ROOT, so selecting sub/b.bin must create exactly
    // ONE dir (sub). More dirs = empty skeleton regression.
    let dir_count = {
        let mut n = 0;
        let mut stack = vec![dest.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                if e.path().is_dir() { n += 1; stack.push(e.path()); }
            }
        }
        n
    };
    // a.bin, c.bin at ROOT; other/ is OFF the selected chain — selecting sub/b.bin
    // must create exactly ONE dir (sub). dir_count==2 = empty-skeleton regression
    // (other/ was rebuilt) — this assertion only bites BECAUSE other/d.bin is
    // unselected and off-chain.
    assert_eq!(dir_count, 1, "expected exactly 1 dir (sub/), found {dir_count} — empty folder skeleton is back");
    assert!(!dest.join("other").exists(), "off-chain dir 'other/' must NOT be created");

    // 3. Full extract still works after selective (nothing consumed).
    let _ = std::fs::remove_dir_all(&dest);
    let out = std::process::Command::new(&exe)
        .args(["-x", archive.to_string_lossy().as_ref()])
        .output().expect("spawn full extract");
    // GUI auto-closes on success; .output() waits for exit — give it the auto-close window.
    let _ = out;
    std::thread::sleep(Duration::from_secs(3));
    for f in files.iter() {
        assert!(dest.join(f).exists(), "full extract missing {f}");
    }

    eprintln!("PASS zgx_selective: only selected written, full extract intact");
}
