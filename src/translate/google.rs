use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::config::ProviderMode;

use super::{TranslateRequest, Translator};

pub struct GoogleTranslate {
    pub mode: ProviderMode,
    pub api_key: Option<String>,
    pub timeout_secs: u64,
}

impl Translator for GoogleTranslate {
    fn translate(&self, req: TranslateRequest) -> Result<String> {
        match self.mode {
            ProviderMode::Public => self.translate_public(req),
            ProviderMode::Private => self.translate_private(req),
        }
    }
}

impl GoogleTranslate {
    /// The free, unofficial endpoint `translate.google.com`'s own web UI
    /// calls — no API key, no billing. Undocumented and could change or
    /// rate-limit without notice, but it's what lets this app translate out
    /// of the box with zero configuration.
    fn translate_public(&self, req: TranslateRequest) -> Result<String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(self.timeout_secs))
            .build()?;

        let resp = client
            .get("https://translate.googleapis.com/translate_a/single")
            .query(&[
                ("client", "gtx"),
                ("sl", req.source_lang),
                ("tl", req.target_lang),
                ("dt", "t"),
                ("q", req.text),
            ])
            .send()
            .context("request to the public Google Translate endpoint failed")?;

        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        if !status.is_success() {
            bail!("public Google Translate endpoint returned HTTP {status}: {body}");
        }
        parse_public_response(&body)
    }

    /// Google Cloud Translation API v2 (simple API-key auth, not the full Cloud SDK).
    fn translate_private(&self, req: TranslateRequest) -> Result<String> {
        let Some(api_key) = &self.api_key else {
            bail!(
                "Google Translate in private mode requires api_key or api_key_env \
                 (or set mode: public to use the free endpoint instead)"
            );
        };

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(self.timeout_secs))
            .build()?;

        let mut form = vec![
            ("q", req.text),
            ("target", req.target_lang),
            ("format", "text"),
        ];
        if req.source_lang != "auto" {
            form.push(("source", req.source_lang));
        }

        let resp = client
            .post("https://translation.googleapis.com/language/translate/v2")
            .query(&[("key", api_key.as_str())])
            .form(&form)
            .send()
            .context("request to Google Translate failed")?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().unwrap_or_default();
            bail!("Google Translate returned HTTP {status}: {text}");
        }
        let parsed: GoogleResponse = resp
            .json()
            .context("unexpected response shape from Google Translate")?;
        Ok(parsed
            .data
            .translations
            .into_iter()
            .next()
            .map(|t| t.translated_text)
            .unwrap_or_default())
    }
}

/// The public endpoint replies with a loosely-typed nested JSON array (not a
/// stable documented schema), roughly `[[[translated, original, ...], ...], ...]`
/// — parsed as untyped `Value` and concatenated rather than fought with typed structs.
fn parse_public_response(body: &str) -> Result<String> {
    let value: serde_json::Value = serde_json::from_str(body)
        .context("unexpected response from the public Google Translate endpoint")?;
    let segments = value
        .get(0)
        .and_then(|v| v.as_array())
        .context("unexpected response shape from the public Google Translate endpoint")?;

    let mut out = String::new();
    for segment in segments {
        if let Some(s) = segment.get(0).and_then(|v| v.as_str()) {
            out.push_str(s);
        }
    }
    Ok(out)
}

#[derive(Deserialize)]
struct GoogleResponse {
    data: GoogleData,
}

#[derive(Deserialize)]
struct GoogleData {
    translations: Vec<GoogleTranslation>,
}

#[derive(Deserialize)]
struct GoogleTranslation {
    #[serde(rename = "translatedText")]
    translated_text: String,
}
