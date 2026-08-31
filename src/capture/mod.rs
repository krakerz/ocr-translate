mod external;
#[cfg(target_os = "linux")]
mod monitor;
#[cfg(target_os = "linux")]
mod pointer;
#[cfg(target_os = "linux")]
mod portal;
#[cfg(target_os = "linux")]
pub mod screencast;
mod selector;
#[cfg(target_os = "windows")]
mod windows_backend;
#[cfg(target_os = "windows")]
mod windows_video;

use anyhow::Result;
use image::DynamicImage;

pub use selector::{select_crop, select_region_rect, show_regions, ExistingRegion};

/// The continuous-capture session Live Region Translate (`watch-region`)
/// polls for frames — a single per-OS implementation aliased under one
/// name, same `#[cfg]`-dispatch approach as `grab_active_monitor`/
/// `primary_monitor` below rather than a trait, since there's exactly one
/// implementation per OS and no runtime switching needed.
///
/// - Linux: `screencast::ScreenCastSession`, a `org.freedesktop.portal.ScreenCast`
///   + PipeWire session.
/// - Windows: `windows_video::RegionSession`, an `xcap` `VideoRecorder`
///   session (DXGI Desktop Duplication).
/// - Elsewhere: a stub below whose `start()` returns a clear "not
///   implemented on this OS yet" error, matching `grab_active_monitor`'s
///   not-linux-not-windows arm.
#[cfg(target_os = "linux")]
pub use screencast::ScreenCastSession as RegionSession;
#[cfg(target_os = "windows")]
pub use windows_video::RegionSession;

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub struct RegionSession;

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
impl RegionSession {
    pub fn start() -> Result<Self> {
        anyhow::bail!("Live Region Translate isn't implemented on this OS yet")
    }

    pub fn latest_frame(&self) -> Option<image::RgbaImage> {
        None
    }
}

use crate::config::{CaptureBackend, CaptureConfig, CaptureWindowConfig};

/// A monitor's geometry in the platform's native pixel space (matches
/// whatever coordinate space that platform's screenshot API returns, so a
/// screenshot can always be cropped/positioned using these numbers directly).
// x/y/primary are read by the Linux backend (portal.rs, monitor.rs) to crop
// to the right monitor; the Windows backend gets an already-correctly-cropped
// image straight from xcap, so it never reads them back off this struct.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct MonitorRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

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
/// multi-monitor setup never hands OCR a giant combined image).
///
/// - Linux: via the xdg-desktop-portal Screenshot API — the only portable
///   way to get pixels on Wayland (works across GNOME/KDE/wlroots without
///   any compositor-specific code). This app targets Wayland sessions only
///   on Linux; there is no X11 capture backend to fall back to.
/// - Windows: via `xcap` (GDI-based) — see `windows_backend.rs`.
#[cfg(target_os = "linux")]
pub fn grab_active_monitor() -> Result<DynamicImage> {
    portal::grab_active_monitor()
}

#[cfg(target_os = "windows")]
pub fn grab_active_monitor() -> Result<DynamicImage> {
    windows_backend::grab_active_monitor()
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn grab_active_monitor() -> Result<DynamicImage> {
    anyhow::bail!(
        "BuiltIn capture isn't implemented on this OS yet — set capture.backend: external instead"
    )
}

/// A requested window size gets clamped to at most this many pixels less
/// than the primary monitor's actual resolution — see [`clamp_to_screen`].
const SCREEN_SIZE_SAFETY_MARGIN: f32 = 24.0;

/// Clamps a requested window size (e.g. `popup.width`/`height`) so it never
/// reaches the primary monitor's exact resolution.
///
/// Confirmed by testing on a KDE/KWin Wayland session: requesting an
/// `egui::ViewportBuilder` inner size where *both* dimensions are within
/// roughly 10px of the output's real resolution (3440x1440 in testing)
/// causes the window to render as a tiny fallback size in the top-left
/// corner instead — the same broken result happens with `with_maximized(true)`
/// too, so this isn't fixable by asking for maximized instead. Sizes with
/// just one dimension near the monitor's bound (or comfortably under it,
/// e.g. 3400x1400 on that same 3440x1440 output) rendered correctly, so a
/// small safety margin is enough; this doesn't attempt to size windows to
/// fill the screen precisely, just to avoid the broken edge case if a user
/// configures a popup/live-window size matching (or exceeding) their
/// monitor's resolution, which is a reasonable thing to want ("make it as
/// big as possible"). Falls back to the untouched size if monitor geometry
/// can't be determined at all (including on platforms where `primary_monitor`
/// isn't implemented yet).
pub fn clamp_to_screen(width: f32, height: f32) -> (f32, f32) {
    let Some(m) = primary_monitor() else {
        return (width, height);
    };
    let max_w = (m.width as f32 - SCREEN_SIZE_SAFETY_MARGIN).max(200.0);
    let max_h = (m.height as f32 - SCREEN_SIZE_SAFETY_MARGIN).max(150.0);
    let (clamped_w, clamped_h) = (width.min(max_w), height.min(max_h));
    if clamped_w != width || clamped_h != height {
        tracing::debug!(
            "requested window size {width}x{height} clamped to {clamped_w}x{clamped_h} \
             (primary monitor is {}x{}) to avoid a known Wayland windowing bug near full-screen sizes",
            m.width,
            m.height
        );
    }
    (clamped_w, clamped_h)
}

#[cfg(target_os = "linux")]
fn primary_monitor() -> Option<MonitorRect> {
    monitor::primary_monitor()
}

#[cfg(target_os = "windows")]
fn primary_monitor() -> Option<MonitorRect> {
    windows_backend::primary_monitor()
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn primary_monitor() -> Option<MonitorRect> {
    None
}
