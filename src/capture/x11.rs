use anyhow::{Context, Result};
use image::DynamicImage;

use super::pointer;

/// Grabs only the monitor under the cursor (multi-monitor aware), falling
/// back to the primary monitor if the pointer position can't be determined.
/// Uses a direct X11 call, which also works fine under XWayland.
pub fn grab_active_monitor() -> Result<DynamicImage> {
    let monitor = match pointer::global_position() {
        Some((x, y)) => xcap::Monitor::from_point(x, y).context("no monitor under the cursor")?,
        None => {
            let monitors = xcap::Monitor::all().context("failed to enumerate X11 monitors")?;
            let idx = monitors.iter().position(|m| m.is_primary()).unwrap_or(0);
            monitors.into_iter().nth(idx).context("no monitors found")?
        }
    };
    let image = monitor
        .capture_image()
        .context("X11 screen capture failed")?;
    Ok(DynamicImage::ImageRgba8(image))
}
