//! E10 conformance tests: TLS для `repo serve`.
//!
//! `rcgen` генерирует self-signed cert/key на лету, `rustls` — клиент.
//! Сервер запускается на порту 0, клиент шлёт `GET /healthz`.

#![allow(dead_code)]

#[path = "../src/repo/mod.rs"]
mod repo;

pub fn log(_msg: impl AsRef<str>) {}

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

use repo::{layout, server, tls, util};

// ============================================================================
// helpers
// ============================================================================

fn make_repo() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    fs::create_dir_all(root.join("blobs/sha256")).unwrap();
    fs::create_dir_all(root.join("packages")).unwrap();
    fs::create_dir_all(root.join("refs")).unwrap();
    fs::create_dir_all(root.join("index")).unwrap();
    fs::write(root.join("index/.keep"), b"").unwrap();

    let cfg = layout::Config {
        repo: layout::RepoMeta {
            name: "test/e10".into(),
            description: String::new(),
            created_at: util::now_iso8601(),
            generator: "appspec test".into(),
            layout: 1,
        },
        policy: Default::default(),
        signing: Default::default(),
        server: Default::default(),
        ipfs: Default::default(),
    };
    fs::write(
        root.join("config.toml"),
        toml::to_string_pretty(&cfg).unwrap(),
    )
    .unwrap();
    fs::write(root.join("index.json"), b"{\"schema\":1,\"packages\":{}}").unwrap();
    (tmp, root)
}

/// Self-signed cert+key на диске. Возвращает (cert_pem_path, key_pem_path, cert_der).
fn make_self_signed(
    dir: &std::path::Path,
) -> (PathBuf, PathBuf, rustls::pki_types::CertificateDer<'static>) {
    let ck = rcgen::generate_simple_self_signed(vec!["localhost".into()])
        .expect("rcgen: generate_simple_self_signed");
    let cert_pem = ck.cert.pem();
    let key_pem = ck.key_pair.serialize_pem();

    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    fs::write(&cert_path, cert_pem.as_bytes()).unwrap();
    fs::write(&key_path, key_pem.as_bytes()).unwrap();

    let der = ck.cert.der().clone();
    (cert_path, key_path, der)
}

/// Клиент: TLS GET /healthz. Возвращает status.
fn tls_get_healthz(
    port: u16,
    root_cert_der: rustls::pki_types::CertificateDer<'static>,
) -> std::io::Result<u16> {
    let mut root_store = rustls::RootCertStore::empty();
    root_store
        .add(root_cert_der)
        .map_err(|e| std::io::Error::other(format!("add root: {e}")))?;
    let cfg = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();

    let server_name = rustls::pki_types::ServerName::try_from("localhost")
        .map_err(|e| std::io::Error::other(format!("ServerName: {e}")))?
        .to_owned();

    let conn = rustls::ClientConnection::new(Arc::new(cfg), server_name)
        .map_err(|e| std::io::Error::other(format!("client conn: {e}")))?;
    let sock = TcpStream::connect(("127.0.0.1", port))?;
    let mut tls = rustls::StreamOwned::new(conn, sock);

    tls.write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
    tls.flush()?;

    // Толерантное чтение: TLS-сервер может не прислать close_notify
    // (зависит от версии rustls и настроек). Читаем до EOF или до
    // UnexpectedEof, накопленное — разбираем.
    let mut resp_bytes = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match tls.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => resp_bytes.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e),
        }
    }
    let resp = String::from_utf8_lossy(&resp_bytes);

    let status = resp
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    Ok(status)
}

/// Запустить server в фоне. Возвращает (port, handle).
fn spawn_server(root_path: &std::path::Path) -> (u16, std::thread::JoinHandle<()>) {
    // bind на :0, узнать порт, запустить serve через fork-поток.
    // `repo::server::serve` берёт listen из config.toml — поэтому
    // модифицируем config перед spawn.
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);

    // перезапишем config.toml
    let mut cfg: layout::Config =
        toml::from_str(&fs::read_to_string(root_path.join("config.toml")).unwrap()).unwrap();
    cfg.server.listen = format!("127.0.0.1:{}", port);
    fs::write(
        root_path.join("config.toml"),
        toml::to_string_pretty(&cfg).unwrap(),
    )
    .unwrap();

    let root_path = root_path.to_path_buf();
    let handle = std::thread::spawn(move || {
        let root = layout::RepoRoot::open(&root_path).unwrap();
        let _ = server::serve(&root);
    });

    // ждём готовности
    for _ in 0..50 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    (port, handle)
}

// ============================================================================
// tests
// ============================================================================

#[test]
fn tls_load_config_ok() {
    let tmp = TempDir::new().unwrap();
    let (cert, key, _der) = make_self_signed(tmp.path());

    let cfg = tls::load_server_config(cert.to_str().unwrap(), key.to_str().unwrap())
        .expect("load_server_config");
    // Проверяем только успешность; детали — в rustls.
    let _: Arc<rustls::ServerConfig> = cfg;
}

#[test]
fn tls_load_config_bad_pem() {
    let tmp = TempDir::new().unwrap();
    let cert = tmp.path().join("cert.pem");
    let key = tmp.path().join("key.pem");
    fs::write(&cert, b"not a pem\n").unwrap();
    fs::write(&key, b"not a pem\n").unwrap();
    assert!(tls::load_server_config(cert.to_str().unwrap(), key.to_str().unwrap()).is_err());
}

#[test]
fn tls_resolve_rejects_half_config() {
    let cert = Some("cert.pem".to_string());
    let key = None::<String>;
    assert!(tls::resolve(&cert, &key).is_err());
}

#[test]
fn tls_resolve_accepts_full_or_empty() {
    assert!(tls::resolve(&None, &None).unwrap().is_none());
    let c = Some("c.pem".to_string());
    let k = Some("k.pem".to_string());
    assert!(tls::resolve(&c, &k).unwrap().is_some());
}

#[test]
fn tls_handshake_and_healthz() {
    let (_tmp, root) = make_repo();
    let (_cert_pem, _key_pem, cert_der) = make_self_signed(_tmp.path());

    // прописываем пути в config
    let mut cfg: layout::Config =
        toml::from_str(&fs::read_to_string(root.join("config.toml")).unwrap()).unwrap();
    cfg.server.tls.cert = Some(_cert_pem.to_str().unwrap().to_string());
    cfg.server.tls.key = Some(_key_pem.to_str().unwrap().to_string());
    fs::write(
        root.join("config.toml"),
        toml::to_string_pretty(&cfg).unwrap(),
    )
    .unwrap();

    let (port, _h) = spawn_server(&root);
    let status = tls_get_healthz(port, cert_der).expect("tls_get_healthz");
    assert_eq!(status, 200, "expected 200 from /healthz over TLS");
}

#[test]
fn plain_http_still_works_without_tls() {
    let (_tmp, root) = make_repo();
    let (port, _h) = spawn_server(&root);

    // plain HTTP
    let mut sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
    sock.write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut resp = String::new();
    sock.read_to_string(&mut resp).unwrap();
    let status: u16 = resp
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    assert_eq!(status, 200, "plain HTTP regression: {resp:?}");
}
