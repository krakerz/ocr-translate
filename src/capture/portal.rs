use anyhow::{Context, Result};
use ashpd::desktop::screenshot::Screenshot;
use image::DynamicImage;

use super::pointer;

/// Requests a screenshot through the xdg-desktop-portal `org.freedesktop.portal.Screenshot`
/// interface. This is the only portable way to capture pixels on Wayland: the
/// compositor (GNOME Shell, KWin, wlroots' portal, ...) takes the shot and hands
/// back a file URI, so this code has no compositor-specific branches. The
/// portal always returns the full virtual desktop (every monitor), so this
/// then crops down to just the monitor under the cursor.
pub fn grab_active_monitor() -> Result<DynamicImage> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to start async runtime for the desktop portal request")?;
    let full = rt.block_on(grab_fullscreen_async())?;
    Ok(crop_to_active_monitor(full))
}

async fn grab_fullscreen_async() -> Result<DynamicImage> {
    let response = Screenshot::request()
        .interactive(false)
        .modal(true)
        .send()
        .await
        .context("xdg-desktop-portal Screenshot request failed (no portal backend running?)")?
        .response()
        .context("xdg-desktop-portal Screenshot request was denied or cancelled")?;

    let uri = response.uri();
    let path = uri
        .to_file_path()
        .map_err(|_| anyhow::anyhow!("portal returned a non-local URI: {uri}"))?;
    let img = image::open(&path)
        .with_context(|| format!("failed to decode screenshot at {}", path.display()))?;
    let _ = std::fs::remove_file(&path);
    Ok(img)
}

/// Crops the full multi-monitor portal screenshot down to just the monitor
/// under the cursor. Monitor geometry comes from `xcap` (queried over X11 /
/// XWayland), which may report either logical or physical pixel coordinates
/// depending on the platform, so both are tried before giving up and
/// returning the uncropped image.
fn crop_to_active_monitor(full: DynamicImage) -> DynamicImage {
    let Some((px, py)) = pointer::global_position() else {
        tracing::debug!("could not determine cursor position; using the full portal screenshot");
        return full;
    };
    let Ok(monitor) = xcap::Monitor::from_point(px, py) else {
        tracing::debug!(
            "could not resolve the monitor under the cursor; using the full portal screenshot"
        );
        return full;
    };

    let scale = monitor.scale_factor();
    let candidates = [
        (monitor.x(), monitor.y(), monitor.width(), monitor.height()),
        (
            (monitor.x() as f32 * scale) as i32,
            (monitor.y() as f32 * scale) as i32,
            (monitor.width() as f32 * scale) as u32,
            (monitor.height() as f32 * scale) as u32,
        ),
    ];

    for (x, y, w, h) in candidates {
        if x >= 0
            && y >= 0
            && (x as u32 + w) <= full.width()
            && (y as u32 + h) <= full.height()
            && w > 0
            && h > 0
        {
            return full.crop_imm(x as u32, y as u32, w, h);
        }
    }

    tracing::debug!(
        "monitor geometry didn't line up with the portal screenshot; using the full image"
    );
    full
}
