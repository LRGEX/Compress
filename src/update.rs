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

/// Cheap version check: fetch latest.json, compare to the running version.
/// Returns `Some(UpdateInfo)` if a newer version exists, else `None`.
/// Silently returns `None` on any network error — never blocks or prompts.
pub fn check_version_only() -> Option<UpdateInfo> {
    let current = env!("CARGO_PKG_VERSION");

    let response = match ureq::get(MANIFEST_URL)
        .timeout(std::time::Duration::from_secs(10))
        .call()
    {
        Ok(r) => r,
        Err(_) => return None,
    };

    let manifest: Manifest = match response.into_json() {
        Ok(m) => m,
        Err(_) => return None,
    };

    if !is_newer(&manifest.version, current) {
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
    let exe_path = std::env::current_exe().unwrap_or_default();

    let confirm = rfd::MessageDialog::new()
        .set_title("Update Available")
        .set_description(&format!(
            "Version {} is available (you have v{}).\n\nUpdate now?",
            info.version, current
        ))
        .set_buttons(rfd::MessageButtons::YesNo)
        .show();
    if confirm != rfd::MessageDialogResult::Yes {
        return;
    }

    let temp_installer = std::env::temp_dir().join("lrgex-compress-update-setup.exe");

    let resp = match ureq::get(&format!("{}?v={}", info.url, info.version))
        .timeout(std::time::Duration::from_secs(120))
        .call()
    {
        Ok(r) => r,
        Err(e) => {
            show_error(&format!("Download failed: {}", e));
            return;
        }
    };

    let mut reader = resp.into_reader();
    let mut data = Vec::new();
    if let Err(e) = reader.read_to_end(&mut data) {
        show_error(&format!("Read failed: {}", e));
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

    if let Err(e) = std::fs::write(&temp_installer, &data) {
        show_error(&format!("Save failed: {}", e));
        return;
    }

    // Spawn a .bat that waits for the installer to finish, then shows success/failure.
    let bat_path = std::env::temp_dir().join("lrgex-compress-updater.bat");
    let bat = format!(
        "@echo off\r\n\"{}\" /VERYSILENT /NORESTART /NOCANCEL /SP-\r\nif %errorlevel% equ 0 (\r\n  msg * \"LRGEX Compress updated successfully to v{}\"\r\n) else (\r\n  msg * \"Update failed (installer exit code %errorlevel%)\"\r\n)\r\ndel \"{}\" >nul 2>&1\r\ndel \"%~f0\"\r\n",
        temp_installer.to_string_lossy(),
        info.version,
        temp_installer.to_string_lossy()
    );
    let _ = std::fs::write(&bat_path, bat);

    rfd::MessageDialog::new()
        .set_title("Updating")
        .set_description(&format!(
            "Signature verified. Downloaded v{} installer.\n\nThe app will close and update will install automatically.",
            info.version
        ))
        .set_buttons(rfd::MessageButtons::Ok)
        .show();

    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("cmd.exe")
        .args(["/c", bat_path.to_str().unwrap_or("")])
        .creation_flags(0x08000000u32)
        .spawn();

    // Exit immediately — installer will close this app via CloseApplications=force.
    std::process::exit(0);
}

fn verify_signature(data: &[u8], sig_hex: &str) -> Result<(), String> {
    let pub_hex = UPDATE_PUBKEY_HEX.trim();
    let pub_bytes = hex::decode(pub_hex).map_err(|e| format!("Bad public key: {}", e))?;
    let mut pub_arr = [0u8; 32];
    pub_arr.copy_from_slice(&pub_bytes);
    let verifying_key = VerifyingKey::from_bytes(&pub_arr).map_err(|e| format!("Bad public key: {}", e))?;

    let sig_bytes = hex::decode(sig_hex).map_err(|e| format!("Bad signature format: {}", e))?;
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
