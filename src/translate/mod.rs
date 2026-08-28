mod bing;
mod google;
mod openai_compat;

use anyhow::{bail, Result};

use crate::config::{AppConfig, ProviderConfig, ProviderKind};

pub struct TranslateRequest<'a> {
    pub text: &'a str,
    pub source_lang: &'a str,
    pub target_lang: &'a str,
}

pub trait Translator {
    fn translate(&self, req: TranslateRequest) -> Result<String>;
}

pub fn build(cfg: &AppConfig) -> Result<Box<dyn Translator>> {
    let provider = cfg.providers.get(&cfg.active_provider).ok_or_else(|| {
        anyhow::anyhow!(
            "active_provider '{}' not found in config",
            cfg.active_provider
        )
    })?;
    build_for(provider, cfg)
}

pub fn build_named(name: &str, cfg: &AppConfig) -> Result<Box<dyn Translator>> {
    let provider = cfg
        .providers
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("provider '{name}' not found in config"))?;
    build_for(provider, cfg)
}

fn build_for(provider: &ProviderConfig, cfg: &AppConfig) -> Result<Box<dyn Translator>> {
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
            api_key: provider.resolve_api_key(),
            timeout_secs: provider.timeout_secs,
        })),
        ProviderKind::BingTranslate => {
            let Some(api_key) = provider.resolve_api_key() else {
                bail!("Bing/Azure Translator requires api_key or api_key_env");
            };
            Ok(Box::new(bing::BingTranslate {
                api_key,
                region: provider.region.clone(),
                timeout_secs: provider.timeout_secs,
            }))
        }
    }
}

pub(crate) fn render_prompt(template: &str, req: &TranslateRequest) -> String {
    template
        .replace("{source_lang}", req.source_lang)
        .replace("{target_lang}", req.target_lang)
        .replace("{text}", req.text)
}
