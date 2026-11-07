//! Remembering, for a while, which endpoints are rate-limiting us.
//!
//! A free endpoint that has started answering 429 (or a captcha page) keeps
//! doing so for minutes, and every request sent to it in that time extends
//! the block. So a limited endpoint is left alone for a cooling-off period
//! that doubles with each consecutive refusal, from [`Backoff::base`] up to
//! [`Backoff::max`], and starts over after the endpoint answers again.
//!
//! Times are passed in rather than read from the clock, so the tests can
//! walk through a cooling-off period without sleeping.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
struct Strike {
    /// Consecutive refusals since the endpoint last answered.
    count: u32,
    /// Not to be asked again before this.
    until: Instant,
}

#[derive(Debug)]
pub(crate) struct Backoff {
    base: Duration,
    max: Duration,
    strikes: Mutex<HashMap<&'static str, Strike>>,
}

impl Default for Backoff {
    /// Thirty seconds, doubling to ten minutes: long enough that a burst of
    /// popups does not keep a block alive, short enough that the service
    /// comes back on its own within a coffee break.
    fn default() -> Self {
        Self::new(Duration::from_secs(30), Duration::from_secs(600))
    }
}

impl Backoff {
    pub(crate) fn new(base: Duration, max: Duration) -> Self {
        Self { base, max: max.max(base), strikes: Mutex::new(HashMap::new()) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<&'static str, Strike>> {
        self.strikes.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// How much longer `key` should be left alone, if at all.
    pub(crate) fn cooling(&self, key: &'static str, now: Instant) -> Option<Duration> {
        self.lock().get(key).and_then(|s| s.until.checked_duration_since(now)).filter(|d| !d.is_zero())
    }

    /// Record a refusal from `key` and return how long to leave it alone:
    /// the doubled period, or what the server asked for if that is longer
    /// (never past [`Backoff::max`]).
    pub(crate) fn limited(&self, key: &'static str, retry_after_secs: Option<u64>, now: Instant) -> Duration {
        let mut strikes = self.lock();
        let count = strikes.get(key).map_or(0, |s| s.count).saturating_add(1);
        let doubled = self.base.saturating_mul(1u32 << (count - 1).min(16));
        let asked = Duration::from_secs(retry_after_secs.unwrap_or(0));
        let wait = doubled.max(asked).min(self.max);
        strikes.insert(key, Strike { count, until: now + wait });
        wait
    }

    /// `key` answered: forget its refusals.
    pub(crate) fn ok(&self, key: &'static str) {
        let mut strikes = self.lock();
        if strikes.contains_key(key) {
            strikes.remove(key);
        }
    }
}

/// Whole seconds, rounded up, for a "try again in N s" message.
pub(crate) fn ceil_secs(d: Duration) -> u64 {
    d.as_secs() + u64::from(d.subsec_nanos() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: Duration = Duration::from_secs(1);

    #[test]
    fn refusals_double_the_wait_up_to_the_cap() {
        let b = Backoff::new(10 * S, 60 * S);
        let t0 = Instant::now();
        assert_eq!(b.cooling("a", t0), None);
        assert_eq!(b.limited("a", None, t0), 10 * S);
        assert_eq!(b.cooling("a", t0 + 4 * S), Some(6 * S));
        assert_eq!(b.cooling("a", t0 + 10 * S), None);
        // Asked again after the wait and refused again: twice as long.
        assert_eq!(b.limited("a", None, t0 + 10 * S), 20 * S);
        assert_eq!(b.limited("a", None, t0 + 30 * S), 40 * S);
        assert_eq!(b.limited("a", None, t0 + 70 * S), 60 * S);
        assert_eq!(b.limited("a", None, t0 + 130 * S), 60 * S);
        // Other keys are untouched.
        assert_eq!(b.cooling("b", t0), None);
    }

    #[test]
    fn an_answer_starts_over() {
        let b = Backoff::new(10 * S, 60 * S);
        let t0 = Instant::now();
        b.limited("a", None, t0);
        b.limited("a", None, t0 + 10 * S);
        b.ok("a");
        assert_eq!(b.cooling("a", t0 + 11 * S), None);
        assert_eq!(b.limited("a", None, t0 + 11 * S), 10 * S);
    }

    #[test]
    fn retry_after_is_honoured_within_the_cap() {
        let b = Backoff::new(10 * S, 60 * S);
        let t0 = Instant::now();
        assert_eq!(b.limited("a", Some(25), t0), 25 * S);
        assert_eq!(b.limited("b", Some(3), t0), 10 * S);
        assert_eq!(b.limited("c", Some(3600), t0), 60 * S);
    }

    #[test]
    fn seconds_round_up() {
        assert_eq!(ceil_secs(Duration::from_millis(1500)), 2);
        assert_eq!(ceil_secs(2 * S), 2);
    }
}
