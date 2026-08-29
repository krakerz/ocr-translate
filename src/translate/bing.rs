use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::ProviderMode;

use super::session::SessionCounter;
use super::{TranslateRequest, Translator};

/// Microsoft/Azure Translator ("Bing Translator" API), v3.0 REST endpoint.
pub struct BingTranslate {
    pub name: String,
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

/// A cached `bing.com/translator` session for one provider entry: the `IG`
/// and `IID` values the page embeds in its HTML (read by its own JS to call
/// the internal `ttranslatev3` endpoint), the `token`/`key` pair its
/// `AbusePreventionHelper` embeds and appends to every translate request,
/// plus the cookies that page load set — all required for the endpoint to
/// accept the request instead of rejecting it with `{"statusCode":205}`
/// ("stale session, reload the page"). See [`refresh`].
///
/// The `token`/`key` requirement isn't implemented by
/// XUnity.AutoTranslator's BingTranslateEndpoint (only `IG`/`IID` are) —
/// confirmed by testing that XUnity's exact request shape gets rejected by
/// the live endpoint today with that same `statusCode: 205`. Found the
/// missing piece by reading `bing.com/translator`'s own page script: it
/// embeds `params_AbusePreventionHelper = [key, token, ttlMs]` and its
/// `AbusePreventionHelper.getEndpointAuthParams()` appends
/// `&token={token}&key={key}` to the request body — Bing likely added this
/// requirement after XUnity's implementation was last touched.
struct BingSession {
    client: reqwest::blocking::Client,
    ig: Option<String>,
    iid: Option<String>,
    auth_token: Option<String>,
    auth_key: Option<String>,
    counter: SessionCounter,
    /// Bing's frontend suffixes `IID` with a per-request counter
    /// (`data-iid.N`); XUnity.AutoTranslator's BingTranslateEndpoint does
    /// the same (`{IID}.{translationCount}`).
    request_count: u64,
}

impl BingSession {
    fn new(timeout_secs: u64) -> Result<Self> {
        Ok(Self {
            client: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(timeout_secs))
                .cookie_store(true)
                .user_agent(CHROME_USER_AGENT)
                .build()
                .context("failed to build an HTTP client for Bing Translator")?,
            ig: None,
            iid: None,
            auth_token: None,
            auth_key: None,
            counter: SessionCounter::default(),
            request_count: 0,
        })
    }

    /// GETs `bing.com/translator` (through the same cookie-carrying client
    /// used for translations) and scrapes `IG`, `data-iid`, and
    /// `params_AbusePreventionHelper` out of the page. On failure this just
    /// logs and proceeds without whatever couldn't be found — the actual
    /// translate request will likely then get rejected too, but that surfaces
    /// as a normal translation failure rather than a panic here.
    fn refresh(&mut self) {
        let result = self
            .client
            .get("https://www.bing.com/translator")
            .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
            .send()
            .and_then(|r| r.text());
        let html = match result {
            Ok(html) => html,
            Err(e) => {
                tracing::debug!(
                    "failed to refresh Bing Translator's session, proceeding without: {e}"
                );
                self.ig = None;
                self.iid = None;
                self.auth_token = None;
                self.auth_key = None;
                return;
            }
        };
        self.ig = lookup(&html, "\",IG:\"");
        self.iid = lookup(&html, "data-iid=\"");
        self.request_count = 0;
        if self.ig.is_none() || self.iid.is_none() {
            tracing::debug!(
                "could not locate Bing Translator's IG/IID in the page; proceeding without"
            );
        }
        match lookup_abuse_prevention_params(&html) {
            Some((key, token)) => {
                self.auth_key = Some(key);
                self.auth_token = Some(token);
            }
            None => {
                tracing::debug!(
                    "could not locate Bing Translator's auth token/key in the page; proceeding without"
                );
                self.auth_key = None;
                self.auth_token = None;
            }
        }
    }
}

fn lookup(html: &str, marker: &str) -> Option<String> {
    let start = html.find(marker)? + marker.len();
    let end = html[start..].find('"')? + start;
    Some(html[start..end].to_string())
}

/// Parses `params_AbusePreventionHelper = [<key>,"<token>",<ttlMs>]` out of
/// the page (a plain JS array literal, not JSON — no surrounding braces or
/// keys), returning `(key, token)`.
fn lookup_abuse_prevention_params(html: &str) -> Option<(String, String)> {
    let marker = "params_AbusePreventionHelper";
    let after = &html[html.find(marker)? + marker.len()..];
    let start = after.find('[')? + 1;
    let end = after[start..].find(']')? + start;
    let inner = &after[start..end];
    let mut parts = inner.splitn(3, ',');
    let key = parts.next()?.trim().to_string();
    let token = parts.next()?.trim().trim_matches('"').to_string();
    Some((key, token))
}

static SESSIONS: OnceLock<Mutex<HashMap<String, BingSession>>> = OnceLock::new();

const CHROME_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

impl BingTranslate {
    /// The free, unofficial endpoint `bing.com/translator`'s own page calls
    /// internally (`www.bing.com/ttranslatev3`) — same approach as
    /// XUnity.AutoTranslator's BingTranslateEndpoint: load the translator
    /// page once to get its `IG`/`IID` values and session cookies, then
    /// reuse both across translations (refreshed periodically, not on every
    /// call — see [`SessionCounter`]) since that's exactly what a real
    /// browser visiting the page once and translating repeatedly would do.
    /// Undocumented and could change or rate-limit without notice.
    fn translate_public(&self, req: TranslateRequest) -> Result<String> {
        let sessions = SESSIONS.get_or_init(Default::default);
        let mut sessions = sessions.lock().unwrap();
        let session = match sessions.entry(self.name.clone()) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(BingSession::new(self.timeout_secs)?)
            }
        };
        if session.counter.tick() {
            session.refresh();
        }
        session.request_count += 1;

        let from = fix_language(req.source_lang);
        let to = fix_language(req.target_lang);
        let mut form = vec![
            ("fromLang", from.as_str()),
            ("text", req.text),
            ("to", to.as_str()),
        ];
        if let (Some(token), Some(key)) = (&session.auth_token, &session.auth_key) {
            form.push(("token", token.as_str()));
            form.push(("key", key.as_str()));
        }

        let url = match (&session.ig, &session.iid) {
            (Some(ig), Some(iid)) => {
                format!(
                    "https://www.bing.com/ttranslatev3?isVertical=1&IG={ig}&IID={iid}.{}",
                    session.request_count
                )
            }
            _ => "https://www.bing.com/ttranslatev3?isVertical=1".to_string(),
        };

        let resp = session
            .client
            .post(&url)
            .header(reqwest::header::ACCEPT, "*/*")
            .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
            .header(reqwest::header::REFERER, "https://www.bing.com/translator")
            .header("Origin", "https://www.bing.com")
            .form(&form)
            .send()
            .context("request to the public Bing Translator endpoint failed")?;

        let status = resp.status();
        let body_text = resp.text().unwrap_or_default();
        if !status.is_success() {
            bail!("public Bing Translator endpoint returned HTTP {status}: {body_text}");
        }
        let parsed: Vec<BingResponseItem> = serde_json::from_str(&body_text).with_context(|| {
            format!("unexpected response shape from the public Bing Translator endpoint: {body_text}")
        })?;
        let text = extract_translation(parsed);
        if text.is_empty() {
            bail!("public Bing Translator endpoint returned no translation (it may have rejected the session)");
        }
        Ok(text)
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

/// Bing's public endpoint uses slightly different language codes than the
/// Azure API — same mapping XUnity.AutoTranslator's BingTranslateEndpoint
/// applies before sending a request.
fn fix_language(lang: &str) -> String {
    match lang {
        "auto" => "auto-detect".to_string(),
        "zh-CN" | "zh" => "zh-Hans".to_string(),
        "zh-TW" => "zh-Hant".to_string(),
        other => other.to_string(),
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
