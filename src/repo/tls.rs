//! TLS-конфиг для `repo serve` (E10).
//!
//! Источник — `[server.tls].cert` / `.key` в `config.toml`.
//! `client_ca` (mTLS) пока не поддерживается: требует `WebPkiClientVerifier`
//! и явного allowlist CA — отложено до v2.
//!
//! Схема:
//!   * `load_server_config(cert, key)` — PEM → `rustls::ServerConfig`.
//!   * `serve` в `server.rs` оборачивает `TcpStream` в `rustls::StreamOwned`
//!     когда конфиг задан; иначе — plain HTTP (обратная совместимость).

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::sync::Arc;

pub fn load_server_config(cert_path: &str, key_path: &str) -> Result<Arc<rustls::ServerConfig>> {
    let cert_bytes =
        std::fs::read(cert_path).with_context(|| format!("reading TLS cert {}", cert_path))?;
    let key_bytes =
        std::fs::read(key_path).with_context(|| format!("reading TLS key {}", key_path))?;

    let certs: Vec<_> = rustls_pemfile::certs(&mut cert_bytes.as_slice())
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("parsing PEM certs from {}", cert_path))?;
    if certs.is_empty() {
        bail!("no certificates in {}", cert_path);
    }

    let key = rustls_pemfile::private_key(&mut key_bytes.as_slice())
        .with_context(|| format!("parsing PEM key from {}", key_path))?
        .ok_or_else(|| anyhow::anyhow!("no private key in {}", key_path))?;

    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .with_context(|| {
            format!(
                "building rustls::ServerConfig from {} + {}",
                cert_path, key_path
            )
        })?;
    Ok(Arc::new(config))
}

/// Проверить, что заданы оба пути (cert и key) — либо ни один.
/// Возвращает `Some((cert, key))` если TLS включён, `None` — plain.
pub fn resolve(cert: &Option<String>, key: &Option<String>) -> Result<Option<(String, String)>> {
    match (cert, key) {
        (Some(c), Some(k)) if !c.is_empty() && !k.is_empty() => Ok(Some((c.clone(), k.clone()))),
        (None, None) => Ok(None),
        (Some(c), None) if c.is_empty() => Ok(None),
        (None, Some(k)) if k.is_empty() => Ok(None),
        _ => bail!("[server.tls]: нужно задать и `cert`, и `key` (или ни одного для plain HTTP)"),
    }
}

/// Для тестов: сборка `ServerConfig` прямо из DER cert/key (без PEM-файлов).
#[allow(dead_code)] // используется в tests/e10.rs
pub fn server_config_from_der(
    cert: rustls::pki_types::CertificateDer<'static>,
    key: rustls::pki_types::PrivateKeyDer<'static>,
) -> Result<Arc<rustls::ServerConfig>> {
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .map_err(|e| anyhow::anyhow!("rustls::ServerConfig: {e}"))?;
    Ok(Arc::new(config))
}

/// Проверить, что `Path` указывает на существующий файл (для preflight).
#[allow(dead_code)]
pub fn preflight_path(p: &str) -> Result<()> {
    if !Path::new(p).is_file() {
        bail!("TLS file not found: {}", p);
    }
    Ok(())
}
