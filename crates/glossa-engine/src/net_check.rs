//! "Test connection" for the network settings: one request through a
//! proxy configuration that may not be saved yet.
//!
//! The front ends call this with the settings on screen, so a proxy can be
//! tried before it replaces the one that works.

use std::time::{Duration, Instant};

use glossa_core::config::Network;
use glossa_core::{Error, Result};
use glossa_net::{Http, HttpClient, HttpRequest};

/// Where the test request goes. `example.com` is the domain IANA reserves
/// for exactly this: it is reachable from everywhere, belongs to no
/// service a user might not want to be seen contacting, and answers over
/// HTTPS, so the request goes through the proxy the way a translation
/// does (a `CONNECT` tunnel), not the way plain HTTP would.
pub const TEST_URL: &str = "https://example.com/";

/// What the test request came back with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionTest {
    /// The HTTP status of the answer.
    pub status: u16,
    pub took: Duration,
    /// The proxy the request went through, as `glossa_net::proxy_in_use`
    /// describes it (no password).
    pub via: String,
}

/// Send one request to [`TEST_URL`] with `network`'s proxy and timeout.
/// Any answer from the site means the connection works; an error says why
/// it does not (the proxy refused, the name did not resolve, it timed out).
/// Blocks for up to the timeout: call it from a worker.
pub fn test_connection(network: &Network) -> Result<ConnectionTest> {
    // One attempt: a test that retries only takes longer to say no.
    let network = Network { max_retries: 0, ..network.clone() };
    let client = HttpClient::new(&network)?;
    let (status, took) = probe(&client)?;
    Ok(ConnectionTest { status, took, via: glossa_net::proxy_in_use(&network) })
}

fn probe(http: &dyn Http) -> Result<(u16, Duration)> {
    let started = Instant::now();
    let response = http.send(HttpRequest::get(TEST_URL).provider("connection test"))?;
    // A proxy that wants a login answers 407 itself; the site was not reached.
    if response.status == 407 {
        return Err(Error::Config("the proxy asks for a user name and password, or refused the ones given".into()));
    }
    Ok((response.status, started.elapsed()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_net::mock::MockHttp;
    use glossa_net::Method;

    #[test]
    fn any_answer_from_the_site_is_a_working_connection() {
        let http = MockHttp::new().on(Method::Get, TEST_URL, 200, "ok");
        assert_eq!(probe(&http).unwrap().0, 200);
        let http = MockHttp::new().on(Method::Get, TEST_URL, 503, "busy");
        assert_eq!(probe(&http).unwrap().0, 503, "the site answered, so the proxy works");
    }

    #[test]
    fn a_proxy_asking_for_a_login_is_a_failure() {
        let http = MockHttp::new().on(Method::Get, TEST_URL, 407, "");
        assert!(probe(&http).is_err());
    }
}
