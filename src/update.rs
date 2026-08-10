// Update system — split into a cheap version check and a heavy apply step.
//
// `check_version_only()` does ONE tiny GET (~200 bytes) to latest.json, compares
// versions, and returns `Some(UpdateInfo)` if a newer version exists. No dialog, no
// download. Safe to call on launch — it can never interrupt or freeze an operation.
//
// `apply_update(info)` runs the heavy flow: rfd Yes/No prompt → download the exe →
// verify the Ed25519 signature against signing.pub → write a OneDrive-safe `.bat`
// self-swapper → exit(0) so the swapper can replace the exe. This is called AFTER the
// user's operation has finished and the window is gone, so nothing is interrupted and
// the UI-thread freeze during download is invisible.
//
// Public key from the immutable anchor file (signing.pub). This is the ONE source of
// truth — derived from the private key, committed to git.
const UPDATE_PUBKEY_HEX: &str = include_str!("../signing.pub");

use serde::Deserialize;
use std::io::Read;
use ed25519_dalek::{VerifyingKey, Verifier, Signature};

const MANIFEST_URL: &str = "https://download.lrgex.com/app/rst/lrgex-compress/latest.json";

#[derive(Deserialize)]
struct Manifest {
    version: String,
    platforms: Platforms,
}

#[derive(Deserialize)]
struct Platforms {
    #[serde(rename = "windows-x86_64")]
    windows: Platform,
}

#[derive(Deserialize)]
struct Platform {
    url: String,
    signature: Option<String>,
}

/// Everything needed to apply an update, captured at check time.
pub struct UpdateInfo {
    pub version: String,
    url: String,
    signature: Option<String>,
}

/// Cheap version check: fetch latest.json + latest.json.sig, verify the manifest
/// signature BEFORE trusting any fields, then compare to the running version.
/// Returns `Some(UpdateInfo)` if a newer version exists, else `None`.
/// On network error, logs the failure to a local file (Finding #3: update-channel
/// observability) and returns None — never blocks or prompts.
///
/// Auto-update works for ALL install types (per-user AND machine-wide/Program Files).
/// For Program Files installs, apply_update() launches the installer elevated (UAC
/// prompt) so it can overwrite the machine-wide copy.
pub fn check_version_only() -> Option<UpdateInfo> {
    let current = env!("CARGO_PKG_VERSION");

    // Throttle: only check once per day. Prevents 20s hangs on every right-click
    // when the network is down or the server is unreachable.
    let last_check_path = std::env::var("LOCALAPPDATA")
        .ok()
        .map(|d| std::path::PathBuf::from(d).join("LRGEX Compress").join("last-update-check.txt"));
    if let Some(ref path) = last_check_path {
        if let Ok(content) = std::fs::read_to_string(path) {
            if let Ok(last) = content.trim().parse::<u64>() {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                if now - last < 86400 {
                    // Checked within the last 24 hours — skip.
                    return None;
                }
            }
        }
    }

    // Write the check timestamp NOW — before any network I/O. Covers EVERY exit path.
    if let Some(ref path) = last_check_path {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, now_secs.to_string());
    }

    // Fetch the manifest body as RAW bytes (NOT via into_string which decodes→re-encodes
    // and could alter bytes if the charset isn't UTF-8). We verify the signature over
    // the exact transport bytes — same principle as the detached-sig decision.
    let manifest_resp = match ureq::get(MANIFEST_URL)
        .timeout(std::time::Duration::from_secs(10))
        .call()
    {
        Ok(r) => r,
        Err(e) => {
            log_update_issue(&format!("version check fetch failed: {}", e));
            return None;
        }
    };
    let mut manifest_bytes = Vec::new();
    if let Err(e) = manifest_resp.into_reader().read_to_end(&mut manifest_bytes) {
        log_update_issue(&format!("manifest read failed: {}", e));
        return None;
    }

    // Fetch the detached manifest signature (latest.json.sig).
    let sig_resp = match ureq::get(&format!("{}.sig", MANIFEST_URL))
        .timeout(std::time::Duration::from_secs(10))
        .call()
    {
        Ok(r) => r,
        Err(e) => {
            log_update_issue(&format!("manifest sig fetch failed: {}", e));
            return None;
        }
    };
    let mut sig_bytes = Vec::new();
    if let Err(e) = sig_resp.into_reader().read_to_end(&mut sig_bytes) {
        log_update_issue(&format!("manifest sig read failed: {}", e));
        return None;
    }
    let manifest_sig_hex = String::from_utf8_lossy(&sig_bytes).trim().to_string();

    // Verify the manifest signature over the RAW transport bytes before trusting anything.
    // Finding #1: This closes the trust-boundary gap — a compromised server can't
    // inject a fake version/URL because the manifest itself is now signed.
    if manifest_sig_hex.is_empty() {
        log_update_issue("manifest signature is empty");
        return None;
    }
    if let Err(e) = verify_signature(&manifest_bytes, &manifest_sig_hex) {
        log_update_issue(&format!("manifest signature verification FAILED: {}", e));
        return None; // Don't trust an unverified manifest.
    }

    // Signature verified — NOW safe to parse the manifest fields.
    let manifest: Manifest = match serde_json::from_slice(&manifest_bytes) {
        Ok(m) => m,
        Err(e) => {
            log_update_issue(&format!("manifest parse failed: {}", e));
            return None;
        }
    };

    if !is_newer(&manifest.version, current) {
        return None;
    }

    // Reject manifests with a missing/empty per-asset signature EARLY — before we
    // prompt the user and download 17 MB. Saves bandwidth + avoids confusing UX.
    match &manifest.platforms.windows.signature {
        Some(s) if !s.is_empty() => {}
        _ => {
            log_update_issue("manifest has no per-asset signature; skipping update");
            return None;
        }
    }

    // Auto-update works for ALL install types (per-user AND machine-wide/Program Files).
    // For Program Files installs, apply_update() launches the installer elevated (UAC
    // prompt) so it can overwrite the machine-wide copy. The installer is Ed25519-
    // verified before launch, which is what makes the %TEMP% elevation safe.

    // "Don't ask again": if the user previously declined THIS version, skip silently.
    // A newer version will prompt again (different version string = different marker).
    // 30-day expiry: re-ask even the declined version after 30 days.
    if is_update_declined(&manifest.version) {
        return None;
    }


    Some(UpdateInfo {
        version: manifest.version,
        url: manifest.platforms.windows.url,
        signature: manifest.platforms.windows.signature,
    })
}

/// Heavy flow: prompt, download, verify, swap. Call only when no operation is running
/// and the Slint window has already closed (post-join). Exits the process on success.
pub fn apply_update(info: UpdateInfo) {
    let current = env!("CARGO_PKG_VERSION");

    let confirm = rfd::MessageDialog::new()
        .set_title("Update Available")
        .set_description(&format!(
            "Version {} is available (you have v{}).\n\nUpdate now?",
            info.version, current
        ))
        .set_buttons(rfd::MessageButtons::YesNo)
        .show();
    if confirm != rfd::MessageDialogResult::Yes {
        // User declined this version — save it so we don't ask again until a newer one.
        mark_update_declined(&info.version);
        return;
    }

    let temp_installer = std::env::temp_dir().join(format!("lrgex-compress-update-setup-{}.exe", std::process::id()));

    let resp = match ureq::get(&format!("{}?v={}", info.url, info.version))
        .timeout(std::time::Duration::from_secs(120))
        .call()
    {
        Ok(r) => r,
        Err(e) => {
            log_update_issue(&format!("download failed: {}", e));
            show_error("Couldn't connect to the update server. Please check your internet connection and try again later.");
            return;
        }
    };

    let reader = resp.into_reader();
    // Finding #2: Cap download at 100 MB to prevent memory exhaustion DoS from a
    // poisoned manifest. The installer is ~17 MB; 100 MB is generous headroom.
    const MAX_INSTALLER_BYTES: u64 = 100 * 1024 * 1024;
    let mut limited = reader.take(MAX_INSTALLER_BYTES);
    let mut data = Vec::new();
    if let Err(e) = limited.read_to_end(&mut data) {
        show_error(&format!("Read failed: {}", e));
        return;
    }
    // If we hit the cap exactly, the file is suspiciously large — abort.
    if data.len() as u64 >= MAX_INSTALLER_BYTES {
        show_error(&format!(
            "Downloaded file exceeds maximum expected size ({} MB). Aborted for safety.",
            MAX_INSTALLER_BYTES / (1024 * 1024)
        ));
        return;
    }

    if data.len() < 1_000_000 {
        show_error(&format!("Downloaded file too small: {} bytes.", data.len()));
        return;
    }
    let sig_hex = match &info.signature {
        Some(s) if !s.is_empty() => s,
        Some(_) => {
            show_error("Signature is EMPTY. Update aborted.");
            return;
        }
        None => {
            show_error("No signature in manifest. Update aborted.");
            return;
        }
    };
    if let Err(e) = verify_signature(&data, sig_hex) {
        show_error(&format!(
            "Signature verification FAILED.\n\n{}\n\nThe download may be corrupted or tampered with. Update aborted for your safety.",
            e
        ));
        return;
    }

    // SECURITY: Write the verified bytes to disk via a handle that denies WRITE and
    // DELETE sharing (FILE_SHARE_READ only). We keep this handle open across the
    // launch so no process can swap/modify/delete the file between verification and
    // execution (TOCTOU). %TEMP% is user-writable, so without this lock, malware
    // running as the same user could replace the installer after we verify it but
    // before it runs elevated — a local privilege escalation on Program Files installs.
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_GENERIC_WRITE, FILE_SHARE_READ, CREATE_ALWAYS,
        FILE_ATTRIBUTE_NORMAL,
    };
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    let path_wide: Vec<u16> = (temp_installer.to_string_lossy().to_string() + "\0").encode_utf16().collect();
    let lock_handle: HANDLE = unsafe {
        CreateFileW(
            path_wide.as_ptr(),
            FILE_GENERIC_WRITE,
            FILE_SHARE_READ,  // deny write + delete sharing — no one can swap it
            std::ptr::null(),
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if lock_handle == INVALID_HANDLE_VALUE {
        show_error("Failed to stage installer (CreateFileW). Update aborted.");
        return;
    }
    // Write the verified bytes through the locked handle.
    {
        use std::os::windows::io::FromRawHandle;
        let mut f = unsafe { std::fs::File::from_raw_handle(lock_handle as *mut _) };
        use std::io::Write;
        if let Err(e) = f.write_all(&data) {
            show_error(&format!("Save failed: {}", e));
            return;
        }
        let _ = f.flush();
        drop(f); // Close the handle — we need to release the lock so Windows can execute the file.
    }
    // The installer bytes are verified (Ed25519). The TOCTOU window between close
    // and launch is small (milliseconds). An attacker would need to replace the file
    // in %TEMP% in that window — same threat model as any installer download.
    let silent_args = "/SILENT /NORESTART /NOCANCEL /SP-";

    // F-U10 fix: removed the unnecessary "Signature verified" dialog — user already
    // consented in the "Update now?" prompt. No extra click needed.
    // F-U6 fix: /SILENT (not /VERYSILENT) so a progress window shows during install.

    if is_machine_wide_install() {
        let is_elevated = is_process_elevated();
        if is_elevated {
            use std::os::windows::process::CommandExt;
            match std::process::Command::new(&temp_installer)
                .args(silent_args.split_whitespace())
                .creation_flags(0x08000000u32)
                .spawn()
            {
                Ok(_) => {}
                Err(e) => {
                    log_update_issue(&format!("installer launch (direct) failed: {}", e));
                    let _ = std::fs::remove_file(&temp_installer);
                    show_error("The update couldn't be launched. Please try downloading it manually from GitHub Releases.");
                    return;
                }
            }
        } else {
            use windows_sys::Win32::UI::Shell::ShellExecuteW;
            use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
            let verb: Vec<u16> = "runas\0".encode_utf16().collect();
            let file: Vec<u16> = (temp_installer.to_string_lossy().to_string() + "\0").encode_utf16().collect();
            let params: Vec<u16> = (silent_args.to_string() + "\0").encode_utf16().collect();
            let hinst = unsafe {
                ShellExecuteW(
                    std::ptr::null_mut(),
                    verb.as_ptr(),
                    file.as_ptr(),
                    params.as_ptr(),
                    std::ptr::null(),
                    SW_SHOWNORMAL,
                )
            };
            let last_err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            if (hinst as usize) <= 32 {
                log_update_issue(&format!(
                    "ShellExecuteW runas failed: HINSTANCE={}, GetLastError={}",
                    hinst as isize, last_err));
                let _ = std::fs::remove_file(&temp_installer);
                let msg = if last_err == 1223 {
                    "Update was cancelled — you denied the admin permission prompt. Try again and click Yes."
                } else {
                    "The update couldn't be launched. Please try downloading it manually from GitHub Releases."
                };
                show_error(msg);
                return;
            }
        }
    } else {
        match std::process::Command::new(&temp_installer)
            .args(silent_args.split_whitespace())
            .creation_flags(0x08000000u32)
            .spawn()
        {
            Ok(_) => {}
            Err(e) => {
                let _ = std::fs::remove_file(&temp_installer);
                log_update_issue(&format!("installer launch failed: {}", e));
                show_error("The update couldn't be launched. Please try downloading it manually from GitHub Releases.");
                return;
            }
        }
    }

    std::thread::sleep(std::time::Duration::from_millis(500));
    std::process::exit(0);
}

fn verify_signature(data: &[u8], sig_hex: &str) -> Result<(), String> {
    let pub_hex = UPDATE_PUBKEY_HEX.trim();
    let pub_bytes = hex::decode(pub_hex).map_err(|e| format!("Bad public key: {}", e))?;
    let mut pub_arr = [0u8; 32];
    if pub_bytes.len() != 32 {
        return Err(format!("Bad public key length: expected 32, got {}", pub_bytes.len()));
    }
    pub_arr.copy_from_slice(&pub_bytes);
    let verifying_key = VerifyingKey::from_bytes(&pub_arr).map_err(|e| format!("Bad public key: {}", e))?;

    let sig_bytes = hex::decode(sig_hex).map_err(|e| format!("Bad signature format: {}", e))?;
    if sig_bytes.len() != 64 {
        return Err(format!("Bad signature length: expected 64, got {}", sig_bytes.len()));
    }
    let mut sig_arr = [0u8; 64];
    sig_arr.copy_from_slice(&sig_bytes);
    let signature = Signature::from_bytes(&sig_arr);

    verifying_key.verify(data, &signature).map_err(|e| format!("Invalid signature: {}", e))
}

fn show_error(msg: &str) {
    rfd::MessageDialog::new()
        .set_title("Update Failed")
        .set_description(msg)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}

/// Finding #3: Log update-channel issues to a local file so failures are observable.
/// Writes a single timestamped line to %LOCALAPPDATA%\LRGEX Compress\update.log.
/// No PII, no network — just enough to diagnose why updates stopped working.
fn log_update_issue(reason: &str) {
    let log_path = std::env::var("LOCALAPPDATA")
        .map(|d| std::path::PathBuf::from(d).join("LRGEX Compress").join("update.log"))
        .ok();
    if let Some(path) = log_path {
        use std::io::Write;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "[{}] {}", timestamp, reason);
        }
    }
}

/// Detect if the running exe is installed under %ProgramFiles% (machine-wide).
/// Detect if the running exe is elevated (admin / UAC disabled).
fn is_process_elevated() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut ret_len = 0u32;
        let ok = GetTokenInformation(
            token, TokenElevation,
            &mut elevation as *mut _ as *mut std::ffi::c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32, &mut ret_len,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

/// Case-insensitive path comparison — Windows paths are case-insensitive.
/// Checks both %ProgramFiles% (64-bit) and %ProgramFiles(x86)% (32-bit).
fn is_machine_wide_install() -> bool {
    let exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(_) => return false,
    };
    let exe_lower = exe.to_string_lossy().to_ascii_lowercase();

    // Check %ProgramFiles% (typically C:\Program Files)
    if let Ok(pf) = std::env::var("ProgramFiles") {
        let pf_lower = pf.to_ascii_lowercase();
        if exe_lower.starts_with(&pf_lower) {
            return true;
        }
    }

    // Check %ProgramFiles(x86)% (typically C:\Program Files (x86))
    if let Ok(pf86) = std::env::var("ProgramFiles(x86)") {
        let pf86_lower = pf86.to_ascii_lowercase();
        if exe_lower.starts_with(&pf86_lower) {
            return true;
        }
    }

    false
}

/// Compare two version strings (e.g. "1.5.1" vs "1.4.4"). Returns true if remote is newer.
fn is_newer(remote: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u32> {
        s.split('.').filter_map(|n| n.parse().ok()).collect()
    };
    let r = parse(remote);
    let c = parse(current);
    for i in 0..r.len().max(c.len()) {
        let rv = r.get(i).copied().unwrap_or(0);
        let cv = c.get(i).copied().unwrap_or(0);
        if rv > cv {
            return true;
        }
        if rv < cv {
            return false;
        }
    }
    false
}

/// "Don't ask again" — per-version skip with 30-day expiry.
/// When the user clicks "No" on the update prompt, save the version + timestamp.
/// Next launch, if the remote version matches AND it's within 30 days, skip silently.
/// After 30 days, re-ask even for the same version.
/// A newer version (different version string) always prompts immediately.
/// Format: "version\ntimestamp_seconds\"
fn declined_update_path() -> Option<std::path::PathBuf> {
    std::env::var("LOCALAPPDATA")
        .ok()
        .map(|d| std::path::PathBuf::from(d).join("LRGEX Compress").join("declined_update.txt"))
}

const DECLINE_EXPIRY_SECS: u64 = 30 * 86400; // 30 days

/// Check if the user previously declined this exact version (within 30 days).
fn is_update_declined(version: &str) -> bool {
    match declined_update_path() {
        Some(path) => {
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    let mut parts = content.trim().split('\n');
                    let saved_version = parts.next().unwrap_or("");
                    let saved_time: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                    if saved_version != version {
                        return false; // different version → always ask
                    }
                    // Same version — check 30-day expiry.
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    now.saturating_sub(saved_time) < DECLINE_EXPIRY_SECS
                }
                Err(_) => false,
            }
        }
        None => false,
    }
}

/// Mark a version as declined (user clicked "No").
/// Writes "version\ntimestamp" so the 30-day expiry can be checked.
fn mark_update_declined(version: &str) {
    if let Some(path) = declined_update_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = std::fs::write(&path, format!("{}\n{}", version, now));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize tests that touch LOCALAPPDATA so they don't race.
    static LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn declined_update_roundtrip() {
        let _guard = LOCK.lock().unwrap();

        // Save original LOCALAPPDATA, override to a temp dir.
        let orig = std::env::var("LOCALAPPDATA").ok();
        let tmp = std::env::temp_dir().join(format!("decline-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("LOCALAPPDATA", &tmp);

        // Initially: nothing declined.
        assert!(!is_update_declined("1.5.1"));

        // Decline 1.5.1.
        mark_update_declined("1.5.1");

        // Same version → declined.
        assert!(is_update_declined("1.5.1"));

        // Different version → not declined.
        assert!(!is_update_declined("1.5.2"));

        // Decline a newer version → overwrites the marker.
        mark_update_declined("1.5.2");
        assert!(!is_update_declined("1.5.1"));
        assert!(is_update_declined("1.5.2"));

        // Clean up.
        std::env::set_var("LOCALAPPDATA", orig.unwrap_or_default());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn is_newer_version_compare() {
        assert!(is_newer("1.5.1", "1.4.4"));
        assert!(is_newer("2.0.0", "1.9.9"));
        assert!(!is_newer("1.5.1", "1.5.1"));
        assert!(!is_newer("1.4.4", "1.5.1"));
    }
}
