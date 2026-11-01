//! E4 unit tests: server routes + client + push.
//!
//! Модули `src/repo/*` подключаются через один `#[path]` к `mod.rs`;
//! Rust сам находит `add.rs`, `http.rs` и т.д. рядом с ним.
//! Для `server.rs` (использует `crate::log`) — заглушка в корне теста.

#![allow(dead_code)]

#[path = "../src/repo/mod.rs"]
mod repo;

/// Заглушка `crate::log` для `server.rs` / `add.rs`.
pub fn log(_msg: impl AsRef<str>) {}

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine as _;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use repo::{http, layout, server, util};

// ============================================================================
// helpers
// ============================================================================

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

/// Хелпер для будущих тестов; отметим `#[allow]`, чтобы не ловить dead_code.
#[allow(dead_code)]
fn make_labels(pkg_name: &str, version: &str, release: &str) -> (Vec<u8>, serde_json::Value) {
    let spec = format!(
        "[package]\nname = \"{pkg_name}\"\nversion = \"{version}\"\n"
    ).into_bytes();
    let spec_b64 = base64::engine::general_purpose::STANDARD.encode(&spec);
    let spec_sha = sha256_hex(&spec);
    let labels = serde_json::json!({
        "data": { "attributes": { "labels": {
            "org.appspec.package.name": pkg_name,
            "org.appspec.package.version": version,
            "org.appspec.package.release": release,
            "org.appspec.package.epoch": "0",
            "org.appspec.package.arch": "amd64",
            "org.appspec.spec.content.base64": spec_b64,
            "org.appspec.spec.sha256": spec_sha,
            "org.appspec.spec.filename": format!("{pkg_name}.appspec"),
            "org.appspec.generator": "appspec 0.3.4"
        }}}
    });
    (spec, labels)
}

// ============================================================================
// mock HTTP server
// ============================================================================

fn mock_server<F>(responder: F) -> (u16, std::thread::JoinHandle<()>)
where
    F: FnOnce(Vec<u8>) -> Vec<u8> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            if stream.read_exact(&mut byte).is_err() { break; }
            buf.push(byte[0]);
            if buf.ends_with(b"\r\n\r\n") { break; }
        }
        let header_str = String::from_utf8_lossy(&buf);
        let cl: usize = header_str.lines()
            .find_map(|l| {
                let lower = l.to_ascii_lowercase();
                if lower.starts_with("content-length:") {
                    l.split(':').nth(1).and_then(|v| v.trim().parse().ok())
                } else { None }
            })
            .unwrap_or(0);
        let mut body = vec![0u8; cl];
        if cl > 0 { let _ = stream.read_exact(&mut body); }
        let resp = responder(body);
        let _ = stream.write_all(&resp);
    });
    (port, handle)
}

fn ok_json(v: serde_json::Value) -> Vec<u8> {
    let b = serde_json::to_vec(&v).unwrap();
    let mut out = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        b.len()
    ).into_bytes();
    out.extend_from_slice(&b);
    out
}

fn status_json(status: u16, reason: &str, v: serde_json::Value) -> Vec<u8> {
    let b = serde_json::to_vec(&v).unwrap();
    let mut out = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        status, reason, b.len()
    ).into_bytes();
    out.extend_from_slice(&b);
    out
}

// ============================================================================
// server state
// ============================================================================

fn make_state() -> (TempDir, Arc<server::ServerState>) {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("blobs/sha256")).unwrap();
    fs::create_dir_all(tmp.path().join("packages")).unwrap();
    fs::create_dir_all(tmp.path().join("refs")).unwrap();
    fs::create_dir_all(tmp.path().join("index")).unwrap();
    let cfg = layout::Config {
        repo: layout::RepoMeta {
            name: "test/e4".into(),
            description: String::new(),
            created_at: util::now_iso8601(),
            generator: "appspec test".into(),
            layout: 1,
        },
        policy: Default::default(),
        signing: Default::default(),
        server: layout::Server {
            listen: "127.0.0.1:0".into(),
            max_push_bytes: 10_000_000,
            read_timeout_secs: 30,
            write_timeout_secs: 30,
            tokens: vec![layout::ServerToken {
                name: "test".into(),
                hash: format!("sha256:{}", sha256_hex(b"secret-token")),
            }],
            tls: Default::default(),
        },
        ipfs: Default::default(),
    };
    fs::write(tmp.path().join("config.toml"), toml::to_string_pretty(&cfg).unwrap()).unwrap();
    fs::write(tmp.path().join("index.json"), b"{}").unwrap();

    let state = server::ServerState {
        root_path: tmp.path().to_path_buf(),
        config: cfg,
        write_lock: std::sync::Mutex::new(()),
    };
    (tmp, Arc::new(state))
}

fn req(method: &str, path: &str, headers: &[(&str, &str)], body: Vec<u8>) -> http::Request {
    let mut h = BTreeMap::new();
    for (k, v) in headers {
        h.insert(k.to_ascii_lowercase(), v.to_string());
    }
    http::Request {
        method: method.into(),
        path: path.into(),
        version: "HTTP/1.1".into(),
        headers: h,
        body,
    }
}

// ============================================================================
// server routes
// ============================================================================

#[test]
fn server_healthz_returns_200() {
    let (_tmp, state) = make_state();
    let r = req("GET", "/healthz", &[], Vec::new());
    let resp = server::route(&r, &state);
    assert_eq!(resp.status, 200);
    let v: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(v["status"], "ok");
}

#[test]
fn server_unknown_path_returns_404() {
    let (_tmp, state) = make_state();
    let r = req("GET", "/nope", &[], Vec::new());
    assert_eq!(server::route(&r, &state).status, 404);
}

#[test]
fn server_blocks_path_traversal() {
    let (_tmp, state) = make_state();
    let r = req("GET", "/packages/../etc/passwd", &[], Vec::new());
    let resp = server::route(&r, &state);
    assert!(resp.status == 400 || resp.status == 404,
        "expected 400/404, got {}", resp.status);
}

#[test]
fn server_push_without_auth_returns_401() {
    let (_tmp, state) = make_state();
    let r = req("POST", "/api/v1/push", &[], b"fake".to_vec());
    assert_eq!(server::route(&r, &state).status, 401);
}

#[test]
fn server_push_with_wrong_token_returns_401() {
    let (_tmp, state) = make_state();
    let r = req("POST", "/api/v1/push",
        &[("Authorization", "Bearer wrong")], b"fake".to_vec());
    assert_eq!(server::route(&r, &state).status, 401);
}

#[test]
fn server_push_with_empty_body_returns_400() {
    let (_tmp, state) = make_state();
    let r = req("POST", "/api/v1/push",
        &[("Authorization", "Bearer secret-token")], Vec::new());
    assert_eq!(server::route(&r, &state).status, 400);
}

// ============================================================================
// client
// ============================================================================

#[test]
fn client_get_parses_response() {
    let (port, h) = mock_server(|_| ok_json(serde_json::json!({"ok": true})));
    let url = format!("http://127.0.0.1:{}/test", port);
    let (status, _hdrs, body) = http::http_request(&url, "GET", &[], None).unwrap();
    assert_eq!(status, 200);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["ok"], true);
    h.join().unwrap();
}

#[test]
fn client_post_sends_body() {
    let (port, h) = mock_server(|body| {
        let b = String::from_utf8_lossy(&body).to_string();
        ok_json(serde_json::json!({"echo": b}))
    });
    let url = format!("http://127.0.0.1:{}/echo", port);
    let (_s, _hd, body) = http::http_request(&url, "POST", &[], Some(b"hello")).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["echo"], "hello");
    h.join().unwrap();
}

#[test]
fn client_rejects_https() {
    let r = http::http_request("https://example.org/x", "GET", &[], None);
    assert!(r.is_err());
    assert!(format!("{:#}", r.unwrap_err()).contains("only http"));
}

// ============================================================================
// push_one
// ============================================================================

#[test]
fn push_one_success_returns_outcome() {
    let (port, h) = mock_server(|_body| {
        status_json(201, "Created", serde_json::json!({
            "sha256": "abc123",
            "package": "demo",
            "version": "0.0.1-1",
            "already_present": false
        }))
    });
    let tmp = TempDir::new().unwrap();
    let sif = tmp.path().join("x.sif");
    fs::write(&sif, b"fake").unwrap();
    let url = format!("http://127.0.0.1:{}", port);
    let out = repo::push::push_one(&sif, &url, "tok").unwrap();
    assert_eq!(out.package, "demo");
    assert_eq!(out.version, "0.0.1-1");
    assert!(!out.already_present);
    h.join().unwrap();
}

#[test]
fn push_one_401_is_error() {
    let (port, h) = mock_server(|_| {
        status_json(401, "Unauthorized", serde_json::json!({"error":"unauthorized"}))
    });
    let tmp = TempDir::new().unwrap();
    let sif = tmp.path().join("x.sif");
    fs::write(&sif, b"fake").unwrap();
    let url = format!("http://127.0.0.1:{}", port);
    let r = repo::push::push_one(&sif, &url, "wrong");
    assert!(r.is_err());
    assert!(format!("{:#}", r.unwrap_err()).contains("401"));
    h.join().unwrap();
}

#[test]
fn push_one_409_is_error() {
    let (port, h) = mock_server(|_| {
        status_json(409, "Conflict", serde_json::json!({
            "error":"conflict", "reason":"demo:0.0.1-1 already exists"
        }))
    });
    let tmp = TempDir::new().unwrap();
    let sif = tmp.path().join("x.sif");
    fs::write(&sif, b"fake").unwrap();
    let url = format!("http://127.0.0.1:{}", port);
    let r = repo::push::push_one(&sif, &url, "tok");
    assert!(r.is_err());
    assert!(format!("{:#}", r.unwrap_err()).contains("409"));
    h.join().unwrap();
}

// ============================================================================
// smoke: make_labels (чтобы хелпер не был dead_code)
// ============================================================================

#[test]
fn make_labels_helper_works() {
    let (spec, labels) = make_labels("demo", "0.0.1", "1");
    assert_eq!(labels["data"]["attributes"]["labels"]["org.appspec.package.name"], "demo");
    assert!(!spec.is_empty());
    let _ = PathBuf::from("/tmp");
}