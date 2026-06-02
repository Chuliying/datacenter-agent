//! Builds an `async-openai` client (for OpenRouter).

use anyhow::{Context, Result};
use async_openai::{config::OpenAIConfig, Client};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use crate::model::GenerationConfig;

/// Construct an `async-openai` client.
///
/// OpenRouter attribution headers (`HTTP-Referer`, `X-Title`) baked in as
/// reqwest default headers when supplied.
pub(crate) fn build_client(cfg: &GenerationConfig) -> Result<Client<OpenAIConfig>> {
    let mut headers = HeaderMap::new();
    if let Some(url) = &cfg.app_url {
        headers.insert(
            HeaderName::from_static("http-referer"),
            HeaderValue::from_str(url).context("invalid OPENROUTER_APP_URL header value")?,
        );
    }
    if let Some(title) = &cfg.app_title {
        headers.insert(
            HeaderName::from_static("x-title"),
            HeaderValue::from_str(title).context("invalid OPENROUTER_APP_TITLE header value")?,
        );
    }

    let http = reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .context("failed to build reqwest client")?;

    let oai_cfg = OpenAIConfig::new()
        .with_api_base(&cfg.base_url)
        .with_api_key(&cfg.api_key);

    Ok(Client::with_config(oai_cfg).with_http_client(http))
}
