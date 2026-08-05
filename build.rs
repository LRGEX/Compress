// Embed assets/icon.ico into the Windows executable so the exe itself shows the
// branded icon in Explorer / taskbar / right-click verbs.
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if cfg!(target_os = "windows") && std::path::Path::new("assets/icon.ico").exists() {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        if let Err(e) = res.compile() {
            // Don't fail the whole build over a missing resource compiler; warn loudly.
            println!("cargo:warning=LRGEX icon embed failed (is rc.exe / Windows SDK installed?): {}", e);
        }
    }
}
