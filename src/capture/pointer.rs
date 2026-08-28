/// Queries the current pointer position in global desktop coordinates.
///
/// This talks to the X server directly. On a pure X11 session that's the real
/// pointer; on Wayland it still works in practice because KDE/GNOME/wlroots
/// all run XWayland, which mirrors the real cursor position for X11 clients —
/// there is no portable Wayland API for "where is the cursor right now"
/// outside a focused surface, so this is the same trick most cross-desktop
/// screenshot tools rely on. Returns `None` if no X server is reachable at all
/// (e.g. a wlroots session with no XWayland).
pub fn global_position() -> Option<(i32, i32)> {
    use x11rb::connection::Connection;

    let (conn, screen_num) = x11rb::connect(None).ok()?;
    let screen = &conn.setup().roots[screen_num];
    let reply = x11rb::protocol::xproto::query_pointer(&conn, screen.root)
        .ok()?
        .reply()
        .ok()?;
    Some((reply.root_x as i32, reply.root_y as i32))
}
