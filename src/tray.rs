use std::sync::mpsc::Sender;
use std::sync::{Arc, RwLock};

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::TrayIconBuilder;

use crate::config::AppConfig;
use crate::daemon::DaemonEvent;
use crate::history::HistoryEntry;

const CAPTURE_ID: &str = "capture";
const QUIT_ID: &str = "quit";
const CLEAR_HISTORY_ID: &str = "history:clear";
const HISTORY_ID_PREFIX: &str = "history:";

/// Builds the tray icon with a "Capture" / "History" / "Quit" menu and
/// starts forwarding clicks into `tx`. On Linux this needs a live GTK main
/// loop, so tray setup, menu refresh, and the loop all run on one dedicated
/// background thread; a second thread just blocks on the (thread-independent)
/// menu event channel and forwards matches into the daemon's event channel.
///
/// The History submenu is rebuilt periodically (every 2 seconds) rather than
/// once at startup: captures run in a separate process (see `daemon::run`),
/// so this process only learns about new entries by re-reading
/// `history.jsonl` from disk. Each refresh also reads `cfg.history` fresh, so
/// e.g. `tray_menu_entries` edits in the config file take effect live too —
/// see `daemon::watch_config`.
pub fn spawn(tx: Sender<DaemonEvent>, cfg: Arc<RwLock<AppConfig>>) {
    #[cfg(target_os = "linux")]
    std::thread::spawn(move || {
        if let Err(e) = run_gtk_tray(cfg) {
            tracing::warn!("tray icon unavailable ({e}); use `ocr-translate capture` instead");
        }
    });
    #[cfg(not(target_os = "linux"))]
    std::thread::spawn(move || {
        if let Err(e) = run_native_tray(cfg) {
            tracing::warn!("tray icon unavailable ({e}); use `ocr-translate capture` instead");
        }
    });

    std::thread::spawn(move || {
        let receiver = MenuEvent::receiver();
        while let Ok(event) = receiver.recv() {
            let id: &str = event.id.as_ref();
            if id == CAPTURE_ID {
                let _ = tx.send(DaemonEvent::Capture);
            } else if id == QUIT_ID {
                let _ = tx.send(DaemonEvent::Quit);
            } else if id == CLEAR_HISTORY_ID {
                let _ = tx.send(DaemonEvent::ClearHistory);
            } else if let Some(index) = id
                .strip_prefix(HISTORY_ID_PREFIX)
                .and_then(|s| s.parse().ok())
            {
                let _ = tx.send(DaemonEvent::ShowHistory(index));
            }
        }
    });
}

fn build_menu(history: &[HistoryEntry]) -> anyhow::Result<Menu> {
    let menu = Menu::new();
    menu.append(&MenuItem::with_id(CAPTURE_ID, "Capture region", true, None))?;

    let history_menu = Submenu::new("History", true);
    if history.is_empty() {
        history_menu.append(&MenuItem::new("No history yet", false, None))?;
    } else {
        for (i, entry) in history.iter().enumerate() {
            let id = format!("{HISTORY_ID_PREFIX}{i}");
            history_menu.append(&MenuItem::with_id(id, snippet(&entry.original), true, None))?;
        }
        history_menu.append(&PredefinedMenuItem::separator())?;
        history_menu.append(&MenuItem::with_id(
            CLEAR_HISTORY_ID,
            "Clear History",
            true,
            None,
        ))?;
    }
    menu.append(&history_menu)?;

    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&MenuItem::with_id(QUIT_ID, "Quit", true, None))?;
    Ok(menu)
}

/// A one-line, single-space-collapsed, length-capped label for a menu item —
/// OCR text is often multi-line and menu labels shouldn't be.
fn snippet(text: &str) -> String {
    const MAX_CHARS: usize = 40;
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > MAX_CHARS {
        format!(
            "{}...",
            flat.chars().take(MAX_CHARS - 3).collect::<String>()
        )
    } else if flat.is_empty() {
        "(no text)".to_string()
    } else {
        flat
    }
}

/// A cheap fingerprint of "what's currently in the history list", so the
/// menu is only rebuilt when it actually changed.
fn fingerprint(history: &[HistoryEntry]) -> String {
    let mut s = format!("{}", history.len());
    if let Some(first) = history.first() {
        s.push('|');
        s.push_str(&first.timestamp);
    }
    s
}

fn tray_menu_entries(cfg: &RwLock<AppConfig>) -> usize {
    cfg.read().unwrap().history.tray_menu_entries
}

#[cfg(target_os = "linux")]
fn run_gtk_tray(cfg: Arc<RwLock<AppConfig>>) -> anyhow::Result<()> {
    gtk::init()?;

    let initial = crate::history::load_recent(tray_menu_entries(&cfg)).unwrap_or_default();
    let mut last_fingerprint = fingerprint(&initial);
    let menu = build_menu(&initial)?;

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("ocr-translate")
        .with_icon(crate::icon::tray_icon(64)?)
        .build()?;

    gtk::glib::source::timeout_add_seconds_local(2, move || {
        match crate::history::load_recent(tray_menu_entries(&cfg)) {
            Ok(entries) => {
                let current = fingerprint(&entries);
                if current != last_fingerprint {
                    match build_menu(&entries) {
                        Ok(menu) => tray.set_menu(Some(Box::new(menu))),
                        Err(e) => tracing::debug!("failed to rebuild tray history menu: {e:#}"),
                    }
                    last_fingerprint = current;
                }
            }
            Err(e) => tracing::debug!("failed to read history for the tray menu: {e:#}"),
        }
        gtk::glib::ControlFlow::Continue
    });

    gtk::main();
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn run_native_tray(cfg: Arc<RwLock<AppConfig>>) -> anyhow::Result<()> {
    let initial = crate::history::load_recent(tray_menu_entries(&cfg)).unwrap_or_default();
    let mut last_fingerprint = fingerprint(&initial);
    let menu = build_menu(&initial)?;

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("ocr-translate")
        .with_icon(crate::icon::tray_icon(64)?)
        .build()?;

    loop {
        std::thread::sleep(std::time::Duration::from_secs(2));
        match crate::history::load_recent(tray_menu_entries(&cfg)) {
            Ok(entries) => {
                let current = fingerprint(&entries);
                if current != last_fingerprint {
                    if let Ok(menu) = build_menu(&entries) {
                        tray.set_menu(Some(Box::new(menu)));
                    }
                    last_fingerprint = current;
                }
            }
            Err(e) => tracing::debug!("failed to read history for the tray menu: {e:#}"),
        }
    }
}
