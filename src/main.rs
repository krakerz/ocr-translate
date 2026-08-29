// Suppresses the console window Windows would otherwise auto-allocate when
// this exe is double-clicked (or launched by a hotkey/tray with no parent
// console) — matches how a tray-resident GUI app is expected to behave.
// Doesn't affect stdout when run from an existing terminal: subsystem only
// controls whether Windows *creates* a new console at startup, not whether
// an inherited stdout handle can be written to, so `test-provider`,
// `init-config`, etc. still print normally when run interactively (`cmd.exe`
// specifically won't block waiting for a GUI-subsystem child the way it does
// for a console one — not a concern for this project's actual invocation
// paths: hotkeys/the tray call the exe directly, not through cmd.exe).
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod capture;
mod config;
mod daemon;
mod fonts;
mod history;
mod icon;
mod live_region;
mod live_translate;
mod ocr;
mod popup;
mod region_ipc;
mod session_lock;
mod translate;
mod tray;

use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};

use config::AppConfig;

#[derive(Parser)]
#[command(
    name = "ocr-translate",
    version,
    about = "Screen OCR + LLM/translation-API popup tool"
)]
struct Cli {
    /// Path to a config file (.yaml or .conf). Defaults to the per-user
    /// ocr-translation config directory (see `--help` on `init-config`).
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run in the system tray, listening for the configured hotkey (default).
    Run,
    /// Perform one capture -> OCR -> translate -> popup cycle and exit.
    /// Bind this to a custom shortcut in your compositor/WM if the built-in
    /// hotkey listener doesn't fire on your desktop (common on Sway, Hyprland, i3).
    Capture,
    /// Translate the given provider against a quick test string, to sanity-check
    /// API keys / base URLs without doing a screen capture.
    TestProvider {
        /// Provider name from the config's [providers] table; defaults to active_provider.
        #[arg(long)]
        provider: Option<String>,
        #[arg(default_value = "Hello, world!")]
        text: String,
    },
    /// Show one history entry (0 = most recent) in the popup window. Used
    /// internally by the tray's History submenu.
    ShowHistory { index: usize },
    /// Delete all saved history.
    ClearHistory,
    /// Watch the clipboard and show a live-updating translation popup: copy
    /// something, see it translated; copy something else, it updates in
    /// place. Never recorded to history.
    WatchClipboard,
    /// Pick a fixed screen region (via a PipeWire ScreenCast session) and
    /// show a live-updating translation popup: OCRs the region repeatedly
    /// and re-translates whenever the recognized text changes. Never
    /// recorded to history.
    WatchRegion,
    /// Do a one-shot capture inside an already-running Live Region
    /// Translate session — same pipeline as `capture` (including history),
    /// but shown in that session's own window instead of a separate popup.
    /// Bind this to a key the same way as `capture`. Errors if Live Region
    /// Translate isn't currently running.
    RegionCapture,
    /// Show a read-only preview of every region an already-running Live
    /// Region Translate session is currently watching. Errors if it isn't
    /// currently running.
    RegionShow,
    /// Stop watching a region in an already-running Live Region Translate
    /// session, by its id (the number shown as "Region <id>" in its
    /// window). Errors if it isn't currently running.
    RegionDelete { id: usize },
    /// Rename a region in an already-running Live Region Translate session.
    /// Errors if it isn't currently running.
    RegionRename { id: usize, name: String },
    /// Reset the config file(s) in the default config directory to the
    /// bundled example. Config is created automatically on first run, so
    /// this is only needed to restore defaults.
    InitConfig {
        /// "yaml" or "conf". Defaults to yaml.
        #[arg(long, default_value = "yaml")]
        format: String,
        #[arg(long)]
        force: bool,
    },
}

/// Sets up `tracing` output. On Windows, this exe is a GUI-subsystem app
/// with no console (see the `windows_subsystem` attribute above), so
/// stdout log output is completely invisible unless launched from an
/// existing terminal — no help at all for diagnosing an issue on a real
/// end user's machine. Logs go to `ocr-translate.log` in the same per-user
/// config directory as `config.yaml`/`history.jsonl` instead, opened in
/// append mode so a chronological history builds up across every
/// capture/tray/etc. process (each subcommand is its own process, see
/// `daemon::spawn_subcommand`) rather than each one truncating the last
/// run's log. Falls back to stdout if the config directory can't be
/// determined or the log file can't be opened, rather than failing to
/// start over a logging problem. Linux/other platforms keep the original
/// stdout-only behavior — a terminal is already available there.
#[cfg(target_os = "windows")]
fn init_logging() {
    let filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    let log_file = config::app_config_dir().and_then(|dir| {
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("ocr-translate.log"))
            .ok()
    });
    match log_file {
        Some(file) => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(file))
            .init(),
        None => tracing_subscriber::fmt().with_env_filter(filter).init(),
    }
}

#[cfg(not(target_os = "windows"))]
fn init_logging() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
}

fn main() -> Result<()> {
    init_logging();

    let cli = Cli::parse();

    if let Some(Command::InitConfig { format, force }) = &cli.command {
        return config::reset_default_config(format, *force);
    }
    if let Some(Command::ClearHistory) = &cli.command {
        return history::clear();
    }

    let cfg = config::load(cli.config.as_deref())?;

    match cli.command.unwrap_or(Command::Run) {
        Command::Run => daemon::run(cfg, cli.config),
        Command::Capture => run_capture_cycle(&cfg),
        Command::TestProvider { provider, text } => test_provider(&cfg, provider, &text),
        Command::ShowHistory { index } => show_history(&cfg, index),
        Command::WatchClipboard => live_translate::run(&cfg),
        Command::WatchRegion => live_region::run(&cfg),
        Command::RegionCapture => send_region_command(region_ipc::RegionCommand::QuickCapture),
        Command::RegionShow => send_region_command(region_ipc::RegionCommand::ShowRegions),
        Command::RegionDelete { id } => {
            send_region_command(region_ipc::RegionCommand::Delete { id })
        }
        Command::RegionRename { id, name } => {
            send_region_command(region_ipc::RegionCommand::Rename { id, name })
        }
        Command::ClearHistory | Command::InitConfig { .. } => unreachable!("handled above"),
    }
}

/// Full pipeline: acquire a screen region (see `capture::acquire`), OCR it,
/// send it to the configured translation provider (with fallback), then show
/// the result in a popup and record it to history. Any failure is also
/// surfaced in a popup window, since this typically runs with no attached
/// terminal (triggered by a hotkey or the tray menu).
pub fn run_capture_cycle(cfg: &AppConfig) -> Result<()> {
    match run_capture_cycle_inner(cfg) {
        Ok(()) => Ok(()),
        Err(e) => {
            tracing::error!("{e:#}");
            let _ = popup::show_error(&format!("{e:#}"));
            Err(e)
        }
    }
}

fn run_capture_cycle_inner(cfg: &AppConfig) -> Result<()> {
    // Live Region Translate holds an active screen-capture session (DXGI
    // Desktop Duplication on Windows, a PipeWire ScreenCast stream on
    // Linux) that a one-shot screenshot can contend with — see
    // session_lock.rs. Bound to `_`, not a named variable: this is a
    // momentary check, not something `capture` holds for its own lifetime,
    // so the lock is released again immediately after this line. Not part
    // of `run_capture_pipeline` below: that's also called by Live Region
    // Translate's own "Quick Capture" (`live_region.rs`), which is already
    // holding this exact lock itself at that point — checking again there
    // would see its own lock as "held" and absurdly offer to stop itself.
    let mut region_lock = session_lock::SessionLock::open("region")?;
    let Some(_) =
        session_lock::resolve_conflict(&mut region_lock, "capture", "Live Region Translate")?
    else {
        tracing::info!("cancelled: Live Region Translate is active");
        return Ok(());
    };

    let Some((text, translated, provider_used)) = run_capture_pipeline(cfg)? else {
        return Ok(());
    };
    popup::show_result(&text, &translated, &provider_used, &cfg.translate)?;
    Ok(())
}

/// Capture → crop → OCR → translate → record to history, exactly once,
/// returning the recognized text, its translation, and which provider
/// translated it — or `None` if the user cancelled the region selection.
/// Shared by the standalone `capture` command (`run_capture_cycle_inner`,
/// which also shows a popup and handles the region-lock conflict check) and
/// Live Region Translate's "Quick Capture" button/CLI command
/// (`region-capture`, see `live_region.rs`), which shows the result inline
/// in its own window instead of a separate popup — using this same
/// function (not a re-implementation) is what makes Quick Capture
/// genuinely identical to a manual `capture`, history recording included.
pub fn run_capture_pipeline(cfg: &AppConfig) -> Result<Option<(String, String, String)>> {
    tracing::info!("capturing...");
    let Some(cropped) = capture::acquire(&cfg.capture, &cfg.popup)? else {
        tracing::info!("selection cancelled");
        return Ok(None);
    };

    tracing::info!("running OCR...");
    let text = ocr::recognize(&cropped, &cfg.ocr)?;
    if text.trim().is_empty() {
        bail!("no text detected in the selected region");
    }
    tracing::info!("OCR text: {text}");

    tracing::info!(
        "translating (active provider: '{}')...",
        cfg.active_provider
    );
    let (provider_used, translated) = translate::translate_with_fallback(
        cfg,
        translate::TranslateRequest {
            text: &text,
            source_lang: &cfg.general.source_lang,
            target_lang: &cfg.general.target_lang,
        },
    )?;
    tracing::info!("translated via '{provider_used}'");

    if cfg.history.enabled {
        let entry = history::HistoryEntry {
            timestamp: chrono::Local::now().to_rfc3339(),
            provider: provider_used.clone(),
            source_lang: cfg.general.source_lang.clone(),
            target_lang: cfg.general.target_lang.clone(),
            original: text.clone(),
            translated: translated.clone(),
        };
        if let Err(e) = history::append(&entry, cfg.history.max_entries) {
            tracing::warn!("failed to record history: {e:#}");
        }
    }

    Ok(Some((text, translated, provider_used)))
}

/// `region-capture`/`region-show`/`region-delete`/`region-rename` are meant
/// to be bound to a hotkey the same way `capture` is (see
/// `region_ipc::RegionCommand`'s doc comment) — no attached terminal to see
/// an error on, so a failure (most commonly: Live Region Translate isn't
/// running) is also surfaced as a popup, same as `run_capture_cycle` does.
fn send_region_command(command: region_ipc::RegionCommand) -> Result<()> {
    match region_ipc::send(command) {
        Ok(()) => Ok(()),
        Err(e) => {
            tracing::error!("{e:#}");
            let _ = popup::show_error(&format!("{e:#}"));
            Err(e)
        }
    }
}

fn test_provider(cfg: &AppConfig, provider: Option<String>, text: &str) -> Result<()> {
    let translator = match provider {
        Some(name) => translate::build_named(&name, cfg)?,
        None => translate::build(cfg)?,
    };
    let translated = translator.translate(translate::TranslateRequest {
        text,
        source_lang: &cfg.general.source_lang,
        target_lang: &cfg.general.target_lang,
    })?;
    println!("{translated}");
    Ok(())
}

fn show_history(cfg: &AppConfig, index: usize) -> Result<()> {
    let Some(entry) = history::get(index)? else {
        bail!("no history entry at index {index}");
    };
    popup::show_result(
        &entry.original,
        &entry.translated,
        &entry.provider,
        &cfg.history_popup,
    )?;
    Ok(())
}
