use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::channel;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};

use crate::config::AppConfig;
use crate::hotkey::X11HotkeyHandle;

pub enum DaemonEvent {
    Capture,
    ShowHistory(usize),
    ClearHistory,
    WatchClipboard,
    Quit,
}

/// Runs the tray + hotkey daemon until "Quit" is chosen from the tray menu.
///
/// Each capture (and each "show this history entry" click) is run in a
/// freshly spawned child process rather than in-process: the tray keeps a
/// live GTK main loop running on its own thread, and opening an eframe/winit
/// window from another thread in that same process can silently hang (GTK
/// and winit both talking to the X11/Wayland connection at once). A separate
/// process has no GTK state at all, matching the already-working standalone
/// `capture` command. Since each of those child processes reloads config
/// from disk itself, they're already "hot-reloaded" for free — see
/// `watch_config` for the state that *does* live in this long-running
/// process and needs its own reload path (hotkey registration, the tray's
/// History submenu settings).
pub fn run(cfg: AppConfig, config_path: Option<PathBuf>) -> Result<()> {
    let (tx, rx) = channel::<DaemonEvent>();
    let shared_cfg = Arc::new(RwLock::new(cfg));

    crate::tray::spawn(tx.clone(), shared_cfg.clone());

    let mut x11_hotkey: Option<X11HotkeyHandle> = None;
    let mut any_hotkey_backend = false;
    {
        let cfg = shared_cfg.read().unwrap();
        if crate::hotkey::is_wayland() {
            if cfg.hotkey.enable_portal {
                crate::hotkey::spawn_portal_listener(tx.clone(), cfg.hotkey.capture_region.clone());
                any_hotkey_backend = true;
            }
        } else if cfg.hotkey.enable_x11 {
            match crate::hotkey::spawn_x11_listener(tx.clone(), &cfg.hotkey.capture_region) {
                Ok(handle) => {
                    x11_hotkey = Some(handle);
                    any_hotkey_backend = true;
                }
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
    }

    watch_config(config_path.clone(), shared_cfg, x11_hotkey);

    for event in rx {
        match event {
            DaemonEvent::Capture => {
                if let Err(e) = spawn_subcommand(config_path.as_deref(), "capture", None) {
                    tracing::error!("failed to launch capture: {e:#}");
                }
            }
            DaemonEvent::ShowHistory(index) => {
                if let Err(e) = spawn_subcommand(
                    config_path.as_deref(),
                    "show-history",
                    Some(index.to_string()),
                ) {
                    tracing::error!("failed to show history entry: {e:#}");
                }
            }
            DaemonEvent::ClearHistory => {
                if let Err(e) = crate::history::clear() {
                    tracing::error!("failed to clear history: {e:#}");
                }
            }
            DaemonEvent::WatchClipboard => {
                if let Err(e) = spawn_subcommand(config_path.as_deref(), "watch-clipboard", None) {
                    tracing::error!("failed to launch clipboard watcher: {e:#}");
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

fn spawn_subcommand(
    config_path: Option<&Path>,
    subcommand: &str,
    arg: Option<String>,
) -> Result<()> {
    let exe = std::env::current_exe().context("could not determine our own executable path")?;
    let mut cmd = Command::new(exe);
    cmd.arg(subcommand);
    if let Some(arg) = arg {
        cmd.arg(arg);
    }
    if let Some(path) = config_path {
        cmd.arg("--config").arg(path);
    }
    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn `ocr-translate {subcommand}`"))?;
    // Reap it once it exits instead of leaving a zombie: `Child`'s Drop impl
    // does not call wait() (that would risk blocking), and this daemon never
    // otherwise waits on its children, so every capture/show-history/
    // watch-clipboard spawned over a long-running session would otherwise
    // accumulate as a <defunct> process (confirmed happening in practice).
    let subcommand = subcommand.to_string();
    std::thread::spawn(move || match child.wait() {
        Ok(status) if !status.success() => {
            tracing::warn!("`ocr-translate {subcommand}` exited with {status}");
        }
        Ok(_) => {}
        Err(e) => tracing::debug!("failed to wait on `ocr-translate {subcommand}`: {e}"),
    });
    Ok(())
}

/// Polls the resolved config file's mtime every 2 seconds; on change,
/// reloads it and swaps it into `shared`. A reload that fails to parse (e.g.
/// mid-edit, or a typo) is logged and the old config is kept rather than
/// replaced with a broken one.
///
/// Of the daemon's own long-lived state, only two things actually need a
/// live-reload path: the X11 hotkey registration (re-bound here if
/// `hotkey.capture_region` changed) and the tray's History submenu settings
/// (it already reads `shared` fresh on every refresh tick — see
/// `tray::spawn` — so no extra action is needed for that one). The
/// Wayland/portal hotkey session has no clean way to rebind live, so a
/// change there is logged as needing a restart instead of attempted.
fn watch_config(
    config_path: Option<PathBuf>,
    shared: Arc<RwLock<AppConfig>>,
    x11_hotkey: Option<X11HotkeyHandle>,
) {
    std::thread::spawn(move || {
        let mut last_mtime = config_mtime(config_path.as_deref());
        loop {
            std::thread::sleep(Duration::from_secs(2));
            let mtime = config_mtime(config_path.as_deref());
            if mtime == last_mtime {
                continue;
            }
            last_mtime = mtime;

            match crate::config::load(config_path.as_deref()) {
                Ok(new_cfg) => {
                    let old_capture_region = shared.read().unwrap().hotkey.capture_region.clone();
                    if new_cfg.hotkey.capture_region != old_capture_region {
                        if let Some(handle) = &x11_hotkey {
                            match handle.update(&new_cfg.hotkey.capture_region) {
                                Ok(()) => tracing::info!(
                                    "hotkey updated to {}",
                                    new_cfg.hotkey.capture_region
                                ),
                                Err(e) => tracing::warn!("failed to update hotkey: {e:#}"),
                            }
                        } else if crate::hotkey::is_wayland() {
                            tracing::warn!(
                                "hotkey.capture_region changed, but the Wayland portal-based \
                                 hotkey can't be rebound live; restart to apply, or bind \
                                 `ocr-translate capture` natively in your compositor instead"
                            );
                        }
                    }
                    *shared.write().unwrap() = new_cfg;
                    tracing::info!("config file changed; reloaded");
                }
                Err(e) => tracing::warn!(
                    "config file changed but failed to reload (keeping the previous config): {e:#}"
                ),
            }
        }
    });
}

fn config_mtime(explicit_path: Option<&Path>) -> Option<SystemTime> {
    let path = crate::config::resolve_path(explicit_path)?;
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}
