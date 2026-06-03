//! One-shot chat completion against OpenRouter.

use anyhow::{anyhow, Context, Result};
use async_openai::error::OpenAIError;
use tracing::{debug, error, info};

use super::client::build_client;
use crate::model::GenerationConfig;

/// Issue one chat completion and return the model's reply.
///
/// The caller should also provide the system prompt.
/// 
/// # Errors
///
/// Returns `Err` if:
/// - request fails to build
/// - HTTP call fails
/// - OpenRouter API returns a non-2xx response
/// - response cannot be deserialized
/// - response contains no `choice` with content.
pub async fn generate(cfg: &GenerationConfig, system: &str) -> Result<String> {
    info!(
        model = %cfg.model,
        history_len = cfg.history.len(),
        data_docs = cfg.prompt.data.len(),
        "llm.generate.start"
    );

    let client = build_client(cfg)?;
    let req = cfg
        .to_chat_request(system)
        .context("failed to build chat completion request")?;

    debug!(messages = ?req.messages, "llm.generate.request_built");

    let resp = match client.chat().create(req).await {
        Ok(r) => r,
        Err(OpenAIError::ApiError(api_err)) => {
            error!(
                status = %api_err.status_code,
                message = %api_err.api_error.message,
                r#type = ?api_err.api_error.r#type,
                code = ?api_err.api_error.code,
                "openrouter api error"
            );
            return Err(anyhow!(OpenAIError::ApiError(api_err)))
                .context("OpenRouter chat completion failed");
        }
        Err(e) => {
            error!(error = %e, "openrouter transport error");
            return Err(anyhow!(e)).context("OpenRouter chat completion failed");
        }
    };

    if let Some(usage) = &resp.usage {
        info!(
            prompt_tokens = usage.prompt_tokens,
            completion_tokens = usage.completion_tokens,
            total_tokens = usage.total_tokens,
            "llm.generate.ok"
        );
    } else {
        info!("llm.generate.ok (no usage reported)");
    }

    let content = resp
        .choices
        .into_iter()
        .next()
        .and_then(|c| c.message.content)
        .ok_or_else(|| anyhow!("OpenRouter returned no content"))?;

    Ok(content)
}
