use anyhow::{Context, Result};
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;

/// A monitor's geometry in the X11/XWayland screen's pixel space (which
/// matches the pixel space of a portal `Screenshot`, since both come from
/// the same underlying compositor output configuration).
#[derive(Debug, Clone, Copy)]
pub struct MonitorRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

/// Finds the monitor containing a point, via a direct XRandR `GetMonitors`
/// query over XWayland — not via `xcap`, which on Linux has every geometry
/// accessor (and `Monitor::from_point()`) unconditionally call an internal
/// Wayland scale-factor lookup that hangs indefinitely on at least this
/// project's KDE/KWin setup (see `Cargo.toml`/CLAUDE.md). This queries the
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
/// can't be determined at all.
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
