mod capture;
mod config;
mod daemon;
mod hotkey;
mod icon;
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

    let cfg = config::load(cli.config.as_deref())?;

    match cli.command.unwrap_or(Command::Run) {
        Command::Run => daemon::run(cfg, cli.config),
        Command::Capture => run_capture_cycle(&cfg),
        Command::TestProvider { provider, text } => test_provider(&cfg, provider, &text),
        Command::InitConfig { .. } => unreachable!("handled above"),
    }
}

/// Full pipeline: grab the active monitor, let the user crop a region, OCR it,
/// send it to the configured translation provider, then show the result in a
/// popup. Any failure is also surfaced in a popup window, since this typically
/// runs with no attached terminal (triggered by a hotkey or the tray menu).
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
    tracing::info!("capturing the active monitor...");
    let full = capture::grab_active_monitor()?;

    let Some(cropped) = capture::select_crop(&full)? else {
        tracing::info!("selection cancelled");
        return Ok(());
    };

    tracing::info!("running OCR...");
    let text = ocr::recognize(&cropped, &cfg.ocr)?;
    if text.trim().is_empty() {
        bail!("no text detected in the selected region");
    }
    tracing::info!("OCR text: {text}");

    tracing::info!("translating via '{}'...", cfg.active_provider);
    let translator = translate::build(cfg)?;
    let translated = translator.translate(translate::TranslateRequest {
        text: &text,
        source_lang: &cfg.general.source_lang,
        target_lang: &cfg.general.target_lang,
    })?;

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
