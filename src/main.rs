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
mod progress;
mod update;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use slint::{Timer, TimerMode};

slint::slint! {
    import { VerticalBox, HorizontalBox, Button } from "std-widgets.slint";

    export component ProgressWindow inherits Window {
        title: "LRGEX Compress";
        background: #1e1e1e;
        preferred-width: 440px;
        preferred-height: 175px;

        in property <string> op-label: "Working";
        in property <float> progress-fraction: 0;     // 0.0 .. 1.0
        in property <string> detail: "Starting...";
        in property <bool> done: false;
        in property <bool> cancelling: false;
        in property <bool> cancellable: false;
        in property <string> result: "";
        in property <color> result-color: #4caf50;
        callback close-clicked();
        callback cancel-clicked();

        VerticalBox {
            Text {
                text: root.op-label;
                color: #f0f0f0;
                font-size: 15px;
                font-weight: 700;
            }
            Rectangle {
                height: 24px;
                background: #2d2d2d;
                border-radius: 6px;
                clip: true;
                Rectangle {
                    x: 0;
                    y: 0;
                    height: 100%;
                    width: parent.width * root.progress-fraction;
                    background: #cb803c;
                    animate width { duration: 120ms; }
                }
            }
            Text {
                text: root.detail;
                color: #cb803c;
                font-size: 13px;
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
            if root.done : HorizontalBox {
                Text {
                    text: root.result;
                    color: root.result-color;
                    font-size: 14px;
                    vertical-alignment: center;
                }
                Button {
                    text: "Close";
                    clicked => { root.close-clicked(); }
                }
            }
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let is_extract = args.len() >= 3 && args[1] == "-x";

    let (op_label, archive, folder): (String, Option<PathBuf>, Option<PathBuf>) = if is_extract {
        let a = PathBuf::from(&args[2]);
        let name = a.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        (format!("Extracting {}", name), Some(a), None)
    } else if args.len() >= 2 {
        let f = PathBuf::from(&args[1]);
        let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        (format!("Compressing {}", name), None, Some(f))
    } else {
        show_error("No folder or archive was given.");
        return;
    };

    // Validate input before opening the progress window.
    if let Some(f) = &folder {
        if !f.is_dir() && !f.is_file() {
            show_error(&format!("Not found:\n{}", f.display()));
            return;
        }
    }
    if let Some(a) = &archive {
        if !a.is_file() {
            show_error(&format!("Archive not found:\n{}", a.display()));
            return;
        }
    }

    // Destination path.
    let dest: PathBuf = if is_extract {
        archive.as_ref().unwrap().with_extension("")
    } else {
        let f = folder.as_ref().unwrap();
        let name = f
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        match f.parent() {
            Some(p) => p.join(format!("{}.zgx", name)),
            None => PathBuf::from(format!("{}.zgx", name)),
        }
    };

    let app = match ProgressWindow::new() {
        Ok(a) => a,
        Err(e) => {
            show_error(&format!("UI error: {}", e));
            return;
        }
    };
    app.set_op_label(op_label.into());
    app.set_cancellable(!is_extract); // Cancel is available during compress only.

    let cancel = Arc::new(AtomicBool::new(false));

    // Run the operation in a background thread; the timer reads the heartbeat file.
    let cancel_for_thread = cancel.clone();
    let op_handle = std::thread::spawn(move || {
        if is_extract {
            let _ = extract::extract_archive(&archive.unwrap(), &dest);
        } else {
            let _ = compress::compress_folder(&folder.unwrap(), &dest, &[], &cancel_for_thread);
        }
    });

    let timer = Timer::default();
    let weak = app.as_weak();
    timer.start(TimerMode::Repeated, Duration::from_millis(200), move || {
        let app = match weak.upgrade() {
            Some(a) => a,
            None => return,
        };
        if let Some(s) = progress::read_status() {
            let pct = if s.bytes_total > 0 {
                ((s.bytes_done as f64 / s.bytes_total as f64) * 100.0).min(100.0) as i32
            } else {
                0
            };
            app.set_progress_fraction((pct as f32) / 100.0);
            if s.bytes_total > 0 {
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
                app.set_detail(
                    format!("{:.0} MB processed", s.bytes_done as f64 / 1_048_576.0).into(),
                );
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
                } else if s.phase == 4 {
                    app.set_done(true);
                    app.set_result("Failed".into());
                    app.set_result_color(slint::Color::from_rgb_u8(0xf4, 0x43, 0x36));
                } else if s.phase == 5 {
                    app.set_done(true);
                    app.set_result("Cancelled".into());
                    app.set_result_color(slint::Color::from_rgb_u8(0xcb, 0x80, 0x3c));
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
    let _ = op_handle.join();
    let _ = std::fs::remove_file(progress::status_path());

    // Now the operation is finished and the window is closed — safe to run the heavy
    // update flow. apply_update exits the process on success, returns normally on No/failure.
    let info = pending_update.lock().unwrap().take();
    if let Some(info) = info {
        update::apply_update(info);
    }
}
