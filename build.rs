// Embeds assets/icon.ico as the built exe's icon resource on Windows, so it
// shows up in Explorer/the taskbar/Alt-Tab for the file itself, not just the
// in-app window/tray icons (those are set at runtime, see src/icon.rs).
//
// Checks `CARGO_CFG_TARGET_OS` (the target being built for) rather than
// `#[cfg(target_os = "windows")]`, deliberately: build scripts always
// compile for the *host* running the build, not `--target`, so a `#[cfg]`
// here would reflect the host, not the target — confirmed by testing that
// this exact mistake silently skips icon embedding when cross-compiling for
// Windows from a non-Windows host. See the Cargo.toml comment on the
// `winresource` build-dependency for why it's a plain (non-target-gated)
// dependency to make this possible.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        if let Err(e) = res.compile() {
            // Non-fatal: a missing/bad icon shouldn't break the whole build.
            println!("cargo:warning=failed to embed Windows icon resource: {e}");
        }
    }
}
