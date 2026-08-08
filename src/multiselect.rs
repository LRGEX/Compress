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
//      full collected set — owning path + any sibling paths.
//   3. Every later launch (FORWARDER) appends its path to the roster file, then exits.
//
// Why temp-file, not named pipes: zero extra dependencies, no pipe-flush races, survives
// coordinator startup delay (forwarder just writes a line and leaves), and the file is
// trivially unique per session via the PID. WinRAR-class collectors use this pattern.
//
// ROBUSTNESS (v1.4.1): two real-world failure modes are now handled so a second launch
// can NEVER silently no-op (which previously produced "no archive, no error"):
//   1. WAIT_ABANDONED — if a previous coordinator crashed holding the mutex, the kernel
//      hands us ownership WITH WAIT_ABANDONED. We treat that as "we own it" (it is).
//   2. Stale coordinator.pid lockfile — if a forwarder reads a PID whose process is no
//      longer alive (previous coordinator finished/crashed), it clears the stale lockfile
//      and becomes the coordinator itself, instead of forwarding into the void and exiting
//      with no archive produced.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, STILL_ACTIVE, WAIT_ABANDONED, WAIT_OBJECT_0},
    System::Threading::{
        CreateMutexExW, GetExitCodeProcess, OpenProcess, ReleaseMutex, WaitForSingleObject,
        PROCESS_QUERY_LIMITED_INFORMATION,
    },
};

const MUTEX_NAME: &str = "Global\\LRGEX-Compress-MultiSelect";
const QUIET_WINDOW: Duration = Duration::from_millis(400);
const HARD_CAP: Duration = Duration::from_secs(3);
// When selecting many files (e.g. select-all in a folder with 50+ files), Explorer
// can take 200-500ms to launch all instances. The old 60ms early-poll was too short
// — the coordinator saw only 1-2 files and bailed early, compressing just the first
// file instead of the full selection. 300ms gives Explorer time to fire all launches.
const EARLY_POLL: Duration = Duration::from_millis(300);

fn roster_path() -> PathBuf {
    // PID-unique so concurrent sessions (rare) never collide.
    std::env::temp_dir().join(format!("lrgex-compress-session-{}.txt", std::process::id()))
}

/// Is the given PID a currently-running process AND did it start when the lockfile
/// says it did? Checking PID-alive alone is NOT enough — Windows reuses PIDs, so a
/// dead coordinator's PID can be reassigned to an unrelated process, making a stale
/// lockfile look live. By also checking the process creation time (passed in from the
/// lockfile as `pid:ctime`), we defeat PID reuse: the unrelated process has a
/// different creation time.
fn is_process_alive_with_ctime(pid: u32, expected_ctime: u64) -> bool {
    use windows_sys::Win32::System::Threading::GetProcessTimes;
    use windows_sys::Win32::Foundation::FILETIME;
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            // Can't open (doesn't exist OR access-denied cross-integrity). We can't
            // tell the difference safely. Treat as DEAD — a live coordinator we can't
            // query would hold the MUTEX, and try_acquire_mutex below would fail,
            // preventing the dual-coordinator race. So clearing the lockfile here is
            // safe: if the coordinator is alive, the mutex acquire fails and we forward.
            return false;
        }
        let mut create: FILETIME = std::mem::zeroed();
        let mut exit: FILETIME = std::mem::zeroed();
        let mut kernel: FILETIME = std::mem::zeroed();
        let mut user: FILETIME = std::mem::zeroed();
        let ok = GetProcessTimes(h, &mut create, &mut exit, &mut kernel, &mut user);
        let _ = CloseHandle(h);
        if ok == 0 {
            // GetProcessTimes failed (access-denied on a live process at a different
            // integrity level). CONSERVATIVE: treat as ALIVE so we DON'T clear the
            // lockfile and race a real coordinator. The cost is one lost forwarder
            // invocation — far better than two coordinators corrupting each other.
            return true;
        }
        let actual_ctime = ((create.dwHighDateTime as u64) << 32) | (create.dwLowDateTime as u64);
        actual_ctime == expected_ctime
    }
}

/// Read this process's own creation time — used to write into the coordinator lockfile
/// so forwarders can defeat PID reuse via is_process_alive_with_ctime.
fn own_creation_time() -> u64 {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    use windows_sys::Win32::Foundation::FILETIME;
    unsafe {
        let mut create: FILETIME = std::mem::zeroed();
        let mut exit: FILETIME = std::mem::zeroed();
        let mut kernel: FILETIME = std::mem::zeroed();
        let mut user: FILETIME = std::mem::zeroed();
        if GetProcessTimes(GetCurrentProcess(), &mut create, &mut exit, &mut kernel, &mut user) == 0 {
            return 0;
        }
        ((create.dwHighDateTime as u64) << 32) | (create.dwLowDateTime as u64)
    }
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
        let r = WaitForSingleObject(h, 0);
        // WAIT_OBJECT_0 (0)  = we own it cleanly.
        // WAIT_ABANDONED (128) = we own it AFTER a previous holder crashed without
        //   releasing. Either way WE are the legitimate owner now — do not treat
        //   WAIT_ABANDONED as "taken" (that was the v1.4 silent-no-op bug).
        // WAIT_TIMEOUT (258) / WAIT_FAILED (0xffffffff) = genuinely held by a live owner.
        if r == WAIT_OBJECT_0 || r == WAIT_ABANDONED {
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
        // Sanity check: reject lines that don't parse as real filesystem paths (e.g.
        // injected garbage from a same-user process writing to the roster file).
        // A valid path must be absolute or contain a normal component.
        if line.contains('\0') {
            continue; // embedded NUL — can't be a real path
        }
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
/// Coordinator PID is stashed in a tiny lockfile that forwarders read. If a forwarder
/// finds a lockfile whose PID is no longer alive (previous coordinator finished or
/// crashed), it clears the stale lockfile and becomes the coordinator itself — so a
/// stale lockfile can NEVER cause a silent no-op (the v1.4 bug where a second rapid
/// launch produced no archive and no error).
pub fn collect_paths(own: PathBuf) -> Vec<PathBuf> {
    let pid_lock = std::env::temp_dir().join("lrgex-compress-coordinator.pid");

    // First try: maybe we're the coordinator.
    let first_acquire = try_acquire_mutex();
    if let Some(handle) = first_acquire {
        let pid = std::process::id();
        let ctime = own_creation_time();
        // Clear any stale lockfile before claiming the role (another instance may have
        // died without cleaning up). We just acquired the mutex, so we are authoritative.
        // Write `pid:ctime` so forwarders can defeat PID reuse via creation-time check.
        let _ = std::fs::write(&pid_lock, format!("{}:{}", pid, ctime));
        let paths = collect_as_coordinator(own, pid);
        let _ = std::fs::remove_file(&pid_lock);
        unsafe {
            let _ = ReleaseMutex(handle);
            CloseHandle(handle);
        }
        return paths;
    }

    // We're a forwarder (mutex held by another live coordinator). Read its pid:ctime.
    // RETRY briefly: the coordinator may have just acquired the mutex and not yet
    // finished writing/flushing the lockfile. Reading in that window yields empty /
    // partial → coord_pid=0 → silent drop of this forwarder's file. A short retry loop
    // covers the write-visibility gap.
    let mut coord_pid: u32 = 0;
    let mut coord_ctime: u64 = 0;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
    while std::time::Instant::now() < deadline {
        if let Some((p, c)) = std::fs::read_to_string(&pid_lock)
            .ok()
            .and_then(|s| {
                let mut parts = s.trim().splitn(2, ':');
                let pid = parts.next().and_then(|p| p.parse::<u32>().ok())?;
                if pid == 0 { return None; }
                let ctime = parts.next().and_then(|c| c.parse::<u64>().ok()).unwrap_or(0);
                Some((pid, ctime))
            })
        {
            coord_pid = p;
            coord_ctime = c;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    // LIVENESS CHECK: if the coordinator PID is dead (or its creation time doesn't
    // match — defeats PID reuse), the lockfile is STALE. Forwarding into it would
    // silently lose this invocation. Instead: clear the stale lockfile, retry as coordinator.
    let alive = coord_pid != 0 && is_process_alive_with_ctime(coord_pid, coord_ctime);
    if coord_pid == 0 || !alive {
        // Clear the stale state and try to become the coordinator.
        let _ = std::fs::remove_file(&pid_lock);
        let second_acquire = try_acquire_mutex();
        if let Some(handle) = second_acquire {
            let pid = std::process::id();
            let ctime = own_creation_time();
            let _ = std::fs::write(&pid_lock, format!("{}:{}", pid, ctime));
            let paths = collect_as_coordinator(own, pid);
            let _ = std::fs::remove_file(&pid_lock);
            unsafe {
                let _ = ReleaseMutex(handle);
                CloseHandle(handle);
            }
            return paths;
        }
        // Extremely unlikely: mutex still held but no live coordinator owns the lockfile.
        // Fall through and forward anyway (best-effort) so we don't block the user.
    }

    // Genuine forwarder: a live coordinator is running. Hand our path off and exit empty.
    if coord_pid != 0 {
        forward_path(&own, coord_pid);
    }
    Vec::new()
}
