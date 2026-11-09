//! E4 unit tests: server routes + client + push.
//!
//! Модули `src/repo/*` подключаются через один `#[path]` к `mod.rs`;
//! Rust сам находит `add.rs`, `http.rs` и т.д. рядом с ним.
//! Для `server.rs` (использует `crate::log`) — заглушка в корне теста.
//!
//! Плюс HTTP e2e: `search`/`ls`/`show`/`verify` против реального
//! `server::serve` с `$APPSPEC_REPO=http://127.0.0.1:<port>`.

#![allow(dead_code)]

#[path = "../src/repo/mod.rs"]
mod repo;

/// Заглушка `crate::log` для `server.rs` / `add.rs`.
pub fn log(_msg: impl AsRef<str>) {}

use assert_cmd::Command as CliCommand;
use base64::Engine as _;
use predicates::prelude::*;
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tempfile::TempDir;

use repo::{http, layout, meta, server, util};

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
    let spec = format!("[package]\nname = \"{pkg_name}\"\nversion = \"{version}\"\n").into_bytes();
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
            if stream.read_exact(&mut byte).is_err() {
                break;
            }
            buf.push(byte[0]);
            if buf.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let header_str = String::from_utf8_lossy(&buf);
        let cl: usize = header_str
            .lines()
            .find_map(|l| {
                let lower = l.to_ascii_lowercase();
                if lower.starts_with("content-length:") {
                    l.split(':').nth(1).and_then(|v| v.trim().parse().ok())
                } else {
                    None
                }
            })
            .unwrap_or(0);
        let mut body = vec![0u8; cl];
        if cl > 0 {
            let _ = stream.read_exact(&mut body);
        }
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
    )
    .into_bytes();
    out.extend_from_slice(&b);
    out
}

fn status_json(status: u16, reason: &str, v: serde_json::Value) -> Vec<u8> {
    let b = serde_json::to_vec(&v).unwrap();
    let mut out = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        status,
        reason,
        b.len()
    )
    .into_bytes();
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
    fs::write(
        tmp.path().join("config.toml"),
        toml::to_string_pretty(&cfg).unwrap(),
    )
    .unwrap();
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
// HTTP e2e: real server::serve + CLI subprocess
// ============================================================================
//
// `assert_cmd` запускает appspec как дочерний процесс — env-переменные
// (`APPSPEC_REPO`) не протекают в другие тесты и не требуют ENV_LOCK.

fn appspec() -> CliCommand {
    CliCommand::cargo_bin("appspec").expect("appspec binary built")
}

/// Репозиторий с двумя версиями `demo`: 1.0.0-1 (старее), 2.0.0-1 (новее).
/// Строим структуру напрямую (`tests/e8.rs::make_repo_with_packages`-стиль),
/// без `add` CLI и без `apptainer`: `search`/`ls`/`show`/`verify` работают
/// поверх `index.json` + `meta.json`/`spec.appspec`/blobs.
fn make_repo_with_demo_versions() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    fs::create_dir_all(root.join("blobs/sha256")).unwrap();
    fs::create_dir_all(root.join("packages")).unwrap();
    fs::create_dir_all(root.join("refs")).unwrap();
    fs::create_dir_all(root.join("index")).unwrap();
    fs::write(root.join("index/.keep"), b"").unwrap();

    let cfg = layout::Config {
        repo: layout::RepoMeta {
            name: "test/e4-http".into(),
            description: String::new(),
            created_at: util::now_iso8601(),
            generator: "appspec test".into(),
            layout: layout::LAYOUT_VERSION,
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

    let mut pkg_versions: BTreeMap<String, meta::IndexVersion> = BTreeMap::new();

    // (ver_key, ver, rel, sif_bytes)
    let versions: &[(&str, &str, &str, &[u8])] = &[
        ("1.0.0-1", "1.0.0", "1", b"sif-demo-v1"),
        ("2.0.0-1", "2.0.0", "1", b"sif-demo-v2"),
    ];

    for &(ver_key, ver, rel, sif_bytes) in versions {
        let sha = sha256_hex(sif_bytes);
        let blob = util::blob_path(&root, &sha);
        fs::create_dir_all(blob.parent().unwrap()).unwrap();
        fs::write(&blob, sif_bytes).unwrap();

        let spec_body = format!("# demo spec {}\n", ver);
        let spec_bytes = spec_body.as_bytes();
        let spec_sha = sha256_hex(spec_bytes);

        pkg_versions.insert(
            ver_key.to_string(),
            meta::IndexVersion {
                epoch: 0,
                version: ver.to_string(),
                release: rel.to_string(),
                arch: "amd64".into(),
                sif_sha256: sha.clone(),
                sif_size: sif_bytes.len() as u64,
                spec_sha256: spec_sha.clone(),
                spec_size: spec_bytes.len() as u64,
                built_at: "2026-11-09T00:00:00Z".into(),
                generator: "appspec test".into(),
                base_digest: None,
                tags: vec![],
                requires: vec![],
                provides: vec![],
                ipfs_cid: None,
            },
        );

        let pd = root.join("packages/demo").join(ver_key);
        fs::create_dir_all(&pd).unwrap();
        fs::write(pd.join("spec.appspec"), spec_bytes).unwrap();

        let meta_v1 = meta::MetaV1 {
            schema: 1,
            name: "demo".into(),
            version: ver.to_string(),
            release: rel.to_string(),
            epoch: 0,
            arch: "amd64".into(),
            summary: Some("demo package".into()),
            description: None,
            license: Some("MIT".into()),
            url: None,
            vendor: None,
            packager: None,
            sif_sha256: sha,
            sif_size: sif_bytes.len() as u64,
            spec_sha256: spec_sha,
            spec_size: spec_bytes.len() as u64,
            spec_filename: "spec.appspec".into(),
            generator: "appspec test".into(),
            built_at: "2026-11-09T00:00:00Z".into(),
            base_digest: None,
            requires: vec![],
            provides: vec![],
            conflicts: vec![],
            ipfs_cid: None,
            raw_labels: Default::default(),
        };
        fs::write(
            pd.join("meta.json"),
            serde_json::to_vec_pretty(&meta_v1).unwrap(),
        )
        .unwrap();
    }

    let mut packages = BTreeMap::new();
    packages.insert(
        "demo".to_string(),
        meta::IndexPackage {
            summary: Some("demo package".into()),
            license: Some("MIT".into()),
            vendor: None,
            url: None,
            versions: pkg_versions,
        },
    );

    let idx = meta::IndexV1 {
        schema: 1,
        repo: "test/e4-http".into(),
        updated: "2026-11-09T00:00:00Z".into(),
        generator: "appspec test".into(),
        signature: String::new(),
        packages,
    };
    fs::write(
        root.join("index.json"),
        serde_json::to_vec_pretty(&idx).unwrap(),
    )
    .unwrap();

    (tmp, root)
}

/// Запуск `server::serve` в фоне на выбранном свободном порту.
///
/// Возвращает порт. JoinHandle намеренно не возвращаем: `serve`
/// крутится в `listener.incoming()` вечно, join'а не будет.
/// Тестовый процесс умрёт — поток умрёт вместе с ним.
///
/// Та же схема, что в `tests/e10.rs::spawn_server` (там — TLS-вариант).
fn spawn_server(root_path: &Path) -> u16 {
    // 1. Узнать свободный порт
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);

    // 2. Прописать его в config.toml — serve читает listen оттуда
    let mut cfg: layout::Config =
        toml::from_str(&fs::read_to_string(root_path.join("config.toml")).unwrap()).unwrap();
    cfg.server.listen = format!("127.0.0.1:{}", port);
    fs::write(
        root_path.join("config.toml"),
        toml::to_string_pretty(&cfg).unwrap(),
    )
    .unwrap();

    // 3. Запустить serve в фоне
    let root_path = root_path.to_path_buf();
    std::thread::spawn(move || {
        let root = layout::RepoRoot::open(&root_path).unwrap();
        let _ = server::serve(&root);
    });

    // 4. Дождаться, пока порт начнёт принимать соединения.
    //    До 2 секунд (100 × 20 мс) — с большим запасом.
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return port;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("server::serve didn't start listening on 127.0.0.1:{}", port);
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
    assert!(
        resp.status == 400 || resp.status == 404,
        "expected 400/404, got {}",
        resp.status
    );
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
    let r = req(
        "POST",
        "/api/v1/push",
        &[("Authorization", "Bearer wrong")],
        b"fake".to_vec(),
    );
    assert_eq!(server::route(&r, &state).status, 401);
}

#[test]
fn server_push_with_empty_body_returns_400() {
    let (_tmp, state) = make_state();
    let r = req(
        "POST",
        "/api/v1/push",
        &[("Authorization", "Bearer secret-token")],
        Vec::new(),
    );
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
        status_json(
            201,
            "Created",
            serde_json::json!({
                "sha256": "abc123",
                "package": "demo",
                "version": "0.0.1-1",
                "already_present": false
            }),
        )
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
        status_json(
            401,
            "Unauthorized",
            serde_json::json!({"error":"unauthorized"}),
        )
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
        status_json(
            409,
            "Conflict",
            serde_json::json!({
                "error":"conflict", "reason":"demo:0.0.1-1 already exists"
            }),
        )
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
    assert_eq!(
        labels["data"]["attributes"]["labels"]["org.appspec.package.name"],
        "demo"
    );
    assert!(!spec.is_empty());
    let _ = PathBuf::from("/tmp");
}

// ============================================================================
// HTTP e2e: search / ls / show / verify против $APPSPEC_REPO = http://…
// ============================================================================

#[test]
fn search_ls_show_verify_against_http_repo() {
    let (tmp, root) = make_repo_with_demo_versions();

    // --- 1. Подписать index.json до старта сервера.
    //     `sign` требует локальный путь — передаём `--root` явно
    //     (без него `$APPSPEC_REPO=http://…` дал бы "локальная операция").
    let key_path = tmp.path().join("signing.key");
    let sign_out = appspec()
        .args([
            "repo",
            "sign",
            "--key",
            key_path.to_str().unwrap(),
            "--root",
            root.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let sign_str = String::from_utf8_lossy(&sign_out);
    let pubkey = sign_str
        .lines()
        .find_map(|l| l.strip_prefix("pubkey: "))
        .expect("no pubkey in sign output")
        .trim();
    let pub_path = tmp.path().join("pub.txt");
    fs::write(&pub_path, pubkey.as_bytes()).unwrap();

    // --- 2. Запустить сервер.
    let port = spawn_server(&root);
    let url = format!("http://127.0.0.1:{}", port);

    // --- 3. search: новые версии сверху.
    let out = appspec()
        .env("APPSPEC_REPO", &url)
        .args(["repo", "search", "demo"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8_lossy(&out);
    assert!(s.contains("demo"), "missing package name:\n{s}");
    let p_new = s.find("2.0.0-1").expect("2.0.0-1 missing in output");
    let p_old = s.find("1.0.0-1").expect("1.0.0-1 missing in output");
    assert!(
        p_new < p_old,
        "search must show newest version first, got:\n{s}"
    );

    // --- 4. ls <package>: новые версии сверху.
    let out = appspec()
        .env("APPSPEC_REPO", &url)
        .args(["repo", "ls", "demo"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8_lossy(&out);
    let p_new = s.find("2.0.0-1").expect("2.0.0-1 missing");
    let p_old = s.find("1.0.0-1").expect("1.0.0-1 missing");
    assert!(
        p_new < p_old,
        "ls <pkg> must show newest version first, got:\n{s}"
    );

    // --- 5. ls (без пакета): latest — по semver, не по BTreeMap-ключу.
    appspec()
        .env("APPSPEC_REPO", &url)
        .args(["repo", "ls"])
        .assert()
        .success()
        .stdout(predicate::str::contains("latest=2.0.0-1"))
        .stdout(predicate::str::contains("latest=1.0.0-1").not());

    // --- 6. show <pkg>: meta в человекочитаемом виде.
    appspec()
        .env("APPSPEC_REPO", &url)
        .args(["repo", "show", "demo:2.0.0-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Name:        demo"))
        .stdout(predicate::str::contains("Version:     2.0.0-1"));

    // --- 7. show --spec: байт-в-байт совпадает с локальным spec.appspec.
    let out = appspec()
        .env("APPSPEC_REPO", &url)
        .args(["repo", "show", "demo:2.0.0-1", "--spec"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let local_spec = fs::read(root.join("packages/demo/2.0.0-1/spec.appspec")).unwrap();
    assert_eq!(
        out, local_spec,
        "spec over HTTP must be byte-for-byte identical to on-disk spec"
    );

    // --- 8. show --sif: sha256 проверяется, содержимое — то же.
    let sif_out = tmp.path().join("out.sif");
    appspec()
        .env("APPSPEC_REPO", &url)
        .args(["repo", "show", "demo:2.0.0-1", "--sif"])
        .arg(&sif_out)
        .assert()
        .success();
    assert_eq!(
        fs::read(&sif_out).unwrap(),
        b"sif-demo-v2",
        "sif bytes over HTTP must match"
    );

    // --- 9. verify OK (index подписан).
    appspec()
        .env("APPSPEC_REPO", &url)
        .args(["repo", "verify", "--pubkey", pub_path.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("verify: OK"));

    // --- 10. verify без --pubkey → ошибка (удалённый не отдаёт config.toml).
    appspec()
        .env("APPSPEC_REPO", &url)
        .args(["repo", "verify"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no public key"));

    // --- 11. tamper index.json на диске → verify падает на подписи.
    //     Сервер читает index.json с диска на каждый GET (serve_file →
    //     std::fs::read), кэша нет — tamper виден сразу.
    let idx_path = root.join("index.json");
    let mut v: serde_json::Value = serde_json::from_slice(&fs::read(&idx_path).unwrap()).unwrap();
    v.as_object_mut().unwrap().insert(
        "repo".to_string(),
        serde_json::Value::String("tampered".into()),
    );
    fs::write(&idx_path, serde_json::to_vec_pretty(&v).unwrap()).unwrap();

    appspec()
        .env("APPSPEC_REPO", &url)
        .args(["repo", "verify", "--pubkey", pub_path.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("mismatch"));
}

// ============================================================================
// HTTP e2e: локальные операции отвергают $APPSPEC_REPO = URL
// ============================================================================
//
// Сервер здесь не нужен: `require_local_root` падает ДО любого сетевого
// вызова. Достаточно, чтобы в env стоял `http://…`.

#[test]
fn local_ops_reject_http_repo() {
    let url = "http://127.0.0.1:1"; // порт неважен — до сети не доходит

    let cases: &[(&str, &[&str])] = &[
        ("add", &["fake.sif"]),
        ("check", &[]),
        ("gc", &[]),
        ("reindex", &[]),
        ("sign", &[]),
    ];

    for &(cmd, extra) in cases {
        appspec()
            .env("APPSPEC_REPO", url)
            .args(["repo", cmd])
            .args(extra)
            .assert()
            .failure()
            .stderr(predicate::str::contains("локальная операция"));
    }
}
