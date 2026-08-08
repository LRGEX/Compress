# LRGEX Patch — vendor/libz-ng-sys (v1.1.29)

## Upstream
- **Crate:** `libz-ng-sys` 1.1.29
- **Source:** https://github.com/rust-lang/libz-sys (the libz-ng-sys subdirectory)
- **Registry:** crates.io

## What was changed
Added four CMake defines to the `build_zlib_ng()` function in `zng/cmake.rs`:

```rust
.define("CMAKE_POLICY_DEFAULT_CMP0091", "NEW")
.define("CMAKE_MSVC_RUNTIME_LIBRARY", "MultiThreaded")
.define("CMAKE_BUILD_TYPE", "Release")
.define("CMAKE_C_FLAGS_RELEASE", "/MT /O2 /Ob2 /DNDEBUG")
```

## Why
The upstream crate builds zlib-ng via CMake, which defaults to the **dynamic** MSVC runtime (`/MD`). When the consuming app uses `+crt-static` (static CRT), this mismatch causes link failures:

- `LNK4098`: MSVCRT conflicts with static libs
- `LNK4217`: `malloc`, `free`, `abort` imported from dynamic UCRT by zlib-ng objects
- `LNK2019`: `__imp__wassert` unresolved — the dynamic UCRT import thunk for `_wassert`, referenced by `slide_hash_sse2.c`, `crc32_pclmulqdq.c`, and `crc32_vpclmulqdq.c` (which contain `assert()` calls)

### Why each define is necessary:
1. **`CMAKE_POLICY_DEFAULT_CMP0091=NEW`** — Without policy NEW, `CMAKE_MSVC_RUNTIME_LIBRARY` is silently ignored. This was the root cause of the first two failed attempts (env var + CFLAGS). CMake requires the policy to be explicitly set to NEW at the project level or via `CMAKE_POLICY_DEFAULT_CMP0091`.
2. **`CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded`** — Tells CMake to use `/MT` (static CRT) instead of `/MD` (dynamic CRT). Only honored after CMP0091=NEW.
3. **`CMAKE_BUILD_TYPE=Release`** — Ensures CMake uses `CMAKE_C_FLAGS_RELEASE` (which contains the `/DNDEBUG` that removes `assert()` calls).
4. **`CMAKE_C_FLAGS_RELEASE="/MT /O2 /Ob2 /DNDEBUG"`** — Belt-and-suspenders: guarantees `/MT` reaches every C file including the SIMD sources, and `NDEBUG` removes all `assert()` calls so `_wassert` is never referenced even if the policy or runtime library variable fails to propagate to a specific target.

## What breaks if the patch is lost
- `cargo build --release` with `+crt-static` fails with `LNK2019: unresolved external symbol __imp__wassert`
- The app requires the VC++ Redistributable (`MSVCP140.dll`, `VCRUNTIME140.dll`, `VCRUNTIME140_1.dll`) to run on clean Windows machines
- Users see "MSVCP140.dll was not found" on first launch after install

## How to regenerate
```bash
# Get pristine upstream
cargo download libz-ng-sys==1.1.29 -x pristine-libz-ng-sys

# Diff against vendored copy
diff -ru pristine-libz-ng-sys/zng/cmake.rs vendor/libz-ng-sys/zng/cmake.rs > libz-ng-sys-lrgex-patch.diff
```

The only file modified is `zng/cmake.rs`. All other files in the crate are unchanged.
