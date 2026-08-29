use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::channel;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};

use crate::config::AppConfig;

pub enum DaemonEvent {
    Capture,
    ShowHistory(usize),
    ClearHistory,
    WatchClipboard,
    Quit,
}

/// Runs the tray daemon until "Quit" is chosen from the tray menu.
///
/// Each capture (and each "show this history entry" click) is run in a
/// freshly spawned child process rather than in-process: the tray keeps a
/// live GTK main loop running on its own thread, and opening an eframe/winit
/// window from another thread in that same process can silently hang (GTK
/// and winit both talking to the Wayland connection at once). A separate
/// process has no GTK state at all, matching the already-working standalone
/// `capture` command. Since each of those child processes reloads config
/// from disk itself, they're already "hot-reloaded" for free — see
/// `watch_config` for the state that *does* live in this long-running
/// process and needs its own reload path (currently just the tray's History
/// submenu settings, which it already reads fresh on every refresh tick —
/// see `tray::spawn` — so `watch_config` mainly exists to keep `shared`
/// itself up to date for whatever future daemon-side state needs it).
///
/// There is no in-process hotkey: the `GlobalShortcuts` portal only grants
/// shortcuts to Flatpak/Snap-sandboxed apps in practice (confirmed by
/// testing — a plain binary gets `NotAllowed: An app id is required`), so
/// it's not implemented here at all. Bind `ocr-translate capture` to a key
/// in your compositor/DE instead (see README) — that's the only mechanism
/// that has ever reliably worked in this project.
pub fn run(cfg: AppConfig, config_path: Option<PathBuf>) -> Result<()> {
    let (tx, rx) = channel::<DaemonEvent>();
    let shared_cfg = Arc::new(RwLock::new(cfg));

    crate::tray::spawn(tx.clone(), shared_cfg.clone());
    tracing::info!("running in the system tray — use the tray menu, or bind `ocr-translate capture` to a key in your compositor/DE");

    watch_config(config_path.clone(), shared_cfg);

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
fn watch_config(config_path: Option<PathBuf>, shared: Arc<RwLock<AppConfig>>) {
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
