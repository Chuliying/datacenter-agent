//! `datacenter-agent` HTTP server.
//!
//! Loads the top-level `config.toml`, connects to the datacenter MCP server and
//! discovers its tools, builds the LLM defaults at startup, hands all of it to
//! the axum app, and serves until SIGINT / SIGTERM.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use mimalloc::MiMalloc;
use tokio::net::TcpListener;
use tracing::{info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use datacenter_agent::appstate::{
    load_global_token, load_mcp_url, AppState, LlmDefaults, PromptBank,
};
use datacenter_agent::config::AppConfig;
use datacenter_agent::mcp_client::McpClient;
use datacenter_agent::server::{build_router, greeting};

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

/// CLI arguments. Every flag has an env fallback so the same binary
/// works for `cargo run` and for container deployments.
#[derive(Parser, Debug)]
#[command(name = env!("CARGO_PKG_NAME"), version)]
struct Cli {
    /// Bind address.
    #[arg(long, env = "HOST", default_value = "0.0.0.0")]
    host: String,

    /// Bind port.
    #[arg(long, env = "PORT", default_value_t = 8080)]
    port: u16,

    /// Top-level application config TOML. Every path inside it is
    /// resolved relative to this file's parent directory, so mounting
    /// the whole `config/` folder into a container "just works".
    #[arg(long = "config", default_value = "config/config.toml")]
    config: PathBuf,

    /// Enable verbose (debug-level) logging for this crate.
    #[arg(short = 'v', long = "debug")]
    debug: bool,
}

// ──── banner ───

/// Print a fancy startup banner to stdout.
fn print_banner(cli: &Cli, llm_setting: &LlmDefaults, mcp_url: &str) {
    let name = env!("CARGO_PKG_NAME");
    let version = env!("CARGO_PKG_VERSION");
    let level = if cli.debug { "DEBUG" } else { "INFO" };

    println!("   __ _____  ___  ____  ___  _____  ___                __");
    println!("  / // / _ \\/ _ \\/ __/ / _ \\/ ___/ / _ |___ ____ ___  / /_");
    println!(" / _  / // / , _/ _/  / // / /__  / __ / _ `/ -_) _ \\/ __/");
    println!("/_//_/____/_/|_/___/ /____/\\___/ /_/ |_\\_, /\\__/_//_/\\__/ ");
    println!("                                      /___/ ── Analytic datacenter agent");

    println!("────────────────────────────────────────────────────────────────────────");
    println!("{name} v{version}");
    println!();

    let bind = format!("{}:{}", cli.host, cli.port);
    println!("•  Bind           {bind}");
    println!("•  MCP            {mcp_url}");
    println!("•  Model          {}", llm_setting.model);
    println!("•  Config file    {}", cli.config.display());
    println!("•  Log level      {level}");
    println!("────────────────────────────────────────────────────────────────────────");
}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse command-line arguments.
    let cli = Cli::parse();

    // Initialize log tracing with optional debug-level verbosity.
    init_tracing(cli.debug);

    // Load environment variables from a .env file if it exists.
    let _ = dotenvy::dotenv();

    // Load the main configuration TOML file.
    let app_config = AppConfig::load(&cli.config)
        .with_context(|| format!("load app config {}", cli.config.display()))?;

    // Build the prompt bank from the loaded configuration.
    let prompts = Arc::new(PromptBank::from_app_config(&app_config)?);

    // Load LLM defaults (e.g. model, temperature) from environment variables.
    let llm = LlmDefaults::from_env()?;

    // Load the global authorization token used to authenticate incoming requests.
    let token = load_global_token()?;

    // ──── connect to MCP and discover its tools ───

    // Retrieve the target MCP URL from environment / config.
    let mcp_url = load_mcp_url()?;

    // Output the visual application startup banner to stdout.
    print_banner(&cli, &llm, &mcp_url);

    // Establish HTTP connection to the MCP server.
    let mcp_client = McpClient::connect_http(&mcp_url)
        .await
        .with_context(|| format!("mcp_error: failed to connect MCP at {mcp_url}"))?;

    // Extract the client handle for interacting with the MCP server.
    let mcp = mcp_client.handle();

    // Query the MCP server to retrieve all available OpenRouter-compatible tools.
    let tools = mcp
        .list_openrouter_tools()
        .await
        .context("listing MCP tools")?;
    let tool_names: Vec<&String> = tools.iter().map(|t| &t.function.name).collect();
    info!(count = tools.len(), names = ?tool_names, "discovered MCP tools");

    // Fetch custom system/server instructions if provided by the MCP client.
    let instructions = mcp_client.server_instructions();

    // Construct the global shared application state with all dependencies.
    let state = AppState::new(
        mcp,
        Arc::new(tools),
        Arc::new(instructions),
        llm,
        prompts,
        token,
    )?;

    // Spawn async background tasks to periodically run greeting sequences/checks.
    greeting::spawn_greeting_tasks(state.clone(), 5);

    // Build the Axum routing tree populated with middleware and routes.
    let app = build_router(state);

    // Parse the host and port into a socket address.
    let addr: SocketAddr = format!("{}:{}", cli.host, cli.port)
        .parse()
        .with_context(|| format!("parse bind address {}:{}", cli.host, cli.port))?;

    // Bind the TCP listener to the parsed address.
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind {addr}"))?;
    info!(%addr, "HDRE DC agent listening");

    // Start the Axum HTTP server and run it with graceful shutdown enabled.
    let serve_result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error");

    // Close the MCP connection cleanly regardless of how serving ended.
    if let Err(e) = mcp_client.shutdown().await {
        warn!(error = %e, "error during MCP shutdown");
    }
    serve_result?;

    info!("HDRE DC agent stopped");
    Ok(())
}

/// Initialise `tracing-subscriber`.
///
/// Precedence: `RUST_LOG` wins if set; otherwise `--debug` flips the
/// crate filter to `debug`. Emits a one-shot warn when both are set so
/// the user knows `RUST_LOG` is taking over.
fn init_tracing(debug_flag: bool) {
    // Check if the user has explicitly defined the RUST_LOG environment variable.
    let env_set = std::env::var("RUST_LOG").is_ok();

    // Determine the environment filter configuration based on RUST_LOG or the debug CLI flag.
    let filter = if env_set {
        EnvFilter::try_from_default_env().expect("RUST_LOG already validated")
    } else if debug_flag {
        EnvFilter::new(
            "datacenter_agent=debug,tower_http=debug,axum=debug,info,hyper=warn,h2=warn,rustls=warn",
        )
    } else {
        EnvFilter::new("info,hyper=warn,h2=warn,rustls=warn")
    };

    // Initialize the global tracing registry with formatting and filters.
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stderr))
        .init();

    // Warn if both are set to clarify that the environment variable overrides the debug flag.
    if env_set && debug_flag {
        warn!("RUST_LOG is set; --debug flag is ignored in favour of RUST_LOG");
    }
}

/// Cross-platform graceful shutdown: SIGINT/SIGTERM on Unix, Ctrl+C on
/// Windows. Returns on the first signal.
async fn shutdown_signal() {
    // Future that resolves when Ctrl+C (SIGINT) is captured.
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install Ctrl+C handler");
    };

    // Future that resolves when SIGTERM is captured (Unix only).
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };

    // Pending future fallback for non-Unix platforms.
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    // Wait for the first of the signals to arrive.
    tokio::select! {
        _ = ctrl_c => info!("SIGINT received, shutting down"),
        _ = terminate => info!("SIGTERM received, shutting down"),
    }
}
