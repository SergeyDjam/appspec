//! `repo serve`: HTTP-сервер поверх `TcpListener`. Thread-per-connection.
//! Опционально TLS (E10), если в `config.toml` задан `[server.tls]`.
//!
//! Маршруты:
//!   GET  /healthz
//!   GET  /index.json
//!   GET  /blobs/sha256/<ab>/<cdef...>
//!   GET  /packages/<name>/<v>/{meta.json,spec.appspec}
//!   GET  /refs/<name>.toml
//!   POST /api/v1/push

use anyhow::{Context, Result};
use std::io::{BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::http::{read_request, write_response, Request, Response};
use super::layout::{Config, RepoRoot};
use super::util::sha256_hex;

pub struct ServerState {
    pub root_path: PathBuf,
    pub config: Config,
    pub write_lock: Mutex<()>,
}

impl ServerState {
    pub fn from_root(root: &RepoRoot) -> Self {
        ServerState {
            root_path: root.path.clone(),
            config: root.config.clone(),
            write_lock: Mutex::new(()),
        }
    }
}

pub fn serve(root: &RepoRoot) -> Result<()> {
    let listen = root.config.server.listen.clone();

    let tls_cfg: Option<Arc<rustls::ServerConfig>> =
        match super::tls::resolve(&root.config.server.tls.cert, &root.config.server.tls.key)? {
            Some((cert, key)) => {
                crate::log(format!("repo serve: TLS enabled (cert={})", cert));
                Some(super::tls::load_server_config(&cert, &key)?)
            }
            None => None,
        };

    let listener = TcpListener::bind(&listen).with_context(|| format!("bind {}", listen))?;
    let scheme = if tls_cfg.is_some() { "https" } else { "http" };
    crate::log(format!("repo serve: listening on {}://{}", scheme, listen));

    let state = Arc::new(ServerState::from_root(root));

    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let state = state.clone();
                let tls = tls_cfg.clone();
                std::thread::spawn(move || {
                    let peer = s.peer_addr().ok();
                    if let Err(e) = handle(s, peer, state, tls) {
                        if let Some(p) = peer {
                            eprintln!("[server] {p}: {e:#}");
                        } else {
                            eprintln!("[server] connection error: {e:#}");
                        }
                    }
                });
            }
            Err(e) => eprintln!("[server] accept: {e}"),
        }
    }
    Ok(())
}

fn handle(
    stream: TcpStream,
    peer: Option<SocketAddr>,
    state: Arc<ServerState>,
    tls: Option<Arc<rustls::ServerConfig>>,
) -> Result<()> {
    let read_to = state.config.server.read_timeout_secs;
    let write_to = state.config.server.write_timeout_secs;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(read_to)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(write_to)))?;

    match tls {
        Some(cfg) => {
            let conn = rustls::ServerConnection::new(cfg)
                .map_err(|e| anyhow::anyhow!("rustls handshake setup: {e}"))?;
            let mut tls_stream = rustls::StreamOwned::new(conn, stream);
            let result = handle_conn(&mut tls_stream, peer, state);
            // Явный TLS shutdown: close_notify в очередь + flush в TCP.
            // StreamOwned::drop пытается то же самое, но в rustls 0.23
            // это не гарантирует, что alert попадёт на провод до drop.
            tls_stream.conn.send_close_notify();
            while tls_stream.conn.wants_write() {
                if tls_stream.conn.write_tls(&mut tls_stream.sock).is_err() {
                    break;
                }
            }
            result
        }
        None => {
            let mut plain = stream;
            handle_conn(&mut plain, peer, state)
        }
    }
}

fn handle_conn<R: Read + Write>(
    conn: &mut R,
    peer: Option<SocketAddr>,
    state: Arc<ServerState>,
) -> Result<()> {
    // BufReader живёт в блоке — после чтения conn освобождается для write.
    let req_result = {
        let mut reader = BufReader::new(&mut *conn);
        read_request(&mut reader, state.config.server.max_push_bytes)
    };

    let req = match req_result {
        Ok(r) => r,
        Err(e) => {
            let resp = if format!("{e:#}").contains("body too large") {
                Response::too_large(state.config.server.max_push_bytes)
            } else {
                Response::bad_request(&format!("{e:#}"))
            };
            return write_response(conn, &resp);
        }
    };

    let started = std::time::Instant::now();
    let resp = route(&req, &state);
    write_response(conn, &resp)?;

    let ms = started.elapsed().as_millis();
    eprintln!(
        "{}",
        serde_json::json!({
            "ts": super::util::now_iso8601(),
            "peer": peer.map(|p| p.to_string()),
            "method": req.method,
            "path": req.path,
            "status": resp.status,
            "bytes": resp.body.len(),
            "ms": ms
        })
    );
    Ok(())
}

pub fn route(req: &Request, state: &Arc<ServerState>) -> Response {
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/healthz") => Response::json(200, "OK", serde_json::json!({"status":"ok"})),
        ("GET", "/index.json") => serve_file(state, "index.json", "application/json"),
        ("POST", "/api/v1/push") => handle_push(req, state),
        ("GET", p) if p.starts_with("/blobs/sha256/") => {
            serve_file(state, &p[1..], "application/octet-stream")
        }
        ("GET", p) if p.starts_with("/packages/") || p.starts_with("/refs/") => {
            let ct = if p.ends_with(".json") {
                "application/json"
            } else {
                "text/plain"
            };
            serve_file(state, &p[1..], ct)
        }
        _ => Response::not_found(),
    }
}

fn serve_file(state: &ServerState, rel: &str, ctype: &str) -> Response {
    if rel.contains("..") || rel.starts_with('/') {
        return Response::bad_request("invalid path");
    }
    let path = state.root_path.join(rel);
    match std::fs::read(&path) {
        Ok(bytes) => Response::bytes(200, "OK", ctype, bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Response::not_found(),
        Err(e) => Response::internal(&format!("{e}")),
    }
}

fn handle_push(req: &Request, state: &Arc<ServerState>) -> Response {
    let auth = req
        .headers
        .get("authorization")
        .map(String::as_str)
        .unwrap_or("");
    let token = match auth.strip_prefix("Bearer ") {
        Some(t) => t,
        None => return Response::unauthorized(),
    };
    if !verify_token(&state.config, token) {
        return Response::unauthorized();
    }

    if req.body.is_empty() {
        return Response::bad_request("empty body");
    }

    let body_sha = sha256_hex(&req.body);
    if let Some(expected) = req.headers.get("x-sif-sha256") {
        if expected != &body_sha {
            return Response::bad_request(&format!(
                "X-Sif-Sha256 mismatch: header={} body={}",
                expected, body_sha
            ));
        }
    }

    let tmp_dir = state.root_path.join(".incoming");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let tmp_path = tmp_dir.join(format!("{}.sif", body_sha));
    if let Err(e) = std::fs::write(&tmp_path, &req.body) {
        return Response::internal(&format!("write temp: {e}"));
    }

    let _guard = state.write_lock.lock().unwrap();

    // ServerState уже держит Config в памяти — повторный discovery
    // не нужен и может привести к race при hot-reload server config.
    let root = RepoRoot::open_with_config(&state.root_path, state.config.clone());

    let result = super::add::add_one(
        &root,
        super::add::AddParams {
            sif: &tmp_path,
            arch: None,
            release_suffix: None,
            force: false,
            pin_ipfs: false,
            tag: None,
        },
    );
    let _ = std::fs::remove_file(&tmp_path);

    match result {
        Ok(out) => {
            if let Some(expected_spec_sha) = req.headers.get("x-spec-sha256") {
                let meta_path = root
                    .packages_dir()
                    .join(&out.name)
                    .join(format!("{}-{}", out.version, out.release))
                    .join("meta.json");
                if let Ok(text) = std::fs::read_to_string(&meta_path) {
                    if let Ok(meta) = serde_json::from_str::<super::meta::MetaV1>(&text) {
                        if &meta.spec_sha256 != expected_spec_sha {
                            return Response::bad_request(&format!(
                                "X-Spec-Sha256 mismatch: header={} actual={}",
                                expected_spec_sha, meta.spec_sha256
                            ));
                        }
                    }
                }
            }

            if !out.already_present {
                let _ = super::index_build::rebuild_all(&root);
            }

            Response::json(
                201,
                "Created",
                serde_json::json!({
                    "sha256": out.sif_sha256,
                    "package": out.name,
                    "version": format!("{}-{}", out.version, out.release),
                    "already_present": out.already_present
                }),
            )
        }
        Err(e) => {
            let msg = format!("{e:#}");
            if msg.contains("conflict") {
                Response::conflict("?", "?", &body_sha)
            } else {
                Response::bad_request(&msg)
            }
        }
    }
}

fn verify_token(config: &Config, provided: &str) -> bool {
    let provided_sha = format!("sha256:{}", sha256_hex(provided.as_bytes()));
    for tok in &config.server.tokens {
        if constant_time_eq(tok.hash.as_bytes(), provided_sha.as_bytes()) {
            return true;
        }
    }
    false
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}
