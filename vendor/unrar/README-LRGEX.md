# LRGEX Patch — vendor/unrar (v0.5.8)

## Upstream
- **Crate:** `unrar` 0.5.8
- **Source:** https://github.com/muja/unrar.rs
- **Registry:** crates.io

## What was changed
Added a global `AtomicU64` byte counter (`PROCESSED_BYTES`) that is incremented inside the vendored UnRAR C library's `UCM_PROCESSDATA` callback. Exposed as two public Rust functions:
- `unrar::processed_bytes() -> u64` — read the current counter
- `unrar::reset_processed()` — zero the counter before each extraction

## Why
The upstream `unrar` crate provides no sub-file progress during `.rar` extraction. The callback receives data chunks via `UCM_PROCESSDATA` but doesn't expose a byte count to Rust. Without this patch, `.rar` extraction shows an indeterminate progress bar (no % or MB/s) because the extractor can't track decompressed bytes.

## What breaks if the patch is lost
- `.rar` extraction progress bar becomes indeterminate (no percentage, no speed, no ETA)
- The `unrar::processed_bytes()` / `reset_processed()` calls in `src/extract.rs` will fail to compile

## How to regenerate
```bash
# Get pristine upstream
cargo download unrar==0.5.8 -x pristine-unrar

# Diff against vendored copy
diff -ru pristine-unrar/src/ vendor/unrar/src/ > unrar-lrgex-patch.diff
```

The key files modified are:
- `src/open_archive.rs` — the `UCM_PROCESSDATA` callback incrementing the counter
- `src/lib.rs` — the `processed_bytes()` / `reset_processed()` public API + the `PROCESSED_BYTES` static
