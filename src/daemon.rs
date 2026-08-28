use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::channel;

use anyhow::{Context, Result};

use crate::config::AppConfig;

pub enum DaemonEvent {
    Capture,
    Quit,
}

/// Runs the tray + hotkey daemon until "Quit" is chosen from the tray menu.
///
/// Each capture is run in a freshly spawned `ocr-translate capture` child
/// process rather than in-process: the tray keeps a live GTK main loop
/// running on its own thread, and opening an eframe/winit window from another
/// thread in that same process can silently hang (GTK and winit both talking
/// to the X11/Wayland connection at once). A separate process has no GTK
/// state at all, matching the already-working standalone `capture` command.
pub fn run(cfg: AppConfig, config_path: Option<PathBuf>) -> Result<()> {
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
                if let Err(e) = spawn_capture(config_path.as_deref()) {
                    tracing::error!("failed to launch capture: {e:#}");
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

fn spawn_capture(config_path: Option<&Path>) -> Result<()> {
    let exe = std::env::current_exe().context("could not determine our own executable path")?;
    let mut cmd = Command::new(exe);
    cmd.arg("capture");
    if let Some(path) = config_path {
        cmd.arg("--config").arg(path);
    }
    cmd.spawn()
        .context("failed to spawn `ocr-translate capture`")?;
    Ok(())
}
