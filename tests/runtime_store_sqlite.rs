//! Slice 1 integration tests: SQLite repositories (S-RUNTIME-SEC-01).
//!
//! Covers AC-001, AC-002, AC-003, AC-004, AC-005, AC-011, AC-012, AC-013 and
//! ERR-001, ERR-002, ERR-004, ERR-006 plus the under-settlement release rule.
//! Every test injects its own `now`; no wall-clock reads.

use chrono::{DateTime, Duration, Utc};
use datacenter_agent::runtime::store::sqlite::SqliteRuntimeStore;
use datacenter_agent::runtime::store::{
    ReserveOutcome, SettleOutcome, StoreConfig, StoreError, TurnSummaryInput, MONTHLY_BUDGET_MICRO,
};
use tempfile::TempDir;

fn utc(s: &str) -> DateTime<Utc> {
    s.parse().expect("test timestamp must parse")
}

fn t0() -> DateTime<Utc> {
    utc("2026-08-13T04:00:00Z")
}

fn turn(label: &str) -> TurnSummaryInput {
    TurnSummaryInput {
        turn_id: format!("turn-{label}"),
        user_summary: format!("user asked about {label}"),
        answer_summary: format!("answered {label}"),
        intent: Some("revenue".into()),
        metric: None,
        asset: None,
        time_range_label: None,
        option_id: None,
    }
}

async fn open_store(dir: &TempDir) -> SqliteRuntimeStore {
    SqliteRuntimeStore::open(StoreConfig::new(dir.path().join("store.db")))
        .await
        .expect("store should open in a writable tempdir")
}

/// AC-001: committed user, session, summary, and reservation survive closing
/// and reopening the same database file.
#[tokio::test]
async fn ac001_committed_state_survives_reopen() {
    let dir = TempDir::new().unwrap();
    let now = t0();
    {
        let store = open_store(&dir).await;
        store.ensure_session("actor-a", "s1", now).await.unwrap();
        store
            .append_turn("actor-a", "s1", turn("alpha"), now)
            .await
            .unwrap();
        let outcome = store
            .reserve("actor-a", "g1", 1_000_000, now)
            .await
            .unwrap();
        assert!(matches!(outcome, ReserveOutcome::Reserved { .. }));
    }

    let reopened = open_store(&dir).await;
    let record = reopened.ensure_session("actor-a", "s1", now).await.unwrap();
    assert_eq!(record.actor_key, "actor-a");
    let turns = reopened.load_recent("actor-a", "s1", now).await.unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].user_summary, "user asked about alpha");
    let snapshot = reopened.ledger_snapshot("actor-a", now).await.unwrap();
    assert_eq!(snapshot.reserved_micro, 1_000_000);
    assert_eq!(snapshot.spent_micro, 0);
}

/// AC-002: a different actor gets a typed conflict and none of the owner's data.
#[tokio::test]
async fn ac002_ownership_conflict_across_actors() {
    let dir = TempDir::new().unwrap();
    let now = t0();
    let store = open_store(&dir).await;
    store.ensure_session("actor-a", "s1", now).await.unwrap();
    store
        .append_turn("actor-a", "s1", turn("secret"), now)
        .await
        .unwrap();

    let load = store.load_recent("actor-b", "s1", now).await;
    assert!(matches!(load, Err(StoreError::OwnershipConflict)));
    let append = store
        .append_turn("actor-b", "s1", turn("hijack"), now)
        .await;
    assert!(matches!(append, Err(StoreError::OwnershipConflict)));
    let ensure = store.ensure_session("actor-b", "s1", now).await;
    assert!(matches!(ensure, Err(StoreError::OwnershipConflict)));
}

/// AC-003: six summaries with sensitive fixtures leave only the five newest,
/// sanitized rows — verified against the raw database file, not the API.
#[tokio::test]
async fn ac003_persisted_memory_is_minimized() {
    let dir = TempDir::new().unwrap();
    let now = t0();
    let store = open_store(&dir).await;
    store.ensure_session("actor-a", "s1", now).await.unwrap();

    let fixtures = [
        "reach me at alice@example.com please",
        "peer ip was 203.0.113.9 today",
        "sent Cookie: session=deadbeefcafe; theme=dark",
        "used Authorization: Bearer sk-live-4242 upstream",
        "lots   of\t\twhitespace\n\nhere",
        &"月".repeat(600),
    ];
    for (i, text) in fixtures.iter().enumerate() {
        let mut input = turn(&format!("f{i}"));
        input.user_summary = text.to_string();
        store
            .append_turn("actor-a", "s1", input, now)
            .await
            .unwrap();
    }

    let turns = store.load_recent("actor-a", "s1", now).await.unwrap();
    assert_eq!(turns.len(), 5, "cap is five newest");
    assert_eq!(turns[0].turn_id, "turn-f1", "oldest (f0) was dropped");
    assert!(turns.iter().all(|t| t.user_summary.chars().count() <= 500));

    // Database inspection: none of the raw sensitive fixtures exist anywhere
    // in the persisted rows (NFR data minimization).
    let raw = rusqlite_dump(dir.path().join("store.db"));
    for leaked in [
        "alice@example.com",
        "203.0.113.9",
        "deadbeefcafe",
        "sk-live-4242",
    ] {
        assert!(!raw.contains(leaked), "raw db leaked {leaked}");
    }
    assert!(
        !raw.contains(&"月".repeat(501)),
        "over-limit text persisted"
    );
}

/// Read every persisted turn field straight from the file with plain rusqlite.
fn rusqlite_dump(path: std::path::PathBuf) -> String {
    use tokio_rusqlite::rusqlite;
    let conn = rusqlite::Connection::open(path).unwrap();
    let mut stmt = conn
        .prepare("SELECT turn_id, user_summary, answer_summary FROM session_turns")
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            Ok(format!(
                "{}|{}|{}",
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?
            ))
        })
        .unwrap();
    rows.map(|r| r.unwrap()).collect::<Vec<_>>().join("\n")
}

/// AC-004: with 1,000,000 micro-USD remaining, two concurrent 750,000
/// reservations through the same store admit exactly one.
#[tokio::test]
async fn ac004_concurrent_reserve_never_oversubscribes() {
    let dir = TempDir::new().unwrap();
    let now = t0();
    let store = open_store(&dir).await;
    let spent = MONTHLY_BUDGET_MICRO - 1_000_000;
    store.reserve("actor-a", "seed", spent, now).await.unwrap();
    store.settle("seed", Some(spent), now).await.unwrap();

    let (a, b) = tokio::join!(
        store.reserve("actor-a", "g-left", 750_000, now),
        store.reserve("actor-a", "g-right", 750_000, now),
    );
    let outcomes = [a.unwrap(), b.unwrap()];
    let reserved = outcomes
        .iter()
        .filter(|o| matches!(o, ReserveOutcome::Reserved { .. }))
        .count();
    assert_eq!(reserved, 1, "exactly one reservation may win");

    let snapshot = store.ledger_snapshot("actor-a", now).await.unwrap();
    assert!(snapshot.spent_micro + snapshot.reserved_micro <= MONTHLY_BUDGET_MICRO);
}

/// AC-004 hardening: the same race across two separate connections to the
/// same file is still serialized by `BEGIN IMMEDIATE`.
#[tokio::test]
async fn ac004_cross_connection_concurrent_reserve_never_oversubscribes() {
    let dir = TempDir::new().unwrap();
    let now = t0();
    let first = open_store(&dir).await;
    let second = open_store(&dir).await;
    let spent = MONTHLY_BUDGET_MICRO - 1_000_000;
    first.reserve("actor-a", "seed", spent, now).await.unwrap();
    first.settle("seed", Some(spent), now).await.unwrap();

    let (a, b) = tokio::join!(
        first.reserve("actor-a", "g-left", 750_000, now),
        second.reserve("actor-a", "g-right", 750_000, now),
    );
    let outcomes = [a.unwrap(), b.unwrap()];
    let reserved = outcomes
        .iter()
        .filter(|o| matches!(o, ReserveOutcome::Reserved { .. }))
        .count();
    assert_eq!(reserved, 1, "cross-connection race must admit exactly one");

    let snapshot = first.ledger_snapshot("actor-a", now).await.unwrap();
    assert!(snapshot.spent_micro + snapshot.reserved_micro <= MONTHLY_BUDGET_MICRO);
}

/// AC-005: replaying the same settlement does not double-charge.
#[tokio::test]
async fn ac005_idempotent_settle_charges_once() {
    let dir = TempDir::new().unwrap();
    let now = t0();
    let store = open_store(&dir).await;
    store
        .reserve("actor-a", "g1", 1_000_000, now)
        .await
        .unwrap();

    let first = store.settle("g1", Some(1_000_000), now).await.unwrap();
    assert_eq!(first, SettleOutcome::Settled);
    let second = store.settle("g1", Some(1_000_000), now).await.unwrap();
    assert_eq!(second, SettleOutcome::AlreadySettled);

    let snapshot = store.ledger_snapshot("actor-a", now).await.unwrap();
    assert_eq!(snapshot.spent_micro, 1_000_000);
    assert_eq!(snapshot.reserved_micro, 0);
}

/// Under-settlement releases the unused difference in the same transaction.
#[tokio::test]
async fn under_settlement_releases_unused_reservation() {
    let dir = TempDir::new().unwrap();
    let now = t0();
    let store = open_store(&dir).await;
    store
        .reserve("actor-a", "g1", 2_000_000, now)
        .await
        .unwrap();

    let outcome = store.settle("g1", Some(1_500_000), now).await.unwrap();
    assert_eq!(outcome, SettleOutcome::Settled);

    let snapshot = store.ledger_snapshot("actor-a", now).await.unwrap();
    assert_eq!(snapshot.spent_micro, 1_500_000);
    assert_eq!(snapshot.reserved_micro, 0);
    assert_eq!(snapshot.remaining_micro, MONTHLY_BUDGET_MICRO - 1_500_000);
}

/// AC-011 / ERR-006: a known over-settlement charges the authoritative cost,
/// overdraws the month, and freezes every new reservation.
#[tokio::test]
async fn ac011_over_settlement_overdraws_and_freezes_month() {
    let dir = TempDir::new().unwrap();
    let now = t0();
    let store = open_store(&dir).await;
    store
        .reserve("actor-a", "seed", 16_000_000, now)
        .await
        .unwrap();
    store.settle("seed", Some(16_000_000), now).await.unwrap();
    store
        .reserve("actor-a", "g2", 4_000_000, now)
        .await
        .unwrap();

    let outcome = store.settle("g2", Some(9_000_000), now).await.unwrap();
    assert_eq!(
        outcome,
        SettleOutcome::SettledWithOverage {
            overdrawn_by_micro: 5_000_000
        }
    );

    let snapshot = store.ledger_snapshot("actor-a", now).await.unwrap();
    assert_eq!(snapshot.spent_micro, 25_000_000);
    assert_eq!(snapshot.remaining_micro, 0, "remaining clamps at zero");

    let frozen = store.reserve("actor-a", "g3", 1, now).await.unwrap();
    assert!(matches!(frozen, ReserveOutcome::BudgetExceeded { .. }));
}

/// ERR-004: unknown final cost keeps the reservation pending at its reserved
/// amount; a later authoritative settle completes it.
#[tokio::test]
async fn err004_unknown_cost_requires_reconciliation() {
    let dir = TempDir::new().unwrap();
    let now = t0();
    let store = open_store(&dir).await;
    store
        .reserve("actor-a", "g1", 3_000_000, now)
        .await
        .unwrap();

    let outcome = store.settle("g1", None, now).await.unwrap();
    assert_eq!(outcome, SettleOutcome::ReconciliationRequired);
    let snapshot = store.ledger_snapshot("actor-a", now).await.unwrap();
    assert_eq!(snapshot.reserved_micro, 3_000_000, "still counted");

    let settled = store.settle("g1", Some(2_000_000), now).await.unwrap();
    assert_eq!(settled, SettleOutcome::Settled);
    let snapshot = store.ledger_snapshot("actor-a", now).await.unwrap();
    assert_eq!(snapshot.spent_micro, 2_000_000);
    assert_eq!(snapshot.reserved_micro, 0);
}

/// Reserve idempotency: same id + actor + month + amount is a no-op; a
/// different amount is a typed mismatch.
#[tokio::test]
async fn duplicate_reservation_is_idempotent_only_on_full_match() {
    let dir = TempDir::new().unwrap();
    let now = t0();
    let store = open_store(&dir).await;
    store
        .reserve("actor-a", "g1", 1_000_000, now)
        .await
        .unwrap();

    let replay = store
        .reserve("actor-a", "g1", 1_000_000, now)
        .await
        .unwrap();
    assert!(matches!(replay, ReserveOutcome::Reserved { .. }));
    let snapshot = store.ledger_snapshot("actor-a", now).await.unwrap();
    assert_eq!(snapshot.reserved_micro, 1_000_000, "no double count");

    let mismatch = store.reserve("actor-a", "g1", 2_000_000, now).await;
    assert!(matches!(mismatch, Err(StoreError::ReservationMismatch)));
    let stolen = store.reserve("actor-b", "g1", 1_000_000, now).await;
    assert!(matches!(stolen, Err(StoreError::ReservationMismatch)));
}

/// AC-012: 30 days after the last append the session is gone for everyone and
/// the ID is claimable as a brand-new session.
#[tokio::test]
async fn ac012_expiry_is_deterministic_and_releases_the_id() {
    let dir = TempDir::new().unwrap();
    let created = t0();
    let store = open_store(&dir).await;
    store
        .ensure_session("actor-a", "s2", created)
        .await
        .unwrap();
    store
        .append_turn("actor-a", "s2", turn("old"), created)
        .await
        .unwrap();

    let after_expiry = created + Duration::days(30) + Duration::seconds(1);
    let turns = store
        .load_recent("actor-a", "s2", after_expiry)
        .await
        .unwrap();
    assert!(turns.is_empty(), "expired session returns no summaries");

    let claimed = store
        .ensure_session("actor-b", "s2", after_expiry)
        .await
        .unwrap();
    assert_eq!(claimed.actor_key, "actor-b");
    let fresh = store
        .load_recent("actor-b", "s2", after_expiry)
        .await
        .unwrap();
    assert!(fresh.is_empty(), "claimed session starts empty");
}

/// Reads never refresh expiry: a load one second before the boundary does not
/// extend the session's life.
#[tokio::test]
async fn reads_never_refresh_expiry() {
    let dir = TempDir::new().unwrap();
    let created = t0();
    let store = open_store(&dir).await;
    store
        .ensure_session("actor-a", "s2", created)
        .await
        .unwrap();

    let just_before = created + Duration::days(30) - Duration::seconds(1);
    store
        .load_recent("actor-a", "s2", just_before)
        .await
        .unwrap();
    let after = created + Duration::days(30) + Duration::seconds(1);
    let claimed = store.ensure_session("actor-b", "s2", after).await.unwrap();
    assert_eq!(
        claimed.actor_key, "actor-b",
        "read must not have refreshed TTL"
    );
}

/// AC-013: explicit clear removes the session and its turns and releases the ID.
#[tokio::test]
async fn ac013_clear_releases_ownership() {
    let dir = TempDir::new().unwrap();
    let now = t0();
    let store = open_store(&dir).await;
    store.ensure_session("actor-a", "s3", now).await.unwrap();
    store
        .append_turn("actor-a", "s3", turn("one"), now)
        .await
        .unwrap();
    store
        .append_turn("actor-a", "s3", turn("two"), now)
        .await
        .unwrap();

    store.clear_session("actor-a", "s3", now).await.unwrap();

    let claimed = store.ensure_session("actor-b", "s3", now).await.unwrap();
    assert_eq!(claimed.actor_key, "actor-b");
    let fresh = store.load_recent("actor-b", "s3", now).await.unwrap();
    assert!(fresh.is_empty());
}

/// ERR-001: an unopenable path is a typed unavailable error, never a fallback.
#[tokio::test]
async fn err001_unopenable_path_is_unavailable() {
    let dir = TempDir::new().unwrap();
    let blocker = dir.path().join("not-a-directory");
    std::fs::write(&blocker, b"file, not dir").unwrap();

    let result = SqliteRuntimeStore::open(StoreConfig::new(blocker.join("store.db"))).await;
    assert!(matches!(result, Err(StoreError::Unavailable(_))));
}

/// Structurally invalid identifiers are typed validation errors (never
/// triggered by summary content).
#[tokio::test]
async fn invalid_identifiers_are_rejected() {
    let dir = TempDir::new().unwrap();
    let now = t0();
    let store = open_store(&dir).await;

    let empty_actor = store.ensure_session("", "s1", now).await;
    assert!(matches!(empty_actor, Err(StoreError::InvalidIdentifier(_))));
    let long_session = store.ensure_session("actor-a", &"s".repeat(200), now).await;
    assert!(matches!(
        long_session,
        Err(StoreError::InvalidIdentifier(_))
    ));
}

/// A new Asia/Taipei month starts a fresh ledger without deleting the prior one.
#[tokio::test]
async fn month_rollover_starts_fresh_ledger_and_keeps_history() {
    let dir = TempDir::new().unwrap();
    let store = open_store(&dir).await;
    let august = utc("2026-08-31T15:59:59Z");
    let september = utc("2026-08-31T16:00:00Z");
    store
        .reserve("actor-a", "g-aug", 5_000_000, august)
        .await
        .unwrap();
    store
        .settle("g-aug", Some(5_000_000), august)
        .await
        .unwrap();

    let sept_snapshot = store.ledger_snapshot("actor-a", september).await.unwrap();
    assert_eq!(sept_snapshot.month_key, "2026-09");
    assert_eq!(sept_snapshot.spent_micro, 0);

    let aug_snapshot = store.ledger_snapshot("actor-a", august).await.unwrap();
    assert_eq!(aug_snapshot.month_key, "2026-08");
    assert_eq!(aug_snapshot.spent_micro, 5_000_000, "prior ledger retained");
}

/// TC-B07: `max_turns` above the POC maximum (5) is refused at open.
#[tokio::test]
async fn max_turns_above_poc_limit_is_refused_at_open() {
    let dir = TempDir::new().unwrap();
    let mut config = StoreConfig::new(dir.path().join("store.db"));
    config.max_turns = 9;

    let result = SqliteRuntimeStore::open(config).await;
    assert!(
        matches!(result, Err(StoreError::Unavailable(_))),
        "max_turns > 5 must be refused, got an open store"
    );

    let mut zero = StoreConfig::new(dir.path().join("store2.db"));
    zero.max_turns = 0;
    let result = SqliteRuntimeStore::open(zero).await;
    assert!(matches!(result, Err(StoreError::Unavailable(_))));
}

/// TC-ERR03: exceeding the month rejects with a snapshot + next reset and
/// creates no reservation row (verified against the raw file).
#[tokio::test]
async fn err003_budget_exceeded_creates_no_reservation() {
    let dir = TempDir::new().unwrap();
    let now = t0();
    let store = open_store(&dir).await;
    let spent = MONTHLY_BUDGET_MICRO - 1;
    store.reserve("actor-a", "seed", spent, now).await.unwrap();
    store.settle("seed", Some(spent), now).await.unwrap();

    let outcome = store.reserve("actor-a", "g-over", 2, now).await.unwrap();
    let ReserveOutcome::BudgetExceeded {
        snapshot,
        next_reset_utc,
    } = outcome
    else {
        panic!("expected BudgetExceeded, got {outcome:?}");
    };
    assert_eq!(snapshot.spent_micro, spent);
    assert_eq!(next_reset_utc, utc("2026-08-31T16:00:00Z"));

    use tokio_rusqlite::rusqlite;
    let conn = rusqlite::Connection::open(dir.path().join("store.db")).unwrap();
    let rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM budget_reservations WHERE reservation_id = 'g-over'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rows, 0, "rejection must not create a reservation row");
}

/// TC-B06: non-positive reservation amounts are typed errors.
#[tokio::test]
async fn reserve_rejects_zero_and_negative_amounts() {
    let dir = TempDir::new().unwrap();
    let now = t0();
    let store = open_store(&dir).await;

    for amount in [0, -5] {
        let result = store.reserve("actor-a", "g-bad", amount, now).await;
        assert!(
            matches!(result, Err(StoreError::InvalidIdentifier(_))),
            "amount {amount} must be rejected"
        );
    }
}

/// ERR-001 (corrupt-file variant): a non-SQLite file is a typed unavailable
/// error, never a silent re-initialization or fallback.
#[tokio::test]
async fn err001_corrupt_file_is_unavailable() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("store.db");
    std::fs::write(&path, b"this is definitely not an sqlite database file").unwrap();

    let result = SqliteRuntimeStore::open(StoreConfig::new(path)).await;
    assert!(matches!(result, Err(StoreError::Unavailable(_))));
}
