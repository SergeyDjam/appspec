//! ed25519 подписи `index.json` + JCS-канонизация (RFC 8785, минимум).
//!
//! Формат подписи: `"signature": "ed25519:<hex(64B)>"`. Пустая строка —
//! не подписано (§5.5/§10.1 ТЗ). Канонизируется копия `index.json` без
//! поля `signature`.
//!
//! Приватный ключ: `~/.config/appspec/signing.key`, 0600, формат
//! `ed25519:<hex(32B seed)>`.
//!
//! Trusted keys: `~/.config/appspec/trusted-keys.toml` —
//! `[[keys]] name / pubkey` (§10.1/§10.2).

use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};

// Публично реэкспортируем типы ключей: `sign::SigningKey`, `sign::VerifyingKey`
// доступны и для `commands.rs`, и для тестов.
pub use ed25519_dalek::{SigningKey, VerifyingKey};
use ed25519_dalek::{Signature, Signer, Verifier};

pub const SIG_PREFIX: &str = "ed25519:";
pub type TrustedKey = (String, VerifyingKey);

// ===========================================================================
// JCS (RFC 8785), минимум
// ===========================================================================

pub fn jcs_canonicalize(v: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    jcs_write(v, &mut out);
    out
}

fn jcs_write(v: &Value, out: &mut Vec<u8>) {
    match v {
        Value::Null        => out.extend_from_slice(b"null"),
        Value::Bool(true)  => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.extend_from_slice(i.to_string().as_bytes());
            } else if let Some(u) = n.as_u64() {
                out.extend_from_slice(u.to_string().as_bytes());
            } else if let Some(f) = n.as_f64() {
                out.extend_from_slice(format_ecma_number(f).as_bytes());
            }
        }
        Value::String(s) => jcs_write_string(s, out),
        Value::Array(arr) => {
            out.push(b'[');
            for (i, x) in arr.iter().enumerate() {
                if i > 0 { out.push(b','); }
                jcs_write(x, out);
            }
            out.push(b']');
        }
        Value::Object(map) => {
            out.push(b'{');
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            for (i, k) in keys.iter().enumerate() {
                if i > 0 { out.push(b','); }
                jcs_write_string(k, out);
                out.push(b':');
                jcs_write(&map[*k], out);
            }
            out.push(b'}');
        }
    }
}

fn jcs_write_string(s: &str, out: &mut Vec<u8>) {
    out.push(b'"');
    for c in s.chars() {
        match c {
            '"'   => out.extend_from_slice(b"\\\""),
            '\\'  => out.extend_from_slice(b"\\\\"),
            '\x08'=> out.extend_from_slice(b"\\b"),
            '\x0c'=> out.extend_from_slice(b"\\f"),
            '\n'  => out.extend_from_slice(b"\\n"),
            '\r'  => out.extend_from_slice(b"\\r"),
            '\t'  => out.extend_from_slice(b"\\t"),
            c if (c as u32) < 0x20 => {
                out.extend_from_slice(format!("\\u{:04x}", c as u32).as_bytes());
            }
            c => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    out.push(b'"');
}

fn format_ecma_number(f: f64) -> String {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e21 {
        format!("{}", f as i64)
    } else {
        format!("{}", f)
    }
}

/// Каноническая форма `index.json` без поля `signature`.
pub fn canonical_index_bytes(index_text: &str) -> Result<Vec<u8>> {
    let mut v: Value = serde_json::from_str(index_text)
        .context("index.json is not valid JSON")?;
    if let Some(obj) = v.as_object_mut() {
        obj.remove("signature");
    }
    Ok(jcs_canonicalize(&v))
}

// ===========================================================================
// Ключи
// ===========================================================================

pub fn keygen() -> SigningKey {
    let mut rng = rand_core::OsRng;
    SigningKey::generate(&mut rng)
}

pub fn pubkey_to_string(key: &VerifyingKey) -> String {
    format!("{}{}", SIG_PREFIX, hex_encode(key.as_bytes()))
}

pub fn parse_pubkey(s: &str) -> Result<VerifyingKey> {
    let hex = s.trim().strip_prefix(SIG_PREFIX).unwrap_or(s.trim());
    let bytes = hex_decode(hex)?;
    if bytes.len() != 32 {
        bail!("public key must be 32 bytes, got {}", bytes.len());
    }
    let arr: [u8; 32] = bytes.as_slice().try_into().unwrap();
    VerifyingKey::from_bytes(&arr).map_err(|e| anyhow::anyhow!("invalid public key: {e}"))
}

pub fn load_key(path: &Path) -> Result<SigningKey> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading signing key {}", path.display()))?;
    let hex = text.trim().strip_prefix(SIG_PREFIX).unwrap_or(text.trim());
    let bytes = hex_decode(hex)?;
    if bytes.len() != 32 {
        bail!("signing key must be 32 bytes, got {}", bytes.len());
    }
    Ok(SigningKey::from_bytes(bytes.as_slice().try_into().unwrap()))
}

pub fn save_key(path: &Path, key: &SigningKey) -> Result<()> {
    if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
    let text = format!("{}{}\n", SIG_PREFIX, hex_encode(&key.to_bytes()));
    std::fs::write(path, text)
        .with_context(|| format!("writing signing key {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut m = std::fs::metadata(path)?.permissions();
        m.set_mode(0o600);
        std::fs::set_permissions(path, m)?;
    }
    Ok(())
}

pub fn default_key_path() -> PathBuf {
    if let Ok(v) = std::env::var("APPSPEC_SIGNING_KEY") {
        if !v.is_empty() { return PathBuf::from(v); }
    }
    if let Ok(v) = std::env::var("XDG_CONFIG_HOME") {
        if !v.is_empty() { return PathBuf::from(v).join("appspec/signing.key"); }
    }
    if let Ok(h) = std::env::var("HOME") {
        if !h.is_empty() { return PathBuf::from(h).join(".config/appspec/signing.key"); }
    }
    PathBuf::from("/tmp/appspec-signing.key")
}

pub fn default_trusted_keys_path() -> PathBuf {
    if let Ok(v) = std::env::var("XDG_CONFIG_HOME") {
        if !v.is_empty() { return PathBuf::from(v).join("appspec/trusted-keys.toml"); }
    }
    if let Ok(h) = std::env::var("HOME") {
        if !h.is_empty() { return PathBuf::from(h).join(".config/appspec/trusted-keys.toml"); }
    }
    PathBuf::from("/tmp/appspec-trusted-keys.toml")
}

pub fn load_trusted_keys(extra: Option<&Path>) -> Result<Vec<TrustedKey>> {
    let mut out: Vec<TrustedKey> = Vec::new();

    let path = default_trusted_keys_path();
    if path.is_file() {
        out.extend(parse_trusted_file(&path)?);
    }

    if let Some(p) = extra {
        if !p.is_file() {
            bail!("--trusted-key not found: {}", p.display());
        }
        let text = std::fs::read_to_string(p)?;
        match toml::from_str::<toml::Value>(&text) {
            Ok(v) if v.get("keys").is_some() => {
                out.extend(parse_trusted_toml(&v, &p.display().to_string())?);
            }
            _ => out.push((format!("cli:{}", p.display()), parse_pubkey(text.trim())?)),
        }
    }

    Ok(out)
}

fn parse_trusted_file(path: &Path) -> Result<Vec<TrustedKey>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let v: toml::Value = toml::from_str(&text)
        .with_context(|| format!("parsing {}", path.display()))?;
    parse_trusted_toml(&v, &path.display().to_string())
}

fn parse_trusted_toml(v: &toml::Value, origin: &str) -> Result<Vec<TrustedKey>> {
    let mut out = Vec::new();
    if let Some(arr) = v.get("keys").and_then(|x| x.as_array()) {
        for k in arr {
            let name = k.get("name").and_then(|x| x.as_str()).unwrap_or("?").to_string();
            let pk = k.get("pubkey").and_then(|x| x.as_str())
                .ok_or_else(|| anyhow::anyhow!("{}: key entry without `pubkey`", origin))?;
            out.push((name, parse_pubkey(pk)?));
        }
    }
    Ok(out)
}

// ===========================================================================
// Sign / verify index.json
// ===========================================================================

pub fn sign_index(index_text: &str, key: &SigningKey) -> Result<String> {
    let canon = canonical_index_bytes(index_text)?;
    let sig = key.sign(&canon);
    Ok(format!("{}{}", SIG_PREFIX, hex_encode(&sig.to_bytes())))
}

pub fn verify_index(index_text: &str, pubkey: &VerifyingKey) -> Result<()> {
    let v: Value = serde_json::from_str(index_text)
        .context("index.json is not valid JSON")?;
    let sig_str = v.get("signature").and_then(|s| s.as_str()).unwrap_or("");
    if sig_str.is_empty() {
        bail!("index.json is not signed");
    }
    let hex_sig = sig_str.strip_prefix(SIG_PREFIX)
        .ok_or_else(|| anyhow::anyhow!(
            "unsupported signature algorithm: expected prefix `{}`", SIG_PREFIX
        ))?;
    let bytes = hex_decode(hex_sig)?;
    if bytes.len() != 64 {
        bail!("signature must be 64 bytes, got {}", bytes.len());
    }
    let sig_arr: [u8; 64] = bytes.as_slice().try_into().unwrap();
    let sig = Signature::from_bytes(&sig_arr);
    let canon = canonical_index_bytes(index_text)?;
    pubkey.verify(&canon, &sig)
        .map_err(|e| anyhow::anyhow!("signature mismatch: {e}"))
}

pub fn verify_remote_index(
    index_text: &str,
    trusted: &[TrustedKey],
    allow_unsigned: bool,
) -> Result<()> {
    if trusted.is_empty() {
        eprintln!("warning: no trusted keys configured; index signature not verified");
        return Ok(());
    }

    let v: Value = serde_json::from_str(index_text)
        .context("remote index.json is not valid JSON")?;
    let sig_str = v.get("signature").and_then(|s| s.as_str()).unwrap_or("");
    if sig_str.is_empty() {
        if allow_unsigned {
            eprintln!("warning: remote index.json is not signed; --allow-unsigned accepts it");
            return Ok(());
        }
        bail!(
            "signature required: remote index.json is not signed \
             (add --allow-unsigned to override, or remove trusted keys from allowlist)"
        );
    }

    for (name, key) in trusted {
        if verify_index(index_text, key).is_ok() {
            crate::log(format!("repo: index signed by `{}` — OK", name));
            return Ok(());
        }
    }
    bail!(
        "signature mismatch: no trusted key verifies the remote index.json \
         ({} trusted key(s))", trusted.len()
    );
}

// ===========================================================================
// hex
// ===========================================================================

pub fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes { s.push_str(&format!("{:02x}", b)); }
    s
}

fn hex_decode(s: &str) -> Result<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        bail!("hex string must have even length");
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(s.len() / 2);
    let mut i = 0;
    while i < bytes.len() {
        let hi = hex_val(bytes[i])?;
        let lo = hex_val(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

fn hex_val(b: u8) -> Result<u8> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => bail!("invalid hex character: {}", b as char),
    }
}
