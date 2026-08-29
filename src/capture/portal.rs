use anyhow::{Context, Result};
use ashpd::desktop::screenshot::Screenshot;
use image::DynamicImage;

use super::monitor;
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
/// under the cursor. Monitor geometry comes from a direct XRandR query (see
/// `capture::monitor`), which reports pixel coordinates in the same space
/// XWayland presents to the compositor — the same space the portal
/// screenshot is captured in.
fn crop_to_active_monitor(full: DynamicImage) -> DynamicImage {
    let Some((px, py)) = pointer::global_position() else {
        tracing::debug!("could not determine cursor position; using the full portal screenshot");
        return full;
    };
    let found = monitor::monitor_at(px, py).or_else(monitor::primary_monitor);
    let Some(m) = found else {
        tracing::debug!("could not resolve monitor geometry; using the full portal screenshot");
        return full;
    };

    if m.x >= 0
        && m.y >= 0
        && (m.x as u32 + m.width) <= full.width()
        && (m.y as u32 + m.height) <= full.height()
        && m.width > 0
        && m.height > 0
    {
        return full.crop_imm(m.x as u32, m.y as u32, m.width, m.height);
    }

    tracing::debug!(
        "monitor geometry didn't line up with the portal screenshot; using the full image"
    );
    full
}
