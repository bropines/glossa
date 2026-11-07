//! HTTP for the cases: recorded answers (the default, offline), or the real
//! network with or without recording.
//!
//! Both sit below the module's permission check (the runtimes wrap whatever
//! [`Http`] they get in `ScopedHttp`), so a request to a host the manifest
//! does not list fails the same way it would in the program.

use std::path::Path;
use std::sync::Mutex;

use base64::Engine as _;
use glossa_core::Error;
use glossa_net::{Http, HttpRequest, HttpResponse, Method};

use crate::fixture::Exchange;

/// Answers requests from recorded exchanges.
///
/// Each exchange answers the first request whose method matches and whose
/// URL starts with the exchange's `url`, in order; once all matching
/// exchanges are used, the last one keeps answering, so a module that asks
/// twice for the same thing does not need two copies.
pub struct Replay {
    routes: Mutex<Vec<(Exchange, HttpResponse, bool)>>,
    seen: Mutex<Vec<String>>,
}

impl Replay {
    /// `base` is the folder `body_file` paths are relative to.
    pub fn new(exchanges: &[Exchange], base: &Path) -> Result<Replay, String> {
        let mut routes = Vec::new();
        for x in exchanges {
            routes.push((x.clone(), response(x, base)?, false));
        }
        Ok(Replay { routes: Mutex::new(routes), seen: Mutex::new(Vec::new()) })
    }

    /// The requests made so far, as `METHOD url`.
    pub fn requests(&self) -> Vec<String> {
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Http for Replay {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, Error> {
        let line = format!("{} {}", request.method.as_str(), request.url);
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).push(line);
        let mut routes = self.routes.lock().unwrap_or_else(|e| e.into_inner());
        let matches = |x: &Exchange| {
            Method::parse(&x.method.to_ascii_uppercase()) == Some(request.method) && request.url.starts_with(&x.url)
        };
        let pick = routes
            .iter()
            .position(|(x, _, used)| !used && matches(x))
            .or_else(|| routes.iter().rposition(|(x, _, _)| matches(x)));
        match pick {
            Some(i) => {
                routes[i].2 = true;
                Ok(HttpResponse { codepage: request.codepage, ..routes[i].1.clone() })
            }
            None => Err(Error::Network(format!(
                "no recorded response for {} {} (add a [[case.http]] entry, or run with --record)",
                request.method.as_str(),
                glossa_net::redact(&request.url)
            ))),
        }
    }
}

fn response(x: &Exchange, base: &Path) -> Result<HttpResponse, String> {
    let body = if !x.body_file.is_empty() {
        let path = base.join(&x.body_file);
        std::fs::read(&path).map_err(|e| format!("body_file {}: {e}", path.display()))?
    } else if !x.body_base64.is_empty() {
        base64::engine::general_purpose::STANDARD
            .decode(x.body_base64.trim())
            .map_err(|e| format!("body_base64 for {}: {e}", x.url))?
    } else {
        x.body.clone().into_bytes()
    };
    if Method::parse(&x.method.to_ascii_uppercase()).is_none() {
        return Err(format!("unknown HTTP method {:?} for {}", x.method, x.url));
    }
    let content_type =
        if x.content_type.is_empty() { guess_content_type(&body).to_owned() } else { x.content_type.clone() };
    Ok(HttpResponse { status: x.status, headers: vec![("content-type".into(), content_type)], body, codepage: None })
}

fn guess_content_type(body: &[u8]) -> &'static str {
    match body.iter().find(|b| !b.is_ascii_whitespace()) {
        Some(b'{' | b'[') => "application/json; charset=utf-8",
        Some(b'<') => "text/html; charset=utf-8",
        _ => "text/plain; charset=utf-8",
    }
}

/// The real network, optionally keeping what went by.
pub struct Live {
    inner: glossa_net::HttpClient,
    record: bool,
    /// Option values to keep out of recordings (secret options).
    secrets: Vec<String>,
    exchanges: Mutex<Vec<Exchange>>,
    seen: Mutex<Vec<String>>,
}

impl Live {
    pub fn new(record: bool, secrets: Vec<String>) -> Live {
        Live {
            inner: glossa_net::HttpClient::plain(),
            record,
            secrets: secrets.into_iter().filter(|s| s.len() >= 4).collect(),
            exchanges: Mutex::new(Vec::new()),
            seen: Mutex::new(Vec::new()),
        }
    }

    pub fn requests(&self) -> Vec<String> {
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn recorded(&self) -> Vec<Exchange> {
        self.exchanges.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// A URL with everything from the first secret on cut off. The rest is
    /// still a prefix of the real URL, so the recording keeps matching.
    fn scrub_url(&self, url: &str) -> String {
        let cut = self.secrets.iter().filter_map(|s| url.find(s.as_str())).min();
        match cut {
            Some(i) => url[..i].to_owned(),
            None => url.to_owned(),
        }
    }

    fn scrub_text(&self, text: &str) -> String {
        self.secrets.iter().fold(text.to_owned(), |t, s| t.replace(s.as_str(), "REDACTED"))
    }
}

impl Http for Live {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, Error> {
        let method = request.method;
        let url = request.url.clone();
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).push(format!(
            "{} {}",
            method.as_str(),
            glossa_net::redact(&self.scrub_url(&url))
        ));
        let response = self.inner.send(request)?;
        if self.record {
            let content_type = response.header("content-type").unwrap_or_default().to_owned();
            let (body, body_base64) = match std::str::from_utf8(&response.body) {
                Ok(text) => (self.scrub_text(text), String::new()),
                Err(_) => (String::new(), base64::engine::general_purpose::STANDARD.encode(&response.body)),
            };
            self.exchanges.lock().unwrap_or_else(|e| e.into_inner()).push(Exchange {
                method: method.as_str().to_owned(),
                url: self.scrub_url(&url),
                status: response.status,
                content_type,
                body,
                body_base64,
                body_file: String::new(),
            });
        }
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn x(method: &str, url: &str, body: &str) -> Exchange {
        Exchange {
            method: method.into(),
            url: url.into(),
            status: 200,
            content_type: String::new(),
            body: body.into(),
            body_base64: String::new(),
            body_file: String::new(),
        }
    }

    #[test]
    fn replay_matches_by_method_and_prefix_in_order() {
        let r = Replay::new(
            &[x("GET", "https://a/x", "1"), x("GET", "https://a/x", "2"), x("post", "https://a/", "<p>")],
            Path::new("."),
        )
        .unwrap();
        let get = |u: &str| r.send(HttpRequest::get(u)).map(|r| r.text());
        assert_eq!(get("https://a/x?q=1").unwrap(), "1");
        assert_eq!(get("https://a/x?q=2").unwrap(), "2");
        assert_eq!(get("https://a/x?q=3").unwrap(), "2", "the last match keeps answering");
        let posted = r.send(HttpRequest::post("https://a/form")).unwrap();
        assert!(posted.header("content-type").unwrap().starts_with("text/html"));
        let e = get("https://b/").unwrap_err().to_string();
        assert!(e.contains("no recorded response for GET https://b/"), "{e}");
        assert_eq!(r.requests().len(), 5);
    }

    #[test]
    fn recordings_never_keep_a_secret() {
        let live = Live::new(true, vec!["s3cr3t-value".into(), "ab".into()]);
        assert_eq!(live.scrub_url("https://api/v1?x=1&key=s3cr3t-value&y=2"), "https://api/v1?x=1&key=");
        assert_eq!(live.scrub_text("token s3cr3t-value ab"), "token REDACTED ab", "short values are not secrets");
    }
}
