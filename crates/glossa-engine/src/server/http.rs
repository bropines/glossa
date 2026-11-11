//! The control API's HTTP/1.1 listener: just enough of the protocol for
//! `curl`, scripts and `glossa-cli`, with every limit under our control.
//!
//! A general-purpose server library was used here before; it started a
//! thread per connection without a cap, never timed a slow client out, and
//! on dropping a request read the rest of its declared body into one
//! allocation of that size, so a single `Content-Length: 99999999999999`
//! from any local process could end the program (a failed allocation
//! aborts). Here instead:
//!
//! - at most [`MAX_CONNECTIONS`] connections are served at once; one more
//!   gets a `503` with `Retry-After` from the accept thread and is closed;
//! - the request head is at most [`MAX_HEAD`] bytes and must arrive within
//!   [`HEAD_TIMEOUT`]; a body must arrive within [`BODY_TIMEOUT`];
//! - a body is read only when a route asks for it, and only up to that
//!   route's limit: a longer one is refused by its `Content-Length` before
//!   a byte of it is read or allocated;
//! - a failed `accept` is logged and the loop carries on.
//!
//! Every answer closes the connection. Chunked request bodies are refused
//! (`411`): every client this API is for sends a `Content-Length`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub use glossa_net::Method;

/// Connections served at once. Each is a thread, and may hold a body in
/// memory; key bindings and scripts need a handful.
pub const MAX_CONNECTIONS: usize = 32;
/// The request line and headers together.
const MAX_HEAD: u64 = 16 * 1024;
const MAX_HEADERS: usize = 64;
/// How long a client may take to send its request line and headers.
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a body may take, whole.
const BODY_TIMEOUT: Duration = Duration::from_secs(60);
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// One request: its head, parsed, and the connection its body waits on.
pub struct Request {
    method: Method,
    url: String,
    headers: Vec<(String, String)>,
    peer: SocketAddr,
    /// `Content-Length`; 0 without one.
    declared: u64,
    stream: BufReader<TcpStream>,
    /// How much of the body has been read.
    consumed: u64,
}

/// Why a body could not be read.
#[derive(Debug)]
pub enum BodyError {
    /// Longer than the route accepts; nothing was read.
    TooLarge { limit: u64 },
    /// The client stopped sending or took too long.
    Io(std::io::Error),
}

/// An answer: status, headers and body.
pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
    pub content_type: &'static str,
    /// The `Allow` header of a 405.
    pub allow: Option<&'static str>,
    /// `Retry-After`, in seconds, of a 503.
    pub retry_after: Option<u32>,
}

impl Request {
    pub fn method(&self) -> Method {
        self.method
    }

    /// The request target as sent: path and query.
    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// The address the connection came from, as the system reports it:
    /// nothing the client sends can change it.
    pub fn peer(&self) -> SocketAddr {
        self.peer
    }

    /// The body, when it is at most `limit` bytes. A longer one is refused
    /// without reading it. A second call returns nothing more.
    pub fn body(&mut self, limit: u64) -> Result<Vec<u8>, BodyError> {
        let left = self.declared - self.consumed;
        if left > limit {
            return Err(BodyError::TooLarge { limit });
        }
        if left == 0 {
            return Ok(Vec::new());
        }
        // curl announces a body over a megabyte with `Expect: 100-continue`
        // and waits a second for the go-ahead before sending it anyway.
        if self.consumed == 0 && self.header("Expect").is_some_and(|v| v.eq_ignore_ascii_case("100-continue")) {
            let stream = self.stream.get_mut();
            stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").and_then(|()| stream.flush()).map_err(BodyError::Io)?;
        }
        let deadline = Instant::now() + BODY_TIMEOUT;
        let mut body = Vec::with_capacity(left.min(64 * 1024) as usize);
        let mut buf = [0u8; 16 * 1024];
        while (body.len() as u64) < left {
            let now = Instant::now();
            if now >= deadline {
                return Err(BodyError::Io(std::io::Error::new(std::io::ErrorKind::TimedOut, "the body took too long")));
            }
            // Each read waits at most until the deadline, so a client that
            // drips a byte now and then cannot hold the connection for ever.
            let _ = self.stream.get_ref().set_read_timeout(Some((deadline - now).min(HEAD_TIMEOUT)));
            let want = (left - body.len() as u64).min(buf.len() as u64) as usize;
            match self.stream.read(&mut buf[..want]) {
                Ok(0) => {
                    return Err(BodyError::Io(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "the body is shorter than its Content-Length",
                    )))
                }
                Ok(n) => {
                    body.extend_from_slice(&buf[..n]);
                    self.consumed += n as u64;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(BodyError::Io(e)),
            }
        }
        Ok(body)
    }

    /// Send `reply` and close the connection.
    pub fn respond(self, reply: Reply) {
        let head_only = self.method == Method::Head;
        let unread = self.declared > self.consumed;
        let mut stream = self.stream.into_inner();
        if write_reply(&mut stream, &reply, head_only).is_ok() && unread {
            // Closing with unread input makes the system reset the
            // connection, and the client may lose the answer it has not
            // read yet (a 413 above all). Say we are done, then let a
            // little of what is still on its way arrive before closing.
            let _ = stream.shutdown(Shutdown::Write);
            let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
            let mut sink = [0u8; 8192];
            let mut drained = 0usize;
            // One second in all: a client trickling bytes must not hold a
            // connection slot for as long as it likes.
            let until = std::time::Instant::now() + Duration::from_secs(1);
            while drained < 256 * 1024 && std::time::Instant::now() < until {
                match stream.read(&mut sink) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => drained += n,
                }
            }
        }
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        411 => "Length Required",
        412 => "Precondition Failed",
        413 => "Payload Too Large",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Status",
    }
}

fn write_reply(stream: &mut TcpStream, reply: &Reply, head_only: bool) -> std::io::Result<()> {
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        reply.status,
        reason(reply.status),
        reply.content_type,
        reply.body.len()
    );
    if let Some(allow) = reply.allow {
        head.push_str(&format!("Allow: {allow}\r\n"));
    }
    if let Some(secs) = reply.retry_after {
        head.push_str(&format!("Retry-After: {secs}\r\n"));
    }
    head.push_str("\r\n");
    let mut out = head.into_bytes();
    if !head_only {
        out.extend_from_slice(&reply.body);
    }
    stream.write_all(&out)?;
    stream.flush()
}

fn plain_error(status: u16, message: &str) -> Reply {
    Reply {
        status,
        body: serde_json::json!({ "error": message }).to_string().into_bytes(),
        content_type: "application/json; charset=utf-8",
        allow: None,
        retry_after: None,
    }
}

enum LineError {
    /// The client closed the connection or it failed.
    Gone,
    /// The head's size limit ran out before the line ended.
    TooLong,
    /// The deadline passed.
    Late,
}

/// One line of the head, with its line end, taken from `budget`. Every
/// read waits no longer than until `deadline`, so a client sending a byte
/// now and then is cut off like one sending nothing.
fn read_line(reader: &mut BufReader<TcpStream>, budget: &mut u64, deadline: Instant) -> Result<Vec<u8>, LineError> {
    let mut line = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(LineError::Late);
        }
        let _ = reader.get_ref().set_read_timeout(Some(left));
        let available = match reader.fill_buf() {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                return Err(LineError::Late)
            }
            Err(_) => return Err(LineError::Gone),
        };
        if available.is_empty() {
            return Err(LineError::Gone);
        }
        let end = available.iter().position(|&b| b == b'\n').map(|i| i + 1);
        let take = end.unwrap_or(available.len());
        if take as u64 > *budget {
            return Err(LineError::TooLong);
        }
        line.extend_from_slice(&available[..take]);
        reader.consume(take);
        *budget -= take as u64;
        if end.is_some() {
            return Ok(line);
        }
    }
}

/// Read a request's head from a new connection. An error is the answer to
/// send instead (`None`: the client sent nothing, or went away).
fn read_head(stream: TcpStream, peer: SocketAddr) -> Result<Request, (TcpStream, Option<Reply>)> {
    let _ = stream.set_read_timeout(Some(HEAD_TIMEOUT));
    let _ = stream.set_nodelay(true);
    let Ok(reading) = stream.try_clone() else { return Err((stream, None)) };
    let mut reader = BufReader::new(reading);
    let fail = |status: u16, message: &str| Some(plain_error(status, message));
    let deadline = Instant::now() + HEAD_TIMEOUT;

    let mut lines: Vec<String> = Vec::new();
    let mut budget = MAX_HEAD;
    loop {
        let line = match read_line(&mut reader, &mut budget, deadline) {
            Ok(line) => line,
            Err(LineError::Gone) => return Err((stream, None)),
            Err(LineError::TooLong) => return Err((stream, fail(431, "the request headers are too large"))),
            Err(LineError::Late) => {
                // Nothing at all: a connection opened and left idle, which
                // deserves no answer.
                let reply = if lines.is_empty() { None } else { fail(408, "the request did not arrive in time") };
                return Err((stream, reply));
            }
        };
        let Ok(text) = String::from_utf8(line) else {
            return Err((stream, fail(400, "the request is not valid UTF-8")));
        };
        let text = text.trim_end_matches(['\r', '\n']);
        if text.is_empty() {
            if lines.is_empty() {
                continue; // a stray blank line before the request line
            }
            break;
        }
        if lines.len() > MAX_HEADERS {
            return Err((stream, fail(431, "too many request headers")));
        }
        lines.push(text.to_owned());
    }

    let mut parts = lines[0].split(' ');
    let (Some(method), Some(url), Some(version), None) = (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err((stream, fail(400, "malformed request line")));
    };
    if !version.starts_with("HTTP/1.") || !url.starts_with('/') {
        return Err((stream, fail(400, "malformed request line")));
    }
    // Method names are case-sensitive; only the ones a route could take.
    let Some(method) = Some(method).filter(|m| m.bytes().all(|b| b.is_ascii_uppercase())).and_then(Method::parse)
    else {
        return Err((stream, fail(501, "unknown method")));
    };
    let mut headers = Vec::with_capacity(lines.len() - 1);
    for line in &lines[1..] {
        let Some((name, value)) = line.split_once(':') else {
            return Err((stream, fail(400, "malformed header")));
        };
        if name.is_empty() || name.contains(|c: char| c.is_whitespace()) {
            return Err((stream, fail(400, "malformed header")));
        }
        headers.push((name.to_owned(), value.trim().to_owned()));
    }
    fn values<'a>(headers: &'a [(String, String)], name: &'a str) -> impl Iterator<Item = &'a str> {
        headers.iter().filter(move |(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    if values(&headers, "Transfer-Encoding").next().is_some() {
        return Err((stream, fail(411, "send the body with a Content-Length, not chunked")));
    }
    let mut declared = None;
    for value in values(&headers, "Content-Length") {
        // Digits only: `+5` and ` 5 5` are not lengths, and two different
        // lengths are how request smuggling starts.
        let parsed = Some(value).filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()));
        match (parsed.and_then(|v| v.parse::<u64>().ok()), declared) {
            (Some(n), None) => declared = Some(n),
            (Some(n), Some(seen)) if n == seen => {}
            _ => return Err((stream, fail(400, "bad Content-Length"))),
        }
    }
    drop(stream);
    Ok(Request {
        method,
        url: url.to_owned(),
        headers,
        peer,
        declared: declared.unwrap_or(0),
        stream: reader,
        consumed: 0,
    })
}

/// A place among the connections being served, given back when dropped.
struct Slot(Arc<AtomicUsize>);

impl Slot {
    fn take(active: &Arc<AtomicUsize>, max: usize) -> Option<Slot> {
        if active.fetch_add(1, Ordering::SeqCst) >= max {
            active.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Slot(active.clone()))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Accept connections until `stop` is raised, serving each on a thread of
/// its own (a slow translation must not hold up a key binding), at most
/// `max` at once. `accept` is the listener's; a failure (the process is out
/// of file descriptors, a connection reset before it was accepted) is
/// logged and the loop goes on, after a pause that grows while it lasts.
pub fn serve(
    mut accept: impl FnMut() -> std::io::Result<(TcpStream, SocketAddr)>,
    stop: &AtomicBool,
    max: usize,
    handler: Arc<dyn Fn(Request) -> Option<(Request, Reply)> + Send + Sync>,
) {
    let active = Arc::new(AtomicUsize::new(0));
    let mut failures = 0u32;
    while !stop.load(Ordering::SeqCst) {
        let (mut stream, peer) = match accept() {
            Ok(conn) => conn,
            Err(_) if stop.load(Ordering::SeqCst) => return,
            Err(e) => {
                failures += 1;
                // The first failure of a run and every hundredth after it.
                if failures % 100 == 1 {
                    tracing::warn!("control API: accepting a connection failed ({e}); still listening");
                }
                std::thread::sleep(Duration::from_millis(u64::from(failures.min(20)) * 50));
                continue;
            }
        };
        if failures > 0 {
            tracing::info!("control API: accepting connections again after {failures} failures");
            failures = 0;
        }
        if stop.load(Ordering::SeqCst) {
            return; // the wake-up connection
        }
        let Some(slot) = Slot::take(&active, max) else {
            // Answered here, without reading anything: the reply fits the
            // socket's buffer, and the short timeout covers the rest.
            let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
            let mut busy = plain_error(503, "too many requests at once; try again");
            busy.retry_after = Some(1);
            let _ = write_reply(&mut stream, &busy, false);
            continue;
        };
        let handler = handler.clone();
        let spawned = std::thread::Builder::new().name("glossa-api-req".into()).spawn(move || {
            let _slot = slot;
            match read_head(stream, peer) {
                Ok(request) => {
                    if let Some((request, reply)) = handler(request) {
                        request.respond(reply);
                    }
                }
                Err((mut stream, Some(reply))) => {
                    let _ = write_reply(&mut stream, &reply, false);
                }
                Err((_, None)) => {}
            }
        });
        if let Err(e) = spawned {
            tracing::warn!("control API: no thread for a request: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A server answering every request with its method, path, peer and
    /// body length (reading bodies up to `limit`).
    fn echo(limit: u64, max: usize) -> (SocketAddr, Arc<AtomicBool>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        std::thread::spawn(move || {
            serve(
                || listener.accept(),
                &stopped,
                max,
                Arc::new(move |mut req: Request| {
                    let reply = match req.body(limit) {
                        Ok(body) => {
                            if req.url().starts_with("/slow") {
                                std::thread::sleep(Duration::from_millis(600));
                            }
                            Reply {
                                status: 200,
                                body: format!(
                                    "{} {} {} {}",
                                    req.method().as_str(),
                                    req.url(),
                                    req.peer().ip(),
                                    body.len()
                                )
                                .into_bytes(),
                                content_type: "text/plain",
                                allow: None,
                                retry_after: None,
                            }
                        }
                        Err(BodyError::TooLarge { .. }) => plain_error(413, "request body too large"),
                        Err(BodyError::Io(e)) => plain_error(400, &e.to_string()),
                    };
                    Some((req, reply))
                }),
            )
        });
        (addr, stop)
    }

    fn exchange(addr: SocketAddr, raw: &[u8]) -> String {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let _ = s.write_all(raw);
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out
    }

    #[test]
    fn requests_are_parsed_and_answered() {
        let (addr, _stop) = echo(1024, 4);
        let out = exchange(addr, b"POST /a?b=c HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\nhello");
        assert!(out.starts_with("HTTP/1.1 200 OK\r\n"), "{out}");
        assert!(out.contains("Connection: close\r\n"), "{out}");
        assert!(out.ends_with("POST /a?b=c 127.0.0.1 5"), "{out}");
        // No body, bare LF line ends, a HEAD.
        assert!(exchange(addr, b"GET / HTTP/1.0\n\n").ends_with("GET / 127.0.0.1 0"));
        let out = exchange(addr, b"HEAD /h HTTP/1.1\r\n\r\n");
        assert!(out.starts_with("HTTP/1.1 200") && out.ends_with("\r\n\r\n"), "a HEAD has no body: {out}");
    }

    #[test]
    fn malformed_requests_are_refused() {
        let (addr, _stop) = echo(1024, 4);
        for (raw, status) in [
            (&b"GET\r\n\r\n"[..], "400"),
            (b"GET / HTTP/1.1 extra\r\n\r\n", "400"),
            (b"GET http://evil.example/ HTTP/1.1\r\n\r\n", "400"),
            (b"GET / SPDY/3\r\n\r\n", "400"),
            (b"BREW / HTTP/1.1\r\n\r\n", "501"),
            (b"get / HTTP/1.1\r\n\r\n", "501"),
            (b"GET / HTTP/1.1\r\nno colon here\r\n\r\n", "400"),
            (b"GET / HTTP/1.1\r\nBad Name: x\r\n\r\n", "400"),
            (b"POST / HTTP/1.1\r\nContent-Length: five\r\n\r\n", "400"),
            (b"POST / HTTP/1.1\r\nContent-Length: +5\r\n\r\nhello", "400"),
            (b"POST / HTTP/1.1\r\nContent-Length: 5\r\nContent-Length: 6\r\n\r\nhello!", "400"),
            (b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n", "411"),
            (b"GET / HTTP/1.1\r\n\xff\xfe: x\r\n\r\n", "400"),
        ] {
            let out = exchange(addr, raw);
            assert!(out.starts_with(&format!("HTTP/1.1 {status} ")), "{:?} -> {out}", String::from_utf8_lossy(raw));
        }
        // The same length twice is one length.
        let out = exchange(addr, b"POST / HTTP/1.1\r\nContent-Length: 2\r\nContent-Length: 2\r\n\r\nok");
        assert!(out.ends_with(" 2"), "{out}");
    }

    #[test]
    fn oversized_heads_and_bodies_are_refused_without_being_read() {
        let (addr, _stop) = echo(1024, 4);
        let mut big = b"GET / HTTP/1.1\r\nX-Padding: ".to_vec();
        big.extend(std::iter::repeat_n(b'a', 20 * 1024));
        big.extend_from_slice(b"\r\n\r\n");
        assert!(exchange(addr, &big).starts_with("HTTP/1.1 431 "));
        let mut many = b"GET / HTTP/1.1\r\n".to_vec();
        for i in 0..100 {
            many.extend_from_slice(format!("X-{i}: v\r\n").as_bytes());
        }
        many.extend_from_slice(b"\r\n");
        assert!(exchange(addr, &many).starts_with("HTTP/1.1 431 "));

        // A body one byte over the limit, and one announced as larger than
        // any memory: refused by the announcement alone, at once.
        let started = Instant::now();
        let out = exchange(addr, b"POST / HTTP/1.1\r\nContent-Length: 1025\r\n\r\n");
        assert!(out.starts_with("HTTP/1.1 413 "), "{out}");
        let out = exchange(addr, b"POST / HTTP/1.1\r\nContent-Length: 99999999999999999\r\n\r\nabc");
        assert!(out.starts_with("HTTP/1.1 413 "), "{out}");
        let out = exchange(addr, b"POST / HTTP/1.1\r\nContent-Length: 999999999999999999999999999\r\n\r\n");
        assert!(out.starts_with("HTTP/1.1 400 "), "not a number we can hold: {out}");
        assert!(started.elapsed() < Duration::from_secs(5), "took {:?}", started.elapsed());
        // At the limit it is read whole.
        let mut exact = b"POST / HTTP/1.1\r\nContent-Length: 1024\r\n\r\n".to_vec();
        exact.extend(std::iter::repeat_n(b'x', 1024));
        assert!(exchange(addr, &exact).ends_with(" 1024"));
        // A body cut short is an error, not a hang until the timeout.
        let mut s = TcpStream::connect(addr).unwrap();
        s.write_all(b"POST / HTTP/1.1\r\nContent-Length: 10\r\n\r\nabc").unwrap();
        s.shutdown(Shutdown::Write).unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        assert!(out.starts_with("HTTP/1.1 400 "), "{out}");
    }

    #[test]
    fn a_large_body_announced_with_expect_gets_the_go_ahead() {
        let (addr, _stop) = echo(1024, 4);
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        s.write_all(b"POST / HTTP/1.1\r\nContent-Length: 4\r\nExpect: 100-continue\r\n\r\n").unwrap();
        let mut go = [0u8; 25];
        s.read_exact(&mut go).unwrap();
        assert_eq!(&go, b"HTTP/1.1 100 Continue\r\n\r\n");
        s.write_all(b"body").unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        assert!(out.starts_with("HTTP/1.1 200 ") && out.ends_with(" 4"), "{out}");
        // Too large: refused before the client sends it.
        let out = exchange(addr, b"POST / HTTP/1.1\r\nContent-Length: 5000\r\nExpect: 100-continue\r\n\r\n");
        assert!(out.starts_with("HTTP/1.1 413 "), "{out}");
    }

    #[test]
    fn connections_beyond_the_cap_are_turned_away_and_the_cap_frees_up() {
        let (addr, _stop) = echo(1024, 2);
        let slow: Vec<_> =
            (0..2).map(|_| std::thread::spawn(move || exchange(addr, b"GET /slow HTTP/1.1\r\n\r\n"))).collect();
        std::thread::sleep(Duration::from_millis(200));
        let out = exchange(addr, b"GET /third HTTP/1.1\r\n\r\n");
        assert!(out.starts_with("HTTP/1.1 503 "), "{out}");
        assert!(out.contains("Retry-After: 1\r\n"), "{out}");
        for t in slow {
            assert!(t.join().unwrap().starts_with("HTTP/1.1 200 "));
        }
        // Idle connections that never send a request hold a place only
        // until they are dropped; afterwards there is room again.
        let idle: Vec<_> = (0..2).map(|_| TcpStream::connect(addr).unwrap()).collect();
        std::thread::sleep(Duration::from_millis(100));
        assert!(exchange(addr, b"GET / HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 503 "));
        drop(idle);
        std::thread::sleep(Duration::from_millis(200));
        assert!(exchange(addr, b"GET / HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 200 "));
    }

    #[test]
    fn a_failing_accept_is_survived() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let mut calls = 0;
        std::thread::spawn(move || {
            serve(
                move || {
                    calls += 1;
                    if calls <= 3 {
                        return Err(std::io::Error::other("out of file descriptors"));
                    }
                    listener.accept()
                },
                &stopped,
                4,
                Arc::new(|req: Request| {
                    let reply = Reply {
                        status: 200,
                        body: b"alive".to_vec(),
                        content_type: "text/plain",
                        allow: None,
                        retry_after: None,
                    };
                    Some((req, reply))
                }),
            )
        });
        assert!(exchange(addr, b"GET / HTTP/1.1\r\n\r\n").ends_with("alive"));
        assert!(exchange(addr, b"GET / HTTP/1.1\r\n\r\n").ends_with("alive"));
    }
}
