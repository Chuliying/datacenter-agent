//! The core HTTP server.

pub mod auth;
pub mod dto;
pub mod error;
pub mod greeting;
pub mod handler;
pub mod route;
pub mod state;

pub use route::build_router;
pub use state::AppState;
