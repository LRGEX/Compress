// Windows metadata round-trip — timestamps, attributes, symlinks.
//
// GOAL: "your data is safe and preserved as-is." Whatever goes into the archive comes
// out identical: LastWriteTime (mtime), CreationTime (ctime), Windows file attributes
// (Hidden, ReadOnly, System, Archive), and symbolic links / junctions.
//
// STORAGE in the .zgx tar stream (tar + zstd):
//   - mtime  → tar's native `mtime` field (seconds since epoch)
//   - ctime  → PAX local extension   `SCHILY.creationtime`  (seconds, ASCII decimal)
//   - attrs  → PAX local extension   `LRGEX.fileattr`       (raw u32 dwFileAttributes, ASCII decimal)
//   - links  → tar EntryType::Symlink, link name = target path (native tar)
//
// We use tar's built-in `Builder::append_pax_extensions()` which emits a proper
// EntryType::XHeader (PAX local) entry — the reader auto-associates it with the NEXT
// entry and surfaces the fields via `entry.pax_extensions()`. Standard POSIX PAX.
//
// SYMLINK ELEVATION: creating a symlink on Windows needs Admin OR Developer Mode.
// Our install is per-user (no UAC). So when extract hits a symlink it CANNOT create,
// we surface ONE prompt ("allow admin?"). If yes, the WHOLE extract re-launches
// elevated via ShellExecuteW("runas") and completes with full rights. The decision is
// latched — we never ask twice in one run.

use std::path::Path;

/// Atomic overwrite-rename that handles the Windows ReadOnly-destination trap.
/// Rust's `std::fs::rename` on Windows calls `MoveFileExW(MOVEFILE_REPLACE_EXISTING)`,
/// which FAILS with ERROR_ACCESS_DENIED when the destination has the ReadOnly
/// attribute set — even though we're replacing it. We try the rename; on access-denied,
/// clear ReadOnly on the EXISTING destination and retry once. Best-effort: any other
/// error (locked, perms, ENOSPC) propagates to the caller.
///
/// Returns Ok(()) on success, or the original rename error if the retry also fails.
pub fn atomic_replace(tmp: &Path, dst: &Path) -> std::io::Result<()> {
    match std::fs::rename(tmp, dst) {
        Ok(()) => Ok(()),
        Err(first_err) => {
            // Only retry on access-denied (the ReadOnly case). Other errors (ENOSPC,
            // ENOENT, EBUSY) won't be fixed by clearing attrs and should propagate.
            if first_err.raw_os_error() != Some(5) {
                return Err(first_err);
            }
            // Clear ReadOnly on the existing dst, then retry.
            clear_readonly_attr(dst);
            match std::fs::rename(tmp, dst) {
                Ok(()) => Ok(()),
                Err(second_err) => {
                    // Retry failed — report the original access-denied (more useful than
                    // whatever the second attempt produced).
                    Err(first_err)
                }
            }
        }
    }
}

/// Clear the FILE_ATTRIBUTE_READONLY bit on an existing path. Best-effort — ignores
/// failure (the caller's rename will surface the real error if attrs weren't the issue).
fn clear_readonly_attr(path: &Path) {
    let wide = match path_to_wide(path) {
        Some(w) => w,
        None => return,
    };
    unsafe {
        let cur = GetFileAttributesW(wide.as_ptr());
        if cur == 0xFFFFFFFF {
            return; // doesn't exist / unreadable
        }
        if cur & ATTR_READONLY != 0 {
            let _ = SetFileAttributesW(wide.as_ptr(), (cur & !ATTR_READONLY) as FILE_FLAGS_AND_ATTRIBUTES);
        }
    }
}
use std::time::{SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::{FILETIME, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, GetFileAttributesW, SetFileAttributesW, SetFileTime, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

/// Windows FILE_ATTRIBUTE_* flags we preserve across the round-trip.
/// Only the user-meaningful bits — NOT directory/reparse/device flags (those are
/// implicit in the entry type or wrong to copy onto a fresh file).
pub const ATTR_HIDDEN: u32 = 0x2;
pub const ATTR_READONLY: u32 = 0x1;
pub const ATTR_SYSTEM: u32 = 0x4;
pub const ATTR_ARCHIVE: u32 = 0x20;
pub const PRESERVED_ATTRS: u32 = ATTR_HIDDEN | ATTR_READONLY | ATTR_SYSTEM | ATTR_ARCHIVE;

#[derive(Clone, Copy, Default, Debug)]
pub struct MetaSnapshot {
    pub mtime: u64, // seconds since UNIX_EPOCH
    pub ctime: u64, // seconds since UNIX_EPOCH
    pub attrs: u32, // raw Windows dwFileAttributes
}

impl MetaSnapshot {
    /// The attributes we actually restore (masked to the user-meaningful subset).
    pub fn restorable_attrs(self) -> u32 {
        self.attrs & PRESERVED_ATTRS
    }
}

/// Read (mtime, ctime, attrs) for a path. Best-effort: any failure → zeros.
/// Uses symlink_metadata so we read the LINK's own metadata, not its target's.
/// Read (mtime, ctime, attrs) from an ALREADY-OBTAINED Metadata (no re-stat).
/// Use this when the caller already has the metadata from a cheaper source (e.g.
/// DirEntry::metadata() served from the read_dir enumeration cache on Windows — avoids
/// the per-file CreateFile that symlink_metadata would do). Best-effort: any failure → zeros.
pub fn read_meta_from(m: &std::fs::Metadata) -> MetaSnapshot {
    let mtime = dur_to_secs(m.modified().ok());
    let ctime = dur_to_secs(m.created().ok());
    use std::os::windows::fs::MetadataExt;
    let attrs = m.file_attributes();
    MetaSnapshot { mtime, ctime, attrs }
}

/// Read (mtime, ctime, attrs) for a path via symlink_metadata (re-opens the file).
/// Use this for symlinks (we need the LINK's own attrs, not the target's) and for paths
/// not coming from a read_dir enumeration. Best-effort: zeros on failure.
pub fn read_meta(path: &Path) -> MetaSnapshot {
    match std::fs::symlink_metadata(path) {
        Ok(m) => read_meta_from(&m),
        Err(_) => MetaSnapshot::default(),
    }
}

fn dur_to_secs(t: Option<SystemTime>) -> u64 {
    t.and_then(|st| st.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ──────────────────────────────────────────────────────────────────────────────
// EXTRACT-SIDE RESTORE
// ──────────────────────────────────────────────────────────────────────────────

/// Apply mtime + ctime to an open file handle we already own (right after writing the
/// file body). Best-effort: ignores failures.
pub fn apply_times_handle(handle: HANDLE, mtime: u64, ctime: u64) {
    unsafe {
        let m_ft = secs_to_filetime(mtime);
        let c_ft = secs_to_filetime(ctime); // declared in OUTER scope so the pointer stays valid
        // ctime==0 means "source had no creation time" → pass NULL to SetFileTime so the
        // file's existing creation time is left untouched. (Note: secs_to_filetime(0) is
        // NOT a zero FILETIME — it's 1601+epoch_diff = 1970-01-01 — so we must branch on
        // the INPUT being 0, not on the converted FILETIME. Branching on the FILETIME
        // was a bug that clobbered ctime to 1970 on every extract via apply_times_path.)
        let _ = SetFileTime(
            handle,
            if ctime == 0 { std::ptr::null() } else { &c_ft as *const FILETIME }, // CreationTime
            std::ptr::null(),                                                      // LastAccessTime (untouched)
            if mtime == 0 { std::ptr::null() } else { &m_ft as *const FILETIME },  // LastWriteTime
        );
    }
}

/// ONE-CALL metadata restore on an open handle: mtime + ctime + Windows attributes in a
/// single SetFileInformationByHandle(FileBasicInfo). This replaces the old pattern of
/// writing the file, closing it, then re-opening it just to set times + attrs. That
/// re-open was a second CreateFile per file (11,131 extra opens on a big archive) AND a
/// fresh Defender scan touchpoint on every just-created file — a real perf bug.
///
/// Pass `attrs` already masked to the restorable subset (restorable_attrs()). Zero
/// times → left unchanged (Windows uses 0 as "don't touch" for FileBasicInfo).
///
/// Safe wrapper — best-effort, ignores failures.
pub fn apply_all_handle(handle: HANDLE, mtime: u64, ctime: u64, attrs: u32) {
    use windows_sys::Win32::Storage::FileSystem::{
        FileBasicInfo, FILE_BASIC_INFO, SetFileInformationByHandle,
    };
    let info = FILE_BASIC_INFO {
        CreationTime: secs_to_100ns_i64(ctime),
        LastAccessTime: 0, // 0 = leave unchanged
        LastWriteTime: secs_to_100ns_i64(mtime),
        ChangeTime: 0,     // 0 = leave unchanged
        FileAttributes: attrs, // 0 = leave unchanged (Windows treats 0 as "no update" here)
    };
    unsafe {
        let _ = SetFileInformationByHandle(
            handle,
            FileBasicInfo,
            &info as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<FILE_BASIC_INFO>() as u32,
        );
    }
}

/// seconds-since-UNIX-epoch → FILETIME's signed 100ns-intervals-since-1601, as i64.
/// 0 means "leave unchanged" (passed straight to FILE_BASIC_INFO).
fn secs_to_100ns_i64(secs: u64) -> i64 {
    if secs == 0 {
        return 0;
    }
    const EPOCH_DIFF: u64 = 11_644_473_600;
    let intervals = secs.saturating_add(EPOCH_DIFF).saturating_mul(10_000_000);
    intervals as i64
}


fn secs_to_filetime(secs: u64) -> FILETIME {
    // FILETIME = 100ns intervals since 1601-01-01. UNIX epoch is 11644473600s after 1601.
    const EPOCH_DIFF: u64 = 11_644_473_600;
    let intervals = secs.saturating_add(EPOCH_DIFF).saturating_mul(10_000_000);
    FILETIME {
        dwLowDateTime: intervals as u32,
        dwHighDateTime: (intervals >> 32) as u32,
    }
}

/// Apply Windows attributes to a path. Masked to PRESERVED_ATTRS. Best-effort.
/// NOTE: this only SETS bits when nonzero; it does NOT clear the Archive bit when the
/// source didn't have it. For full fidelity (clear stray bits from a fresh temp file),
/// use `apply_attrs_normalized` instead.
pub fn apply_attrs(path: &Path, preserved: u32) {
    if preserved == 0 {
        return;
    }
    let wide = match path_to_wide(path) {
        Some(w) => w,
        None => return,
    };
    unsafe {
        let _ = SetFileAttributesW(wide.as_ptr(), preserved as FILE_FLAGS_AND_ATTRIBUTES);
    }
}

/// Normalize the PRESERVED attribute bits on a path to exactly match the source.
/// Read-modify-write: read current attrs, clear the preserved subset, OR-in the
/// source's preserved subset, write back. This clears stray bits (e.g. the Archive
/// bit NTFS sets on a freshly-created temp file) when the source didn't have them,
/// while preserving bits we don't own (DIRECTORY, REPARSE_POINT, etc.).
pub fn apply_attrs_normalized(path: &Path, source_preserved: u32) {
    let wide = match path_to_wide(path) {
        Some(w) => w,
        None => return,
    };
    unsafe {
        // GetFileAttributesW returns the attrs as u32, or INVALID_FILE_ATTRIBUTES
        // (0xFFFFFFFF) on failure. It takes ONLY the path pointer — no out-param.
        let current = GetFileAttributesW(wide.as_ptr());
        if current == 0xFFFFFFFF {
            // Couldn't read — fall back to a plain set of the source bits.
            let _ = SetFileAttributesW(wide.as_ptr(), (source_preserved & PRESERVED_ATTRS) as FILE_FLAGS_AND_ATTRIBUTES);
            return;
        }
        // Clear the preserved subset, OR-in the source's preserved subset.
        let desired = (current & !PRESERVED_ATTRS) | (source_preserved & PRESERVED_ATTRS);
        let _ = SetFileAttributesW(wide.as_ptr(), desired as FILE_FLAGS_AND_ATTRIBUTES);
    }
}

/// Set mtime+ctime on a path by opening it ourselves (used for DIRECTORIES, where we
/// don't own a handle from the write path). Uses FILE_FLAG_BACKUP_SEMANTICS which is
/// required to open a directory. Best-effort.
pub fn apply_times_path(path: &Path, mtime: u64, ctime: u64) {
    let wide = match path_to_wide(path) {
        Some(w) => w,
        None => return,
    };
    const GENERIC_WRITE: u32 = 0x4000_0000;
    unsafe {
        // CreateFileW last param (hTemplateFile) is HANDLE — pass 0 (NULL).
        let h = CreateFileW(
            wide.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(), // hTemplateFile = NULL (HANDLE is *mut c_void)
        );
        if h == INVALID_HANDLE_VALUE {
            return;
        }
        apply_times_handle(h, mtime, ctime);
        let _ = windows_sys::Win32::Foundation::CloseHandle(h);
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// SYMLINK CREATION + ELEVATION
// ──────────────────────────────────────────────────────────────────────────────

/// Outcome of a symlink creation attempt.
pub enum SymlinkResult {
    Created,         // link created successfully
    NeedsElevation,  // failed ONLY due to privilege — ask the user once
    Skipped(String), // permanently skipped (bad target, dest exists, etc.)
}

/// Attempt to create a symlink at `link_path` pointing to `target`.
/// `is_dir` selects directory vs file link semantics on Windows.
pub fn create_symlink(link_path: &Path, target: &str, is_dir: bool) -> SymlinkResult {
    use windows_sys::Win32::Storage::FileSystem::{
        CreateSymbolicLinkW, SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE,
        SYMBOLIC_LINK_FLAG_DIRECTORY, SYMBOLIC_LINK_FLAGS,
    };

    let link_wide = match path_to_wide(link_path) {
        Some(w) => w,
        None => return SymlinkResult::Skipped("invalid path encoding".into()),
    };
    let target_wide = match wide_from_str(target) {
        Some(w) => w,
        None => return SymlinkResult::Skipped("invalid target encoding".into()),
    };

    let mut flags: SYMBOLIC_LINK_FLAGS = if is_dir { SYMBOLIC_LINK_FLAG_DIRECTORY } else { 0 };
    // Try unprivileged creation first (works if Developer Mode is on).
    flags |= SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE;

    let ok = unsafe { CreateSymbolicLinkW(link_wide.as_ptr(), target_wide.as_ptr(), flags) != 0 };
    if ok {
        return SymlinkResult::Created;
    }

    let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
    const ERROR_ACCESS_DENIED: u32 = 5;
    const ERROR_PRIVILEGE_NOT_HELD: u32 = 1314;
    match err {
        ERROR_ACCESS_DENIED | ERROR_PRIVILEGE_NOT_HELD => SymlinkResult::NeedsElevation,
        other => SymlinkResult::Skipped(format!("CreateSymbolicLinkW error {}", other)),
    }
}

/// Re-launch the CURRENT process elevated, replaying `args`, so the extract can
/// complete with admin rights (symlink creation succeeds). Returns true if launched
/// successfully (user accepted UAC). The caller then EXITS — the elevated instance
/// takes over and re-does the whole extraction with full rights.
pub fn relaunch_elevated(args: &[String]) -> bool {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let exe = match current_exe_wide() {
        Some(w) => w,
        None => return false,
    };
    // Rebuild the param string, skipping argv[0] (the exe path), quoting spaced args.
    let params = args
        .iter()
        .skip(1)
        .map(|a| {
            if a.contains(' ') {
                format!("\"{}\"", a)
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let params_wide = wide_from_str(&params).unwrap_or_default();

    // Pass the current working directory as lpDirectory. Elevated processes launched
    // via "runas" often start in System32, not the caller's CWD — any relative path
    // in args would silently resolve against System32 and fail.
    let cwd_wide = std::env::current_dir()
        .ok()
        .and_then(|c| path_to_wide(&c));
    let cwd_ptr = cwd_wide.as_ref().map(|w| w.as_ptr()).unwrap_or(std::ptr::null());

    let hinst = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(), // hwnd = NULL (HWND is *mut c_void)
            wide_from_str("runas").unwrap_or_default().as_ptr(), // "runas" = UAC elevate
            exe.as_ptr(),
            if params_wide.is_empty() {
                std::ptr::null()
            } else {
                params_wide.as_ptr()
            },
            cwd_ptr,
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW returns HINSTANCE > 32 on success (legacy quirk); <= 32 = error.
    (hinst as isize) > 32
}

// ──────────────────────────────────────────────────────────────────────────────
// HELPERS
// ──────────────────────────────────────────────────────────────────────────────

fn path_to_wide(p: &Path) -> Option<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;
    let mut wide: Vec<u16> = p.as_os_str().encode_wide().collect();
    wide.push(0);
    if wide.len() <= 1 { None } else { Some(wide) }
}

fn wide_from_str(s: &str) -> Option<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;
    let mut wide: Vec<u16> = std::ffi::OsStr::new(s).encode_wide().collect();
    wide.push(0);
    if wide.len() <= 1 { None } else { Some(wide) }
}

fn current_exe_wide() -> Option<Vec<u16>> {
    path_to_wide(&std::env::current_exe().ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests pin the ACTUAL CURRENT value of PRESERVED_ATTRS so it can't be changed
    // silently. If someone edits this constant, a test fails and they must justify it.
    //
    // Current contract (as of v1.6.0): Hidden + System + Readonly + Archive are ALL
    // preserved across the round-trip. This means a source file with the Hidden or
    // System bit will be Hidden/System after extract — invisible in Windows Explorer
    // on default settings. That is a KNOWN UX footgun (it once triggered a "data loss"
    // panic that turned out to be invisible-but-present files), but it is the CURRENT
    // intended behavior (matches what 7-Zip/WinRAR do: preserve these bits).
    //
    // If you want to CHANGE this (e.g. drop Hidden/System so extracts are always
    // visible), do it deliberately: edit the constant AND these tests together, and
    // confirm it's the product decision you want. Don't smuggle the change in via a
    // test that lies about the present code.

    #[test]
    fn preserved_attrs_is_exactly_the_current_v1_6_0_set() {
        assert_eq!(PRESERVED_ATTRS,
            ATTR_HIDDEN | ATTR_READONLY | ATTR_SYSTEM | ATTR_ARCHIVE,
            "PRESERVED_ATTRS changed from the v1.6.0 value. If this is intentional \
             (e.g. dropping Hidden/System so extracts are always visible), document the \
             product decision in this test and update the assertion deliberately.");
    }

    #[test]
    fn preserved_attrs_includes_hidden() {
        assert_ne!(PRESERVED_ATTRS & ATTR_HIDDEN, 0, "ATTR_HIDDEN dropped — extracts may now differ from source");
    }

    #[test]
    fn preserved_attrs_includes_system() {
        assert_ne!(PRESERVED_ATTRS & ATTR_SYSTEM, 0, "ATTR_SYSTEM dropped — extracts may now differ from source");
    }

    #[test]
    fn preserved_attrs_includes_readonly() {
        assert_ne!(PRESERVED_ATTRS & ATTR_READONLY, 0);
    }

    #[test]
    fn preserved_attrs_includes_archive() {
        assert_ne!(PRESERVED_ATTRS & ATTR_ARCHIVE, 0);
    }
}
