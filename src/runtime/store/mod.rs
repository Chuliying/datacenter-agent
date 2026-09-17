//! Durable runtime store contracts (S-RUNTIME-SEC-01, Slice 1).
//!
//! Repository-only POC: opaque users, session ownership with TTL, minimized
//! turn summaries, and an atomic per-actor monthly micro-USD ledger, all backed
//! by one SQLite file. Nothing here is wired into the request path; callers are
//! tests and a future integration slice.
//!
//! Every method takes an injected `now: DateTime<Utc>` — the repository never
//! reads the wall clock, so expiry, month boundaries, and pruning are
//! deterministic under test.

pub mod month;
pub mod sanitize;
pub mod sqlite;

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use regex::Regex;

/// POC monthly budget: USD 20 in integer micro-USD.
pub const MONTHLY_BUDGET_MICRO: i64 = 20_000_000;

/// Upper bound for actor keys and session IDs. Anything longer is a typed
/// validation error, never truncated (identifiers are keys, not content).
pub const MAX_IDENTIFIER_CHARS: usize = 128;

/// Store construction parameters (PRD FR-001 input table).
#[derive(Debug, Clone)]
pub struct StoreConfig {
    /// SQLite database file path. Must be openable; the store never falls back
    /// to memory (ERR-001).
    pub db_path: PathBuf,
    /// Maximum retained summaries per session. POC default and maximum: 5.
    pub max_turns: usize,
    /// Session TTL in days from the last successful append. POC: 30.
    pub ttl_days: u32,
    /// SQLite busy timeout in milliseconds. Default 5000.
    pub busy_timeout_ms: u64,
    /// Per-field character limit applied after redaction. Default 500.
    pub summary_char_limit: usize,
    /// Sensitive patterns redacted from every summary field before insert.
    pub redact_patterns: Vec<Regex>,
}

impl StoreConfig {
    /// Config with PRD defaults for everything but the database path.
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            db_path,
            max_turns: 5,
            ttl_days: 30,
            busy_timeout_ms: 5000,
            summary_char_limit: 500,
            redact_patterns: sanitize::default_redact_patterns(),
        }
    }
}

/// Typed repository errors. Error values never carry stored content
/// (NFR Security); budget exhaustion is not an error — it is
/// [`ReserveOutcome::BudgetExceeded`], per PRD FR-003's output table.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The database cannot be opened, migrated, or a transaction failed.
    /// The store never silently falls back to in-memory storage (ERR-001).
    #[error("runtime store unavailable: {0}")]
    Unavailable(String),
    /// The session is owned by a different, non-expired actor (ERR-002).
    /// Deliberately carries no owner key, summaries, or existence details.
    #[error("session is owned by a different actor")]
    OwnershipConflict,
    /// Structurally invalid actor key or session ID (empty or over-length).
    /// Summary content never triggers this (FR-002 boundary).
    #[error("invalid identifier: {0}")]
    InvalidIdentifier(&'static str),
    /// The reservation ID is unknown, already settled, or reused with a
    /// different actor, month, or amount (FR-003 boundary).
    #[error(
        "unknown or mismatched reservation id (wrong actor, month, amount, or already settled)"
    )]
    ReservationMismatch,
}

/// One owned session row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    /// Session ID (caller-supplied, validated bounded identifier).
    pub session_id: String,
    /// Opaque owner key.
    pub actor_key: String,
    /// Creation timestamp (ms since epoch, from the injected instant).
    pub created_at_ms: u64,
    /// Last successful append timestamp (ms); creation counts as the first
    /// append, reads never refresh it.
    pub last_append_ms: u64,
}

/// Caller-supplied summary fields for one turn. Field names align with the
/// existing in-memory [`SessionMemoryTurn`](crate::runtime::memory::store::SessionMemoryTurn)
/// so the future integration slice maps one-to-one; timestamps always come
/// from the injected instant, never from the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnSummaryInput {
    /// Turn id.
    pub turn_id: String,
    /// User-side summary (sanitized before insert).
    pub user_summary: String,
    /// Answer-side summary (sanitized before insert).
    pub answer_summary: String,
    /// Intent id.
    pub intent: Option<String>,
    /// Metric id.
    pub metric: Option<String>,
    /// Asset id/name.
    pub asset: Option<String>,
    /// Time range label.
    pub time_range_label: Option<String>,
    /// Option id.
    pub option_id: Option<String>,
}

/// One persisted, sanitized turn summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredTurn {
    /// Monotonic per-session sequence.
    pub seq: u64,
    /// Turn id.
    pub turn_id: String,
    /// Sanitized user-side summary.
    pub user_summary: String,
    /// Sanitized answer-side summary.
    pub answer_summary: String,
    /// Intent id.
    pub intent: Option<String>,
    /// Metric id.
    pub metric: Option<String>,
    /// Asset id/name.
    pub asset: Option<String>,
    /// Time range label.
    pub time_range_label: Option<String>,
    /// Option id.
    pub option_id: Option<String>,
    /// Persisted-at timestamp (ms since epoch, from the injected instant).
    pub created_at_ms: u64,
}

/// Snapshot of one actor's active-month ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerSnapshot {
    /// Asia/Taipei month key, e.g. `2026-08`.
    pub month_key: String,
    /// Settled spend in micro-USD.
    pub spent_micro: i64,
    /// Pending reserved amount in micro-USD.
    pub reserved_micro: i64,
    /// Remaining budget in micro-USD, clamped at 0 (an overdrawn month
    /// reports 0 remaining, never a negative number).
    pub remaining_micro: i64,
    /// UTC instant at which the next Asia/Taipei month starts.
    pub next_reset_utc: DateTime<Utc>,
}

/// Outcome of one reservation attempt (FR-003).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReserveOutcome {
    /// The amount was reserved atomically.
    Reserved {
        /// Ledger state after the reservation.
        snapshot: LedgerSnapshot,
    },
    /// `spent + reserved + requested` would exceed the monthly budget, or the
    /// month is overdrawn. No reservation was created (ERR-003).
    BudgetExceeded {
        /// Ledger state at rejection time.
        snapshot: LedgerSnapshot,
        /// UTC instant at which reservations resume.
        next_reset_utc: DateTime<Utc>,
    },
}

/// Outcome of one settlement attempt (FR-003 settle result, four states).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettleOutcome {
    /// Cost moved from reserved to spent exactly once.
    Settled,
    /// Authoritative cost exceeded the reservation; the actual amount was
    /// charged and the month is now overdrawn (ERR-006).
    SettledWithOverage {
        /// How far `spent + reserved` now exceeds the monthly budget.
        overdrawn_by_micro: i64,
    },
    /// This reservation was already settled; nothing changed (AC-005).
    AlreadySettled,
    /// No authoritative cost was supplied; the reservation stays pending at
    /// its reserved amount until reconciliation (ERR-004).
    ReconciliationRequired,
}
