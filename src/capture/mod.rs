mod pointer;
mod portal;
mod selector;
mod x11;

use anyhow::Result;
use image::DynamicImage;

pub use selector::select_crop;

fn is_wayland() -> bool {
    std::env::var("WAYLAND_DISPLAY").is_ok()
        || std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland")
}

/// Grabs a screenshot of just the monitor the cursor is currently on (so a
/// multi-monitor setup never hands OCR a giant combined image), using the
/// best available backend: the xdg-desktop-portal Screenshot API on Wayland
/// (works across GNOME/KDE/wlroots without any compositor-specific code), or
/// a direct X11 grab otherwise.
pub fn grab_active_monitor() -> Result<DynamicImage> {
    if is_wayland() {
        match portal::grab_active_monitor() {
            Ok(img) => Ok(img),
            Err(e) => {
                tracing::warn!("portal screenshot failed ({e}); falling back to X11/XWayland grab");
                x11::grab_active_monitor()
            }
        }
    } else {
        x11::grab_active_monitor()
    }
}
