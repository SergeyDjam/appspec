//! HTTP/1.1 helpers: серверный парсинг, сериализация ответа, минимальный
//! клиент. Без async, без chunked, без keep-alive — только `Content-Length`.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};

pub struct Request {
    pub method: String,
    pub path: String,
    #[allow(dead_code)] // зарезервировано: access-log с версией (v2)
    pub version: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

pub struct Response {
    pub status: u16,
    pub reason: &'static str,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json(status: u16, reason: &'static str, v: serde_json::Value) -> Self {
        let body = serde_json::to_vec(&v).unwrap_or_else(|_| b"{}".to_vec());
        Response {
            status,
            reason,
            headers: vec![
                ("Content-Type".into(), "application/json".into()),
                ("Content-Length".into(), body.len().to_string()),
            ],
            body,
        }
    }

    pub fn bytes(status: u16, reason: &'static str, ctype: &str, body: Vec<u8>) -> Self {
        Response {
            status,
            reason,
            headers: vec![
                ("Content-Type".into(), ctype.into()),
                ("Content-Length".into(), body.len().to_string()),
            ],
            body,
        }
    }

    pub fn not_found() -> Self {
        Response::json(404, "Not Found", serde_json::json!({"error":"not_found"}))
    }
    pub fn bad_request(msg: &str) -> Self {
        Response::json(
            400,
            "Bad Request",
            serde_json::json!({"error":"invalid","reason":msg}),
        )
    }
    pub fn unauthorized() -> Self {
        Response::json(
            401,
            "Unauthorized",
            serde_json::json!({"error":"unauthorized"}),
        )
    }
    pub fn too_large(limit: u64) -> Self {
        Response::json(
            413,
            "Payload Too Large",
            serde_json::json!({
                "error":"too_large", "limit_bytes": limit
            }),
        )
    }
    pub fn conflict(name: &str, ver: &str, existing: &str) -> Self {
        Response::json(
            409,
            "Conflict",
            serde_json::json!({
                "error":"conflict",
                "reason": format!("{name}:{ver} already exists"),
                "existing_sha256": existing
            }),
        )
    }
    pub fn internal(msg: &str) -> Self {
        Response::json(
            500,
            "Internal Server Error",
            serde_json::json!({"error":"internal","reason":msg}),
        )
    }
}

pub fn read_request<R: Read>(r: &mut BufReader<R>, max_body: u64) -> Result<Request> {
    let mut request_line = String::new();
    r.read_line(&mut request_line)
        .context("read request line")?;
    let request_line = request_line.trim_end_matches(&['\r', '\n'][..]);
    if request_line.is_empty() {
        bail!("empty request line");
    }
    let parts: Vec<&str> = request_line.splitn(3, ' ').collect();
    if parts.len() != 3 {
        bail!("malformed request line: {}", request_line);
    }
    let method = parts[0].to_string();
    let path = parts[1].to_string();
    let version = parts[2].to_string();

    let mut headers = BTreeMap::new();
    loop {
        let mut header_line = String::new();
        r.read_line(&mut header_line).context("read header line")?;
        let header_line = header_line.trim_end_matches(&['\r', '\n'][..]);
        if header_line.is_empty() {
            break;
        }
        if let Some((k, v)) = header_line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }

    let content_length: u64 = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if content_length > max_body {
        bail!("body too large: {} > {}", content_length, max_body);
    }
    let mut body = vec![0u8; content_length as usize];
    if content_length > 0 {
        r.read_exact(&mut body).context("read body")?;
    }

    Ok(Request {
        method,
        path,
        version,
        headers,
        body,
    })
}

pub fn write_response<W: Write>(w: &mut W, r: &Response) -> Result<()> {
    write!(w, "HTTP/1.1 {} {}\r\n", r.status, r.reason)?;
    for (k, v) in &r.headers {
        write!(w, "{}: {}\r\n", k, v)?;
    }
    write!(w, "\r\n")?;
    w.write_all(&r.body)?;
    w.flush()?;
    Ok(())
}

/// Минимальный HTTP-клиент. Только `http://`. Возвращает `(status, headers, body)`.
pub fn http_request(
    url: &str,
    method: &str,
    extra_headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> Result<(u16, BTreeMap<String, String>, Vec<u8>)> {
    let (host, port, path) = parse_url(url)?;
    let mut stream = std::net::TcpStream::connect((host.as_str(), port))
        .with_context(|| format!("connect {}:{}", host, port))?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(300)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(300)))?;

    write!(stream, "{} {} HTTP/1.1\r\n", method, path)?;
    write!(stream, "Host: {}\r\n", host)?;
    write!(stream, "Connection: close\r\n")?;
    for (k, v) in extra_headers {
        write!(stream, "{}: {}\r\n", k, v)?;
    }
    if let Some(b) = body {
        write!(stream, "Content-Length: {}\r\n", b.len())?;
    }
    write!(stream, "\r\n")?;
    if let Some(b) = body {
        stream.write_all(b)?;
    }
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line)?;
    let parts: Vec<&str> = status_line.trim_end().splitn(3, ' ').collect();
    if parts.len() < 2 {
        bail!("malformed status line: {}", status_line);
    }
    let status: u16 = parts[1].parse().context("status code")?;

    let mut headers = BTreeMap::new();
    loop {
        let mut header_line = String::new();
        reader.read_line(&mut header_line)?;
        let header_line = header_line.trim_end_matches(&['\r', '\n'][..]);
        if header_line.is_empty() {
            break;
        }
        if let Some((k, v)) = header_line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }

    let body = if let Some(cl) = headers.get("content-length") {
        let n: usize = cl.parse().unwrap_or(0);
        let mut buf = vec![0u8; n];
        reader.read_exact(&mut buf)?;
        buf
    } else {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf)?;
        buf
    };

    Ok((status, headers, body))
}

fn parse_url(url: &str) -> Result<(String, u16, String)> {
    let rest = url.strip_prefix("http://").ok_or_else(|| {
        anyhow!(
            "only http:// URLs supported in v1 (use reverse proxy for https): {}",
            url
        )
    })?;
    let (host_port, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match host_port.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().context("port")?),
        None => (host_port.to_string(), 80),
    };
    Ok((host, port, path.to_string()))
}
