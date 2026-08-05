// Multi-select coordinator (single-instance collector).
//
// Problem: Windows Explorer, when multiple items are selected and a cascade context-menu
// verb is chosen, launches the exe N times — once per item — each with a single "%1".
// To produce ONE archive containing every selected item (WinRAR-style), the instances
// must coordinate.
//
// Design (single-instance coordinator via named mutex + temp-file roster):
//   1. Every launch tries to acquire a named mutex.
//   2. The FIRST to acquire it becomes the COORDINATOR. It polls a temp "roster" file,
//      debounces (waits for a quiet window after the last new path), then returns the
//      full collected set — owning path + all sibling paths.
//   3. Every later launch (FORWARDER) appends its path to the roster file, then exits.
//
// Why temp-file, not named pipes: zero extra dependencies, no pipe-flush races, survives
// coordinator startup delay (forwarder just writes a line and leaves), and the file is
// trivially unique per session via the PID. WinRAR-class collectors use this pattern.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0},
    System::Threading::{CreateMutexExW, ReleaseMutex, WaitForSingleObject},
};

const MUTEX_NAME: &str = "Global\\LRGEX-Compress-MultiSelect";
const QUIET_WINDOW: Duration = Duration::from_millis(400);
const HARD_CAP: Duration = Duration::from_secs(3);

fn roster_path() -> PathBuf {
    // PID-unique so concurrent sessions (rare) never collide.
    std::env::temp_dir().join(format!("lrgex-compress-session-{}.txt", std::process::id()))
}

fn try_acquire_mutex() -> Option<HANDLE> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = std::ffi::OsStr::new(MUTEX_NAME)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        // CreateMutexExW is NOT feature-gated (unlike CreateMutexW which needs
        // Win32_Security). dwDesiredAccess = 0x1F0001 = MUTEX_ALL_ACCESS.
        let h = CreateMutexExW(std::ptr::null(), wide.as_ptr(), 0, 0x1F0001);
        if h.is_null() {
            return None;
        }
        // 0 = we own it now; anything else (WAIT_TIMEOUT 0x102, WAIT_FAILED) = taken.
        if WaitForSingleObject(h, 0) == WAIT_OBJECT_0 {
            Some(h)
        } else {
            CloseHandle(h);
            None
        }
    }
}

/// Forwarder: append our path to the coordinator's roster file. Best-effort — if the
/// coordinator never started (race), the path simply isn't counted; never corrupts.
fn forward_path(path: &std::path::Path, coord_pid: u32) {
    let roster = std::env::temp_dir().join(format!("lrgex-compress-session-{}.txt", coord_pid));
    let line = format!("{}\n", path.display());
    // Retry briefly: the coordinator may still be opening the file for the first time.
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        if let Ok(mut f) = OpenOptions::new().append(true).create(true).open(&roster) {
            let _ = f.write_all(line.as_bytes());
            let _ = f.flush();
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Coordinator: poll the roster file until a QUIET_WINDOW passes with no new lines
/// (or HARD_CAP total elapses). Parse all unique paths.
fn collect_as_coordinator(own: PathBuf, pid: u32) -> Vec<PathBuf> {
    let roster = std::env::temp_dir().join(format!("lrgex-compress-session-{}.txt", pid));
    // Wipe any stale roster from a previous run with this PID (extremely unlikely but
    // guarantees a clean start).
    let _ = std::fs::write(&roster, format!("{}\n", own.display()));

    // FAST PATH (single-item): poll briefly for any sibling arrival. Explorer fires all
    // multi-select launches within tens of ms, so if nothing shows up in EARLY_POLL we
    // are almost certainly a single-item invocation — bail out with just our own path.
    // Avoids the full QUIET_WINDOW debounce penalty on the common case.
    const EARLY_POLL: Duration = Duration::from_millis(60);
    let early_deadline = Instant::now() + EARLY_POLL;
    while Instant::now() < early_deadline {
        let lines = std::fs::read_to_string(&roster).unwrap_or_default();
        if lines.lines().filter(|l| !l.trim().is_empty()).count() > 1 {
            break; // a sibling arrived → fall through to full debounce
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let early_lines = std::fs::read_to_string(&roster).unwrap_or_default();
    if early_lines.lines().filter(|l| !l.trim().is_empty()).count() <= 1 {
        // Single-item: no siblings came in the early window. Skip the debounce.
        let _ = std::fs::remove_file(&roster);
        return vec![own];
    }

    // FULL DEBOUNCE (multi-item confirmed): keep collecting until QUIET_WINDOW passes
    // with no new arrivals, or HARD_CAP total elapses.
    let start = Instant::now();
    let mut last_change = Instant::now();
    let mut last_line_count: usize = early_lines.lines().filter(|l| !l.trim().is_empty()).count();

    while Instant::now() - start < HARD_CAP {
        if Instant::now() - last_change >= QUIET_WINDOW {
            break;
        }
        let lines = std::fs::read_to_string(&roster).unwrap_or_default();
        let lc = lines.lines().filter(|l| !l.trim().is_empty()).count();
        if lc != last_line_count {
            last_line_count = lc;
            last_change = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(40));
    }

    // Parse + dedupe, preserving arrival order.
    let raw = std::fs::read_to_string(&roster).unwrap_or_default();
    let mut seen: Vec<PathBuf> = Vec::new();
    seen.push(own); // own is always first
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let p = PathBuf::from(line);
        if !seen.contains(&p) {
            seen.push(p);
        }
    }
    let _ = std::fs::remove_file(&roster);
    seen
}

/// Entry point. Call this FIRST in main(), passing this instance's `%1` path.
/// Returns the full set of paths to archive (own + any siblings).
///
/// IMPORTANT: this function looks at the FIRST running coordinator's PID to find the
/// roster. Since the coordinator writes the roster under its OWN pid, and forwarders
/// must find it, we stash the coordinator's PID in a tiny lockfile that forwarders read.
pub fn collect_paths(own: PathBuf) -> Vec<PathBuf> {
    let pid_lock = std::env::temp_dir().join("lrgex-compress-coordinator.pid");
    if let Some(handle) = try_acquire_mutex() {
        // We are the coordinator. Record our PID for any forwarders, collect, release.
        let pid = std::process::id();
        let _ = std::fs::write(&pid_lock, pid.to_string());
        let paths = collect_as_coordinator(own, pid);
        let _ = std::fs::remove_file(&pid_lock);
        unsafe {
            let _ = ReleaseMutex(handle);
            CloseHandle(handle);
        }
        paths
    } else {
        // We are a forwarder. Read the coordinator's PID, append our path, exit.
        let coord_pid = std::fs::read_to_string(&pid_lock)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(0);
        if coord_pid != 0 {
            forward_path(&own, coord_pid);
        }
        // Empty signals main() to exit silently — the coordinator owns the work.
        Vec::new()
    }
}
