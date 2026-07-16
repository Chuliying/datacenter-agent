//! Domain-neutral agent runtime core.
//!
//! This module is intentionally independent from HTTP, Axum, and server DTOs.

pub mod audit;
pub mod config;
pub mod error;
pub mod eval;
pub mod guardrails;
pub mod input;
pub mod llm_normalizer;
pub mod memory;
pub mod registry;
pub mod schema;
pub mod turn;
