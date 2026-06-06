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

//! The rmcp **client** side of the agent.
//!
//! ## Friendly note
//!
//! [`McpClient`] connects to the datacenter MCP server over HTTP
//!  and performs the MCP `initialize` handshake.
//!
//! During the initilization, the MCP server sends its instructions to the client,
//! which are then stored and sent to the LLM on every request.
//!
//! We store `mcp_handle` in the application state `AppState` and pass it
//! to every request handler.

use anyhow::{Context, Result};
use async_openai::types::chat::{ChatCompletionTool, FunctionObjectArgs};
use rmcp::{
    model::CallToolRequestParams, service::RunningService,
    transport::StreamableHttpClientTransport, Peer, RoleClient, ServiceExt,
};

/// Owns the running MCP connection.
pub struct McpClient {
    service: RunningService<RoleClient, ()>,
}

/// Cloneable call handle, wrapping an rmcp [`Peer`].
#[derive(Clone)]
pub struct McpHandle {
    peer: Peer<RoleClient>,
}

impl McpClient {
    /// Connect to a MCP server over HTTP.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the `initialize` handshake with `url` fails (e.g. the
    /// server is not running or the URL is wrong).
    pub async fn connect_http(url: &str) -> Result<Self> {
        let transport = StreamableHttpClientTransport::from_uri(url);

        // `()` is the unit client handler (we don't service server-initiated
        // requests). `.serve()` runs the `initialize` handshake.
        let service = ()
            .serve(transport)
            .await
            .with_context(|| format!("MCP initialize handshake with {url} failed"))?;

        // Only logs a subset of fiends, avoid display full instruction.
        let (server, version) = service
            .peer()
            .peer_info()
            .map(|info| (info.server_info.name.clone(), info.server_info.version.clone()))
            .unwrap_or_else(|| ("unknown".into(), "unknown".into()));
        tracing::info!(%url, %server, %version, "registered as MCP client");
        Ok(Self { service })
    }

    /// Get a cloneable handle for issuing tool calls.
    pub fn handle(&self) -> McpHandle {
        McpHandle {
            peer: self.service.peer().clone(),
        }
    }

    /// The server's handshake `instructions` block, if any.
    ///
    /// These carry information that the LLM should see once up front,
    /// so LLM doesn't have to guess how to use the tools.
    pub fn server_instructions(&self) -> Option<String> {
        self.service
            .peer()
            .peer_info()
            .and_then(|info| info.instructions.clone())
    }

    /// Gracefully cancel the connection.
    ///
    /// # Errors
    ///
    /// Returns `Err` if cancelling the underlying rmcp service fails.
    pub async fn shutdown(self) -> Result<()> {
        self.service
            .cancel()
            .await
            .context("cancelling the MCP service")?;
        Ok(())
    }
}

impl McpHandle {
    /// List the server's tools and convert each into an async-openai
    /// [`ChatCompletionTool`].
    ///
    /// Serves as the bridge between protocols: an MCP `Tool` exposes a
    /// `name`, an optional `description`, and an `input_schema` (a JSON Schema
    /// object) which resembles `async-openai`'s `FunctionObject`.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the `tools/list` request fails or a returned schema
    /// cannot be turned into a function object.
    pub async fn list_openrouter_tools(&self) -> Result<Vec<ChatCompletionTool>> {
        let tools = self
            .peer
            .list_all_tools()
            .await
            .context("listing tools from the MCP server")?;

        tools
            .into_iter()
            .map(|t| {
                let function = FunctionObjectArgs::default()
                    .name(t.name.to_string())
                    .description(t.description.map(|d| d.to_string()).unwrap_or_default())
                    // The MCP input schema is a JSON Schema object.
                    .parameters(serde_json::Value::Object((*t.input_schema).clone()))
                    .build()
                    .context("building a function object from an MCP tool")?;
                Ok(ChatCompletionTool { function })
            })
            .collect()
    }

    /// Call a tool by name and return its text content joined into a single
    /// string.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the `tools/call` request itself fails at the transport
    /// level.
    ///
    /// A tool that *runs* but reports an error result is returned as `Ok` (with
    /// a warning logged) so the caller can feed the detail back to the model for
    /// self-correction.
    pub async fn call_tool_text(
        &self,
        name: &str,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> Result<String> {
        let params = CallToolRequestParams {
            meta: None,
            name: name.to_string().into(),
            arguments: Some(args),
            task: None,
        };
        let result = self
            .peer
            .call_tool(params)
            .await
            .with_context(|| format!("calling MCP tool `{name}`"))?;

        // A tool result is a list of content blocks; collect the text ones.
        let text = result
            .content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.clone()))
            .collect::<Vec<_>>()
            .join("\n");

        if result.is_error == Some(true) {
            tracing::warn!(tool = %name, "MCP tool reported an error result");
        }
        Ok(text)
    }
}
