use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// Folder name used under the OS's standard per-user config location:
/// `~/.config/ocr-translation` on Linux, `%APPDATA%\ocr-translation` on
/// Windows, `~/Library/Application Support/ocr-translation` on macOS.
const APP_DIR_NAME: &str = "ocr-translation";

const EXAMPLE_YAML: &str = include_str!("../config/config.example.yaml");
const EXAMPLE_CONF: &str = include_str!("../config/config.example.conf");

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub general: GeneralConfig,
    pub ocr: OcrConfig,
    pub capture: CaptureConfig,
    pub popup: PopupConfig,
    /// Sizing/behavior for the popup shown when reopening a history entry
    /// (tray History submenu / `show-history`) — separate from `popup` so it
    /// can be sized differently than the live capture-result popup.
    pub history_popup: PopupConfig,
    pub history: HistoryConfig,
    pub live_translate: LiveTranslateConfig,
    pub prompt: PromptConfig,
    pub active_provider: String,
    /// Tried, in order, if `active_provider` fails (connection error, HTTP
    /// error, missing key, ...). Empty means no fallback.
    pub fallback_providers: Vec<String>,
    pub providers: HashMap<String, ProviderConfig>,
}

impl Default for AppConfig {
    fn default() -> Self {
        let mut providers = HashMap::new();
        providers.insert(
            "lmstudio".to_string(),
            ProviderConfig {
                kind: ProviderKind::OpenAiCompatible,
                mode: ProviderMode::Private,
                base_url: Some("http://localhost:1234/v1".to_string()),
                api_key: None,
                api_key_env: None,
                model: Some("local-model".to_string()),
                region: None,
                timeout_secs: 60,
            },
        );
        Self {
            general: GeneralConfig::default(),
            ocr: OcrConfig::default(),
            capture: CaptureConfig::default(),
            popup: PopupConfig::default(),
            history_popup: PopupConfig::default(),
            history: HistoryConfig::default(),
            live_translate: LiveTranslateConfig::default(),
            prompt: PromptConfig::default(),
            active_provider: "lmstudio".to_string(),
            fallback_providers: Vec::new(),
            providers,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    pub target_lang: String,
    pub source_lang: String,
    pub log_level: String,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            target_lang: "en".to_string(),
            source_lang: "auto".to_string(),
            log_level: "info".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct OcrConfig {
    /// Tesseract language codes, e.g. "eng", "eng+jpn"
    pub languages: String,
    pub tessdata_dir: Option<String>,
    /// Tesseract page segmentation mode (see `tesseract --help-psm`)
    pub psm: Option<i32>,
    /// Grayscale + threshold the crop before OCR to improve accuracy on UI text.
    pub preprocess: bool,
}

impl Default for OcrConfig {
    fn default() -> Self {
        Self {
            languages: "eng".to_string(),
            tessdata_dir: None,
            psm: Some(6),
            preprocess: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum CaptureBackend {
    /// Grab the active monitor ourselves and crop it with our own
    /// zoom/pan/select window (`capture::grab_active_monitor` + `select_crop`).
    /// Portable across every Wayland desktop.
    #[serde(rename = "built_in")]
    BuiltIn,
    /// Run an external screenshot tool that does its own live region
    /// selection on the real desktop and leaves a PNG on the clipboard (e.g.
    /// KDE's `spectacle -r -b -c`), then read that image back via the
    /// clipboard instead of our own crop UI. Nicer where available, but
    /// depends on a specific external tool being installed, so this is opt-in
    /// rather than the default.
    #[serde(rename = "external")]
    External,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CaptureConfig {
    pub backend: CaptureBackend,
    /// Shell command run when `backend = external`. Must leave a PNG image on
    /// the clipboard when it's done (e.g. KDE's `spectacle -r -b -c`, or
    /// `grim -g "$(slurp)" - | wl-copy` on wlroots compositors).
    pub external_command: String,
    /// How long to keep polling the clipboard for an image after the command exits.
    pub external_timeout_secs: u64,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            backend: CaptureBackend::BuiltIn,
            external_command: "spectacle -r -b -c".to_string(),
            external_timeout_secs: 10,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PopupConfig {
    pub width: f32,
    pub height: f32,
    pub font_size: f32,
    pub always_on_top: bool,
    /// Auto-dismiss the popup after N seconds; 0 disables auto-close.
    pub auto_close_secs: u64,
}

impl Default for PopupConfig {
    fn default() -> Self {
        Self {
            width: 520.0,
            height: 380.0,
            font_size: 16.0,
            always_on_top: true,
            auto_close_secs: 0,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct HistoryConfig {
    /// Record each successful translation to `history.jsonl` in the config directory.
    pub enabled: bool,
    /// Oldest entries beyond this are trimmed after each capture.
    pub max_entries: usize,
    /// How many of the most recent entries to list in the tray's History submenu.
    pub tray_menu_entries: usize,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_entries: 50,
            tray_menu_entries: 5,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LiveTranslateConfig {
    pub width: f32,
    pub height: f32,
    pub font_size: f32,
    pub always_on_top: bool,
    /// Initial state of the popup's "Show source" toggle; the user can
    /// still flip it per-session, this is just the starting point.
    pub show_source_by_default: bool,
    /// How often to check whether the clipboard's text changed.
    pub poll_interval_ms: u64,
}

impl Default for LiveTranslateConfig {
    fn default() -> Self {
        Self {
            width: 480.0,
            height: 360.0,
            font_size: 16.0,
            always_on_top: true,
            show_source_by_default: true,
            poll_interval_ms: 500,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PromptConfig {
    pub system: String,
    /// Placeholders: {source_lang} {target_lang} {text}
    pub template: String,
}

impl Default for PromptConfig {
    fn default() -> Self {
        Self {
            system: "You are a precise translation engine. Translate the user's text and reply with ONLY the translation, no notes or quotes.".to_string(),
            template: "Translate the following text from {source_lang} to {target_lang}:\n\n{text}".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum ProviderKind {
    /// Any server exposing an OpenAI-style `/chat/completions` endpoint:
    /// LM Studio, Ollama (OpenAI-compat mode), OpenAI, DeepSeek, etc.
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
    #[serde(rename = "google_translate")]
    GoogleTranslate,
    #[serde(rename = "bing_translate")]
    BingTranslate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum ProviderMode {
    /// The free, unofficial, no-key web endpoint the provider's own
    /// translator page uses (only meaningful for `google_translate` /
    /// `bing_translate`; ignored otherwise). Unofficial and undocumented, so
    /// it can change or rate-limit without notice, but it's what makes
    /// zero-config translation possible out of the box.
    #[serde(rename = "public")]
    Public,
    /// The official, authenticated API — requires `api_key`/`api_key_env`
    /// (and `region` for Bing/Azure).
    #[serde(rename = "private")]
    Private,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    pub mode: ProviderMode,
    pub base_url: Option<String>,
    /// Literal API key. Prefer `api_key_env` to avoid storing secrets in the config file.
    pub api_key: Option<String>,
    /// Name of an environment variable to read the API key from at startup.
    pub api_key_env: Option<String>,
    pub model: Option<String>,
    /// Azure region, required by Bing/Azure Translator in `private` mode.
    pub region: Option<String>,
    pub timeout_secs: u64,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            kind: ProviderKind::OpenAiCompatible,
            mode: ProviderMode::Private,
            base_url: None,
            api_key: None,
            api_key_env: None,
            model: None,
            region: None,
            timeout_secs: 60,
        }
    }
}

impl ProviderConfig {
    /// A blank `api_key` (or a set-but-empty `api_key_env` variable) is
    /// treated the same as none at all — no `Authorization` header is sent.
    /// This matters for servers like LM Studio, where an API key is optional.
    pub fn resolve_api_key(&self) -> Option<String> {
        if let Some(env_name) = &self.api_key_env {
            if let Ok(v) = std::env::var(env_name) {
                if !v.is_empty() {
                    return Some(v);
                }
            }
        }
        self.api_key.clone().filter(|v| !v.is_empty())
    }
}

/// The per-user config directory: `~/.config/ocr-translation` (Linux),
/// `%APPDATA%\ocr-translation` (Windows), `~/Library/Application Support/ocr-translation` (macOS).
pub fn app_config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join(APP_DIR_NAME))
}

/// The file that [`load`] would read (without reading it) — the explicit
/// path if given, otherwise whichever of `config.yaml`/`config.yml`/
/// `config.conf` in [`app_config_dir`] exists. Used both by `load` and by the
/// daemon's config-file watcher (see `daemon::watch_config`), so both agree
/// on which file is "the" config.
pub fn resolve_path(explicit_path: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = explicit_path {
        return Some(path.to_path_buf());
    }
    let dir = app_config_dir()?;
    ["config.yaml", "config.yml", "config.conf"]
        .into_iter()
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
}

/// Loads config from an explicit path if given, otherwise from
/// `config.yaml`/`config.yml`/`config.conf` in [`app_config_dir`]. If none of
/// those exist yet (first run), the directory and a starter `config.yaml`
/// (plus reference `config.example.yaml`/`config.example.conf` copies) are
/// created automatically, and the new `config.yaml` is loaded.
pub fn load(explicit_path: Option<&Path>) -> Result<AppConfig> {
    if let Some(path) = resolve_path(explicit_path) {
        return load_file(&path)
            .with_context(|| format!("failed to load config from {}", path.display()));
    }

    let dir =
        app_config_dir().context("could not determine a config directory for this platform")?;
    let path = create_default_config(&dir)?;
    load_file(&path)
        .with_context(|| format!("failed to load newly created config at {}", path.display()))
}

/// Writes `config.yaml` plus both reference `config.example.*` files into `dir`
/// (creating it if needed) and returns the path to the usable `config.yaml`.
fn create_default_config(dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("failed to create config directory {}", dir.display()))?;

    let config_path = dir.join("config.yaml");
    std::fs::write(&config_path, EXAMPLE_YAML)?;
    std::fs::write(dir.join("config.example.yaml"), EXAMPLE_YAML)?;
    std::fs::write(dir.join("config.example.conf"), EXAMPLE_CONF)?;

    tracing::info!(
        "first run: wrote default config to {} (edit it, then point active_provider at your LLM/translation backend)",
        config_path.display()
    );
    Ok(config_path)
}

/// `init-config` CLI command: (re)writes `config.<format>` and both
/// `config.example.*` reference files in [`app_config_dir`]. Config is
/// otherwise created automatically on first run, so this is only needed to
/// restore the bundled defaults.
pub fn reset_default_config(format: &str, force: bool) -> Result<()> {
    let dir =
        app_config_dir().context("could not determine a config directory for this platform")?;
    std::fs::create_dir_all(&dir)?;

    let (path, contents) = match format {
        "yaml" | "yml" => (dir.join("config.yaml"), EXAMPLE_YAML),
        "conf" | "ini" => (dir.join("config.conf"), EXAMPLE_CONF),
        other => bail!("unknown format '{other}', expected 'yaml' or 'conf'"),
    };
    if path.exists() && !force {
        bail!(
            "{} already exists (use --force to overwrite)",
            path.display()
        );
    }
    std::fs::write(&path, contents)?;
    std::fs::write(dir.join("config.example.yaml"), EXAMPLE_YAML)?;
    std::fs::write(dir.join("config.example.conf"), EXAMPLE_CONF)?;
    println!("wrote {}", path.display());
    Ok(())
}

fn load_file(path: &Path) -> Result<AppConfig> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_lowercase();
    match ext.as_str() {
        "yaml" | "yml" => load_yaml(path),
        "conf" | "ini" => load_ini(path),
        other => bail!("unsupported config extension '{other}' (use .yaml or .conf)"),
    }
}

fn load_yaml(path: &Path) -> Result<AppConfig> {
    let raw = std::fs::read_to_string(path)?;
    let cfg: AppConfig = serde_yaml::from_str(&raw)?;
    Ok(cfg)
}

/// `.conf` files use INI syntax. Top-level scalar sections (`[general]`, `[ocr]`,
/// `[capture]`, `[popup]`, `[history_popup]`, `[history]`,
/// `[live_translate]`, `[prompt]`) map onto the matching struct fields; any
/// section named `[provider.<name>]` becomes an
/// entry in `providers`, e.g.:
///
/// ```ini
/// active_provider = openai
/// fallback_providers = google,bing
///
/// [provider.openai]
/// kind = openai_compatible
/// base_url = https://api.openai.com/v1
/// api_key_env = OPENAI_API_KEY
/// model = gpt-4o-mini
/// ```
fn load_ini(path: &Path) -> Result<AppConfig> {
    let ini =
        ini::Ini::load_from_file(path).map_err(|e| anyhow::anyhow!("invalid INI syntax: {e}"))?;
    let mut cfg = AppConfig::default();
    cfg.providers.clear();

    if let Some(root) = ini.section(None::<String>) {
        if let Some(v) = root.get("active_provider") {
            cfg.active_provider = v.to_string();
        }
        if let Some(v) = root.get("fallback_providers") {
            cfg.fallback_providers = v
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
    }

    for (section_name, props) in ini.iter() {
        let Some(name) = section_name else { continue };
        let get = |k: &str| props.get(k).map(|s| s.to_string());
        let get_bool = |k: &str, default: bool| {
            props
                .get(k)
                .and_then(|v| v.parse::<bool>().ok())
                .unwrap_or(default)
        };
        let get_f32 = |k: &str, default: f32| {
            props
                .get(k)
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(default)
        };
        let get_u64 = |k: &str, default: u64| {
            props
                .get(k)
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(default)
        };
        let get_usize = |k: &str, default: usize| {
            props
                .get(k)
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(default)
        };

        match name {
            "general" => {
                if let Some(v) = get("target_lang") {
                    cfg.general.target_lang = v;
                }
                if let Some(v) = get("source_lang") {
                    cfg.general.source_lang = v;
                }
                if let Some(v) = get("log_level") {
                    cfg.general.log_level = v;
                }
            }
            "ocr" => {
                if let Some(v) = get("languages") {
                    cfg.ocr.languages = v;
                }
                cfg.ocr.tessdata_dir = get("tessdata_dir");
                cfg.ocr.psm = props.get("psm").and_then(|v| v.parse::<i32>().ok());
                cfg.ocr.preprocess = get_bool("preprocess", cfg.ocr.preprocess);
            }
            "capture" => {
                if let Some(v) = get("backend") {
                    cfg.capture.backend = match v.as_str() {
                        "external" => CaptureBackend::External,
                        _ => CaptureBackend::BuiltIn,
                    };
                }
                if let Some(v) = get("external_command") {
                    cfg.capture.external_command = v;
                }
                cfg.capture.external_timeout_secs =
                    get_u64("external_timeout_secs", cfg.capture.external_timeout_secs);
            }
            "popup" => {
                cfg.popup.width = get_f32("width", cfg.popup.width);
                cfg.popup.height = get_f32("height", cfg.popup.height);
                cfg.popup.font_size = get_f32("font_size", cfg.popup.font_size);
                cfg.popup.always_on_top = get_bool("always_on_top", cfg.popup.always_on_top);
                cfg.popup.auto_close_secs = get_u64("auto_close_secs", cfg.popup.auto_close_secs);
            }
            "history_popup" => {
                cfg.history_popup.width = get_f32("width", cfg.history_popup.width);
                cfg.history_popup.height = get_f32("height", cfg.history_popup.height);
                cfg.history_popup.font_size = get_f32("font_size", cfg.history_popup.font_size);
                cfg.history_popup.always_on_top =
                    get_bool("always_on_top", cfg.history_popup.always_on_top);
                cfg.history_popup.auto_close_secs =
                    get_u64("auto_close_secs", cfg.history_popup.auto_close_secs);
            }
            "history" => {
                cfg.history.enabled = get_bool("enabled", cfg.history.enabled);
                cfg.history.max_entries = get_usize("max_entries", cfg.history.max_entries);
                cfg.history.tray_menu_entries =
                    get_usize("tray_menu_entries", cfg.history.tray_menu_entries);
            }
            "live_translate" => {
                cfg.live_translate.width = get_f32("width", cfg.live_translate.width);
                cfg.live_translate.height = get_f32("height", cfg.live_translate.height);
                cfg.live_translate.font_size = get_f32("font_size", cfg.live_translate.font_size);
                cfg.live_translate.always_on_top =
                    get_bool("always_on_top", cfg.live_translate.always_on_top);
                cfg.live_translate.show_source_by_default = get_bool(
                    "show_source_by_default",
                    cfg.live_translate.show_source_by_default,
                );
                cfg.live_translate.poll_interval_ms =
                    get_u64("poll_interval_ms", cfg.live_translate.poll_interval_ms);
            }
            "prompt" => {
                if let Some(v) = get("system") {
                    cfg.prompt.system = v;
                }
                if let Some(v) = get("template") {
                    cfg.prompt.template = v.replace("\\n", "\n");
                }
            }
            other if other.starts_with("provider.") => {
                let provider_name = other.trim_start_matches("provider.").to_string();
                let kind = match get("kind").as_deref() {
                    Some("google_translate") => ProviderKind::GoogleTranslate,
                    Some("bing_translate") => ProviderKind::BingTranslate,
                    _ => ProviderKind::OpenAiCompatible,
                };
                let mode = match get("mode").as_deref() {
                    Some("public") => ProviderMode::Public,
                    _ => ProviderMode::Private,
                };
                let provider = ProviderConfig {
                    kind,
                    mode,
                    base_url: get("base_url"),
                    api_key: get("api_key"),
                    api_key_env: get("api_key_env"),
                    model: get("model"),
                    region: get("region"),
                    timeout_secs: get_u64("timeout_secs", 60),
                };
                cfg.providers.insert(provider_name, provider);
            }
            _ => {}
        }
    }

    if cfg.providers.is_empty() {
        bail!("config has no [provider.<name>] sections defined");
    }
    if !cfg.providers.contains_key(&cfg.active_provider) {
        bail!(
            "active_provider '{}' has no matching [provider.{}] section",
            cfg.active_provider,
            cfg.active_provider
        );
    }
    Ok(cfg)
}
