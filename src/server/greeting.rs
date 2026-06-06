// Copyright 2026 Wayne Hong (h-alice) <contact@halice.art>
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Greeting generating task.
//!
//! Spawns background tasks during server startup. Each runs the greeting
//! prompt through the MCP tool-calling loop and appends the resulting short
//! welcome paragraph to [`AppState::greetings`].
//!
//! `GET /greeting` then serves a random pick.

use anyhow::{Context, Result};
use tracing::{error, info, warn};

use super::AppState;
use crate::llm_connector;

/// Build one greeting by running the greeting prompt through the MCP loop.
///
/// Uses the `greeting_system` prompt and the `greeting_user` stub with no history.
///
/// The model decides whether to call any data tools to make the welcome data-aware.
pub async fn build_one_greeting(state: &AppState) -> Result<String> {
    let cfg = state.generation_config(
        &state.prompts.greeting_system,
        state.prompts.greeting_user.clone(),
        Vec::new(),
    );

    llm_connector::generate(cfg, state.tools.clone(), state.mcp.clone())
        .await
        .context("greeting LLM call failed")
}

/// Spawn background tasks to generate greetings.
///
/// Spawn `count` background tasks that each generate one greeting and
/// append it to `state.greetings` on success.
///
/// Failures are logged and dropped, the endpoint reports 503 until at least
/// one entry exists.
pub fn spawn_greeting_tasks(state: AppState, count: usize) {
    if count == 0 {
        warn!("spawn_greeting_tasks called with count=0; skipping");
        return;
    }
    info!(count, "greeting.spawn");
    for i in 0..count {
        let s = state.clone();
        tokio::spawn(async move {
            match build_one_greeting(&s).await {
                Ok(text) => {
                    let total = {
                        let mut v = s.greetings.lock().await;
                        v.push(text);
                        v.len()
                    };
                    info!(idx = i, total, "greeting.ready");
                }
                Err(e) => {
                    error!(idx = i, error = ?e, "greeting.failed");
                }
            }
        });
    }
}
