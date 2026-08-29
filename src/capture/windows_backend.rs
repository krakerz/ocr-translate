use anyhow::{Context, Result};
use image::DynamicImage;
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

use super::MonitorRect;

/// Grabs a screenshot of just the monitor the cursor is currently on, via
/// `xcap` (see the Cargo.toml comment on the `xcap`/`windows` dependencies
/// for why this is scoped to Windows only and why the `wgc` feature is
/// deliberately left disabled).
///
/// Not yet verified against a real Windows machine/CI — see TODO.md.
pub fn grab_active_monitor() -> Result<DynamicImage> {
    let monitor = active_monitor()?;
    let image = monitor.capture_image().context("xcap screenshot failed")?;
    Ok(DynamicImage::ImageRgba8(image))
}

/// The monitor the cursor is currently on, falling back to the primary (or
/// first) monitor — shared by one-shot capture above and the Live Region
/// Translate video session (`windows_video.rs`), so both pick the same
/// monitor the same way.
pub(crate) fn active_monitor() -> Result<xcap::Monitor> {
    let from_cursor = cursor_position().and_then(|(x, y)| xcap::Monitor::from_point(x, y).ok());
    match from_cursor {
        Some(m) => Ok(m),
        None => primary_monitor_handle(),
    }
}

/// Falls back to the primary monitor (or the first one) when the cursor
/// position couldn't be determined, or didn't land on any known monitor.
fn primary_monitor_handle() -> Result<xcap::Monitor> {
    let mut monitors = xcap::Monitor::all().context("failed to enumerate monitors")?;
    if monitors.is_empty() {
        anyhow::bail!("no monitors found");
    }
    let primary_idx = monitors
        .iter()
        .position(|m| m.is_primary().unwrap_or(false));
    Ok(match primary_idx {
        Some(i) => monitors.remove(i),
        None => monitors.remove(0),
    })
}

pub fn primary_monitor() -> Option<MonitorRect> {
    let monitors = xcap::Monitor::all().ok()?;
    let chosen = monitors
        .iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .or_else(|| monitors.first())?;
    Some(MonitorRect {
        x: chosen.x().ok()?,
        y: chosen.y().ok()?,
        width: chosen.width().ok()?,
        height: chosen.height().ok()?,
        primary: chosen.is_primary().unwrap_or(false),
    })
}

/// The cursor's position in screen coordinates, via the plain Win32
/// `GetCursorPos` — `xcap` has no cursor-position API of its own (it's a
/// screen-capture library, not an input-query one), so this is the one place
/// this module reaches for the `windows` crate directly rather than `xcap`.
fn cursor_position() -> Option<(i32, i32)> {
    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point) }.ok()?;
    Some((point.x, point.y))
}
