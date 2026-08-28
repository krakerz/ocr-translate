use anyhow::{bail, Context, Result};
use serde::Deserialize;

use super::{TranslateRequest, Translator};

/// Google Cloud Translation API v2 (simple API-key auth, not the full Cloud SDK).
pub struct GoogleTranslate {
    pub api_key: Option<String>,
    pub timeout_secs: u64,
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

impl Translator for GoogleTranslate {
    fn translate(&self, req: TranslateRequest) -> Result<String> {
        let Some(api_key) = &self.api_key else {
            bail!("Google Translate requires api_key or api_key_env");
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
