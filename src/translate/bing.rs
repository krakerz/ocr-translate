use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use super::{TranslateRequest, Translator};

/// Microsoft/Azure Translator ("Bing Translator" API), v3.0 REST endpoint.
pub struct BingTranslate {
    pub api_key: String,
    /// Required when the key comes from a multi-service Azure resource.
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
            .header("Ocp-Apim-Subscription-Key", &self.api_key)
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
        Ok(parsed
            .into_iter()
            .next()
            .and_then(|item| item.translations.into_iter().next())
            .map(|t| t.text)
            .unwrap_or_default())
    }
}
