use std::sync::mpsc::Sender;

use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIconBuilder};

use crate::daemon::DaemonEvent;

/// Builds the tray icon with a "Capture" / "Quit" menu and starts forwarding
/// clicks into `tx`. On Linux this needs a live GTK main loop, so tray setup
/// and the loop both run on one dedicated background thread; a second thread
/// just blocks on the (thread-independent) menu event channel and forwards
/// matches into the daemon's event channel.
pub fn spawn(tx: Sender<DaemonEvent>) {
    #[cfg(target_os = "linux")]
    std::thread::spawn(|| {
        if let Err(e) = run_gtk_tray() {
            tracing::warn!("tray icon unavailable ({e}); use `ocr-translate capture` instead");
        }
    });
    #[cfg(not(target_os = "linux"))]
    std::thread::spawn(|| {
        if let Err(e) = run_native_tray() {
            tracing::warn!("tray icon unavailable ({e}); use `ocr-translate capture` instead");
        }
    });

    let (capture_id, quit_id) = ids();
    std::thread::spawn(move || {
        let receiver = MenuEvent::receiver();
        while let Ok(event) = receiver.recv() {
            if event.id == capture_id {
                let _ = tx.send(DaemonEvent::Capture);
            } else if event.id == quit_id {
                let _ = tx.send(DaemonEvent::Quit);
            }
        }
    });
}

fn ids() -> (tray_icon::menu::MenuId, tray_icon::menu::MenuId) {
    ("capture".into(), "quit".into())
}

fn build_menu() -> anyhow::Result<Menu> {
    let (capture_id, quit_id) = ids();
    let menu = Menu::new();
    menu.append(&MenuItem::with_id(capture_id, "Capture region", true, None))?;
    menu.append(&MenuItem::with_id(quit_id, "Quit", true, None))?;
    Ok(menu)
}

#[cfg(target_os = "linux")]
fn run_gtk_tray() -> anyhow::Result<()> {
    gtk::init()?;
    let menu = build_menu()?;
    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("ocr-translate")
        .with_icon(default_icon()?)
        .build()?;
    gtk::main();
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn run_native_tray() -> anyhow::Result<()> {
    let menu = build_menu()?;
    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("ocr-translate")
        .with_icon(default_icon()?)
        .build()?;
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

/// A plain 32x32 circle, so the tray works without shipping/loading an icon asset.
fn default_icon() -> anyhow::Result<Icon> {
    const SIZE: u32 = 32;
    let center = SIZE as f32 / 2.0;
    let radius = center - 1.0;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 + 0.5 - center;
            let dy = y as f32 + 0.5 - center;
            if (dx * dx + dy * dy).sqrt() <= radius {
                rgba.extend_from_slice(&[40, 120, 220, 255]);
            } else {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    Ok(Icon::from_rgba(rgba, SIZE, SIZE)?)
}
