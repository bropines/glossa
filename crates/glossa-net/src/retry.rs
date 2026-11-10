//! The one retry loop.
//!
//! [`HttpClient`](crate::HttpClient) runs every request through [`send`];
//! a caller with a policy of its own (Google Lens, which tries harder and
//! waits longer) calls [`send`] itself with a request marked
//! [`HttpRequest::retries(0)`](crate::HttpRequest::retries), so the client
//! underneath does not retry a second time. A request is therefore never
//! retried by two layers at once.
//!
//! The wait between attempts grows exponentially and is scaled by a random
//! 50–100 %, so several clients (or several panes of one) that failed
//! together do not come back together. A `Retry-After` in seconds on a 429
//! or 503 replaces the computed wait when it fits the policy's cap; when
//! the server asks for longer than that, the answer is handed back at once,
//! since asking again sooner only prolongs the block and the engine has
//! other services to go to. The wait ends early when the request's cancel
//! flag is raised.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use glossa_core::Error;

use crate::{HttpRequest, HttpResponse};

/// How one kind of request is retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Attempts after the first.
    pub max_retries: u32,
    /// The wait before the first retry, before jitter; it doubles each time.
    pub base: Duration,
    /// The longest computed wait, and the longest `Retry-After` obeyed.
    pub cap: Duration,
    /// Whether a timeout is worth another attempt. The client says no: a
    /// timeout has used the whole budget already, and waiting for it again
    /// doubles the time before the engine moves on to the next service.
    pub retry_timeouts: bool,
}

impl RetryPolicy {
    /// The client's policy: quick, since a person is waiting and another
    /// service can answer instead.
    pub fn client(max_retries: u32) -> RetryPolicy {
        RetryPolicy {
            max_retries,
            base: Duration::from_millis(600),
            cap: Duration::from_secs(5),
            retry_timeouts: false,
        }
    }

    /// The wait before retry number `attempt` (from 0), with jitter.
    pub fn backoff(&self, attempt: u32) -> Duration {
        self.backoff_scaled(attempt, jitter())
    }

    /// [`backoff`](Self::backoff) with the random factor given, for tests.
    fn backoff_scaled(&self, attempt: u32, jitter: f64) -> Duration {
        let grown = self.base.saturating_mul(1u32 << attempt.min(16));
        grown.min(self.cap).mul_f64(jitter.clamp(0.5, 1.0))
    }
}

/// A random factor between 0.5 and 1. `RandomState` is seeded by the
/// system for each process and differs per instance, which is all a jitter
/// needs; no random number crate is pulled in for it.
fn jitter() -> f64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    0.5 + 0.5 * (h.finish() % 1000) as f64 / 1000.0
}

/// A `Retry-After` header in seconds. The HTTP-date form is not read: no
/// service Glossa talks to sends it, and the computed wait serves then.
pub fn retry_after(value: &str) -> Option<Duration> {
    value.trim().parse::<u64>().ok().map(Duration::from_secs)
}

/// Statuses that may pass on their own.
fn retryable_status(status: u16) -> bool {
    matches!(status, 429 | 500 | 502 | 503 | 504)
}

/// What to do after one attempt's outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Next {
    /// Hand the outcome back.
    Done,
    /// Wait this long, then try again.
    Wait(Duration),
}

fn decide(
    policy: &RetryPolicy,
    request: &HttpRequest,
    outcome: &Result<HttpResponse, Error>,
    attempt: u32,
    jitter: f64,
) -> Next {
    if attempt >= policy.max_retries {
        return Next::Done;
    }
    match outcome {
        Err(Error::Timeout(_)) if !policy.retry_timeouts => Next::Done,
        Err(e) if e.is_transient() => Next::Wait(policy.backoff_scaled(attempt, jitter)),
        Err(_) => Next::Done,
        Ok(r) if request.no_retry_when_limited && crate::is_rate_limit(r) => Next::Done,
        Ok(r) if retryable_status(r.status) => {
            let asked = r.header("retry-after").and_then(retry_after).filter(|_| matches!(r.status, 429 | 503));
            match asked {
                Some(wait) if wait <= policy.cap => Next::Wait(wait),
                // Longer than we are willing to wait: asking sooner would
                // only be refused again.
                Some(_) => Next::Done,
                None => Next::Wait(policy.backoff_scaled(attempt, jitter)),
            }
        }
        Ok(_) => Next::Done,
    }
}

/// Sleep for `wait`, or until `cancel` is raised: then `Err(Cancelled)`.
pub fn wait(wait: Duration, cancel: Option<&Arc<AtomicBool>>) -> Result<(), Error> {
    let Some(cancel) = cancel else {
        std::thread::sleep(wait);
        return Ok(());
    };
    let until = Instant::now() + wait;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(Error::Cancelled);
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(());
        }
        std::thread::sleep(left.min(Duration::from_millis(25)));
    }
}

/// Run `once` until it succeeds, fails for good, or `policy` gives up; the
/// last outcome is returned as it came (a 503 that never passed is still
/// `Ok` with that status). A raised cancel flag ends it with
/// `Err(Cancelled)` before an attempt or during a wait.
pub fn send(
    policy: &RetryPolicy,
    request: &HttpRequest,
    mut once: impl FnMut(&HttpRequest) -> Result<HttpResponse, Error>,
) -> Result<HttpResponse, Error> {
    let cancel = request.cancel.as_ref();
    let mut attempt = 0;
    loop {
        if cancel.is_some_and(|c| c.load(Ordering::SeqCst)) {
            return Err(Error::Cancelled);
        }
        let outcome = once(request);
        match decide(policy, request, &outcome, attempt, jitter()) {
            Next::Done => return outcome,
            Next::Wait(pause) => {
                attempt += 1;
                let why = match &outcome {
                    Err(e) => e.to_string(),
                    Ok(r) => format!("status {}", r.status),
                };
                tracing::debug!(url = %crate::redact(&request.url), attempt, error = %why, "retrying in {pause:?}");
                wait(pause, cancel)?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: u16, retry_after: Option<&str>) -> Result<HttpResponse, Error> {
        let headers = retry_after.map(|v| ("Retry-After".to_owned(), v.to_owned())).into_iter().collect();
        Ok(HttpResponse { status, headers, body: Vec::new(), codepage: None })
    }

    const QUICK: RetryPolicy = RetryPolicy {
        max_retries: 2,
        base: Duration::from_millis(1),
        cap: Duration::from_millis(4),
        retry_timeouts: false,
    };

    #[test]
    fn the_wait_grows_is_capped_and_is_jittered() {
        let p = RetryPolicy::client(5);
        assert_eq!(p.backoff_scaled(0, 1.0), Duration::from_millis(600));
        assert_eq!(p.backoff_scaled(1, 1.0), Duration::from_millis(1200));
        assert_eq!(p.backoff_scaled(2, 0.5), Duration::from_millis(1200));
        assert_eq!(p.backoff_scaled(10, 1.0), Duration::from_secs(5), "capped");
        assert_eq!(p.backoff_scaled(4000, 1.0), Duration::from_secs(5), "no overflow");
        // The random factor stays in its range, and is not a constant.
        let seen: Vec<f64> = (0..200).map(|_| jitter()).collect();
        assert!(seen.iter().all(|j| (0.5..=1.0).contains(j)), "{seen:?}");
        assert!(seen.iter().any(|j| (j - seen[0]).abs() > 1e-9), "the jitter never varied");
        for attempt in 0..6 {
            let d = p.backoff(attempt);
            assert!(d >= p.backoff_scaled(attempt, 0.5) && d <= p.backoff_scaled(attempt, 1.0));
        }
    }

    #[test]
    fn retry_after_is_read_in_seconds_only() {
        assert_eq!(retry_after(" 3 "), Some(Duration::from_secs(3)));
        assert_eq!(retry_after("600"), Some(Duration::from_secs(600)));
        assert_eq!(retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
        assert_eq!(retry_after("-1"), None);
    }

    #[test]
    fn retry_after_replaces_the_wait_when_it_fits_and_ends_the_retries_when_it_does_not() {
        let p = RetryPolicy::client(3);
        let req = HttpRequest::get("http://x.test/");
        assert_eq!(decide(&p, &req, &response(429, Some("2")), 0, 1.0), Next::Wait(Duration::from_secs(2)));
        assert_eq!(decide(&p, &req, &response(503, Some("5")), 0, 1.0), Next::Wait(Duration::from_secs(5)));
        assert_eq!(decide(&p, &req, &response(429, Some("120")), 0, 1.0), Next::Done, "longer than the cap");
        // Only 429 and 503 carry a Retry-After that means anything here.
        assert_eq!(decide(&p, &req, &response(500, Some("2")), 0, 1.0), Next::Wait(Duration::from_millis(600)));
        assert_eq!(decide(&p, &req, &response(429, None), 1, 1.0), Next::Wait(Duration::from_millis(1200)));
        assert_eq!(decide(&p, &req, &response(429, Some("soon")), 0, 1.0), Next::Wait(Duration::from_millis(600)));
    }

    #[test]
    fn what_is_retried_and_what_is_not() {
        let p = RetryPolicy::client(1);
        let req = HttpRequest::get("http://x.test/");
        let waits = |o: &Result<HttpResponse, Error>, attempt| decide(&p, &req, o, attempt, 1.0) != Next::Done;
        for status in [429, 500, 502, 503, 504] {
            assert!(waits(&response(status, None), 0), "{status}");
        }
        for status in [200, 204, 301, 400, 401, 403, 404] {
            assert!(!waits(&response(status, None), 0), "{status}");
        }
        assert!(waits(&Err(Error::Network("reset".into())), 0));
        assert!(!waits(&Err(Error::Timeout(1000)), 0), "the client does not repeat a timeout");
        assert!(!waits(&Err(Error::Cancelled), 0));
        assert!(!waits(&response(503, None), 1), "out of retries");
        let patient = RetryPolicy { retry_timeouts: true, ..p };
        assert_ne!(decide(&patient, &req, &Err(Error::Timeout(1000)), 0, 1.0), Next::Done);
        let limited = HttpRequest::get("http://x.test/").no_retry_when_limited();
        assert_eq!(decide(&p, &limited, &response(429, None), 0, 1.0), Next::Done);
        assert_eq!(decide(&p, &limited, &response(503, Some("1")), 0, 1.0), Next::Done);
        assert_ne!(decide(&p, &limited, &response(500, None), 0, 1.0), Next::Done);
    }

    #[test]
    fn attempts_are_counted_and_the_last_outcome_comes_back() {
        let req = HttpRequest::get("http://x.test/");
        let mut calls = 0;
        let r = send(&QUICK, &req, |_| {
            calls += 1;
            response(503, None)
        });
        assert_eq!((r.unwrap().status, calls), (503, 3), "one attempt and two retries");

        let mut calls = 0;
        let r = send(&QUICK, &req, |_| {
            calls += 1;
            if calls == 1 {
                Err(Error::Network("reset".into()))
            } else {
                response(200, None)
            }
        });
        assert_eq!((r.unwrap().status, calls), (200, 2));
    }

    #[test]
    fn a_wait_ends_when_the_request_is_cancelled() {
        let flag = Arc::new(AtomicBool::new(false));
        let raise = flag.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            raise.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        assert!(matches!(wait(Duration::from_secs(30), Some(&flag)), Err(Error::Cancelled)));
        assert!(started.elapsed() < Duration::from_secs(5), "took {:?}", started.elapsed());
        // Not raised: the whole wait passes.
        let calm = Arc::new(AtomicBool::new(false));
        let started = Instant::now();
        wait(Duration::from_millis(40), Some(&calm)).unwrap();
        assert!(started.elapsed() >= Duration::from_millis(40));
    }

    #[test]
    fn a_cancelled_request_is_not_sent_and_stops_between_attempts() {
        let flag = Arc::new(AtomicBool::new(true));
        let req = HttpRequest::get("http://x.test/").cancel(flag.clone());
        let mut calls = 0;
        let r = send(&QUICK, &req, |_| {
            calls += 1;
            response(200, None)
        });
        assert!(matches!(r, Err(Error::Cancelled)));
        assert_eq!(calls, 0);

        // Raised by the first attempt's failure handler: the long wait that
        // would follow is cut short.
        flag.store(false, Ordering::SeqCst);
        let slow = RetryPolicy { base: Duration::from_secs(30), cap: Duration::from_secs(30), ..QUICK };
        let started = Instant::now();
        let r = send(&slow, &req, |_| {
            flag.store(true, Ordering::SeqCst);
            response(503, None)
        });
        assert!(matches!(r, Err(Error::Cancelled)));
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
