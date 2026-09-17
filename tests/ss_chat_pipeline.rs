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

//! Live integration test: the **`/ss-chat`** pipeline (星星電力 investor platform) against the
//! real MCP server.
//!
//! The counterpart to `agent_pipeline.rs`, one level up: rather than hand-assembling the stages, it
//! drives [`build_ss_chat_pipeline`] — the *same* function `/ss-chat/stream` calls — with the
//! *same* boot inputs (the `ss_*_system` prompts and the `[ss_chat.grants]` tool grant read from
//! `config/config.toml`). So a passing run exercises the production wiring, not a replica of it.
//!
//! The question set is the seven manager-level cases verified against the live investor-platform
//! API in `eomc-mcp/docs/ss_chatbot_agent_test.md` (Q7 is the adversarial one: it asks for a
//! physically meaningless kWh + kW sum **and** uses the wrong programme name 星火50).
//!
//! It is **`#[ignore]`d** — it needs a running MCP server and an OpenRouter key. Run it by hand:
//!
//! ```text
//! # the adversarial case (default)
//! cargo test --test ss_chat_pipeline -- --ignored --nocapture
//!
//! # one specific case, or the whole set
//! SS_CHAT_QUESTION=4   cargo test --test ss_chat_pipeline -- --ignored --nocapture
//! SS_CHAT_QUESTION=all cargo test --test ss_chat_pipeline -- --ignored --nocapture
//! ```
//!
//! Required environment (a `.env` at the repo root is loaded automatically, like `main`):
//!
//! - `DATACENTER_MCP_URL` — the MCP `/mcp` endpoint (it advertises the `ss_*` tools)
//! - `OPENROUTER_API_KEY` — the LLM key
//! - `OPENROUTER_MODEL` — the model id
//!
//! Optional:
//!
//! - `OPENROUTER_BASE_URL` — defaults to `https://openrouter.ai/api/v1`
//! - `SS_CHAT_QUESTION` — `1`–`7` (default `7`), or `all` to run every case in sequence
//! - `SS_CHAT_PROMPT` — an ad-hoc question, overriding `SS_CHAT_QUESTION`
//!
//! # What is asserted
//!
//! The answers are prose, so this does not grade their content — the transcript is printed for a
//! human to read against the reference document. What it *does* assert mechanically is the wiring
//! contract that a regression would silently break:
//!
//! 1. the pipeline reaches a non-empty `Final` answer;
//! 2. **every data tool the model called was an `ss_*` tool** — the grant never leaks the
//!    EV-charging tools this same MCP server also advertises (the built-in `emit_chart` sink is
//!    the one legitimate non-`ss_` name);
//! 3. the analyst streamed its answer as `ContentDelta` events.

use std::sync::Arc;

use datacenter_agent::agent::clock::{Clock, SystemClock};
use datacenter_agent::agent::config::{Provider, ResolvedLlm};
use datacenter_agent::agent::events::{AgentEvent, ChannelSink, EventSink};
use datacenter_agent::agent::payload::{AgentPayload, InitialPrompt};
use datacenter_agent::agent::pipeline::ss_chat_pipeline_id;
use datacenter_agent::agent::wiring::build_ss_chat_pipeline;
use datacenter_agent::appstate::PromptBank;
use datacenter_agent::config::AppConfig;
use datacenter_agent::mcp_client::McpClient;
use tokio::sync::mpsc;

/// The seven manager-level questions from `eomc-mcp/docs/ss_chatbot_agent_test.md`, in order.
const QUESTIONS: [(&str, &str); 7] = [
    (
        "Q1 日照條件與發電風險",
        "這一季的日照條件怎麼樣？對我們太陽能的發電量預估有影響嗎？",
    ),
    (
        "Q2 儲能收益與資料可信度",
        "儲能那邊今年到四月的收益狀況如何？這些數字可以直接拿去跟董事會報嗎？",
    ),
    (
        "Q3 轉供收益與集中度風險",
        "綠電轉供今年上半年收了多少錢？有沒有過度集中在少數案場的風險？",
    ),
    (
        "Q4 開發管道瓶頸",
        "我們的售電和購電案子現在卡在哪個階段？哪一邊比較需要補資源？",
    ),
    (
        "Q5 工程進度與現金流",
        "表前建置案的工程進度如何？最近有哪些款項可以收？",
    ),
    (
        "Q6 跨領域整體概況",
        "給我一個這季的整體營運概況，各條業務線現在狀況如何？",
    ),
    (
        "Q7 單位陷阱（誘導性問題）",
        "把售電和購電的總量加起來，我們手上總共有多少量體？順便告訴我星火50計畫現在幾件。",
    ),
];

/// Read a required env var, failing with a message that names what to set.
fn require_env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| {
        panic!(
            "integration test needs `{key}` set (see this file's module docs); run with --ignored"
        )
    })
}

/// The `(label, prompt)` cases this run should exercise, per `SS_CHAT_PROMPT` / `SS_CHAT_QUESTION`.
fn selected_cases() -> Vec<(String, String)> {
    if let Ok(prompt) = std::env::var("SS_CHAT_PROMPT") {
        return vec![("SS_CHAT_PROMPT".to_string(), prompt)];
    }
    let selector = std::env::var("SS_CHAT_QUESTION").unwrap_or_else(|_| "7".into());
    if selector.eq_ignore_ascii_case("all") {
        return QUESTIONS
            .iter()
            .map(|(label, prompt)| (label.to_string(), prompt.to_string()))
            .collect();
    }
    let index: usize = selector
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("SS_CHAT_QUESTION must be 1-7 or `all`, got `{selector}`"));
    let (label, prompt) = QUESTIONS
        .get(index.wrapping_sub(1))
        .unwrap_or_else(|| panic!("SS_CHAT_QUESTION must be 1-7 or `all`, got `{index}`"));
    vec![(label.to_string(), prompt.to_string())]
}

/// A one-line rendering of a structural event for the live log.
fn summarize(ev: &AgentEvent) -> String {
    match ev {
        AgentEvent::StageStarted { agent, input } => {
            format!("[stage started] {} <- {input:?}", agent.0)
        }
        AgentEvent::StageProduced { agent, keys } => {
            format!("[stage produced] {} -> {keys:?}", agent.0)
        }
        AgentEvent::StageFinished { agent, outcome } => {
            format!("[stage finished] {} ({outcome:?})", agent.0)
        }
        AgentEvent::ToolStarted { name } => format!("[tool started] {name}"),
        AgentEvent::ToolProduced { name, target } => format!("[tool produced] {name} -> {target}"),
        AgentEvent::ToolRejected { name, reason } => format!("[tool rejected] {name}: {reason}"),
        AgentEvent::ToolCallProposed { id, name } => format!("[tool proposed] {name} (#{id})"),
        AgentEvent::ToolArgsDelta { fragment, .. } => format!("[tool args] {fragment}"),
        AgentEvent::Usage {
            prompt,
            completion,
            reasoning,
            total,
        } => format!(
            "[usage] prompt={prompt} completion={completion} reasoning={reasoning:?} total={total}"
        ),
        AgentEvent::ReasoningDelta { .. } => "[reasoning]".to_string(),
        AgentEvent::ContentDelta { text } => format!("[content] {text}"),
        AgentEvent::Finished { assistant } => {
            format!("[finished] {} chars", assistant.chars().count())
        }
        AgentEvent::Error { message } => format!("[error] {message}"),
    }
}

#[tokio::test]
#[ignore = "touches the live MCP server + OpenRouter; run with --ignored"]
async fn ss_chat_pipeline_answers_the_manager_question_set() {
    let _ = dotenvy::dotenv();

    let mcp_url = require_env("DATACENTER_MCP_URL");
    let api_key = require_env("OPENROUTER_API_KEY");
    let model = require_env("OPENROUTER_MODEL");
    let base_url = std::env::var("OPENROUTER_BASE_URL")
        .unwrap_or_else(|_| "https://openrouter.ai/api/v1".into());

    // ── the boot inputs, loaded exactly as the server loads them ──
    let app_config = AppConfig::load("config/config.toml").expect("load config/config.toml");
    let prompts = PromptBank::from_app_config(&app_config).expect("resolve prompt bank");
    let grants = &app_config.ss_chat_grants;

    // ── connect to the MCP server and discover its tools ──
    let client = McpClient::connect_http(&mcp_url)
        .await
        .unwrap_or_else(|e| panic!("connect MCP at {mcp_url}: {e:#}"));
    let handle = client.handle();
    let discovered = handle
        .list_openrouter_tools()
        .await
        .expect("list MCP tools");
    let instructions = client.server_instructions();

    let resolved = ResolvedLlm {
        provider: Provider::OpenRouter,
        base_url,
        model,
        temperature: 0.2,
        top_p: 0.1,
        // Finding 3 of the reference document: a tight cap truncates a reply mid-table, and with
        // several large tool payloads in context the whole budget can go to reasoning tokens.
        max_tokens: 8192,
        api_key: Some(api_key),
        reasoning_effort: None,
        app_url: None,
        app_title: None,
    };

    for (label, prompt) in selected_cases() {
        eprintln!("\n\n═══════════════ {label} ═══════════════\nUser — {prompt}\n");

        // One shared sink per case, so each run's stages stream independently.
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(8192);
        let sink: Arc<dyn EventSink> = Arc::new(ChannelSink(tx));

        // The production builder, with the production prompts + grant.
        let orchestrator = build_ss_chat_pipeline(
            handle.clone(),
            &discovered,
            instructions.as_deref(),
            &prompts.ss_fetcher_system,
            &prompts.ss_analyst_system,
            &prompts.ss_charter_system,
            &grants.fetcher,
            &grants.charter,
            &resolved,
            Some(sink.clone()),
        )
        .expect("build the /ss-chat pipeline");

        let drive = sink.clone();
        let run = tokio::spawn(async move {
            orchestrator
                .run_emitting(
                    &ss_chat_pipeline_id(),
                    AgentPayload::Initial(InitialPrompt {
                        prompt,
                        history: vec![],
                        now: SystemClock::default().now(), // stamp the turn once at the boundary
                    }),
                    &*drive,
                )
                .await
        });

        // Stop on the terminal event, not on channel close: the orchestrator's stages hold sink
        // clones for the whole run, so waiting on close would deadlock. A real SSE consumer
        // likewise stops on the terminal frame.
        // Production wiring builds bare `McpTool`s (only `StreamingTool` emits `ToolStarted`), so
        // the event that actually names a call here is `ToolCallProposed` — the same one
        // `insight_frames` turns into the SSE `tool_call` frame.
        let mut tools_called: Vec<String> = Vec::new();
        let mut content_deltas = 0usize;
        while let Some(ev) = rx.recv().await {
            let terminal = matches!(ev, AgentEvent::Finished { .. } | AgentEvent::Error { .. });
            match &ev {
                AgentEvent::ContentDelta { text } => {
                    content_deltas += 1;
                    eprint!("{text}");
                }
                AgentEvent::ToolCallProposed { name, .. } | AgentEvent::ToolStarted { name } => {
                    tools_called.push(name.clone());
                    eprintln!("\n{}", summarize(&ev));
                }
                other => eprintln!("\n{}", summarize(other)),
            }
            if terminal {
                break;
            }
        }
        eprintln!();

        let outcome = run.await.expect("join the pipeline task");
        assert!(outcome.is_ok(), "{label}: run failed: {:?}", outcome.err());
        let final_answer = match outcome.expect("checked ok above") {
            AgentPayload::Final(f) => f.assistant,
            other => panic!("{label}: expected Final, got {:?}", other.kind()),
        };
        assert!(
            !final_answer.trim().is_empty(),
            "{label}: the finalizer must produce a non-empty answer"
        );

        // The grant invariant: this MCP server advertises the EV-charging tools too, so a
        // regression in `[ss_chat.grants]` (or a `"*"` creeping back in) would show up here as the
        // SS pipeline reaching for `bill_revenue` — with 星星電力 branding on the answer.
        // `emit_chart` is the charter's built-in code-backed sink, not an MCP tool.
        for name in &tools_called {
            assert!(
                name.starts_with("ss_") || name == "emit_chart",
                "{label}: the SS pipeline called non-SS tool `{name}` (called: {tools_called:?})"
            );
        }
        // Every question in the set is a data question, so at least one `ss_*` tool must have run
        // — otherwise the answer was written without touching the investor platform at all.
        assert!(
            tools_called.iter().any(|name| name.starts_with("ss_")),
            "{label}: no ss_* tool was called; the answer cannot be grounded in real data \
             (called: {tools_called:?})"
        );
        assert!(
            content_deltas >= 1,
            "{label}: the analyst must stream its answer as ContentDelta events"
        );

        eprintln!(
            "\n─── {label}: OK ({} chars, tools: {tools_called:?}) ───",
            final_answer.chars().count()
        );
    }

    let _ = client.shutdown().await;
}
