use std::sync::mpsc::channel;

use anyhow::Result;

use crate::config::AppConfig;

pub enum DaemonEvent {
    Capture,
    Quit,
}

/// Runs the tray + hotkey daemon until "Quit" is chosen from the tray menu.
/// All actual window/OCR/HTTP work happens on this (the main) thread; the
/// tray and hotkey backends only ever send a `DaemonEvent` over a channel.
pub fn run(cfg: AppConfig) -> Result<()> {
    let (tx, rx) = channel::<DaemonEvent>();

    crate::tray::spawn(tx.clone());

    let mut any_hotkey_backend = false;
    if crate::hotkey::is_wayland() {
        if cfg.hotkey.enable_portal {
            crate::hotkey::spawn_portal_listener(tx.clone(), cfg.hotkey.capture_region.clone());
            any_hotkey_backend = true;
        }
    } else if cfg.hotkey.enable_x11 {
        match crate::hotkey::spawn_x11_listener(tx.clone(), &cfg.hotkey.capture_region) {
            Ok(()) => any_hotkey_backend = true,
            Err(e) => tracing::warn!("X11 hotkey registration failed: {e}"),
        }
    }

    if any_hotkey_backend {
        tracing::info!(
            "running in the system tray — press {} or use the tray menu to capture",
            cfg.hotkey.capture_region
        );
    } else {
        tracing::warn!(
            "no in-process hotkey backend active; use the tray menu, or bind \
             `ocr-translate capture` to a key in your window manager"
        );
    }

    for event in rx {
        match event {
            DaemonEvent::Capture => {
                if let Err(e) = crate::run_capture_cycle(&cfg) {
                    tracing::error!("capture cycle failed: {e:#}");
                }
            }
            DaemonEvent::Quit => {
                tracing::info!("quit requested from tray menu");
                break;
            }
        }
    }
    Ok(())
}
