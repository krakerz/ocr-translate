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
    pub hotkey: HotkeyConfig,
    pub popup: PopupConfig,
    pub prompt: PromptConfig,
    pub active_provider: String,
    pub providers: HashMap<String, ProviderConfig>,
}

impl Default for AppConfig {
    fn default() -> Self {
        let mut providers = HashMap::new();
        providers.insert(
            "lmstudio".to_string(),
            ProviderConfig {
                kind: ProviderKind::OpenAiCompatible,
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
            hotkey: HotkeyConfig::default(),
            popup: PopupConfig::default(),
            prompt: PromptConfig::default(),
            active_provider: "lmstudio".to_string(),
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

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct HotkeyConfig {
    /// Human-readable accelerator requested from the desktop portal / X11 grab,
    /// e.g. "CTRL+ALT+O". Wayland compositors without portal shortcut support
    /// will ignore this — bind `ocr-translate capture` manually instead.
    pub capture_region: String,
    pub enable_portal: bool,
    pub enable_x11: bool,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            capture_region: "CTRL+ALT+O".to_string(),
            enable_portal: true,
            enable_x11: true,
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

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    pub base_url: Option<String>,
    /// Literal API key. Prefer `api_key_env` to avoid storing secrets in the config file.
    pub api_key: Option<String>,
    /// Name of an environment variable to read the API key from at startup.
    pub api_key_env: Option<String>,
    pub model: Option<String>,
    /// Azure region, required by Bing/Azure Translator.
    pub region: Option<String>,
    pub timeout_secs: u64,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            kind: ProviderKind::OpenAiCompatible,
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

/// Loads config from an explicit path if given, otherwise from
/// `config.yaml`/`config.yml`/`config.conf` in [`app_config_dir`]. If none of
/// those exist yet (first run), the directory and a starter `config.yaml`
/// (plus reference `config.example.yaml`/`config.example.conf` copies) are
/// created automatically, and the new `config.yaml` is loaded.
pub fn load(explicit_path: Option<&Path>) -> Result<AppConfig> {
    if let Some(path) = explicit_path {
        return load_file(path)
            .with_context(|| format!("failed to load config from {}", path.display()));
    }

    let dir =
        app_config_dir().context("could not determine a config directory for this platform")?;
    for name in ["config.yaml", "config.yml", "config.conf"] {
        let path = dir.join(name);
        if path.is_file() {
            return load_file(&path)
                .with_context(|| format!("failed to load config from {}", path.display()));
        }
    }

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
/// `[hotkey]`, `[popup]`, `[prompt]`) map onto the matching struct fields; any
/// section named `[provider.<name>]` becomes an entry in `providers`, e.g.:
///
/// ```ini
/// active_provider = openai
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
            "hotkey" => {
                if let Some(v) = get("capture_region") {
                    cfg.hotkey.capture_region = v;
                }
                cfg.hotkey.enable_portal = get_bool("enable_portal", cfg.hotkey.enable_portal);
                cfg.hotkey.enable_x11 = get_bool("enable_x11", cfg.hotkey.enable_x11);
            }
            "popup" => {
                cfg.popup.width = get_f32("width", cfg.popup.width);
                cfg.popup.height = get_f32("height", cfg.popup.height);
                cfg.popup.font_size = get_f32("font_size", cfg.popup.font_size);
                cfg.popup.always_on_top = get_bool("always_on_top", cfg.popup.always_on_top);
                cfg.popup.auto_close_secs = get_u64("auto_close_secs", cfg.popup.auto_close_secs);
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
                let provider = ProviderConfig {
                    kind,
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
