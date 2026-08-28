use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use super::{render_prompt, TranslateRequest, Translator};

/// Talks to any server implementing the OpenAI `/v1/chat/completions` shape:
/// LM Studio, Ollama (`OLLAMA_ORIGINS` / OpenAI-compat endpoint), OpenAI itself,
/// DeepSeek, and most other hosted or local LLM servers.
pub struct OpenAiCompatible {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub system_prompt: String,
    pub user_prompt_template: String,
    pub timeout_secs: u64,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    temperature: f32,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Deserialize)]
struct ChoiceMessage {
    content: String,
}

impl Translator for OpenAiCompatible {
    fn translate(&self, req: TranslateRequest) -> Result<String> {
        let user_content = render_prompt(&self.user_prompt_template, &req);
        let body = ChatRequest {
            model: &self.model,
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: &self.system_prompt,
                },
                ChatMessage {
                    role: "user",
                    content: &user_content,
                },
            ],
            temperature: 0.2,
        };

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(self.timeout_secs))
            .build()?;
        let mut req_builder = client.post(&url).json(&body);
        if let Some(key) = &self.api_key {
            req_builder = req_builder.bearer_auth(key);
        }

        let resp = req_builder
            .send()
            .with_context(|| format!("request to {url} failed (is the server running?)"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().unwrap_or_default();
            bail!("LLM provider returned HTTP {status}: {text}");
        }
        let parsed: ChatResponse = resp
            .json()
            .context("unexpected response shape from LLM provider")?;
        let content = parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .unwrap_or_default();
        Ok(content.trim().to_string())
    }
}
