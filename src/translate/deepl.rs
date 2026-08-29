use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde_json::json;

use crate::config::ProviderMode;

use super::session::SessionCounter;
use super::{TranslateRequest, Translator};

pub struct DeepLTranslate {
    pub name: String,
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

/// A cached `deepl.com/translator` JSON-RPC session for one provider entry:
/// the cookies its initial page load sets, plus a running request `id`
/// (DeepL's JSON-RPC `id` field just needs to keep increasing across a
/// session, not reset per request). See [`refresh`].
struct DeepLSession {
    client: reqwest::blocking::Client,
    id: i64,
    counter: SessionCounter,
}

impl DeepLSession {
    fn new(timeout_secs: u64) -> Result<Self> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0) as i64;
        Ok(Self {
            client: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(timeout_secs))
                .cookie_store(true)
                .user_agent(
                    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
                )
                .build()
                .context("failed to build an HTTP client for DeepL Translator")?,
            // A large pseudo-random starting id, same idea as XUnity.AutoTranslator's
            // ExtDeepLTranslate (`10000 * (10000 * random())`) — DeepL's JSON-RPC id
            // just needs to be a plausible, increasing integer.
            id: 10_000_000 + (nanos % 10_000_000),
            counter: SessionCounter::default(),
        })
    }

    /// Loads `deepl.com/translator` (priming cookies) then calls its
    /// `getClientState` JSON-RPC method once, same setup sequence
    /// XUnity.AutoTranslator's ExtDeepLTranslate performs before its first
    /// translation (and again periodically — see [`SessionCounter`]).
    /// Unlike Google/Bing's sessions, a failure here is a real error: DeepL's
    /// endpoint is more aggressive about rejecting requests from a session
    /// that skipped this handshake, so there's no reasonable "proceed
    /// without it" fallback.
    fn refresh(&mut self) -> Result<()> {
        self.client
            .get("https://www.deepl.com/translator")
            .send()
            .and_then(|r| r.error_for_status())
            .context("failed to load deepl.com/translator to prime a session")?;

        self.id += 1;
        let body = json!({
            "jsonrpc": "2.0",
            "method": "getClientState",
            "params": { "v": "20180814" },
            "id": self.id,
        });
        self.client
            .post("https://w.deepl.com/web?request_type=jsonrpc&il=en&method=getClientState")
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .and_then(|r| r.error_for_status())
            .context("DeepL getClientState setup call failed")?;
        Ok(())
    }
}

static SESSIONS: OnceLock<Mutex<HashMap<String, DeepLSession>>> = OnceLock::new();

impl DeepLTranslate {
    /// The free, unofficial JSON-RPC endpoint `deepl.com/translator`'s own
    /// page calls (`LMT_handle_jobs` on `www2.deepl.com/jsonrpc`) — same
    /// approach as XUnity.AutoTranslator's DeepLTranslate/ExtDeepLTranslate:
    /// prime a session (cookies + `getClientState`), then submit translation
    /// "jobs" with a `timestamp` massaged to satisfy DeepL's abuse check (see
    /// [`massaged_timestamp`]). Undocumented and could change, rate-limit, or
    /// (per XUnity's handling of HTTP 429) temporarily block a session
    /// without notice — a 429 here resets the session so the next attempt
    /// starts fresh.
    fn translate_public(&self, req: TranslateRequest) -> Result<String> {
        let sessions = SESSIONS.get_or_init(Default::default);
        let mut sessions = sessions.lock().unwrap();
        let session = match sessions.entry(self.name.clone()) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(DeepLSession::new(self.timeout_secs)?)
            }
        };
        if session.counter.tick() {
            session.refresh()?;
        }

        session.id += 1;
        let now_millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let timestamp = massaged_timestamp(now_millis, req.text);

        let (source, target) = (fix_language(req.source_lang), fix_language(req.target_lang));
        let user_preferred_langs = if source == "auto" {
            vec![target.clone()]
        } else {
            vec![target.clone(), source.clone()]
        };

        let body = json!({
            "jsonrpc": "2.0",
            "method": "LMT_handle_jobs",
            "params": {
                "jobs": [{
                    "kind": "default",
                    "preferred_num_beams": 1,
                    "raw_en_sentence": req.text,
                    "raw_en_context_before": [],
                    "raw_en_context_after": [],
                }],
                "lang": {
                    "user_preferred_langs": user_preferred_langs,
                    "source_lang_user_selected": source,
                    "target_lang": target,
                },
                "priority": -1,
                "timestamp": timestamp,
            },
            "id": session.id,
        });

        let resp = session
            .client
            .post("https://www2.deepl.com/jsonrpc")
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::REFERER, "https://www.deepl.com/translator")
            .header("Origin", "https://www.deepl.com")
            .json(&body)
            .send()
            .context("request to the public DeepL Translator endpoint failed")?;

        let status = resp.status();
        let body_text = resp.text().unwrap_or_default();
        if status.as_u16() == 429 {
            // Reset so the next attempt establishes a fresh session, same as
            // XUnity's ExtDeepLTranslate does on a BlockedException.
            session.counter = SessionCounter::default();
            bail!("public DeepL Translator endpoint returned HTTP 429 (rate-limited); session reset for next attempt: {body_text}");
        }
        if !status.is_success() {
            bail!("public DeepL Translator endpoint returned HTTP {status}: {body_text}");
        }
        parse_response(&body_text)
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
            .timeout(std::time::Duration::from_secs(self.timeout_secs))
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

/// DeepL's free endpoint uses uppercase language codes and doesn't
/// distinguish simplified/traditional Chinese variants the way our config
/// might — same mapping XUnity.AutoTranslator's DeepLTranslate applies.
/// `"auto"` is passed through as-is (lowercase): unlike XUnity (which
/// doesn't support auto-detect for DeepL at all), this is this project's own
/// best-effort based on how deepl.com's web frontend behaves when no source
/// language is explicitly chosen — not something confirmed against
/// XUnity's implementation, since it has no equivalent to check against.
fn fix_language(lang: &str) -> String {
    match lang {
        "auto" => "auto".to_string(),
        "zh-Hans" | "zh-CN" => "ZH".to_string(),
        other => other.to_uppercase(),
    }
}

/// DeepL's abuse check on `LMT_handle_jobs` rejects requests whose
/// `timestamp` doesn't follow a specific rounding rule tied to the request
/// text — ported from XUnity.AutoTranslator's ExtDeepLTranslate, which
/// counts occurrences of the letter `'i'` in the text (`n`) and rounds the
/// current time (`r`, milliseconds since epoch) up to the next multiple of
/// `n`. This has no meaning beyond "what DeepL's own JS happens to check."
fn massaged_timestamp(now_millis: i64, text: &str) -> i64 {
    let n = 1 + text.chars().filter(|&c| c == 'i').count() as i64;
    now_millis + (n - now_millis % n)
}

/// The response shape from `LMT_handle_jobs`:
/// `{"result":{"translations":[{"beams":[{"postprocessed_sentence":"..."}]}]}}`.
fn parse_response(body: &str) -> Result<String> {
    let value: serde_json::Value = serde_json::from_str(body)
        .context("unexpected response from the public DeepL Translator endpoint")?;
    value
        .pointer("/result/translations/0/beams/0/postprocessed_sentence")
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
