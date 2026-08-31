use anyhow::{Context, Result};
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;

use super::MonitorRect;

/// Finds the monitor containing a point, via a direct XRandR `GetMonitors`
/// query over XWayland — not via `xcap`, which on Linux has every geometry
/// accessor (and `Monitor::from_point()`) unconditionally call an internal
/// Wayland scale-factor lookup that hangs indefinitely on at least this
/// project's KDE/KWin setup (see `Cargo.toml`/NOTES.md). This queries the
/// exact same protocol request xcap itself used internally, just without
/// the surrounding logic that hangs.
pub fn monitor_at(x: i32, y: i32) -> Option<MonitorRect> {
    list_monitors()
        .ok()?
        .into_iter()
        .find(|m| x >= m.x && x < m.x + m.width as i32 && y >= m.y && y < m.y + m.height as i32)
}

/// Falls back to the primary monitor (or the first one) when the cursor
/// position couldn't be determined at all.
pub fn primary_monitor() -> Option<MonitorRect> {
    let monitors = list_monitors().ok()?;
    let primary = monitors.iter().find(|m| m.primary).copied();
    primary.or_else(|| monitors.first().copied())
}

fn list_monitors() -> Result<Vec<MonitorRect>> {
    let (conn, screen_num) =
        x11rb::connect(None).context("failed to connect to the X server (via XWayland)")?;
    let screen = &conn.setup().roots[screen_num];
    let reply = conn
        .randr_get_monitors(screen.root, true)
        .context("XRandR GetMonitors request failed")?
        .reply()
        .context("XRandR GetMonitors reply failed")?;
    Ok(reply
        .monitors
        .into_iter()
        .map(|m| MonitorRect {
            x: m.x as i32,
            y: m.y as i32,
            width: m.width as u32,
            height: m.height as u32,
            primary: m.primary,
        })
        .collect())
}
