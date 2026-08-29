mod capture;
mod config;
mod daemon;
mod history;
mod hotkey;
mod icon;
mod live_translate;
mod ocr;
mod popup;
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

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

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
    tracing::info!("capturing...");
    let Some(cropped) = capture::acquire(&cfg.capture)? else {
        tracing::info!("selection cancelled");
        return Ok(());
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
            provider: provider_used,
            source_lang: cfg.general.source_lang.clone(),
            target_lang: cfg.general.target_lang.clone(),
            original: text.clone(),
            translated: translated.clone(),
        };
        if let Err(e) = history::append(&entry, cfg.history.max_entries) {
            tracing::warn!("failed to record history: {e:#}");
        }
    }

    popup::show_result(&text, &translated, &cfg.popup)?;
    Ok(())
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
    popup::show_result(&entry.original, &entry.translated, &cfg.history_popup)?;
    Ok(())
}
