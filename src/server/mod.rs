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

//! The core HTTP server: routing, middleware, auth, handlers, DTOs, error mapping, the
//! OpenAI-compatible layer, and the boot-time greeting task.
//!
//! The submodules are:
//!
//! - [`route`] — router + middleware assembly ([`route::build_router`]): two sub-routers
//!   (standard 5 routes / OpenAI 1 route) each carrying their own timeout and auth layer, merged
//!   under a shared 64 KiB body cap, permissive CORS, trace/compression, and security headers,
//!   with an explicit outer fallback answering a uniform `404`.
//! - [`handler`] — the six handlers: `health` / `ready` / `greeting` / `agent_stream` /
//!   `ss_chat_stream` / `chat_completions`.
//! - [`openai`] — OpenAI-compatible DTOs and mapping (`ChatCompletionRequest`, `map_request`,
//!   `error_type_for_status`, the error envelope).
//! - [`dto`] — request/response types: `AgentRequest`, `StreamFrame` and its data payloads,
//!   `GreetingResponse`, `ReadyBody`.
//! - [`auth`] — the two bearer middlewares: `require_bearer` (`418`) and `require_bearer_openai`
//!   (`401` + OpenAI error envelope), both constant-time.
//! - [`error`] — the outward HTTP error types (`AppError` / `ErrorBody`).
//! - [`greeting`] — the boot-time background task pre-generating greetings through the two-stage
//!   greeting pipeline.

pub mod auth;
pub mod dto;
pub mod error;
pub mod greeting;
pub mod handler;
pub mod openai;
pub mod rate_limit;
pub mod route;

pub use crate::appstate::AppState;
pub use route::build_router;
