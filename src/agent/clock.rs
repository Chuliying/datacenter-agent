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

//! Time as an **injected capability** — the seam that makes a sub-agent time-aware without
//! breaking its "pure async function of its payload" property.
//!
//! An LLM has no clock: handed revenue that ends in the current, in-progress month, it reads the
//! partial figure as a genuine drop. The fix is to state *now* in the conversation — exactly what
//! the legacy serving path already does
//! ([`AppState::generation_config`](crate::appstate::AppState) prepends a `# Current Time` header).
//! The sub-agent port dropped that; this module restores it.
//!
//! Under the payload contract, `now` is **turn data**: a [`Clock`] stamps it once at the boundary
//! into [`InitialPrompt::now`](crate::agent::payload::InitialPrompt::now), it is threaded through
//! every stage unchanged, and each stage renders it — so the clock is read at the boundary, never
//! ambiently inside a stage, and a fixture that pins `now` makes the whole run reproducible.
//!
//! Two pieces:
//!
//! - [`Clock`] — the **boundary's** `now()` provider. [`SystemClock`] is the real one (an
//!   **explicit offset**, default Asia/Taipei `+08:00`, so it is correct regardless of the
//!   container's `TZ`); [`FixedClock`] pins a moment for tests and eval replay.
//! - [`current_time_header`] — the one shared formatter for the `# Current Time` block, so the
//!   legacy path, the eval runner, and the sub-agent stages cannot drift.
//!
//! # References
//!
//! - `src/appstate.rs` — the legacy `# Current Time` injection this restores parity with

use std::fmt;

use chrono::{DateTime, FixedOffset, TimeZone, Utc};

/// Seconds east of UTC for Asia/Taipei (`+08:00`) — Taiwan observes no DST, so a fixed offset is
/// exact and needs no timezone database.
const TAIPEI_OFFSET_SECONDS: i32 = 8 * 3600;

/// A source of the current time, used at the boundary to stamp a turn's `now`.
///
/// Returns a [`DateTime<FixedOffset>`] so the wall-clock instant always carries an explicit zone
/// (never an ambient, container-dependent one). The boundary can hold a concrete [`SystemClock`]
/// or, for a test, a [`FixedClock`].
pub trait Clock: Send + Sync {
    /// The current instant, in this clock's fixed zone.
    fn now(&self) -> DateTime<FixedOffset>;
}

/// The real clock: reads UTC and presents it in a **fixed offset** (default Asia/Taipei `+08:00`).
///
/// Reading `Utc::now().with_timezone(&offset)` — rather than `Local::now()` — makes the reported
/// day correct even when the process `TZ` is unset (containers default to UTC), which is exactly
/// the near-midnight case that would otherwise mislabel the current period.
#[derive(Clone, Copy, Debug)]
pub struct SystemClock {
    offset: FixedOffset,
}

impl SystemClock {
    /// Builds a system clock in the given fixed offset.
    pub fn new(offset: FixedOffset) -> Self {
        Self { offset }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new(
            FixedOffset::east_opt(TAIPEI_OFFSET_SECONDS)
                .expect("+08:00 is a valid fixed offset"),
        )
    }
}

impl Clock for SystemClock {
    fn now(&self) -> DateTime<FixedOffset> {
        Utc::now().with_timezone(&self.offset)
    }
}

/// A clock pinned to a fixed instant — deterministic time for unit tests and eval replay.
#[derive(Clone, Copy, Debug)]
pub struct FixedClock(pub DateTime<FixedOffset>);

impl Clock for FixedClock {
    fn now(&self) -> DateTime<FixedOffset> {
        self.0
    }
}

/// Renders the `# Current Time` header injected ahead of a system prompt.
///
/// The **single** definition of that block — the legacy [`AppState`](crate::appstate::AppState)
/// path, the eval runner, and the sub-agent engine all call this, so the format cannot diverge.
/// Generic over the timezone so it serves both a `DateTime<Local>` (legacy) and a
/// `DateTime<FixedOffset>` (this module's [`Clock`]).
///
/// # Arguments
///
/// - `now`: the instant to render (its offset is shown, e.g. `+08:00`).
///
/// # Returns
///
/// Returns `"# Current Time\n{YYYY-MM-DD HH:MM:SS ±ZZ:ZZ}\n\n"`, ready to prepend to a system base.
pub fn current_time_header<Tz>(now: &DateTime<Tz>) -> String
where
    Tz: TimeZone,
    Tz::Offset: fmt::Display,
{
    format!(
        "# Current Time\n{}\n\n",
        now.format("%Y-%m-%d %H:%M:%S %:z")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse an RFC-3339 instant into the fixed-offset type the [`Clock`] speaks.
    fn at(rfc3339: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(rfc3339).expect("valid rfc3339")
    }

    #[test]
    fn header_renders_the_current_time_block_verbatim() {
        let header = current_time_header(&at("2026-07-11T09:30:00+08:00"));
        assert_eq!(header, "# Current Time\n2026-07-11 09:30:00 +08:00\n\n");
    }

    #[test]
    fn fixed_clock_returns_its_pinned_instant() {
        let clock = FixedClock(at("2026-07-11T09:30:00+08:00"));
        assert_eq!(clock.now(), at("2026-07-11T09:30:00+08:00"));
    }

    #[test]
    fn system_clock_presents_utc_in_its_fixed_offset() {
        // Default is +08:00 regardless of the host TZ — the near-midnight correctness guarantee.
        let clock = SystemClock::default();
        assert_eq!(clock.now().offset().local_minus_utc(), TAIPEI_OFFSET_SECONDS);
    }
}
