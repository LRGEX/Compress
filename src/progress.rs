// Live progress (heartbeat) — adapted verbatim from LRGEX Restore synclog.rs.
// Written every 500ms regardless of file count, so the UI/CLI never looks frozen.
//
// HARD RULE (learned the hard way on Restore): NEVER add `impl Drop for Progress`.
// It is Clone over Arc — Drop would fire on every clone (ByteReader) and kill the
// heartbeat after the first file. Shutdown is explicit via finish() only.

use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Transient status file. PID-unique so two concurrent runs never collide.
pub fn status_path() -> PathBuf {
    std::env::temp_dir().join(format!("lrgex-compress-status-{}.json", std::process::id()))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Clone)]
pub struct Progress {
    inner: Arc<ProgressInner>,
}

struct ProgressInner {
    phase: AtomicUsize, // 0=walk 1=compress/extract 2=flush 3=done 4=error
    files_done: AtomicUsize,
    files_total: AtomicUsize,
    bytes_done: AtomicU64,
    bytes_total: AtomicU64,
    skipped: AtomicUsize,      // files that could not be read (warn, don't fail)
    started: Instant,
    stop: AtomicBool,
    label: Mutex<String>,
    path: PathBuf,
}

impl Progress {
    pub fn new(label: &str) -> Self {
        Progress {
            inner: Arc::new(ProgressInner {
                phase: AtomicUsize::new(0),
                files_done: AtomicUsize::new(0),
                files_total: AtomicUsize::new(0),
                bytes_done: AtomicU64::new(0),
                bytes_total: AtomicU64::new(0),
                skipped: AtomicUsize::new(0),
                started: Instant::now(),
                stop: AtomicBool::new(false),
                label: Mutex::new(label.to_string()),
                path: status_path(),
            }),
        }
    }

    pub fn set_phase(&self, p: usize) {
        self.inner.phase.store(p, Ordering::Relaxed);
    }

    pub fn set_totals(&self, files: usize, bytes: u64) {
        self.inner.files_total.store(files, Ordering::Relaxed);
        self.inner.bytes_total.store(bytes, Ordering::Relaxed);
    }

    /// Number of files that could not be read/archived (shown as a warning, not a failure).
    pub fn set_skipped(&self, n: usize) {
        self.inner.skipped.store(n, Ordering::Relaxed);
    }

    /// Tick bytes only (no file count) — for streaming progress within large files
    /// (compress) and compressed-byte counting (extract).
    pub fn tick_bytes(&self, bytes: u64) {
        self.inner.bytes_done.fetch_add(bytes, Ordering::Relaxed);
    }

    /// N-7/P0-1: accessor for post-extract truncation check.
    pub fn bytes_done(&self) -> u64 {
        self.inner.bytes_done.load(Ordering::Relaxed)
    }

    fn snapshot_json(&self, pid: u32) -> String {
        let done = self.inner.files_done.load(Ordering::Relaxed);
        let total = self.inner.files_total.load(Ordering::Relaxed);
        let bdone = self.inner.bytes_done.load(Ordering::Relaxed);
        let btotal = self.inner.bytes_total.load(Ordering::Relaxed);
        let elapsed = self.inner.started.elapsed().as_secs_f64().max(0.001);
        let rate = bdone as f64 / elapsed;
        let eta = if rate > 1.0 && btotal > bdone {
            ((btotal - bdone) as f64 / rate) as u64
        } else {
            0
        };
        let label = self.inner.label.lock().map(|g| g.clone()).unwrap_or_default();
        let skipped = self.inner.skipped.load(Ordering::Relaxed);
        format!(
            "{{\"heartbeat\":{},\"pid\":{},\"phase\":{},\"label\":{},\"files_done\":{},\"files_total\":{},\"bytes_done\":{},\"bytes_total\":{},\"skipped\":{},\"elapsed\":{:.1},\"rate\":{:.1},\"eta\":{}}}",
            now_secs(),
            pid,
            self.inner.phase.load(Ordering::Relaxed),
            serde_json::to_string(&label).unwrap_or_else(|_| "\"\"".into()),
            done,
            total,
            bdone,
            btotal,
            skipped,
            elapsed,
            rate,
            eta
        )
    }

    /// Spawns a writer that updates the status JSON every 500ms regardless of progress.
    pub fn spawn_writer(&self) -> std::thread::JoinHandle<()> {
        let me = self.clone();
        std::thread::spawn(move || {
            // Checks stop FIRST in the loop — never overwrites the terminal snapshot.
            while !me.inner.stop.load(Ordering::Relaxed) {
                me.write_snapshot();
                std::thread::sleep(Duration::from_millis(500));
            }
        })
    }

    fn write_snapshot(&self) {
        let json = self.snapshot_json(std::process::id());
        let tmp = self.inner.path.with_extension("tmp");
        if std::fs::write(&tmp, json.as_bytes()).is_ok() {
            let _ = std::fs::rename(&tmp, &self.inner.path);
        }
    }

    /// Writes a final snapshot BEFORE setting stop — the reader must see the
    /// terminal phase (3=done / 4=error) on disk before the process exits.
    pub fn finish(&self, phase: usize) {
        self.set_phase(phase);
        self.write_snapshot();
        self.inner.stop.store(true, Ordering::Relaxed);
    }
}

/// Reader wrapper that ticks bytes read into a Progress. Used to stream large
/// files during compress (>8 MB) and to count compressed bytes during extract.
pub struct ByteReader<R: Read> {
    inner: R,
    progress: Progress,
    cancel: Option<*const AtomicBool>,
}

impl<R: Read> ByteReader<R> {
    pub fn new(inner: R, progress: Progress) -> Self {
        Self { inner, progress, cancel: None }
    }

    pub fn with_cancel(inner: R, progress: Progress, cancel: &AtomicBool) -> Self {
        Self { inner, progress, cancel: Some(cancel as *const AtomicBool) }
    }
}

impl<R: Read> Read for ByteReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(cancel_ptr) = self.cancel {
            if unsafe { (*cancel_ptr).load(Ordering::Relaxed) } {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "LRGEX_CANCELLED",
                ));
            }
        }
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.progress.tick_bytes(n as u64);
        }
        Ok(n)
    }
}

/// Parsed status for the CLI reader.
#[derive(serde::Deserialize)]
pub struct Status {
    pub phase: usize,
    pub label: String,
    pub files_done: usize,
    pub files_total: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
    #[serde(default)]
    pub skipped: usize,
    pub elapsed: f64,
    pub rate: f64,
    pub eta: u64,
}

pub fn read_status() -> Option<Status> {
    let raw = std::fs::read_to_string(status_path()).ok()?;
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(&raw); // strip BOM if any
    serde_json::from_str::<Status>(raw).ok()
}

pub fn clear_status() {
    let _ = std::fs::write(status_path(), r#"{"phase":0}"#);
}
