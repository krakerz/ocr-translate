use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// A command sent from a fresh, one-shot CLI invocation (`region-capture`,
/// `region-show`, `region-delete`, `region-rename` — see `main.rs`) to an
/// already-running `watch-region` session (`live_region.rs`), so those
/// actions can be bound to a hotkey the same way `capture` already is,
/// without needing to be run from inside the session's own window.
///
/// Delivered as one JSON line appended to a shared "inbox" file in the
/// per-user config directory; the running session's window polls (and
/// truncates) it every frame (`LiveRegionApp::update`, ~5x/sec, matching
/// its own repaint cadence) — near-instant in practice, but fire-and-forget:
/// the sending process gets no confirmation the command was actually
/// applied, same as a hotkey press invoking `capture` today gives no
/// feedback back to the key itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RegionCommand {
    /// Do a one-shot capture → OCR → translate cycle (identical pipeline to
    /// the standalone `capture` command, including recording to history),
    /// shown in the running window's own "Quick Capture" slot instead of a
    /// separate popup.
    QuickCapture,
    /// Show a read-only preview of every currently-watched region.
    ShowRegions,
    /// Stop watching a region, by its stable id (the number shown as
    /// "Region <id>" in the window).
    Delete { id: usize },
    /// Give a region a custom display name.
    Rename { id: usize, name: String },
}

fn inbox_path() -> Result<PathBuf> {
    let dir = crate::config::app_config_dir()
        .context("could not determine a config directory for this platform")?;
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create config directory {}", dir.display()))?;
    Ok(dir.join("region_inbox.jsonl"))
}

/// Queues a command for the running Live Region Translate session. Errors
/// (rather than silently queuing something nobody will ever read) if no
/// session appears to be active, checked via `session_lock`'s `region`
/// lock — the same check `capture`/`watch-clipboard` already use to detect
/// a running session.
pub fn send(command: RegionCommand) -> Result<()> {
    let lock = crate::session_lock::SessionLock::open("region")?;
    if !lock.is_active() {
        anyhow::bail!(
            "Live Region Translate isn't running — start it from the tray or `ocr-translate watch-region` first"
        );
    }

    let path = inbox_path()?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    let line = serde_json::to_string(&command).context("failed to serialize region command")?;
    writeln!(file, "{line}").context("failed to write region command")?;
    Ok(())
}

/// Reads and clears every pending command — called once per frame by the
/// running session. Best-effort: a line that fails to parse (e.g. written
/// by a future version with a command this build doesn't know) is logged
/// and skipped rather than treated as fatal for the whole session. A
/// command queued in the narrow window between this reading the file and
/// truncating it is lost, not duplicated — an acceptable trade-off for a
/// fire-and-forget hotkey action, not something requiring guaranteed
/// delivery.
pub fn drain() -> Vec<RegionCommand> {
    let Ok(path) = inbox_path() else {
        return Vec::new();
    };
    let Ok(contents) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    if contents.is_empty() {
        return Vec::new();
    }
    let _ = fs::write(&path, "");
    contents
        .lines()
        .filter_map(|line| match serde_json::from_str(line) {
            Ok(cmd) => Some(cmd),
            Err(e) => {
                tracing::warn!("failed to parse a queued region command, skipping: {e:#}");
                None
            }
        })
        .collect()
}
