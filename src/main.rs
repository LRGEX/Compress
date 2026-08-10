// LRGEX Compress — GUI entry (Slint progress window).
//   lrgex-compress <folder>      compress folder -> folder.zgx
//   lrgex-compress -x <archive>  extract archive -> <archive>\ folder
//
// A native window pops up with a progress bar + % + MB/s + ETA (WinRAR-style).
// The operation runs in a background thread; a Slint timer reads the heartbeat
// status file every 200ms and updates the window (same pattern as LRGEX Restore).

#![windows_subsystem = "windows"] // GUI app — no console window

mod compress;
mod extract;
mod metaattr;
mod multiselect;
mod progress;
mod segment;
mod update;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use slint::{Timer, TimerMode};

slint::slint! {
    import { VerticalBox, HorizontalBox, Button, LineEdit } from "std-widgets.slint";

    // Effect 1: gentle glow behind the logo — pulses in place, no expansion.
    component Halo inherits Rectangle {
        in property <bool> active: false;
        property <float> t: Math.mod(animation-tick() / 1ms, 2000) / 2000;
        width: 48px;
        height: 48px;

        Rectangle {
            width: 46px;
            height: 46px;
            x: (parent.width - self.width) / 2;
            y: (parent.height - self.height) / 2;
            border-radius: self.width / 2;
            border-width: 3px;
            border-color: #cb803c;
            background: transparent;
            // Gentle pulse — triangle wave, oscillates 0.4↔1.0↔0.4, clearly visible.
            opacity: root.active ? 0.4 + 0.6 * (root.t < 0.5 ? root.t * 2 : (1 - root.t) * 2) : 0.0;
        }
    }

    export component ProgressWindow inherits Window {
        title: "LRGEX Compress";
        icon: @image-url("../assets/logo.png");
        background: #1e1e1e;
        preferred-width: 440px;
        preferred-height: 180px;
        max-width: 440px;
        max-height: 180px;

        in property <string> op-label: "Working";
        in property <string> op-detail: "";
        in property <float> progress-fraction: 0;     // 0.0 .. 1.0
        in property <string> detail: "Starting...";
        in property <bool> done: false;
        in property <bool> cancelling: false;
        in property <bool> cancellable: false;
        in property <bool> indeterminate: false;
        in property <string> result: "";
        in property <color> result-color: #4caf50;
        in property <string> version-text: "";
        callback close-clicked();
        callback cancel-clicked();

        VerticalBox {
            spacing: 8px;

            HorizontalLayout {
                spacing: 12px;
                alignment: start;

                Rectangle {
                    width: 48px;
                    height: 44px;
                    Image {
                        source: @image-url("../assets/logo.png");
                        height: 34px;
                        image-fit: contain;
                        x: (parent.width - self.width) / 2;
                        y: (parent.height - self.height) / 2;
                    }
                }

                VerticalLayout {
                    alignment: center;
                    spacing: 2px;
                    Text {
                        text: root.op-label;
                        color: #f0f0f0;
                        font-size: 15px;
                        font-weight: 700;
                        wrap: word-wrap;
                    }
                    Text {
                        text: root.op-detail;
                        color: #aaaaaa;
                        font-size: 12px;
                        wrap: word-wrap;
                    }
                }
            }

            // Progress bar
            Rectangle {
                height: 24px;
                width: 100%;
                background: #2d2d2d;
                border-radius: 6px;
                clip: true;

                Rectangle {
                    visible: !root.indeterminate;
                    x: 0;
                    y: 0;
                    height: 100%;
                    width: parent.width * root.progress-fraction;
                    background: #cb803c;
                    clip: true;
                    animate width { duration: 120ms; }

                    // Subtle shimmer — a thin white stripe sweeping across the fill.
                    Rectangle {
                        property <float> s: Math.mod(animation-tick() / 1ms, 1400) / 1400;
                        visible: !root.done && root.progress-fraction > 0.0;
                        width: 40px;
                        height: parent.height;
                        x: -40px + self.s * (parent.width + 80px);
                        background: @linear-gradient(90deg,
                            #ffffff00 0%, #ffffff40 50%, #ffffff00 100%);
                    }
                }

                sweep := Rectangle {
                    property <float> s: Math.mod(animation-tick() / 1ms, 1600) / 1600;
                    visible: root.indeterminate;
                    width: parent.width * 0.28;
                    x: (parent.width - self.width)
                       * (self.s < 0.5 ? self.s * 2 : (1.0 - self.s) * 2);
                    background: #cb803c;
                }
            }

            Text {
                text: root.detail;
                color: #cb803c;
                font-size: 13px;
                wrap: word-wrap;
            }
            if root.cancellable && !root.done && !root.cancelling : Button {
                text: "Cancel";
                clicked => { root.cancel-clicked(); }
            }
            if !root.done && root.cancelling : Text {
                text: "Cancelling...";
                color: #aaaaaa;
                font-size: 13px;
            }
            if root.done : Text {
                text: root.result;
                color: root.result-color;
                font-size: 14px;
                horizontal-alignment: center;
            }
            if root.done : Button {
                text: "Close";
                clicked => { root.close-clicked(); }
            }
        }

        // Version badge in the lower right corner
        Text {
            text: root.version-text;
            color: #555555;
            font-size: 10px;
            x: parent.width - self.width - 8px;
            y: parent.height - self.height - 24px;
        }
    }

    export component ErrorWindow inherits Window {
        title: "LRGEX Compress";
        background: #1e1e1e;
        preferred-width: 380px;
        preferred-height: 130px;
        in property <string> message: "";
        callback close-clicked();
        VerticalBox {
            Text {
                text: root.message;
                color: #f44336;
                wrap: word-wrap;
                font-size: 14px;
            }
            Button {
                text: "Close";
                clicked => { root.close-clicked(); }
            }
        }
    }

    export component SplitWindow inherits Window {
        title: "LRGEX Compress - Split";
        background: #1e1e1e;
        preferred-width: 340px;
        preferred-height: 200px;
        in-out property <string> size-text: "30";
        callback accepted(bool);

        VerticalBox {
            Text {
                text: "Split into volumes of:";
                color: #e0e0e0;
                font-size: 14px;
            }

            HorizontalBox {
                LineEdit {
                    text <=> root.size-text;
                    preferred-width: 120px;
                }
                Text {
                    text: "MB";
                    color: #888;
                    vertical-alignment: center;
                }
            }

            Text {
                text: "Quick:";
                color: #888;
                font-size: 12px;
            }

            HorizontalBox {
                Button { text: "50"; clicked => { root.size-text = "50"; } }
                Button { text: "100"; clicked => { root.size-text = "100"; } }
                Button { text: "700"; clicked => { root.size-text = "700"; } }
                Button { text: "4G"; clicked => { root.size-text = "4096"; } }
            }

            HorizontalBox {
                Button { text: "Cancel"; clicked => { root.accepted(false); } }
                Button { text: "Compress"; clicked => { root.accepted(true); } }
            }
        }
    }
}

fn show_help() {
    let app = match ErrorWindow::new() {
        Ok(a) => a,
        Err(_) => return,
    };
    app.set_message(
        "LRGEX Compress — usage:\n\n\
         lrgex-compress <folder-or-file>      Compress → <name>.zgx\n\
         lrgex-compress -x <archive>          Extract → <name>\\ folder\n\
         lrgex-compress -x -h <archive>       Extract here (into the archive's folder)\n\
         lrgex-compress --split [--size <MB>] <folder-or-file>\n\
                                              Compress → split .partNNN.zgx\n\
                                              (default 30 MB; GUI prompts if --size omitted)\n\
         lrgex-compress --help / -h / /?      Show this help\n\n\
         Supported formats:\n\
         • Compress: .zgx (tar + zstd)\n\
         • Extract:  .zgx, .zip, .rar\n\n\
         Or right-click any file/folder in Explorer."
            .into());
    app.on_close_clicked(|| {
        let _ = slint::quit_event_loop();
    });
    let _ = app.run();
}

fn show_error(msg: &str) {
    let app = match ErrorWindow::new() {
        Ok(a) => a,
        Err(_) => return,
    };
    app.set_message(msg.into());
    app.on_close_clicked(|| {
        let _ = slint::quit_event_loop();
    });
    let _ = app.run();
}

/// WinRAR-style overwrite confirmation. If the destination archive already exists,
/// prompt the user before starting compression. Returns true to proceed (overwrite
/// or no conflict), false to cancel. Only used for COMPRESS — extraction keeps its
/// existing silent-overwrite behavior.
fn confirm_overwrite(dest: &std::path::Path) -> bool {
    if !dest.exists() {
        return true; // no conflict
    }
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    rfd::MessageDialog::new()
        .set_title("Confirm Replace")
        .set_description(&format!(
            "{} already exists.\n\nDo you want to replace it?",
            name
        ))
        .set_buttons(rfd::MessageButtons::YesNo)
        .show()
        == rfd::MessageDialogResult::Yes
}

/// WinRAR-style overwrite confirmation for EXTRACTION. Called when the archive
/// contains at least one file that would overwrite an existing file in the dest.
/// `dest_name` is the destination folder/location shown in the prompt.
fn confirm_extract_overwrite(dest_name: &str) -> bool {
    rfd::MessageDialog::new()
        .set_title("Confirm Replace")
        .set_description(&format!(
            "One or more files in '{}' already exist.\n\nDo you want to replace them?",
            dest_name
        ))
        .set_buttons(rfd::MessageButtons::YesNo)
        .show()
        == rfd::MessageDialogResult::Yes
}

/// Guard so the VEH logs exactly once (no recursion on re-entry).
static CRASH_LOGGED: AtomicBool = AtomicBool::new(false);

/// Install a vectored exception handler that catches ACCESS_VIOLATION (0xC0000005)
/// and other hardware exceptions. Rust panic hooks do NOT catch these. Writes a
/// detailed crash log with a full backtrace to %LOCALAPPDATA%\LRGEX Compress\crash-log.txt
/// so no crash is ever silent.
fn install_crash_handler() {
    use windows_sys::Win32::System::Diagnostics::Debug::{
        AddVectoredExceptionHandler, EXCEPTION_CONTINUE_SEARCH,
        EXCEPTION_POINTERS,
    };
    use windows_sys::Win32::Foundation::EXCEPTION_ACCESS_VIOLATION;
    unsafe extern "system" fn handler(except_info: *mut EXCEPTION_POINTERS) -> i32 {
        if CRASH_LOGGED.swap(true, Ordering::SeqCst) {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        if except_info.is_null() { return EXCEPTION_CONTINUE_SEARCH; }
        let record = (*except_info).ExceptionRecord;
        if record.is_null() { return EXCEPTION_CONTINUE_SEARCH; }
        let code = (*record).ExceptionCode;
        let fault_addr = (*record).ExceptionAddress as usize;
        let info = (*record).ExceptionInformation;
        let access_type = if info.len() > 0 { info[0] } else { 0 };
        let bad_addr = if info.len() > 1 { info[1] } else { 0 };
        let tid = windows_sys::Win32::System::Threading::GetCurrentThreadId();
        let module_base = windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) as usize;
        let rva = fault_addr.wrapping_sub(module_base);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs()).unwrap_or(0);
        let exe = std::env::current_exe()
            .map(|e| e.display().to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        // Capture a full backtrace — the faulting thread's stack is still intact.
        // dbghelp resolves symbols via the PDB (must be next to the exe).
        let bt = backtrace::Backtrace::new();
        let access_name = if access_type == 0 { "read" } else if access_type == 1 { "write" } else { "exec" };
        let log = format!(
            "LRGEX Compress CRASH\nTime(epoch): {}\nException: 0x{:08X} {}\nFault RVA: 0x{:X}\nFault addr: 0x{:016X}\nAccess: {} address 0x{:016X}\nThread: {}\nExe: {}\nVersion: {}\nArgs: {:?}\n\n=== BACKTRACE ===\n{:?}\n",
            timestamp, code as u32,
            if code == EXCEPTION_ACCESS_VIOLATION { "ACCESS_VIOLATION" } else { "OTHER" },
            rva, fault_addr, access_name, bad_addr,
            tid, exe, env!("CARGO_PKG_VERSION"),
            std::env::args().collect::<Vec<_>>(), bt,
        );
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            let dir = std::path::PathBuf::from(local).join("LRGEX Compress");
            let _ = std::fs::create_dir_all(&dir);
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("crash-log.txt")) {
                let _ = f.write_all(log.as_bytes());
                let _ = f.write_all(b"\n---\n\n");
            }
        }
        EXCEPTION_CONTINUE_SEARCH
    }
    unsafe { AddVectoredExceptionHandler(1, Some(handler)); }
}

fn main() {
    install_crash_handler();
    let args_raw: Vec<String> = std::env::args().collect();
    // Detect and strip the internal `--elevated-rerun` sentinel (set by symlink-elevation
    // relaunch). When present, extract skips regular files that already exist and only
    // recreates symlinks. Strip it so it doesn't pollute positional arg parsing.
    let elevated_rerun = args_raw.iter().any(|a| a == "--elevated-rerun");
    if elevated_rerun {
        crate::extract::ELEVATED_RERUN.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    let args: Vec<String> = args_raw.into_iter().filter(|a| a != "--elevated-rerun").collect();

    // --- HELP path ---
    if args.len() >= 2 && (args[1] == "--help" || args[1] == "-h" || args[1] == "/?") {
        show_help();
        return;
    }

    // --- SPLIT COMPRESS path ---
    let is_split = args.len() >= 2 && args[1] == "--split";
    if is_split {
        let mut has_size = false;
        let mut size_mb: u32 = 30;
        let mut folder: Option<PathBuf> = None;
        let mut i = 2;
        while i < args.len() {
            if args[i] == "--size" && i + 1 < args.len() {
                has_size = true;
                size_mb = args[i + 1].parse::<u32>().unwrap_or(30).max(1);
                i += 2;
            } else {
                folder = Some(PathBuf::from(&args[i]));
                i += 1;
            }
        }
        let folder = match folder {
            Some(f) if f.is_dir() || f.is_file() => f,
            _ => { show_error("No folder specified for split compress."); return; }
        };

        // Multiselect coordination — same as regular compress.
        // Explorer spawns N instances for N selected files; the coordinator collects them.
        let paths = multiselect::collect_paths(folder.clone());
        if paths.is_empty() {
            return; // forwarder, done
        }

        // If --size wasn't given on CLI, show the Slint SplitWindow for input.
        if !has_size {
            let app = match SplitWindow::new() {
                Ok(a) => a,
                Err(e) => { show_error(&format!("UI error: {}", e)); return; }
            };
            let accepted = Arc::new(AtomicBool::new(false));
            let accepted_clone = accepted.clone();
            app.on_accepted(move |ok| {
                accepted_clone.store(ok, Ordering::Relaxed);
                let _ = slint::quit_event_loop();
            });
            let _ = app.run();
            if !accepted.load(Ordering::Relaxed) { return; }
            size_mb = app.get_size_text().parse::<u32>().unwrap_or(30).max(1);
        }

        // Single vs multi: same logic as regular compress.
        if paths.len() == 1 {
            let f = paths.into_iter().next().unwrap();
            let name = f.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "Archive".to_string());
            let dest_base = f.parent()
                .map(|p| p.join(&name))
                .unwrap_or_else(|| PathBuf::from(&name));
            // Overwrite check for split parts.
            let parent = dest_base.parent().unwrap_or(std::path::Path::new("."));
            let stale_parts: Vec<PathBuf> = std::fs::read_dir(parent)
                .into_iter().flatten().flatten()
                .map(|e| e.path())
                .filter(|p| {
                    let s = p.file_name().unwrap_or_default().to_string_lossy();
                    s.starts_with(&format!("{}.part", name)) && s.ends_with(".zgx")
                })
                .collect();
            if !stale_parts.is_empty() {
                if !confirm_overwrite(&stale_parts[0]) { return; }
                for p in &stale_parts { let _ = std::fs::remove_file(p); }
            }
            run_one(
                "Compressing (Split)".to_string(),
                Some(format!("{} MB parts", size_mb)),
                true, dest_base,
                OpKind::CompressSplit(f, size_mb),
            );
        } else {
            // Multi: archive named after shared parent, placed in that parent.
            let parent = paths.iter()
                .filter_map(|p| p.parent().map(|x| x.to_path_buf()))
                .next()
                .unwrap_or_else(|| PathBuf::from("."));
            let label = parent.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "Archive".to_string());
            let dest_base = parent.join(&label);
            let op_detail = format!("{} ({} items)", label, paths.len());
            // Overwrite check.
            let stale_parts: Vec<PathBuf> = std::fs::read_dir(&parent)
                .into_iter().flatten().flatten()
                .map(|e| e.path())
                .filter(|p| {
                    let s = p.file_name().unwrap_or_default().to_string_lossy();
                    s.starts_with(&format!("{}.part", label)) && s.ends_with(".zgx")
                })
                .collect();
            if !stale_parts.is_empty() {
                if !confirm_overwrite(&stale_parts[0]) { return; }
                for p in &stale_parts { let _ = std::fs::remove_file(p); }
            }
            // Multi-select split: compress ALL files into ONE split archive.
            run_one(
                "Compressing (Split)".to_string(),
                Some(op_detail),
                true, dest_base,
                OpKind::CompressSplitMany(paths, size_mb),
            );
        }
        return;
    }

    let is_extract = args.len() >= 3 && args[1] == "-x";

    // --- EXTRACT path (no multi-select) ---
    if is_extract {
        // -x <archive> = Extract to subfolder (current behavior)
        // -xh <archive> = Extract Here (contents dumped into archive's parent)
        let extract_here = args.len() >= 4 && args[2] == "-h";
        let archive_path = if extract_here { &args[3] } else { &args[2] };
        let archive = PathBuf::from(archive_path);
        // Finding #5: Path-length guard — Windows MAX_PATH is 260; paths near or over
        // that fail with a confusing "not found". Give a clear error instead.
        if archive.as_os_str().len() > 247 {
            show_error(&format!("Path too long ({} characters).\nMove the file to a shorter path and try again.\n\n{}", archive.as_os_str().len(), archive.display()));
            return;
        }
        if !archive.is_file() {
            show_error(&format!("Archive not found:\n{}", archive.display()));
            return;
        }
        let name = archive.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let op_label = "Extracting".to_string();
        let op_detail = name.clone();
        let dest: PathBuf = if extract_here {
            archive.parent().unwrap_or(PathBuf::from(".").as_path()).to_path_buf()
        } else if let Some((base, _)) = crate::segment::parse_split_part(&archive) {
            // Split archive: strip .partNNN.zgx → base name (e.g. "src" not "src.part001").
            base
        } else {
            archive.with_extension("")
        };
        // WinRAR-style overwrite check: scan the archive for entries that would
        // overwrite existing files in the destination. If ANY conflict, prompt.
        // Works for normal extract AND extract-here (both check archive contents).
        // SKIP on elevated rerun — the non-elevated pass already wrote regular files
        // (so they 'exist' now) and the user already approved. The rerun only recreates
        // symlinks; prompting again would confuse the user and risk 'No' → silent loss.
        if !elevated_rerun && extract::has_conflicts(&archive, &dest) {
            let dest_name = if extract_here {
                dest.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
            } else {
                dest.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
            };
            if !confirm_extract_overwrite(&dest_name) {
                return; // user clicked No — abort
            }
        }
        run_one(op_label, Some(op_detail), false, dest, OpKind::Extract(archive));
        return;
    }

    // --- COMPRESS path ---
    if args.len() < 2 {
        // F-U5 fix: show help instead of silently exiting. User double-clicked the exe
        // or found it with no args — give them usage guidance, not a silent close.
        show_help();
        return;
    }
    let own = PathBuf::from(&args[1]);
    // Finding #5: Path-length guard for compress path too.
    if own.as_os_str().len() > 247 {
        show_error(&format!("Path too long ({} characters).\nMove the item to a shorter path and try again.\n\n{}", own.as_os_str().len(), own.display()));
        return;
    }
    if !own.is_dir() && !own.is_file() {
        show_error(&format!("Not found:\n{}", own.display()));
        return;
    }

    // Multi-select coordination: returns this instance's path + any siblings Explorer
    // spawned. Empty = we were a forwarder and already handed our path off — exit.
    let paths = multiselect::collect_paths(own);
    if paths.is_empty() {
        return; // forwarder, done
    }

    // Single vs. multi.
    if paths.len() == 1 {
        let f = paths.into_iter().next().unwrap();
        let is_dir = f.is_dir();
        let raw_name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        // For folders, strip the last .ext from the name (x.md → x). Guard against
        // dot-prefixed folders (.git, .vscode) and names that would become empty.
        let name = if is_dir {
            let stem = PathBuf::from(&raw_name);
            match stem.file_stem().and_then(|s| s.to_str()) {
                Some(s) if !s.is_empty() && !raw_name.starts_with('.') => s.to_string(),
                _ => raw_name.clone(),
            }
        } else {
            raw_name.clone()
        };
        let op_label = "Compressing".to_string();
        let op_detail = raw_name;
        let dest = match f.parent() {
            Some(p) => p.join(format!("{}.zgx", name)),
            None => PathBuf::from(format!("{}.zgx", name)),
        };
        if !confirm_overwrite(&dest) { return; }
        run_one(op_label, Some(op_detail), true, dest, OpKind::CompressOne(f));
    } else {
        // Multi: archive is named after the shared parent folder, placed in that parent.
        let parent = paths
            .iter()
            .filter_map(|p| p.parent().map(|x| x.to_path_buf()))
            .next()
            .unwrap_or_else(|| PathBuf::from("."));
        let label = parent
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Archive".to_string());
        let op_label = "Compressing".to_string();
        let op_detail = format!("{} ({} items)", label, paths.len());
        let dest = parent.join(format!("{}.zgx", label));
        if !confirm_overwrite(&dest) { return; }
        run_one(op_label, Some(op_detail), true, dest, OpKind::CompressMany(paths));
    }
}

enum OpKind {
    Extract(PathBuf),
    CompressOne(PathBuf),
    CompressMany(Vec<PathBuf>),
    CompressSplit(PathBuf, u32), // (folder, segment_size_mb)
    CompressSplitMany(Vec<PathBuf>, u32), // (inputs, segment_size_mb)
}

fn run_one(op_label: String, op_detail: Option<String>, cancellable: bool, dest: PathBuf, op: OpKind) {
    let is_extract = matches!(op, OpKind::Extract(_));

    let app = match ProgressWindow::new() {
        Ok(a) => a,
        Err(e) => {
            show_error(&format!("UI error: {}", e));
            return;
        }
    };
    app.set_op_label(op_label.into());
    app.set_op_detail(op_detail.unwrap_or_default().into());
    app.set_version_text(format!("v{}", env!("CARGO_PKG_VERSION")).into());

    app.set_cancellable(cancellable && !is_extract); // Cancel is available during compress only.

    let cancel = Arc::new(AtomicBool::new(false));

    // Run the operation in a background thread; the timer reads the heartbeat file.
    let cancel_for_thread = cancel.clone();
    let op_handle = std::thread::spawn(move || {
        // Catch panics so a thread panic surfaces as a graceful error instead of
        // silently killing the worker thread (which would leave the GUI hanging).
        let panic_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let result = match op {
            OpKind::Extract(a) => extract::extract_archive(&a, &dest),
            OpKind::CompressOne(f) => {
                let r = compress::compress_folder(&f, &dest, &[], &cancel_for_thread);
                (r.0, if r.1.is_empty() { String::new() } else { r.1.join(", ") })
            }
            OpKind::CompressMany(inputs) => {
                let label = dest.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                let r = compress::compress_paths(&inputs, &dest, &label, &cancel_for_thread);
                (r.0, if r.1.is_empty() { String::new() } else { r.1.join(", ") })
            }
            OpKind::CompressSplit(f, seg_mb) => {
                let r = compress::compress_folder_split(&f, &dest, seg_mb, &[], &cancel_for_thread);
                (r.0, if r.1.is_empty() { String::new() } else { r.1.join(", ") })
            }
            OpKind::CompressSplitMany(inputs, seg_mb) => {
                let label = dest.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                let r = compress::compress_paths_split(&inputs, &dest, &label, seg_mb, &cancel_for_thread);
                (r.0, if r.1.is_empty() { String::new() } else { r.1.join(", ") })
            }
        };
        result
        })); // end catch_unwind
        let result = match panic_result {
            Ok(r) => r,
            Err(panic) => {
                let msg = if let Some(s) = panic.downcast_ref::<&str>() { s.to_string() }
                    else if let Some(s) = panic.downcast_ref::<String>() { s.clone() }
                    else { "unknown panic".to_string() };
                (false, format!("panic: {}", msg))
            }
        };
        // If the operation failed without writing a terminal phase to the status file
        // (e.g. unrecognized format), write one now so the UI shows "Failed" instead of
        // hanging forever.
        if !result.0 {
            // Check if the status file already has a terminal phase (3 or 4).
            let already_terminal = progress::read_status()
                .map(|s| s.phase >= 3)
                .unwrap_or(false);
            if !already_terminal {
                progress::clear_status();
                let prog = progress::Progress::new("");
                prog.finish(4); // error
            }
        }
    });

    let timer = Timer::default();
    let weak = app.as_weak();
    let auto_close = Arc::new(AtomicBool::new(false));
    let auto_close_clone = auto_close.clone();
    let close_timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(200), move || {
        let app = match weak.upgrade() {
            Some(a) => a,
            None => return,
        };
        if let Some(s) = progress::read_status() {
            if s.bytes_total > 0 {
                app.set_indeterminate(false);
                let pct = ((s.bytes_done as f64 / s.bytes_total as f64) * 100.0).min(100.0) as i32;
                app.set_progress_fraction((pct as f32) / 100.0);
                app.set_detail(
                    format!(
                        "{}%    {:.0} / {:.0} MB    {:.0} MB/s    {}s left",
                        pct,
                        s.bytes_done as f64 / 1_048_576.0,
                        s.bytes_total as f64 / 1_048_576.0,
                        s.rate / 1_048_576.0,
                        s.eta
                    )
                    .into(),
                );
            } else {
                app.set_indeterminate(true);
                app.set_progress_fraction(0.0);
                app.set_detail("Extracting... (size unknown)".into());
            }
            if !app.get_done() {
                if s.phase == 3 {
                    app.set_done(true);
                    if s.skipped > 0 {
                        app.set_result(format!("Done - {} skipped", s.skipped).into());
                        app.set_result_color(slint::Color::from_rgb_u8(0xcb, 0x80, 0x3c));
                    } else {
                        app.set_result("Done".into());
                        app.set_result_color(slint::Color::from_rgb_u8(0x4c, 0xaf, 0x50));
                    }
                    // Start auto-close countdown (2s). The update check runs AFTER
                    // app.run() returns, so closing the window doesn't skip it.
                    auto_close.store(true, Ordering::Relaxed);
                    close_timer.start(TimerMode::SingleShot, Duration::from_secs(2), move || {
                        let _ = slint::quit_event_loop();
                    });
                } else if s.phase == 4 {
                    app.set_indeterminate(false);
                    app.set_done(true);
                    // Show the actual error message, not just 'Failed'.
                    let result_text = if s.error_msg.is_empty() {
                        "Failed".to_string()
                    } else {
                        s.error_msg
                    };
                    app.set_result(result_text.into());
                    app.set_result_color(slint::Color::from_rgb_u8(0xf4, 0x43, 0x36));
                    // No auto-close on failure — user needs to read the error.
                } else if s.phase == 5 {
                    app.set_done(true);
                    app.set_result("Cancelled".into());
                    app.set_result_color(slint::Color::from_rgb_u8(0xcb, 0x80, 0x3c));
                    // Cancel: kill the process immediately after showing the message.
                    // No waiting for the operation thread — it may be stuck in a blocking call.
                    close_timer.start(TimerMode::SingleShot, Duration::from_millis(200), move || {
                        std::process::exit(0);
                    });
                }
            }
        }
    });

    {
        let c = cancel.clone();
        let app_weak = app.as_weak();
        app.on_cancel_clicked(move || {
            c.store(true, Ordering::Relaxed);
            if let Some(a) = app_weak.upgrade() {
                a.set_cancelling(true);
            }
        });
    }

    app.on_close_clicked(|| {
        let _ = slint::quit_event_loop();
    });

    // Cheap version check at +2s: one tiny GET, no dialog/download. Stash the result;
    // the heavy apply (prompt + download + verify + swap) runs AFTER the operation has
    // finished and the window is gone — so it can never interrupt or freeze a job.
    let pending_update = Arc::new(std::sync::Mutex::new(None::<update::UpdateInfo>));
    let pending_clone = Arc::clone(&pending_update);
    let update_timer = slint::Timer::default();
    update_timer.start(slint::TimerMode::SingleShot, std::time::Duration::from_secs(2), move || {
        if let Some(info) = update::check_version_only() {
            *pending_clone.lock().unwrap() = Some(info);
        }
    });

    let _ = app.run();
    drop(timer);
    drop(update_timer);
    let _ = auto_close_clone.load(Ordering::Relaxed);
    let _ = op_handle.join();

    // Check final status — exit non-zero on failure/cancel so CLI callers can tell.
    let final_phase = progress::read_status()
        .map(|s| s.phase)
        .unwrap_or(0);
    let _ = std::fs::remove_file(progress::status_path());

    if final_phase == 4 || final_phase == 5 {
        // Failed or cancelled — skip update prompt, exit non-zero.
        std::process::exit(1);
    }

    // Now the operation is finished and the window is closed — safe to run the heavy
    // update flow. apply_update exits the process on success, returns normally on No/failure.
    let info = pending_update.lock().unwrap().take();
    if let Some(info) = info {
        update::apply_update(info);
    }
}
