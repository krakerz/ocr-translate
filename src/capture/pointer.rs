/// Queries the current pointer position in global desktop coordinates — the
/// same space as the XRandR monitor rects in `capture::monitor` and the
/// portal's full-desktop screenshot (origin at the layout's top-left corner).
///
/// On Hyprland this asks the compositor directly via `hyprctl`: its XWayland
/// never updates the root pointer while the cursor is over native Wayland
/// surfaces, so the X11 query below returns a stale, frozen position there.
/// Everywhere else (KDE/GNOME) XWayland mirrors the real cursor, and there's
/// no portable Wayland API for it, so that's the fallback. Returns `None` if
/// neither works (e.g. no XWayland).
pub fn global_position() -> Option<(i32, i32)> {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        if let Some(pos) = hyprland_position() {
            return Some(pos);
        }
        tracing::debug!("hyprctl cursor query failed; falling back to XWayland");
    }
    xwayland_position()
}

/// Hyprland's layout coordinates can go negative (e.g. a monitor at y=-10),
/// while XWayland/XRandR and the portal image shift the whole layout so its
/// top-left is (0, 0) — so the cursor is offset by the layout's minimum x/y.
fn hyprland_position() -> Option<(i32, i32)> {
    let cursor = hyprctl_json(&["cursorpos", "-j"])?;
    let monitors = hyprctl_json(&["monitors", "-j"])?;
    let monitors = monitors.as_array()?;

    let coord = |v: &serde_json::Value, key: &str| v.get(key)?.as_i64().map(|n| n as i32);
    let min_x = monitors.iter().filter_map(|m| coord(m, "x")).min()?;
    let min_y = monitors.iter().filter_map(|m| coord(m, "y")).min()?;
    Some((coord(&cursor, "x")? - min_x, coord(&cursor, "y")? - min_y))
}

fn hyprctl_json(args: &[&str]) -> Option<serde_json::Value> {
    let out = std::process::Command::new("hyprctl")
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok()
}

fn xwayland_position() -> Option<(i32, i32)> {
    use x11rb::connection::Connection;

    let (conn, screen_num) = x11rb::connect(None).ok()?;
    let screen = &conn.setup().roots[screen_num];
    let reply = x11rb::protocol::xproto::query_pointer(&conn, screen.root)
        .ok()?
        .reply()
        .ok()?;
    Some((reply.root_x as i32, reply.root_y as i32))
}
