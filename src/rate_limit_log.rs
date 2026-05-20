//! Helpers for rate-limiting log output during sustained failure streaks.
//!
//! The bot's outer loop calls back-end services on every tick (currently
//! every 5 s when idle). During a multi-hour outage that produces hundreds
//! of identical error log lines per hour. [`ConsecutiveErrorLog`] tracks
//! consecutive failures and only logs on attempts 1, 2, 4, 8, 16, … —
//! logarithmic rather than linear noise. It also emits a single
//! `tracing::info!` line on recovery so operators can see when the
//! outage cleared.

use anyhow::Error;

/// Returns true if `consecutive` is a "log point": 1, 2, 4, 8, 16, …
///
/// Pulled out as a free function so the policy is trivial to unit-test
/// without depending on the `tracing` side effect.
pub(crate) fn should_log_at(consecutive: u32) -> bool {
    consecutive.is_power_of_two()
}

/// Counts a sequence of consecutive failures and logs at exponentially
/// increasing intervals. Logs once on recovery.
///
/// `context` is a short static label that appears in the log message
/// (e.g. `"DB unreachable"`).
pub struct ConsecutiveErrorLog {
    consecutive: u32,
    context: &'static str,
}

impl ConsecutiveErrorLog {
    pub fn new(context: &'static str) -> Self {
        Self { consecutive: 0, context }
    }

    /// Record a failure. Logs at `error` level on the 1st, 2nd, 4th, 8th, …
    /// consecutive call. The `consecutive` count is included in the log
    /// so operators can see at a glance how long the streak has lasted.
    pub fn note_error(&mut self, err: &Error) {
        self.consecutive = self.consecutive.saturating_add(1);
        if should_log_at(self.consecutive) {
            tracing::error!(
                consecutive = self.consecutive,
                error = %err,
                "{}",
                self.context,
            );
        }
    }

    /// Record a success. If we were in a failure streak, logs a single
    /// `info` line documenting the recovery and resets the counter.
    pub fn note_success(&mut self) {
        if self.consecutive > 0 {
            tracing::info!(cleared_after = self.consecutive, "{}: recovered", self.context);
        }
        self.consecutive = 0;
    }

    #[cfg(test)]
    pub fn consecutive(&self) -> u32 {
        self.consecutive
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    #[test]
    fn should_log_at_powers_of_two() {
        assert!(should_log_at(1));
        assert!(should_log_at(2));
        assert!(!should_log_at(3));
        assert!(should_log_at(4));
        assert!(!should_log_at(5));
        assert!(!should_log_at(6));
        assert!(!should_log_at(7));
        assert!(should_log_at(8));
        assert!(should_log_at(16));
        assert!(should_log_at(1024));
    }

    #[test]
    fn should_log_at_zero_is_false() {
        // 0 is NOT a power of two, so a fresh limiter (consecutive=0) does
        // not log. We only log after `note_error` has bumped the counter.
        assert!(!should_log_at(0));
    }

    #[test]
    fn note_error_increments_counter() {
        let mut log = ConsecutiveErrorLog::new("test");
        assert_eq!(log.consecutive(), 0);
        log.note_error(&anyhow!("boom"));
        assert_eq!(log.consecutive(), 1);
        log.note_error(&anyhow!("boom"));
        assert_eq!(log.consecutive(), 2);
    }

    #[test]
    fn note_success_resets_counter() {
        let mut log = ConsecutiveErrorLog::new("test");
        log.note_error(&anyhow!("boom"));
        log.note_error(&anyhow!("boom"));
        log.note_error(&anyhow!("boom"));
        assert_eq!(log.consecutive(), 3);
        log.note_success();
        assert_eq!(log.consecutive(), 0);
    }

    #[test]
    fn note_success_with_no_prior_errors_is_noop() {
        // A long run of healthy ticks must not spam "recovered" lines.
        let mut log = ConsecutiveErrorLog::new("test");
        log.note_success();
        log.note_success();
        log.note_success();
        assert_eq!(log.consecutive(), 0);
    }

    #[test]
    fn counter_saturates() {
        let mut log = ConsecutiveErrorLog::new("test");
        // Fast-path the counter near the saturating boundary; calling
        // note_error 4 billion times in a test is not actually feasible.
        // We rely on saturating_add via the public API by inspecting state.
        log.consecutive = u32::MAX - 1;
        log.note_error(&anyhow!("boom"));
        assert_eq!(log.consecutive(), u32::MAX);
        log.note_error(&anyhow!("boom"));
        // Saturated — no panic, no wraparound.
        assert_eq!(log.consecutive(), u32::MAX);
    }
}
