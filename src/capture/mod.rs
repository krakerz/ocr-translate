mod external;
mod monitor;
mod pointer;
mod portal;
pub mod screencast;
mod selector;

use anyhow::Result;
use image::DynamicImage;

pub use monitor::clamp_to_screen;
pub use selector::{select_crop, select_region_rect};

use crate::config::{CaptureBackend, CaptureConfig, CaptureWindowConfig};

/// Gets the final, already-cropped screen region ready for OCR, using
/// whichever backend `cfg.backend` selects. Returns `None` if the user
/// cancelled the selection.
///
/// - `BuiltIn`: grab the active monitor ourselves (see [`grab_active_monitor`])
///   and crop it with our own zoom/pan/select window ([`select_crop`]),
///   sized per `window_cfg`.
/// - `External`: run an external tool that does its own live region
///   selection on the real desktop (`capture::external`); no further
///   cropping needed, its output is already the selected region
///   (`window_cfg` is unused in this path).
pub fn acquire(
    cfg: &CaptureConfig,
    window_cfg: &CaptureWindowConfig,
) -> Result<Option<DynamicImage>> {
    match cfg.backend {
        CaptureBackend::BuiltIn => {
            let full = grab_active_monitor()?;
            select_crop(&full, window_cfg)
        }
        CaptureBackend::External => external::capture(cfg),
    }
}

/// Grabs a screenshot of just the monitor the cursor is currently on (so a
/// multi-monitor setup never hands OCR a giant combined image), via the
/// xdg-desktop-portal Screenshot API — the only portable way to get pixels
/// on Wayland (works across GNOME/KDE/wlroots without any compositor-specific
/// code). This app targets Wayland sessions only; there is no X11 capture
/// backend to fall back to.
pub fn grab_active_monitor() -> Result<DynamicImage> {
    portal::grab_active_monitor()
}
