use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::json;

use crate::config::ProviderMode;

use super::{TranslateRequest, Translator};

pub struct DeepLTranslate {
    pub mode: ProviderMode,
    pub api_key: Option<String>,
    pub timeout_secs: u64,
}

impl Translator for DeepLTranslate {
    fn translate(&self, req: TranslateRequest) -> Result<String> {
        match self.mode {
            ProviderMode::Public => self.translate_public(req),
            ProviderMode::Private => self.translate_private(req),
        }
    }
}

impl DeepLTranslate {
    /// The free, unofficial "oneshot" endpoint DeepL's own Chrome extension
    /// and iOS app use for stateless single-shot translation
    /// (`POST https://oneshot-free.www.deepl.com/v1/translate`).
    ///
    /// DeepL retired the legacy JSON-RPC backend (`LMT_handle_jobs` on
    /// `www2.deepl.com/jsonrpc`) for the web translator — it now sits behind
    /// an aggressive per-IP WAF returning HTTP 429 (code 1042911) within a
    /// handful of calls, which surfaced as "You're using an older version of
    /// DeepL" in this app. The oneshot endpoint lives on a separate rate-limit
    /// pool and accepts anonymous traffic with a literal `Authorization: None`
    /// header.
    ///
    /// Unlike the old JSON-RPC path it is stateless: no session priming
    /// (cookies + `getClientState`), no timestamp/i-count abuse-check trick,
    /// no running request id. Language detection is server-side.
    fn translate_public(&self, req: TranslateRequest) -> Result<String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .user_agent(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36",
            )
            .build()
            .context("failed to build an HTTP client for DeepL Translator")?;

        let body = json!({
            "text": [req.text],
            "target_lang": to_oneshot_lang(req.target_lang),
            "source_lang": to_oneshot_lang(req.source_lang),
            "usage_type": "Translate",
        });

        let resp = client
            .post("https://oneshot-free.www.deepl.com/v1/translate")
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "*/*")
            .header("Authorization", "None")
            .header(
                "Origin",
                "chrome-extension://cofdbpoegempjloogbagkncekinflcnj",
            )
            .header("Sec-Fetch-Site", "cross-site")
            .header("Sec-Fetch-Mode", "cors")
            .header("Sec-Fetch-Dest", "empty")
            .json(&body)
            .send()
            .context("request to the public DeepL Translator endpoint failed")?;

        let status = resp.status();
        let body_text = resp.text().unwrap_or_default();
        if status.as_u16() == 429 {
            bail!("public DeepL Translator endpoint returned HTTP 429 (rate-limited): {body_text}");
        }
        if !status.is_success() {
            bail!("public DeepL Translator endpoint returned HTTP {status}: {body_text}");
        }
        parse_oneshot_response(&body_text)
    }

    /// DeepL's official API (Free or Pro tier, both use the same request
    /// shape — only the base URL and quota differ).
    fn translate_private(&self, req: TranslateRequest) -> Result<String> {
        let Some(api_key) = &self.api_key else {
            bail!(
                "DeepL Translator in private mode requires api_key or api_key_env \
                 (or set mode: public to use the free endpoint instead)"
            );
        };
        // DeepL API Free keys are suffixed ":fx" and must use the free-tier
        // host; Pro keys use the regular host.
        let base = if api_key.ends_with(":fx") {
            "https://api-free.deepl.com"
        } else {
            "https://api.deepl.com"
        };

        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .build()?;

        let mut form = vec![("text", req.text), ("target_lang", req.target_lang)];
        if req.source_lang != "auto" {
            form.push(("source_lang", req.source_lang));
        }

        let resp = client
            .post(format!("{base}/v2/translate"))
            .header("Authorization", format!("DeepL-Auth-Key {api_key}"))
            .form(&form)
            .send()
            .context("request to DeepL Translator failed")?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().unwrap_or_default();
            bail!("DeepL Translator returned HTTP {status}: {text}");
        }
        let parsed: DeepLResponse = resp
            .json()
            .context("unexpected response shape from DeepL Translator")?;
        Ok(parsed
            .translations
            .into_iter()
            .next()
            .map(|t| t.text)
            .unwrap_or_default())
    }
}

/// DeepL's free oneshot endpoint uses lowercase BCP-47-ish language codes
/// (de, en-US, zh-Hans, ...) while DeepL's own codes and our config use
/// uppercase forms (DE, EN, ZH, ...). Known codes are mapped; unknown codes
/// fall through lowercased so new languages work without a code change.
fn to_oneshot_lang(lang: &str) -> String {
    match lang {
        "auto" => "auto".to_string(),
        "zh-Hans" | "zh-CN" | "ZH" => "zh-hans".to_string(),
        "zh-Hant" | "ZH-HANT" => "zh-hant".to_string(),
        other => other.to_lowercase(),
    }
}

/// The response shape from `/v1/translate`:
/// `{"translations":[{"detected_source_language":"en","text":"..."}]}`.
fn parse_oneshot_response(body: &str) -> Result<String> {
    let value: serde_json::Value = serde_json::from_str(body)
        .context("unexpected response from the public DeepL Translator endpoint")?;
    value
        .pointer("/translations/0/text")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .context("unexpected response shape from the public DeepL Translator endpoint")
}

#[derive(serde::Deserialize)]
struct DeepLResponse {
    translations: Vec<DeepLTranslation>,
}

#[derive(serde::Deserialize)]
struct DeepLTranslation {
    text: String,
}
