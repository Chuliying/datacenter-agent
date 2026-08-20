//! SQLite-backed runtime store (Slice 1, steps S5–S6).
//!
//! One `tokio-rusqlite` connection = one dedicated database thread, so every
//! repository call on a handle is naturally serialized; `BEGIN IMMEDIATE`
//! still guards every read-check-write so a second connection to the same
//! file (tests, future tooling) cannot interleave (AC-004).
//!
//! All timestamps come from the injected `now` — the store never reads the
//! wall clock.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio_rusqlite::rusqlite::{self, params, OptionalExtension, Transaction, TransactionBehavior};
use tokio_rusqlite::Connection;

use super::{
    month, sanitize, LedgerSnapshot, ReserveOutcome, SessionRecord, SettleOutcome, StoreConfig,
    StoreError, StoredTurn, TurnSummaryInput, MAX_IDENTIFIER_CHARS, MONTHLY_BUDGET_MICRO,
};

/// Schema version this binary writes and accepts.
const SCHEMA_VERSION: i64 = 1;

/// Async SQLite repository handle. Cloneable; clones share the same dedicated
/// database thread.
#[derive(Clone)]
pub struct SqliteRuntimeStore {
    conn: Connection,
    config: Arc<StoreConfig>,
}

fn unavailable(err: impl std::fmt::Display) -> StoreError {
    StoreError::Unavailable(err.to_string())
}

fn validate_identifier(value: &str, what: &'static str) -> Result<(), StoreError> {
    if value.is_empty() || value.chars().count() > MAX_IDENTIFIER_CHARS {
        return Err(StoreError::InvalidIdentifier(what));
    }
    Ok(())
}

fn epoch_ms(now: DateTime<Utc>) -> i64 {
    now.timestamp_millis()
}

impl SqliteRuntimeStore {
    /// Open (or create) the database file, apply PRAGMAs, and run migrations.
    /// Never falls back to in-memory storage (ERR-001).
    pub async fn open(config: StoreConfig) -> Result<Self, StoreError> {
        // FR-001: the POC accepts 1..=5 retained turns; refuse loudly instead
        // of clamping so a mis-set config cannot silently change retention.
        if config.max_turns == 0 || config.max_turns > 5 {
            return Err(StoreError::Unavailable(format!(
                "max_turns must be within 1..=5 for this POC, got {}",
                config.max_turns
            )));
        }
        let conn = Connection::open(&config.db_path)
            .await
            .map_err(unavailable)?;
        let busy_timeout = Duration::from_millis(config.busy_timeout_ms);
        run_on(&conn, move |conn| {
            conn.busy_timeout(busy_timeout).map_err(unavailable)?;
            // WAL is requested and *verified*: a filesystem that cannot
            // take WAL (e.g. some network mounts) must fail loudly here,
            // not corrupt silently later.
            let mode: String = conn
                .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
                .map_err(unavailable)?;
            if !mode.eq_ignore_ascii_case("wal") {
                return Err(StoreError::Unavailable(format!(
                    "journal_mode=WAL rejected; filesystem answered {mode}"
                )));
            }
            conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")
                .map_err(unavailable)?;
            migrate(conn)
        })
        .await?;
        Ok(Self {
            conn,
            config: Arc::new(config),
        })
    }

    /// Run `f` on the database thread, flattening transport errors into
    /// [`StoreError::Unavailable`].
    async fn with_conn<T, F>(&self, f: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&mut rusqlite::Connection) -> Result<T, StoreError> + Send + 'static,
    {
        run_on(&self.conn, f).await
    }

    /// Create the session for this actor, or validate existing ownership.
    /// An expired or cleared session ID is claimable as a brand-new session
    /// by any actor (AC-012, AC-013). Creation counts as the first append.
    pub async fn ensure_session(
        &self,
        actor_key: &str,
        session_id: &str,
        now: DateTime<Utc>,
    ) -> Result<SessionRecord, StoreError> {
        validate_identifier(actor_key, "actor key")?;
        validate_identifier(session_id, "session id")?;
        let (actor_key, session_id) = (actor_key.to_string(), session_id.to_string());
        let ttl_ms = self.ttl_ms();
        self.with_conn(move |conn| {
            let tx = immediate(conn)?;
            let record = ensure_session_tx(&tx, &actor_key, &session_id, now, ttl_ms)?;
            tx.commit().map_err(unavailable)?;
            Ok(record)
        })
        .await
    }

    /// Sanitize and append one turn summary, keeping at most `max_turns`
    /// per session, and refresh the session's last-append instant.
    pub async fn append_turn(
        &self,
        actor_key: &str,
        session_id: &str,
        turn: TurnSummaryInput,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        validate_identifier(actor_key, "actor key")?;
        validate_identifier(session_id, "session id")?;
        let turn = self.sanitize_turn(turn);
        let (actor_key, session_id) = (actor_key.to_string(), session_id.to_string());
        let (ttl_ms, max_turns) = (self.ttl_ms(), self.config.max_turns as i64);
        self.with_conn(move |conn| {
            let tx = immediate(conn)?;
            ensure_session_tx(&tx, &actor_key, &session_id, now, ttl_ms)?;
            let now_ms = epoch_ms(now);
            tx.execute(
                "INSERT INTO session_turns(session_id, seq, turn_id, user_summary, \
                 answer_summary, intent, metric, asset, time_range_label, option_id, \
                 created_at_ms) \
                 VALUES (?1, (SELECT COALESCE(MAX(seq), 0) + 1 FROM session_turns \
                 WHERE session_id = ?1), ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    session_id,
                    turn.turn_id,
                    turn.user_summary,
                    turn.answer_summary,
                    turn.intent,
                    turn.metric,
                    turn.asset,
                    turn.time_range_label,
                    turn.option_id,
                    now_ms,
                ],
            )
            .map_err(unavailable)?;
            tx.execute(
                "DELETE FROM session_turns WHERE session_id = ?1 AND seq <= \
                 (SELECT MAX(seq) FROM session_turns WHERE session_id = ?1) - ?2",
                params![session_id, max_turns],
            )
            .map_err(unavailable)?;
            tx.execute(
                "UPDATE sessions SET last_append_ms = ?2 WHERE session_id = ?1",
                params![session_id, now_ms],
            )
            .map_err(unavailable)?;
            tx.commit().map_err(unavailable)?;
            Ok(())
        })
        .await
    }

    /// Load the non-expired sanitized summaries in chronological order.
    /// An expired or unknown session returns an empty list; a live session
    /// owned by a different actor returns [`StoreError::OwnershipConflict`].
    /// Reads never refresh expiry.
    pub async fn load_recent(
        &self,
        actor_key: &str,
        session_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<StoredTurn>, StoreError> {
        validate_identifier(actor_key, "actor key")?;
        validate_identifier(session_id, "session id")?;
        let (actor_key, session_id) = (actor_key.to_string(), session_id.to_string());
        let (ttl_ms, max_turns) = (self.ttl_ms(), self.config.max_turns as i64);
        self.with_conn(move |conn| {
            let tx = conn.transaction().map_err(unavailable)?;
            let Some((owner, last_append_ms)) = session_row(&tx, &session_id)? else {
                return Ok(Vec::new());
            };
            if is_expired(last_append_ms, ttl_ms, now) {
                return Ok(Vec::new());
            }
            if owner != actor_key {
                return Err(StoreError::OwnershipConflict);
            }
            let mut stmt = tx
                .prepare(
                    "SELECT seq, turn_id, user_summary, answer_summary, intent, metric, \
                     asset, time_range_label, option_id, created_at_ms \
                     FROM session_turns WHERE session_id = ?1 ORDER BY seq ASC LIMIT ?2",
                )
                .map_err(unavailable)?;
            let turns = stmt
                .query_map(params![session_id, max_turns], |row| {
                    Ok(StoredTurn {
                        seq: row.get::<_, i64>(0)? as u64,
                        turn_id: row.get(1)?,
                        user_summary: row.get(2)?,
                        answer_summary: row.get(3)?,
                        intent: row.get(4)?,
                        metric: row.get(5)?,
                        asset: row.get(6)?,
                        time_range_label: row.get(7)?,
                        option_id: row.get(8)?,
                        created_at_ms: row.get::<_, i64>(9)? as u64,
                    })
                })
                .map_err(unavailable)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(unavailable)?;
            Ok(turns)
        })
        .await
    }

    /// Delete the session row and its turns in one transaction, releasing
    /// ownership (AC-013). Clearing an unknown session is a no-op.
    pub async fn clear_session(
        &self,
        actor_key: &str,
        session_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        validate_identifier(actor_key, "actor key")?;
        validate_identifier(session_id, "session id")?;
        let (actor_key, session_id) = (actor_key.to_string(), session_id.to_string());
        let ttl_ms = self.ttl_ms();
        self.with_conn(move |conn| {
            let tx = immediate(conn)?;
            let Some((owner, last_append_ms)) = session_row(&tx, &session_id)? else {
                return Ok(());
            };
            if !is_expired(last_append_ms, ttl_ms, now) && owner != actor_key {
                return Err(StoreError::OwnershipConflict);
            }
            tx.execute(
                "DELETE FROM sessions WHERE session_id = ?1",
                params![session_id],
            )
            .map_err(unavailable)?;
            tx.commit().map_err(unavailable)?;
            Ok(())
        })
        .await
    }

    /// Prune expired sessions and their turns (bounded maintenance).
    /// Returns the pruned session count.
    pub async fn prune_expired(&self, now: DateTime<Utc>) -> Result<u64, StoreError> {
        let ttl_ms = self.ttl_ms();
        self.with_conn(move |conn| {
            let tx = immediate(conn)?;
            let pruned = tx
                .execute(
                    "DELETE FROM sessions WHERE last_append_ms + ?1 <= ?2",
                    params![ttl_ms, epoch_ms(now)],
                )
                .map_err(unavailable)?;
            tx.commit().map_err(unavailable)?;
            Ok(pruned as u64)
        })
        .await
    }

    /// Atomically reserve `amount_micro` against this actor's active
    /// Asia/Taipei month (FR-003). Idempotent for a duplicate reservation ID
    /// with the same actor, month, and amount; a mismatch is a typed error.
    pub async fn reserve(
        &self,
        actor_key: &str,
        reservation_id: &str,
        amount_micro: i64,
        now: DateTime<Utc>,
    ) -> Result<ReserveOutcome, StoreError> {
        validate_identifier(actor_key, "actor key")?;
        validate_identifier(reservation_id, "reservation id")?;
        if amount_micro <= 0 {
            return Err(StoreError::InvalidIdentifier(
                "reservation amount must be positive micro-USD",
            ));
        }
        let (actor_key, reservation_id) = (actor_key.to_string(), reservation_id.to_string());
        self.with_conn(move |conn| {
            let month_key = month::taipei_month_key(now);
            let next_reset_utc = month::next_reset_utc(now);
            let tx = immediate(conn)?;

            if let Some((owner, month, amount)) = reservation_identity(&tx, &reservation_id)? {
                if owner == actor_key && month == month_key && amount == amount_micro {
                    let snapshot = snapshot_tx(&tx, &actor_key, &month_key, next_reset_utc)?;
                    tx.commit().map_err(unavailable)?;
                    return Ok(ReserveOutcome::Reserved { snapshot });
                }
                return Err(StoreError::ReservationMismatch);
            }

            let (spent, reserved) = budget_row(&tx, &actor_key, &month_key)?;
            let committed = spent
                .checked_add(reserved)
                .ok_or_else(|| StoreError::Unavailable("ledger overflow".into()))?;
            let would_be = committed
                .checked_add(amount_micro)
                .ok_or_else(|| StoreError::Unavailable("ledger overflow".into()))?;
            // An overdrawn month (committed already at/over the limit) rejects
            // everything until reset or reconciliation (ERR-006).
            if committed >= MONTHLY_BUDGET_MICRO || would_be > MONTHLY_BUDGET_MICRO {
                let snapshot = snapshot_tx(&tx, &actor_key, &month_key, next_reset_utc)?;
                tx.commit().map_err(unavailable)?;
                return Ok(ReserveOutcome::BudgetExceeded {
                    snapshot,
                    next_reset_utc,
                });
            }

            tx.execute(
                "INSERT INTO monthly_budgets(actor_key, month_key) VALUES (?1, ?2) \
                 ON CONFLICT(actor_key, month_key) DO NOTHING",
                params![actor_key, month_key],
            )
            .map_err(unavailable)?;
            tx.execute(
                "UPDATE monthly_budgets SET reserved_micro = reserved_micro + ?3 \
                 WHERE actor_key = ?1 AND month_key = ?2",
                params![actor_key, month_key, amount_micro],
            )
            .map_err(unavailable)?;
            tx.execute(
                "INSERT INTO budget_reservations(reservation_id, actor_key, month_key, \
                 reserved_micro, state, created_at_ms) VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
                params![
                    reservation_id,
                    actor_key,
                    month_key,
                    amount_micro,
                    epoch_ms(now)
                ],
            )
            .map_err(unavailable)?;
            let snapshot = snapshot_tx(&tx, &actor_key, &month_key, next_reset_utc)?;
            tx.commit().map_err(unavailable)?;
            Ok(ReserveOutcome::Reserved { snapshot })
        })
        .await
    }

    /// Settle one reservation with an authoritative cost, or mark it for
    /// reconciliation when the cost is unknown (FR-003, ERR-004, ERR-006).
    /// Settling an unknown reservation ID is a [`StoreError::ReservationMismatch`].
    pub async fn settle(
        &self,
        reservation_id: &str,
        actual_micro: Option<i64>,
        now: DateTime<Utc>,
    ) -> Result<SettleOutcome, StoreError> {
        validate_identifier(reservation_id, "reservation id")?;
        if actual_micro.is_some_and(|amount| amount < 0) {
            return Err(StoreError::InvalidIdentifier(
                "settlement amount must be non-negative micro-USD",
            ));
        }
        let reservation_id = reservation_id.to_string();
        self.with_conn(move |conn| {
            let tx = immediate(conn)?;
            let row = tx
                .query_row(
                    "SELECT actor_key, month_key, reserved_micro, state \
                     FROM budget_reservations WHERE reservation_id = ?1",
                    params![reservation_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .optional()
                .map_err(unavailable)?;
            let Some((actor_key, month_key, reserved_micro, state)) = row else {
                return Err(StoreError::ReservationMismatch);
            };
            if state == "settled" {
                return Ok(SettleOutcome::AlreadySettled);
            }
            let Some(actual) = actual_micro else {
                // Unknown final cost: keep the reserved amount counted and
                // mark the distinct reconcile state (ERR-004).
                tx.execute(
                    "UPDATE budget_reservations SET state = 'reconcile' \
                     WHERE reservation_id = ?1",
                    params![reservation_id],
                )
                .map_err(unavailable)?;
                tx.commit().map_err(unavailable)?;
                return Ok(SettleOutcome::ReconciliationRequired);
            };

            tx.execute(
                "UPDATE monthly_budgets SET spent_micro = spent_micro + ?3, \
                 reserved_micro = reserved_micro - ?4 \
                 WHERE actor_key = ?1 AND month_key = ?2",
                params![actor_key, month_key, actual, reserved_micro],
            )
            .map_err(unavailable)?;
            tx.execute(
                "UPDATE budget_reservations SET state = 'settled', settled_micro = ?2, \
                 settled_at_ms = ?3 WHERE reservation_id = ?1",
                params![reservation_id, actual, epoch_ms(now)],
            )
            .map_err(unavailable)?;
            let (spent, reserved) = budget_row(&tx, &actor_key, &month_key)?;
            tx.commit().map_err(unavailable)?;

            let committed = spent
                .checked_add(reserved)
                .ok_or_else(|| StoreError::Unavailable("ledger overflow".into()))?;
            let overdrawn_by_micro = committed - MONTHLY_BUDGET_MICRO;
            if overdrawn_by_micro > 0 {
                Ok(SettleOutcome::SettledWithOverage { overdrawn_by_micro })
            } else {
                Ok(SettleOutcome::Settled)
            }
        })
        .await
    }

    /// Snapshot this actor's ledger for the month containing `now`.
    pub async fn ledger_snapshot(
        &self,
        actor_key: &str,
        now: DateTime<Utc>,
    ) -> Result<LedgerSnapshot, StoreError> {
        validate_identifier(actor_key, "actor key")?;
        let actor_key = actor_key.to_string();
        self.with_conn(move |conn| {
            let month_key = month::taipei_month_key(now);
            let next_reset_utc = month::next_reset_utc(now);
            let tx = conn.transaction().map_err(unavailable)?;
            snapshot_tx(&tx, &actor_key, &month_key, next_reset_utc)
        })
        .await
    }

    fn ttl_ms(&self) -> i64 {
        i64::from(self.config.ttl_days) * 24 * 60 * 60 * 1000
    }

    fn sanitize_turn(&self, turn: TurnSummaryInput) -> TurnSummaryInput {
        let clean = |value: String| {
            sanitize::sanitize_field(
                &value,
                &self.config.redact_patterns,
                self.config.summary_char_limit,
            )
        };
        TurnSummaryInput {
            turn_id: clean(turn.turn_id),
            user_summary: clean(turn.user_summary),
            answer_summary: clean(turn.answer_summary),
            intent: turn.intent.map(clean),
            metric: turn.metric.map(clean),
            asset: turn.asset.map(clean),
            time_range_label: turn.time_range_label.map(clean),
            option_id: turn.option_id.map(clean),
        }
    }
}

/// Run a typed-error closure on the connection's database thread, flattening
/// transport-level failures into [`StoreError::Unavailable`].
async fn run_on<T, F>(conn: &Connection, f: F) -> Result<T, StoreError>
where
    T: Send + 'static,
    F: FnOnce(&mut rusqlite::Connection) -> Result<T, StoreError> + Send + 'static,
{
    conn.call(f).await.map_err(|err| match err {
        tokio_rusqlite::Error::Error(store_err) => store_err,
        other => StoreError::Unavailable(other.to_string()),
    })
}

/// Begin one `BEGIN IMMEDIATE` transaction.
fn immediate(conn: &mut rusqlite::Connection) -> Result<Transaction<'_>, StoreError> {
    conn.transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(unavailable)
}

/// Idempotent schema migration. A file written by a newer schema is refused
/// rather than reinterpreted.
fn migrate(conn: &mut rusqlite::Connection) -> Result<(), StoreError> {
    let tx = immediate(conn)?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_meta(version INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS users(
             actor_key TEXT PRIMARY KEY,
             first_seen_ms INTEGER NOT NULL,
             last_seen_ms INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS sessions(
             session_id TEXT PRIMARY KEY,
             actor_key TEXT NOT NULL REFERENCES users(actor_key),
             created_at_ms INTEGER NOT NULL,
             last_append_ms INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS session_turns(
             session_id TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
             seq INTEGER NOT NULL,
             turn_id TEXT NOT NULL,
             user_summary TEXT NOT NULL,
             answer_summary TEXT NOT NULL,
             intent TEXT,
             metric TEXT,
             asset TEXT,
             time_range_label TEXT,
             option_id TEXT,
             created_at_ms INTEGER NOT NULL,
             PRIMARY KEY(session_id, seq));
         CREATE TABLE IF NOT EXISTS monthly_budgets(
             actor_key TEXT NOT NULL,
             month_key TEXT NOT NULL,
             spent_micro INTEGER NOT NULL DEFAULT 0 CHECK(spent_micro >= 0),
             reserved_micro INTEGER NOT NULL DEFAULT 0 CHECK(reserved_micro >= 0),
             PRIMARY KEY(actor_key, month_key));
         CREATE TABLE IF NOT EXISTS budget_reservations(
             reservation_id TEXT PRIMARY KEY,
             actor_key TEXT NOT NULL,
             month_key TEXT NOT NULL,
             reserved_micro INTEGER NOT NULL CHECK(reserved_micro > 0),
             state TEXT NOT NULL CHECK(state IN ('pending','settled','reconcile')),
             settled_micro INTEGER,
             created_at_ms INTEGER NOT NULL,
             settled_at_ms INTEGER);",
    )
    .map_err(unavailable)?;
    let version: Option<i64> = tx
        .query_row("SELECT version FROM schema_meta LIMIT 1", [], |row| {
            row.get(0)
        })
        .optional()
        .map_err(unavailable)?;
    match version {
        None => {
            tx.execute(
                "INSERT INTO schema_meta(version) VALUES (?1)",
                params![SCHEMA_VERSION],
            )
            .map_err(unavailable)?;
        }
        Some(SCHEMA_VERSION) => {}
        Some(other) => {
            return Err(StoreError::Unavailable(format!(
                "database schema version {other} is not supported (expected {SCHEMA_VERSION})"
            )));
        }
    }
    tx.commit().map_err(unavailable)
}

fn is_expired(last_append_ms: i64, ttl_ms: i64, now: DateTime<Utc>) -> bool {
    last_append_ms.saturating_add(ttl_ms) <= epoch_ms(now)
}

/// `(actor_key, last_append_ms)` for one session, if present.
fn session_row(
    tx: &Transaction<'_>,
    session_id: &str,
) -> Result<Option<(String, i64)>, StoreError> {
    tx.query_row(
        "SELECT actor_key, last_append_ms FROM sessions WHERE session_id = ?1",
        params![session_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
    )
    .optional()
    .map_err(unavailable)
}

/// Upsert the user, then create/claim/validate the session inside the
/// caller's transaction.
fn ensure_session_tx(
    tx: &Transaction<'_>,
    actor_key: &str,
    session_id: &str,
    now: DateTime<Utc>,
    ttl_ms: i64,
) -> Result<SessionRecord, StoreError> {
    let now_ms = epoch_ms(now);
    tx.execute(
        "INSERT INTO users(actor_key, first_seen_ms, last_seen_ms) VALUES (?1, ?2, ?2) \
         ON CONFLICT(actor_key) DO UPDATE SET last_seen_ms = ?2",
        params![actor_key, now_ms],
    )
    .map_err(unavailable)?;

    let existing = tx
        .query_row(
            "SELECT actor_key, created_at_ms, last_append_ms FROM sessions \
             WHERE session_id = ?1",
            params![session_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(unavailable)?;

    if let Some((owner, created_at_ms, last_append_ms)) = existing {
        if !is_expired(last_append_ms, ttl_ms, now) {
            if owner == actor_key {
                return Ok(SessionRecord {
                    session_id: session_id.to_string(),
                    actor_key: owner,
                    created_at_ms: created_at_ms as u64,
                    last_append_ms: last_append_ms as u64,
                });
            }
            return Err(StoreError::OwnershipConflict);
        }
        // Expired: the ID is claimable as a brand-new session (AC-012).
        tx.execute(
            "DELETE FROM sessions WHERE session_id = ?1",
            params![session_id],
        )
        .map_err(unavailable)?;
    }

    tx.execute(
        "INSERT INTO sessions(session_id, actor_key, created_at_ms, last_append_ms) \
         VALUES (?1, ?2, ?3, ?3)",
        params![session_id, actor_key, now_ms],
    )
    .map_err(unavailable)?;
    Ok(SessionRecord {
        session_id: session_id.to_string(),
        actor_key: actor_key.to_string(),
        created_at_ms: now_ms as u64,
        last_append_ms: now_ms as u64,
    })
}

/// `(actor_key, month_key, reserved_micro)` for one reservation, if present.
fn reservation_identity(
    tx: &Transaction<'_>,
    reservation_id: &str,
) -> Result<Option<(String, String, i64)>, StoreError> {
    tx.query_row(
        "SELECT actor_key, month_key, reserved_micro FROM budget_reservations \
         WHERE reservation_id = ?1",
        params![reservation_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        },
    )
    .optional()
    .map_err(unavailable)
}

/// `(spent_micro, reserved_micro)` for one actor-month, defaulting to zeros.
fn budget_row(
    tx: &Transaction<'_>,
    actor_key: &str,
    month_key: &str,
) -> Result<(i64, i64), StoreError> {
    Ok(tx
        .query_row(
            "SELECT spent_micro, reserved_micro FROM monthly_budgets \
             WHERE actor_key = ?1 AND month_key = ?2",
            params![actor_key, month_key],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(unavailable)?
        .unwrap_or((0, 0)))
}

/// Ledger snapshot inside the caller's transaction; remaining clamps at zero.
fn snapshot_tx(
    tx: &Transaction<'_>,
    actor_key: &str,
    month_key: &str,
    next_reset_utc: DateTime<Utc>,
) -> Result<LedgerSnapshot, StoreError> {
    let (spent_micro, reserved_micro) = budget_row(tx, actor_key, month_key)?;
    let remaining_micro = MONTHLY_BUDGET_MICRO
        .saturating_sub(spent_micro)
        .saturating_sub(reserved_micro)
        .max(0);
    Ok(LedgerSnapshot {
        month_key: month_key.to_string(),
        spent_micro,
        reserved_micro,
        remaining_micro,
        next_reset_utc,
    })
}
