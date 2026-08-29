mod bing;
mod deepl;
mod google;
mod openai_compat;
mod session;

use anyhow::{bail, Result};

use crate::config::{AppConfig, ProviderConfig, ProviderKind};

pub struct TranslateRequest<'a> {
    pub text: &'a str,
    pub source_lang: &'a str,
    pub target_lang: &'a str,
}

impl TranslateRequest<'_> {
    fn reborrow(&self) -> TranslateRequest<'_> {
        TranslateRequest {
            text: self.text,
            source_lang: self.source_lang,
            target_lang: self.target_lang,
        }
    }
}

pub trait Translator {
    fn translate(&self, req: TranslateRequest) -> Result<String>;
}

pub fn build(cfg: &AppConfig) -> Result<Box<dyn Translator>> {
    build_named(&cfg.active_provider, cfg)
}

pub fn build_named(name: &str, cfg: &AppConfig) -> Result<Box<dyn Translator>> {
    let provider = cfg
        .providers
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("provider '{name}' not found in config"))?;
    build_for(name, provider, cfg)
}

/// Tries `active_provider` first, then each of `fallback_providers` in
/// order, returning the first successful translation. Useful when the
/// primary provider is a local server (e.g. LM Studio) that might not be
/// running — a free public translator can still get the job done.
pub fn translate_with_fallback(cfg: &AppConfig, req: TranslateRequest) -> Result<(String, String)> {
    let mut names = vec![cfg.active_provider.clone()];
    names.extend(cfg.fallback_providers.iter().cloned());

    let mut last_err = None;
    for name in &names {
        let attempt = build_named(name, cfg).and_then(|t| t.translate(req.reborrow()));
        match attempt {
            Ok(translated) => return Ok((name.clone(), translated)),
            Err(e) => {
                tracing::warn!("provider '{name}' failed: {e:#}");
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no providers configured")))
}

fn build_for(
    name: &str,
    provider: &ProviderConfig,
    cfg: &AppConfig,
) -> Result<Box<dyn Translator>> {
    match provider.kind {
        ProviderKind::OpenAiCompatible => {
            let Some(base_url) = provider.base_url.clone() else {
                bail!("provider is missing base_url");
            };
            Ok(Box::new(openai_compat::OpenAiCompatible {
                base_url,
                api_key: provider.resolve_api_key(),
                model: provider
                    .model
                    .clone()
                    .unwrap_or_else(|| "gpt-4o-mini".to_string()),
                system_prompt: cfg.prompt.system.clone(),
                user_prompt_template: cfg.prompt.template.clone(),
                timeout_secs: provider.timeout_secs,
            }))
        }
        ProviderKind::GoogleTranslate => Ok(Box::new(google::GoogleTranslate {
            name: name.to_string(),
            mode: provider.mode,
            api_key: provider.resolve_api_key(),
            timeout_secs: provider.timeout_secs,
        })),
        ProviderKind::BingTranslate => Ok(Box::new(bing::BingTranslate {
            name: name.to_string(),
            mode: provider.mode,
            api_key: provider.resolve_api_key(),
            region: provider.region.clone(),
            timeout_secs: provider.timeout_secs,
        })),
        ProviderKind::DeepLTranslate => Ok(Box::new(deepl::DeepLTranslate {
            name: name.to_string(),
            mode: provider.mode,
            api_key: provider.resolve_api_key(),
            timeout_secs: provider.timeout_secs,
        })),
    }
}

pub(crate) fn render_prompt(template: &str, req: &TranslateRequest) -> String {
    template
        .replace("{source_lang}", req.source_lang)
        .replace("{target_lang}", req.target_lang)
        .replace("{text}", req.text)
}
