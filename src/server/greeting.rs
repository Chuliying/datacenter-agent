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
//! Spawns background tasks during server startup. Each runs the two-stage greeting pipeline
//! (fetcher → analyst) and appends the resulting short C-suite welcome paragraph to
//! [`AppState::greetings`].
//!
//! `GET /greeting` then serves a random pick.

use anyhow::{bail, Context, Result};
use tracing::{error, info, warn};

use super::AppState;
use crate::agent::clock::{Clock, SystemClock};
use crate::agent::payload::{AgentPayload, InitialPrompt};
use crate::agent::wiring::{build_greeting_pipeline, greeting_pipeline_id};

/// Build one greeting by running the two-stage greeting pipeline (fetcher → analyst).
///
/// The fetcher pulls a broad datacenter snapshot with its granted tools (the `/insight` fetcher's
/// grant); the **terminal** analyst turns that material into one short executive greeting — its
/// model message is the `Final` answer. Buffered: greetings are collected at boot, not streamed.
pub async fn build_one_greeting(state: &AppState) -> Result<String> {
    let orchestrator = build_greeting_pipeline(
        state.mcp.clone(),
        &state.tools,
        state.instructions.as_deref(),
        &state.insight_grants.fetcher,
        &state.prompts.greeting_fetcher_system,
        &state.prompts.greeting_analyst_system,
        &state.llm.resolved(),
    )
    .context("build greeting pipeline")?;

    let initial = AgentPayload::Initial(InitialPrompt {
        prompt: state.prompts.greeting_user.clone(),
        history: Vec::new(),
        now: SystemClock::default().now(),
    });

    match orchestrator
        .run(&greeting_pipeline_id(), initial)
        .await
        .context("greeting pipeline run failed")?
    {
        AgentPayload::Final(result) => Ok(result.assistant),
        other => bail!(
            "greeting pipeline did not produce a final result (got {:?})",
            other.kind()
        ),
    }
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
