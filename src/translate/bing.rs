use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::ProviderMode;

use super::{TranslateRequest, Translator};

/// Microsoft/Azure Translator ("Bing Translator" API), v3.0 REST endpoint.
pub struct BingTranslate {
    pub mode: ProviderMode,
    pub api_key: Option<String>,
    /// Required when the key comes from a multi-service Azure resource. Only used in private mode.
    pub region: Option<String>,
    pub timeout_secs: u64,
}

#[derive(Serialize)]
struct BingBody<'a> {
    #[serde(rename = "Text")]
    text: &'a str,
}

#[derive(Deserialize)]
struct BingResponseItem {
    translations: Vec<BingTranslation>,
}

#[derive(Deserialize)]
struct BingTranslation {
    text: String,
}

impl Translator for BingTranslate {
    fn translate(&self, req: TranslateRequest) -> Result<String> {
        match self.mode {
            ProviderMode::Public => self.translate_public(req),
            ProviderMode::Private => self.translate_private(req),
        }
    }
}

impl BingTranslate {
    /// Unlike Google, Bing/Azure Translator has no reliable free public
    /// endpoint offered here. The old trick (a short-lived token from
    /// `edge.microsoft.com/translate/auth`) is dead — that route now 404s.
    /// The current bing.com/translator frontend instead calls an internal
    /// `www.bing.com/ttranslatev3` endpoint using tokens scraped out of the
    /// page HTML, and it's actively guarded by an abuse-prevention check
    /// (`ShowCaptcha`) that requires replaying the page-load session's
    /// cookies to pass — confirmed by testing. That's session spoofing to
    /// dodge bot detection, not a stable public API, so it isn't implemented
    /// here; use `mode: private` with an Azure key instead.
    fn translate_public(&self, _req: TranslateRequest) -> Result<String> {
        bail!(
            "Bing/Azure Translator has no reliable free public endpoint (unlike Google); \
             set mode: private with api_key/api_key_env (and region, if needed) instead"
        );
    }

    /// Microsoft/Azure Translator ("Bing Translator" API), v3.0 REST endpoint.
    fn translate_private(&self, req: TranslateRequest) -> Result<String> {
        let Some(api_key) = &self.api_key else {
            bail!(
                "Bing/Azure Translator in private mode requires api_key or api_key_env \
                 (or set mode: public to use the free endpoint instead)"
            );
        };

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(self.timeout_secs))
            .build()?;

        let mut query = vec![("api-version", "3.0"), ("to", req.target_lang)];
        if req.source_lang != "auto" {
            query.push(("from", req.source_lang));
        }

        let mut builder = client
            .post("https://api.cognitive.microsofttranslator.com/translate")
            .query(&query)
            .header("Ocp-Apim-Subscription-Key", api_key)
            .json(&[BingBody { text: req.text }]);
        if let Some(region) = &self.region {
            builder = builder.header("Ocp-Apim-Subscription-Region", region);
        }

        let resp = builder
            .send()
            .context("request to Bing/Azure Translator failed")?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().unwrap_or_default();
            bail!("Bing/Azure Translator returned HTTP {status}: {text}");
        }
        let parsed: Vec<BingResponseItem> = resp
            .json()
            .context("unexpected response shape from Bing Translator")?;
        Ok(extract_translation(parsed))
    }
}

fn extract_translation(parsed: Vec<BingResponseItem>) -> String {
    parsed
        .into_iter()
        .next()
        .and_then(|item| item.translations.into_iter().next())
        .map(|t| t.text)
        .unwrap_or_default()
}
