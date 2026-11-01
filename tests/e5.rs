//! E5 conformance tests: ed25519 подписи index.json, fail-closed pull.
//!
//! Модули `src/repo/*` подключаются через `#[path = "../src/repo/mod.rs"]`.
//! `crate::log` для `sign::verify_remote_index` подменяется заглушкой.

#![allow(dead_code)]

#[path = "../src/repo/mod.rs"]
mod repo;

pub fn log(_msg: impl AsRef<str>) {}

use tempfile::TempDir;

use repo::sign;

// ============================================================================
// helpers
// ============================================================================

fn make_index(packages: serde_json::Value) -> String {
    serde_json::json!({
        "schema": 1,
        "repo": "test/e5",
        "updated": "2026-10-28T00:00:00Z",
        "generator": "appspec test",
        "signature": "",
        "packages": packages
    })
    .to_string()
}

fn sign_text(index_text: &str) -> (sign::SigningKey, String) {
    let key = sign::keygen();
    let sig = sign::sign_index(index_text, &key).unwrap();
    let mut v: serde_json::Value = serde_json::from_str(index_text).unwrap();
    v.as_object_mut()
        .unwrap()
        .insert("signature".into(), serde_json::Value::String(sig));
    (key, serde_json::to_string(&v).unwrap())
}

// ============================================================================
// ключи
// ============================================================================

#[test]
fn keygen_pubkey_roundtrip() {
    let key = sign::keygen();
    let pk = key.verifying_key();
    let s = sign::pubkey_to_string(&pk);
    assert!(s.starts_with("ed25519:"));
    let parsed = sign::parse_pubkey(&s).unwrap();
    assert_eq!(parsed.as_bytes(), pk.as_bytes());
}

#[test]
fn save_load_key_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("signing.key");
    let key = sign::keygen();
    sign::save_key(&path, &key).unwrap();
    let loaded = sign::load_key(&path).unwrap();
    assert_eq!(key.to_bytes(), loaded.to_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let m = std::fs::metadata(&path).unwrap().permissions();
        assert_eq!(m.mode() & 0o777, 0o600);
    }
}

#[test]
fn parse_pubkey_rejects_short_input() {
    assert!(sign::parse_pubkey("ed25519:abcd").is_err());
    assert!(sign::parse_pubkey("not-hex-at-all").is_err());
}

// ============================================================================
// sign / verify локально
// ============================================================================

#[test]
fn sign_then_verify_ok() {
    let index = make_index(serde_json::json!({}));
    let (key, signed) = sign_text(&index);
    sign::verify_index(&signed, &key.verifying_key()).unwrap();
}

#[test]
fn tampered_index_rejected() {
    let index = make_index(serde_json::json!({}));
    let (key, signed) = sign_text(&index);
    let mut v: serde_json::Value = serde_json::from_str(&signed).unwrap();
    v["repo"] = serde_json::Value::String("tampered".into());
    let tampered = serde_json::to_string(&v).unwrap();
    let err = sign::verify_index(&tampered, &key.verifying_key()).unwrap_err();
    assert!(
        format!("{err:#}").contains("mismatch"),
        "expected mismatch, got: {err:#}"
    );
}

#[test]
fn unsigned_index_rejected() {
    let index = make_index(serde_json::json!({}));
    let key = sign::keygen();
    let err = sign::verify_index(&index, &key.verifying_key()).unwrap_err();
    assert!(
        format!("{err:#}").contains("not signed"),
        "expected not-signed, got: {err:#}"
    );
}

#[test]
fn jcs_canonicalize_is_deterministic() {
    let a = serde_json::json!({"b": 1, "a": 2});
    let b = serde_json::json!({"a": 2, "b": 1});
    assert_eq!(sign::jcs_canonicalize(&a), sign::jcs_canonicalize(&b));
    assert_eq!(sign::jcs_canonicalize(&a), b"{\"a\":2,\"b\":1}");
}

#[test]
fn canonical_excludes_signature() {
    let index = make_index(serde_json::json!({}));
    let canon = sign::canonical_index_bytes(&index).unwrap();
    let s = String::from_utf8(canon).unwrap();
    assert!(
        !s.contains("signature"),
        "canonical form must exclude signature: {s}"
    );
}

// ============================================================================
// verify_remote_index — fail-closed (§10.2)
// ============================================================================

#[test]
fn remote_without_trusted_keys_is_warning_only() {
    let index = make_index(serde_json::json!({}));
    sign::verify_remote_index(&index, &[], false).unwrap();
}

#[test]
fn remote_unsigned_with_trusted_keys_fails() {
    let index = make_index(serde_json::json!({}));
    let key = sign::keygen();
    let trusted = vec![("test".to_string(), key.verifying_key())];
    let err = sign::verify_remote_index(&index, &trusted, false).unwrap_err();
    assert!(
        format!("{err:#}").contains("signature required"),
        "expected signature-required, got: {err:#}"
    );
}

#[test]
fn remote_unsigned_with_allow_unsigned_ok() {
    let index = make_index(serde_json::json!({}));
    let key = sign::keygen();
    let trusted = vec![("test".to_string(), key.verifying_key())];
    sign::verify_remote_index(&index, &trusted, true).unwrap();
}

#[test]
fn remote_signed_by_trusted_ok() {
    let index = make_index(serde_json::json!({}));
    let (key, signed) = sign_text(&index);
    let trusted = vec![("release".to_string(), key.verifying_key())];
    sign::verify_remote_index(&signed, &trusted, false).unwrap();
}

#[test]
fn remote_signed_by_untrusted_fails() {
    let index = make_index(serde_json::json!({}));
    let (_signer, signed) = sign_text(&index);
    let other = sign::keygen();
    let trusted = vec![("other".to_string(), other.verifying_key())];
    let err = sign::verify_remote_index(&signed, &trusted, false).unwrap_err();
    assert!(
        format!("{err:#}").contains("mismatch"),
        "expected mismatch, got: {err:#}"
    );
}

#[test]
fn remote_signed_with_tampered_body_fails() {
    let index = make_index(serde_json::json!({}));
    let (key, signed) = sign_text(&index);
    let mut v: serde_json::Value = serde_json::from_str(&signed).unwrap();
    v["repo"] = serde_json::Value::String("evil".into());
    let tampered = serde_json::to_string(&v).unwrap();
    let trusted = vec![("release".to_string(), key.verifying_key())];
    let err = sign::verify_remote_index(&tampered, &trusted, false).unwrap_err();
    assert!(format!("{err:#}").contains("mismatch"));
}

// ============================================================================
// load_trusted_keys
// ============================================================================

#[test]
fn load_trusted_keys_from_toml() {
    let tmp = TempDir::new().unwrap();
    let key = sign::keygen();
    let pk = sign::pubkey_to_string(&key.verifying_key());
    let path = tmp.path().join("trusted.toml");
    std::fs::write(
        &path,
        format!("[[keys]]\nname = \"release\"\npubkey = \"{}\"\n", pk),
    )
    .unwrap();
    let trusted = sign::load_trusted_keys(Some(&path)).unwrap();
    // allowlist может содержать и глобальные ключи из ~/.config/appspec,
    // поэтому проверяем наличие нужного, а не точное количество.
    assert!(trusted.iter().any(|(name, _)| name == "release"));
}

#[test]
fn load_trusted_keys_from_single_line() {
    let tmp = TempDir::new().unwrap();
    let key = sign::keygen();
    let pk = sign::pubkey_to_string(&key.verifying_key());
    let path = tmp.path().join("pub.txt");
    std::fs::write(&path, pk).unwrap();
    let trusted = sign::load_trusted_keys(Some(&path)).unwrap();
    assert!(trusted.iter().any(|(name, _)| name.starts_with("cli:")));
}

#[test]
fn load_trusted_keys_missing_file_errors() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("nope.toml");
    assert!(sign::load_trusted_keys(Some(&path)).is_err());
}
