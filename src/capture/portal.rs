use anyhow::{Context, Result};
use ashpd_screenshot::desktop::screenshot::{AvailableTargets, Screenshot, ScreenshotProxy};
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
    // `target(Screen)` explicitly asks for the whole monitor/workspace, not
    // just a window — without it, confirmed on a KDE session: a
    // non-interactive request can silently return a screenshot of only the
    // *currently focused window's* screen (e.g. Microsoft Teams', if that's
    // the active app) rather than the full virtual desktop this code
    // otherwise assumes it always gets, regardless of where the cursor
    // actually is.
    //
    // Must be checked for support first, not just set unconditionally:
    // confirmed by testing that a portal backend which doesn't advertise
    // `Screen` in its `AvailableTargets` property rejects the whole request
    // outright (`org.freedesktop.portal.Error.InvalidArgument: Unavailable
    // screenshot target 1`) rather than ignoring the option — so setting it
    // blindly would turn "maybe still wrong monitor" into "capture always
    // fails" on any such backend. `available_targets()` needs portal
    // interface version 3+ to even exist; any failure here (older backend,
    // proxy connection issue) is treated the same as "not supported" and
    // just omits the option, falling back to this backend's prior default
    // behavior — never a hard error over this alone.
    let supports_screen_target = match ScreenshotProxy::new().await {
        Ok(proxy) => proxy
            .available_targets()
            .await
            .map(|targets| targets.contains(AvailableTargets::Screen))
            .unwrap_or(false),
        Err(_) => false,
    };
    tracing::debug!("portal supports requesting Screen target: {supports_screen_target}");

    let mut request = Screenshot::request().interactive(false).modal(true);
    if supports_screen_target {
        request = request.target(AvailableTargets::Screen);
    }
    let response = request
        .send()
        .await
        .context("xdg-desktop-portal Screenshot request failed (no portal backend running?)")?
        .response()
        .context("xdg-desktop-portal Screenshot request was denied or cancelled")?;

    // ashpd 0.13's `Uri` is a plain string wrapper (no `to_file_path()` like
    // 0.9's `url::Url`-backed one had) — the portal always hands back a
    // local `file://` URI for a screenshot, so a plain prefix strip is
    // enough; no percent-decoding needed for the simple temp paths it uses.
    let uri = response.uri();
    let path = uri
        .as_str()
        .strip_prefix("file://")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("portal returned a non-local URI: {uri}"))?;
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
    tracing::debug!(
        "portal screenshot is {}x{}",
        full.width(),
        full.height()
    );

    let Some((px, py)) = pointer::global_position() else {
        tracing::debug!("could not determine cursor position; using the full portal screenshot");
        return full;
    };
    let found = monitor::monitor_at(px, py).or_else(monitor::primary_monitor);
    let Some(m) = found else {
        tracing::debug!("could not resolve monitor geometry; using the full portal screenshot");
        return full;
    };
    tracing::debug!(
        "cursor at ({px}, {py}); resolved monitor is {}x{} at ({}, {})",
        m.width,
        m.height,
        m.x,
        m.y
    );

    if m.x >= 0
        && m.y >= 0
        && (m.x as u32 + m.width) <= full.width()
        && (m.y as u32 + m.height) <= full.height()
        && m.width > 0
        && m.height > 0
    {
        return full.crop_imm(m.x as u32, m.y as u32, m.width, m.height);
    }

    // If this fires, the portal likely didn't return the full virtual
    // desktop after all — e.g. it already scoped the screenshot to a
    // single (possibly wrong) screen on its own, so the resolved monitor
    // rect no longer fits inside it. See `target(Screen)` above, which
    // should prevent this on a new-enough portal backend; if it still
    // happens, the portal image dimensions logged above vs. the resolved
    // monitor rect just above are the key numbers to compare.
    tracing::debug!(
        "monitor geometry didn't line up with the portal screenshot; using the full image"
    );
    full
}
