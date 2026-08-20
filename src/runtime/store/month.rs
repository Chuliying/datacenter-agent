//! Asia/Taipei budget-month derivation from an injected UTC instant.
//!
//! Taiwan has had no DST since 1980, so the offset is a fixed +08:00
//! (spec D-002) — no `chrono-tz` dependency.

use chrono::{DateTime, Datelike, FixedOffset, TimeZone, Utc};

const TAIPEI_OFFSET_SECS: i32 = 8 * 3600;

fn taipei_offset() -> FixedOffset {
    FixedOffset::east_opt(TAIPEI_OFFSET_SECS).expect("+08:00 is a valid fixed offset")
}

/// Derive the Asia/Taipei month key (`YYYY-MM`) for a UTC instant.
pub fn taipei_month_key(now_utc: DateTime<Utc>) -> String {
    let local = now_utc.with_timezone(&taipei_offset());
    format!("{:04}-{:02}", local.year(), local.month())
}

/// UTC instant at which the next Asia/Taipei calendar month starts
/// (the first day at `00:00:00 +08:00`, expressed in UTC).
pub fn next_reset_utc(now_utc: DateTime<Utc>) -> DateTime<Utc> {
    let local = now_utc.with_timezone(&taipei_offset());
    let (year, month) = if local.month() == 12 {
        (local.year() + 1, 1)
    } else {
        (local.year(), local.month() + 1)
    };
    taipei_offset()
        .with_ymd_and_hms(year, month, 1, 0, 0, 0)
        .single()
        .expect("first-of-month midnight is unambiguous in a fixed offset")
        .with_timezone(&Utc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(s: &str) -> DateTime<Utc> {
        s.parse().expect("test timestamp must parse")
    }

    /// AC-006: one second before the Taipei month boundary stays in 2026-08.
    #[test]
    fn instant_before_taipei_midnight_uses_prior_month() {
        assert_eq!(taipei_month_key(utc("2026-08-31T15:59:59Z")), "2026-08");
    }

    /// AC-006: exactly 16:00Z is 00:00 +08:00 on Sep 1 — the new month.
    #[test]
    fn instant_at_taipei_midnight_starts_new_month() {
        assert_eq!(taipei_month_key(utc("2026-08-31T16:00:00Z")), "2026-09");
    }

    /// AC-006: the September ledger resets when October starts in Taipei,
    /// which is 2026-09-30T16:00:00Z.
    #[test]
    fn next_reset_is_first_of_next_taipei_month_in_utc() {
        assert_eq!(
            next_reset_utc(utc("2026-08-31T16:00:00Z")),
            Utc.with_ymd_and_hms(2026, 9, 30, 16, 0, 0).unwrap()
        );
    }

    /// December rolls the year over.
    #[test]
    fn december_rolls_into_next_year() {
        assert_eq!(taipei_month_key(utc("2026-12-31T15:59:59Z")), "2026-12");
        assert_eq!(
            next_reset_utc(utc("2026-12-31T15:59:59Z")),
            Utc.with_ymd_and_hms(2026, 12, 31, 16, 0, 0).unwrap()
        );
    }

    /// An instant mid-month resets at that month's end, not its own +1 month.
    #[test]
    fn mid_month_resets_at_month_end() {
        assert_eq!(taipei_month_key(utc("2026-08-13T04:00:00Z")), "2026-08");
        assert_eq!(
            next_reset_utc(utc("2026-08-13T04:00:00Z")),
            Utc.with_ymd_and_hms(2026, 8, 31, 16, 0, 0).unwrap()
        );
    }
}
