# unrar 0.5.8 — LRGEX Compress patched fork

This is unrar 0.5.8 (from crates.io) with one patch for LRGEX Compress.

## The patch

Added a process-global `UCM_PROCESSDATA` byte counter to enable real sub-file
extraction progress for `.rar` archives.

unrar's C library fires `UCM_PROCESSDATA` callbacks during extraction (even when
writing to disk via `extract_with_base`), with `p2` carrying the byte count per
chunk (~4MB). The stock crate ignores this for disk extraction. This patch
captures the cumulative count in a static atomic so the host app can poll it.

## What changed

### `src/open_archive.rs`
- Added `pub static PROCESSED_BYTES: AtomicU64`
- Added `pub fn processed_bytes() -> u64`
- Added `pub fn reset_processed()`
- In the `UCM_PROCESSDATA` callback arm: `PROCESSED_BYTES.fetch_add(p2 as u64, Relaxed)`

### `src/lib.rs`
- Added `pub use open_archive::{processed_bytes, reset_processed};`

## Why

LRGEX Compress shows a progress bar during extraction. For `.zgx` and `.zip`,
the Rust-side `Read` stream is wrapped in a counting reader. For `.rar`, unrar's
C library owns the I/O — there's no Rust stream to wrap. Without this patch,
progress is impossible for single-file archives (the bar jumps 0→100% in one step).

The counter is process-global. Each LRGEX Compress extraction is its own process
(Explorer launches the exe per right-click), so concurrency is not an issue.

## Upstream

Original: https://crates.io/crates/unrar version 0.5.8
Base commit: unrar-0.5.8 from crates.io registry

To regenerate the diff against a pristine copy:
```bash
cp -r ~/.cargo/registry/src/*/unrar-0.5.8 /tmp/unrar-pristine
diff -ru /tmp/unrar-pristine/src/open_archive.rs vendor/unrar/src/open_archive.rs
diff -ru /tmp/unrar-pristine/src/lib.rs vendor/unrar/src/lib.rs
```
