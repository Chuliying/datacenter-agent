//! Live end-to-end integration test for the MCP tool-calling loop.
//!
//! Gated `#[ignore]` AND short-circuits when `OPENROUTER_API_KEY` or
//! `DATACENTER_MCP_URL` is unset, so `cargo test` in CI without credentials / a
//! running MCP server stays green. Run locally (with MCP server booted up)
//! with:
//!
//! ```sh
//! cargo test --test llm_connector -- --ignored
//! ```

use std::sync::Arc;

use datacenter_agent::llm_connector;
use datacenter_agent::mcp_client::McpClient;
use datacenter_agent::model::GenerationConfig;

#[tokio::test]
#[ignore]
async fn live_generates_markdown_via_mcp() {
    let _ = dotenvy::dotenv();

    let api_key = match std::env::var("OPENROUTER_API_KEY") {
        Ok(k) if !k.is_empty() && k != "sk-or-v1-REPLACE_ME" => k,
        _ => {
            eprintln!("skipping: OPENROUTER_API_KEY not set");
            return;
        }
    };
    let mcp_url = match std::env::var("DATACENTER_MCP_URL") {
        Ok(u) if !u.is_empty() => u,
        _ => {
            eprintln!("skipping: DATACENTER_MCP_URL not set");
            return;
        }
    };

    // Connect to the live MCP server and discover its tools.
    let client = McpClient::connect_http(&mcp_url)
        .await
        .expect("connect to datacenter MCP server");
    let mcp = client.handle();
    let tools = Arc::new(mcp.list_openrouter_tools().await.expect("list MCP tools"));
    assert!(!tools.is_empty(), "MCP server exposed no tools");

    let instructions = client.server_instructions();
    let base = "You are a data assistant for an EV-charging network. Use the data \
        tools when a question needs real data, then answer in concise \
        GitHub-Flavored Markdown.";
    let system = match instructions.as_deref() {
        Some(instr) if !instr.trim().is_empty() => format!("{base}\n\n{instr}"),
        _ => base.to_string(),
    };

    let cfg = GenerationConfig {
        system,
        user_prompt: "近三個月的整體營收概況如何？".to_string(),
        history: vec![],
        api_key,
        base_url: std::env::var("OPENROUTER_BASE_URL")
            .unwrap_or_else(|_| "https://openrouter.ai/api/v1".into()),
        model: std::env::var("OPENROUTER_MODEL")
            .unwrap_or_else(|_| "anthropic/claude-sonnet-4.6".into()),
        app_url: std::env::var("OPENROUTER_APP_URL").ok(),
        app_title: std::env::var("OPENROUTER_APP_TITLE").ok(),
        temperature: 0.2,
        top_p: 0.1,
        max_tokens: 1024,
    };

    let md = llm_connector::generate(cfg, tools, mcp)
        .await
        .expect("generate returned an error");

    client.shutdown().await.expect("mcp shutdown");

    assert!(!md.trim().is_empty(), "response was empty");
    let looks_markdown =
        md.contains('#') || md.contains("\n- ") || md.contains('|') || md.contains("```");
    assert!(looks_markdown, "response did not look like Markdown:\n{md}");
}
