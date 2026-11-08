//! Just enough HTTP/1.1 to talk to llama-server on 127.0.0.1: one request
//! per connection (`Connection: close`), the whole body read at once. The
//! server is ours and local, so there is no TLS, proxy or redirect to
//! handle, and no reason to compile in an HTTP client.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

/// A response: status code and body.
pub struct Response {
    pub status: u16,
    pub body: String,
}

/// Send one request to `127.0.0.1:port` and read the whole answer.
pub fn request(port: u16, method: &str, path: &str, body: Option<&str>, timeout: Duration) -> Result<Response, String> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream =
        TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map_err(|e| format!("connecting to {addr}: {e}"))?;
    stream.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(Duration::from_secs(10))).map_err(|e| e.to_string())?;
    let body = body.unwrap_or("");
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nAccept: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(body.as_bytes()))
        .map_err(|e| format!("sending to llama-server: {e}"))?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).map_err(|e| {
        if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) {
            format!("llama-server did not answer within {} s", timeout.as_secs())
        } else {
            format!("reading from llama-server: {e}")
        }
    })?;
    parse(&raw)
}

/// Split a raw response into status and body, undoing chunked encoding.
fn parse(raw: &[u8]) -> Result<Response, String> {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or("llama-server sent no complete HTTP header")?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let mut body = raw[split + 4..].to_vec();
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("not an HTTP status line: {:?}", head.lines().next().unwrap_or("")))?;
    let header = |name: &str| {
        head.lines().skip(1).find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_ascii_lowercase())
        })
    };
    if header("transfer-encoding").is_some_and(|v| v.contains("chunked")) {
        body = dechunk(&body)?;
    } else if let Some(len) = header("content-length").and_then(|v| v.parse::<usize>().ok()) {
        body.truncate(len);
    }
    let body = String::from_utf8(body).map_err(|_| "llama-server answered with text that is not UTF-8".to_owned())?;
    Ok(Response { status, body })
}

fn dechunk(mut data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let eol = data.windows(2).position(|w| w == b"\r\n").ok_or("a chunked body ends early")?;
        let size_text = String::from_utf8_lossy(&data[..eol]);
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| format!("a bad chunk size {size_text:?}"))?;
        data = &data[eol + 2..];
        if size == 0 {
            return Ok(out);
        }
        if data.len() < size {
            return Err("a chunked body ends early".into());
        }
        out.extend_from_slice(&data[..size]);
        data = data.get(size + 2..).unwrap_or(&[]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_and_chunked_bodies() {
        let r = parse(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nokEXTRA").unwrap();
        assert_eq!((r.status, r.body.as_str()), (200, "ok"));
        let r = parse(b"HTTP/1.1 503 Service Unavailable\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n5;x=1\r\npedia\r\n0\r\n\r\n")
            .unwrap();
        assert_eq!((r.status, r.body.as_str()), (503, "Wikipedia"));
        let r = parse("HTTP/1.1 200 OK\r\n\r\nПривет".as_bytes()).unwrap();
        assert_eq!(r.body, "Привет");
        assert!(parse(b"garbage").is_err());
        assert!(parse(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nff\r\nshort").is_err());
    }
}
