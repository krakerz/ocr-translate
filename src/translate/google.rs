use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::config::ProviderMode;

use super::session::SessionCounter;
use super::{TranslateRequest, Translator};

pub struct GoogleTranslate {
    pub name: String,
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

/// A cached "TKK" session for one provider entry (keyed by name so two
/// differently-configured `google_translate` providers don't share state).
/// `m`/`s` are the two halves of Google's anti-abuse token seed, scraped out
/// of `translate.google.com`'s HTML — see [`refresh`]. Kept alive across
/// translations within one process (not just one request) since re-scraping
/// on every single translation would be wasteful; see [`SessionCounter`].
struct GoogleSession {
    client: reqwest::blocking::Client,
    m: i64,
    s: i64,
    counter: SessionCounter,
}

impl GoogleSession {
    fn new(timeout_secs: u64) -> Result<Self> {
        Ok(Self {
            client: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(timeout_secs))
                .cookie_store(true)
                .build()
                .context("failed to build an HTTP client for Google Translate")?,
            // Fallback TKK, used until (or unless) `refresh` finds a real one —
            // the same fallback XUnity.AutoTranslator's GoogleTranslateEndpoint uses.
            m: 427761,
            s: 1179739010,
            counter: SessionCounter::default(),
        })
    }

    /// GETs `translate.google.com` (through the same cookie-carrying client
    /// used for translations) and scrapes `tkk:'m.s'` (or `TKK='m.s'`) out of
    /// the page. On any failure this just logs and keeps whatever `m`/`s`
    /// were already set (the hardcoded fallback, the first time) — mirrors
    /// XUnity's "warn and continue" behavior, since a stale/fallback TKK
    /// still works most of the time, it just risks the request being
    /// rejected occasionally.
    fn refresh(&mut self) {
        let result = self
            .client
            .get("https://translate.google.com/")
            .send()
            .and_then(|r| r.text());
        let html = match result {
            Ok(html) => html,
            Err(e) => {
                tracing::debug!(
                    "failed to refresh Google Translate's TKK session, using previous/fallback values: {e}"
                );
                return;
            }
        };
        for lookup in ["tkk:'", "TKK='"] {
            let Some(start) = html.find(lookup) else {
                continue;
            };
            let rest = &html[start + lookup.len()..];
            let Some(end) = rest.find('\'') else { continue };
            let value = &rest[..end];
            let Some((m_str, s_str)) = value.split_once('.') else {
                continue;
            };
            if let (Ok(m), Ok(s)) = (m_str.parse::<i64>(), s_str.parse::<i64>()) {
                self.m = m;
                self.s = s;
                return;
            }
        }
        tracing::debug!(
            "could not locate Google Translate's TKK value in the page; using previous/fallback values"
        );
    }
}

static SESSIONS: OnceLock<Mutex<HashMap<String, GoogleSession>>> = OnceLock::new();

impl GoogleTranslate {
    /// The free, unofficial endpoint `translate.google.com`'s own web UI
    /// calls — no API key, no billing. Same approach (and even the same
    /// fallback TKK constants) as XUnity.AutoTranslator's GoogleTranslate
    /// endpoint: a "tk" token computed from a per-session seed (`TKK`)
    /// scraped out of the translator page, refreshed periodically rather
    /// than on every call. Undocumented and could change or rate-limit
    /// without notice, but it's what lets this app translate out of the box
    /// with zero configuration.
    fn translate_public(&self, req: TranslateRequest) -> Result<String> {
        let sessions = SESSIONS.get_or_init(Default::default);
        let mut sessions = sessions.lock().unwrap();
        let session = match sessions.entry(self.name.clone()) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(GoogleSession::new(self.timeout_secs)?)
            }
        };
        if session.counter.tick() {
            session.refresh();
        }

        let tk = tk(req.text, session.m, session.s);
        let resp = session
            .client
            .get("https://translate.googleapis.com/translate_a/single")
            .query(&[
                ("client", "webapp"),
                ("sl", req.source_lang),
                ("tl", req.target_lang),
                ("dt", "t"),
                ("tk", tk.as_str()),
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

/// Google's "tk" anti-abuse token, ported from XUnity.AutoTranslator's
/// `GoogleTranslateEndpoint.Tk`/`Vi` (credited there as "stolen from
/// Translation Aggregator r190, all credits to Sinflower") — a checksum over
/// the UTF-16 code units of `text`, seeded by the session's `m`/`s` (see
/// [`GoogleSession::refresh`]). This is reverse-engineered from Google's own
/// (obfuscated, client-side JS) translate frontend, not a documented API;
/// the bit-manipulation below has no deeper meaning than "what Google's JS
/// happens to compute" and must match it exactly to produce a token the
/// server accepts.
fn tk(text: &str, m: i64, s: i64) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut bytes: Vec<i64> = Vec::with_capacity(units.len());
    let mut v = 0usize;
    while v < units.len() {
        let mut a = units[v] as i64;
        if a < 128 {
            bytes.push(a);
        } else {
            if a < 2048 {
                bytes.push((a >> 6) | 192);
            } else if (a & 0xFC00) == 0xD800
                && v + 1 < units.len()
                && (units[v + 1] as i64 & 0xFC00) == 0xDC00
            {
                v += 1;
                a = 65536 + ((a & 1023) << 10) + (units[v] as i64 & 1023);
                bytes.push((a >> 18) | 240);
                bytes.push(((a >> 12) & 63) | 128);
            } else {
                bytes.push((a >> 12) | 224);
                bytes.push(((a >> 6) & 63) | 128);
            }
            bytes.push((63 & a) | 128);
        }
        v += 1;
    }

    const F: &str = "+-a^+6";
    const D: &str = "+-3^+b+-f";
    let mut p = m;
    for b in &bytes {
        p += b;
        p = vi(p, F);
    }
    p = vi(p, D);
    p ^= s;
    if p < 0 {
        p = (2147483647 & p) + 2147483648;
    }
    p %= 1_000_000;
    format!("{p}.{}", p ^ m)
}

fn vi(mut r: i64, o: &str) -> i64 {
    let bytes = o.as_bytes();
    let mut t = 0usize;
    while t < bytes.len() {
        let mut a = bytes[t + 2] as i64;
        a = if a >= b'a' as i64 {
            a - 87
        } else {
            a - b'0' as i64
        };
        a = if bytes[t + 1] == b'+' { r >> a } else { r << a };
        r = if bytes[t] == b'+' {
            (r + a) & 4294967295
        } else {
            r ^ a
        };
        t += 3;
    }
    r
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
