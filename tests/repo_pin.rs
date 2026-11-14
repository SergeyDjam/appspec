//! 0.4.9: `repo pin` + read commands via ipns://ipfs://.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());
fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn appspec() -> Command {
    Command::cargo_bin("appspec").expect("appspec built")
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

struct IpfsMock {
    _tmp: TempDir,
    bin: PathBuf,
    log: PathBuf,
    cat_dir: PathBuf,
}

impl IpfsMock {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let bin = tmp.path().join("ipfs");
        let log = tmp.path().join("ipfs.log");
        let cat_dir = tmp.path().join("cat");
        fs::create_dir_all(&cat_dir).unwrap();

        let script = r#"#!/bin/sh
set -eu
log="${MOCK_IPFS_LOG:-/dev/null}"
printf '%s\n' "$*" >> "$log"
case "$1" in
  add)
    [ -n "${MOCK_IPFS_FAIL:-}" ] && { echo "forced fail" >&2; exit 1; }
    echo "${MOCK_IPFS_ADD_CID:-bafkreigh2akiscaildcqabsyg3dfr6chu3fgpregiymsck7e7aqa4s52zy}"
    ;;
  pin)
    case "$2" in
      add)
        [ -n "${MOCK_IPFS_PIN_FAIL:-}" ] && { echo "pin failed" >&2; exit 1; }
        exit 0 ;;
      ls)
        cid="$4"
        case " ${MOCK_IPFS_PINNED_CIDS:-} " in
          *" $cid "*) echo "$cid recursive"; exit 0 ;;
        esac
        echo "not pinned" >&2; exit 1 ;;
      *) echo "pin $2" >&2; exit 1 ;;
    esac
    ;;
  cat)
    [ -n "${MOCK_IPFS_FAIL:-}" ] && { echo "forced" >&2; exit 1; }
    if [ -n "${MOCK_IPFS_CAT_DIR:-}" ] && [ -f "$MOCK_IPFS_CAT_DIR/$2" ]; then
      cat "$MOCK_IPFS_CAT_DIR/$2"; exit 0
    fi
    echo "no content for $2" >&2; exit 1 ;;
  name)
    case "$2" in
      resolve) echo "/ipfs/${MOCK_IPFS_INDEX_CID:-bafkreiindexcid}" ;;
      *) echo "name $2" >&2; exit 1 ;;
    esac ;;
  *) echo "unexpected: $*" >&2; exit 1 ;;
esac
"#;
        fs::write(&bin, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut m = fs::metadata(&bin).unwrap().permissions();
            m.set_mode(0o755);
            fs::set_permissions(&bin, m).unwrap();
        }
        fs::write(&log, b"").unwrap();
        IpfsMock {
            _tmp: tmp,
            bin,
            log,
            cat_dir,
        }
    }

    fn set_env(&self) {
        std::env::set_var("APPSPEC_IPFS", &self.bin);
        std::env::set_var("MOCK_IPFS_LOG", &self.log);
        std::env::set_var("MOCK_IPFS_CAT_DIR", &self.cat_dir);
        for k in [
            "MOCK_IPFS_FAIL",
            "MOCK_IPFS_PIN_FAIL",
            "MOCK_IPFS_ADD_CID",
            "MOCK_IPFS_PINNED_CIDS",
            "MOCK_IPFS_INDEX_CID",
        ] {
            std::env::remove_var(k);
        }
    }

    fn read_log(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn place_cat(&self, cid: &str, bytes: &[u8]) {
        fs::write(self.cat_dir.join(cid), bytes).unwrap();
    }
}

fn init_repo(root: &Path) {
    fs::create_dir_all(root.join("blobs/sha256")).unwrap();
    fs::create_dir_all(root.join("packages")).unwrap();
    fs::create_dir_all(root.join("refs")).unwrap();
    fs::create_dir_all(root.join("index")).unwrap();
    fs::write(root.join("index/.keep"), b"").unwrap();
    let cfg = serde_json::json!({
        "schema": 1, "repo": "test/pin", "updated": "2026-11-14T00:00:00Z",
        "generator": "appspec test", "signature": "", "packages": {}
    });
    fs::write(
        root.join("index.json"),
        serde_json::to_vec_pretty(&cfg).unwrap(),
    )
    .unwrap();
}

fn add_pkg(root: &Path, name: &str, ver_key: &str, blob: &[u8], ipfs_cid: Option<&str>) {
    let sha = sha256_hex(blob);
    let blob_path = root.join("blobs/sha256").join(&sha[..2]).join(&sha[2..]);
    fs::create_dir_all(blob_path.parent().unwrap()).unwrap();
    fs::write(&blob_path, blob).unwrap();
    let spec = b"# spec\n";
    let spec_sha = sha256_hex(spec);
    let (ver, rel) = ver_key.split_once('-').unwrap_or((ver_key, "1"));
    let meta = serde_json::json!({
        "schema": 1, "name": name, "version": ver, "release": rel, "epoch": 0,
        "arch": "amd64", "sif_sha256": sha, "sif_size": blob.len(),
        "spec_sha256": spec_sha, "spec_size": spec.len(), "spec_filename": "spec.appspec",
        "generator": "appspec test", "built_at": "2026-11-14T00:00:00Z",
        "requires": [], "provides": [], "conflicts": [],
        "ipfs_cid": ipfs_cid, "raw_labels": {}
    });
    let pd = root.join("packages").join(name).join(ver_key);
    fs::create_dir_all(&pd).unwrap();
    fs::write(pd.join("spec.appspec"), spec).unwrap();
    fs::write(
        pd.join("meta.json"),
        serde_json::to_vec_pretty(&meta).unwrap(),
    )
    .unwrap();
}

fn write_index(root: &Path, versions: serde_json::Value) {
    let mut packages = serde_json::Map::new();
    if let Some(obj) = versions.as_object() {
        for (name, v) in obj {
            packages.insert(
                name.clone(),
                serde_json::json!({
                    "summary": format!("{} summary", name),
                    "license": "MIT",
                    "versions": v
                }),
            );
        }
    }
    let idx = serde_json::json!({
        "schema": 1, "repo": "test/pin", "updated": "2026-11-14T00:00:00Z",
        "generator": "appspec test", "signature": "",
        "packages": packages
    });
    fs::write(
        root.join("index.json"),
        serde_json::to_vec_pretty(&idx).unwrap(),
    )
    .unwrap();
}

// ============================================================================
// 1. repo pin — backfill + pin
// ============================================================================
#[test]
fn repo_pin_adds_and_pins_missing() {
    let _g = lock_env();
    let mock = IpfsMock::new();
    mock.set_env();
    std::env::set_var("MOCK_IPFS_ADD_CID", "bafkreitestadded");

    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    init_repo(&root);
    add_pkg(&root, "demo", "0.0.1-1", b"blob-content", None);
    write_index(&root, serde_json::json!({}));

    appspec()
        .args(["repo", "pin", "--root", root.to_str().unwrap()])
        .assert()
        .success();

    // meta.json должен получить ipfs_cid
    let meta: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(root.join("packages/demo/0.0.1-1/meta.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(meta["ipfs_cid"], "bafkreitestadded");

    // log должен содержать pin add
    let log = mock.read_log();
    // 0.5.1: pin::pin_all `continue`s after add_file (add --pin=true
    // already pins the block). 0.4.9 also did redundant `pin add` — accept
    // both, since intent ("blob pinned") is the same.
    assert!(
        log.contains("add --pin=true") || log.contains("pin add bafkreitestadded"),
        "expected add --pin=true (or legacy pin add) in mock log:\n{log}"
    );
}

#[test]
fn repo_pin_pins_existing_skips_pinned() {
    let _g = lock_env();
    let mock = IpfsMock::new();
    mock.set_env();
    std::env::set_var("MOCK_IPFS_PINNED_CIDS", "bafkreicid-b");

    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    init_repo(&root);
    add_pkg(&root, "a", "0.0.1-1", b"aaa", Some("bafkreicid-a"));
    add_pkg(&root, "b", "0.0.1-1", b"bbb", Some("bafkreicid-b"));
    write_index(&root, serde_json::json!({}));

    appspec()
        .args(["repo", "pin", "--root", root.to_str().unwrap()])
        .assert()
        .success();

    let log = mock.read_log();
    assert!(log.contains("pin add bafkreicid-a"), "should pin a:\n{log}");
    assert!(
        !log.contains("pin add bafkreicid-b"),
        "b already pinned:\n{log}"
    );
}

#[test]
fn repo_pin_dry_run_no_changes() {
    let _g = lock_env();
    let mock = IpfsMock::new();
    mock.set_env();

    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    init_repo(&root);
    add_pkg(&root, "demo", "0.0.1-1", b"x", None);
    write_index(&root, serde_json::json!({}));

    appspec()
        .args(["repo", "pin", "--root", root.to_str().unwrap(), "--dry-run"])
        .assert()
        .success()
        .stderr(predicate::str::contains("would add"));

    let meta: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(root.join("packages/demo/0.0.1-1/meta.json")).unwrap(),
    )
    .unwrap();
    assert!(meta["ipfs_cid"].is_null());
}

// ============================================================================
// 2. read commands via ipns://ipfs://
// ============================================================================
fn build_index_json() -> Vec<u8> {
    let idx = serde_json::json!({
        "schema": 1, "repo": "test/read", "updated": "2026-11-14T00:00:00Z",
        "generator": "appspec test", "signature": "",
        "packages": {
            "demo": {
                "summary": "demo summary", "license": "MIT",
                "versions": {
                    "0.0.1-1": {
                        "epoch": 0, "version": "0.0.1", "release": "1",
                        "arch": "amd64", "sif_sha256": "deadbeef",
                        "sif_size": 4, "spec_sha256": "cafebabe", "spec_size": 4,
                        "built_at": "2026-11-14T00:00:00Z", "generator": "appspec test",
                        "tags": [], "requires": [], "provides": []
                    }
                }
            }
        }
    });
    serde_json::to_vec_pretty(&idx).unwrap()
}

#[test]
fn repo_ls_via_ipns() {
    let _g = lock_env();
    let mock = IpfsMock::new();
    mock.set_env();
    std::env::set_var("MOCK_IPFS_INDEX_CID", "bafkreiindexone");
    mock.place_cat("bafkreiindexone", &build_index_json());

    appspec()
        .args(["repo", "ls", "--repo", "ipns://k51testname"])
        .assert()
        .success()
        .stdout(predicate::str::contains("demo"));

    let log = mock.read_log();
    assert!(log.contains("name resolve k51testname"), "log:\n{log}");
    assert!(log.contains("cat bafkreiindexone"), "log:\n{log}");
}

#[test]
fn repo_ls_via_ipfs() {
    let _g = lock_env();
    let mock = IpfsMock::new();
    mock.set_env();
    mock.place_cat("bafkreidirect", &build_index_json());

    appspec()
        .args(["repo", "ls", "--repo", "ipfs://bafkreidirect"])
        .assert()
        .success()
        .stdout(predicate::str::contains("demo"));

    let log = mock.read_log();
    assert!(log.contains("cat bafkreidirect"), "log:\n{log}");
}

#[test]
fn repo_search_via_ipns() {
    let _g = lock_env();
    let mock = IpfsMock::new();
    mock.set_env();
    std::env::set_var("MOCK_IPFS_INDEX_CID", "bafkreiidx");
    mock.place_cat("bafkreiidx", &build_index_json());

    appspec()
        .args(["repo", "search", "demo", "--repo", "ipns://k51t"])
        .assert()
        .success()
        .stdout(predicate::str::contains("demo"));
}

#[test]
fn repo_show_via_ipns_fails_with_hint() {
    appspec()
        .args(["repo", "show", "demo:0.0.1-1", "--repo", "ipns://k51t"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not supported in v1"))
        .stderr(predicate::str::contains("appspec repo pull"));
}

#[test]
fn repo_verify_via_ipns() {
    let _g = lock_env();
    let mock = IpfsMock::new();
    mock.set_env();
    std::env::set_var("MOCK_IPFS_INDEX_CID", "bafkreiidxv");

    // Unsigned index + allow_unsigned doesn't exist for verify.
    // Verify requires --pubkey; if given, signature is checked.
    // Build signed index with known key.
    use ed25519_dalek::Signer;
    let key_bytes = [42u8; 32];
    let key = ed25519_dalek::SigningKey::from_bytes(&key_bytes);
    let idx_json = build_index_json();
    let canon = super_canon(&idx_json);
    let sig = key.sign(&canon);
    let mut v: serde_json::Value = serde_json::from_slice(&idx_json).unwrap();
    v.as_object_mut().unwrap().insert(
        "signature".into(),
        serde_json::Value::String(format!("ed25519:{}", hex_encode(&sig.to_bytes()))),
    );
    let signed = serde_json::to_vec_pretty(&v).unwrap();
    mock.place_cat("bafkreiidxv", &signed);

    // Write pubkey file
    let tmp = TempDir::new().unwrap();
    let pubkey_path = tmp.path().join("pub.txt");
    let pubkey = format!("ed25519:{}", hex_encode(&key.verifying_key().to_bytes()));
    fs::write(&pubkey_path, &pubkey).unwrap();

    appspec()
        .args([
            "repo",
            "verify",
            "--repo",
            "ipns://k51t",
            "--pubkey",
            pubkey_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("verify: OK"));
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

// Minimal JCS canonicalization (test only): remove signature, BTreeMap-sort.
fn super_canon(index_json: &[u8]) -> Vec<u8> {
    let mut v: serde_json::Value = serde_json::from_slice(index_json).unwrap();
    if let Some(o) = v.as_object_mut() {
        o.remove("signature");
    }
    // Use serde_json canonical form approximation — sorted keys via BTreeMap.
    // Good enough for verify test: the production verify uses the real JCS.
    fn canon(v: &serde_json::Value) -> Vec<u8> {
        match v {
            serde_json::Value::Null => b"null".to_vec(),
            serde_json::Value::Bool(true) => b"true".to_vec(),
            serde_json::Value::Bool(false) => b"false".to_vec(),
            serde_json::Value::Number(n) => n.to_string().into_bytes(),
            serde_json::Value::String(s) => {
                let mut out = Vec::new();
                out.push(b'"');
                for c in s.chars() {
                    match c {
                        '"' => out.extend_from_slice(b"\\\""),
                        '\\' => out.extend_from_slice(b"\\\\"),
                        '\n' => out.extend_from_slice(b"\\n"),
                        '\r' => out.extend_from_slice(b"\\r"),
                        '\t' => out.extend_from_slice(b"\\t"),
                        c => {
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                        }
                    }
                }
                out.push(b'"');
                out
            }
            serde_json::Value::Array(a) => {
                let mut out = vec![b'['];
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    out.extend_from_slice(&canon(x));
                }
                out.push(b']');
                out
            }
            serde_json::Value::Object(m) => {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
                let mut out = vec![b'{'];
                for (i, k) in keys.iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    out.extend_from_slice(&canon(&serde_json::Value::String(k.to_string())));
                    out.push(b':');
                    out.extend_from_slice(&canon(&m[*k]));
                }
                out.push(b'}');
                out
            }
        }
    }
    canon(&v)
}

/// Regression: `search` and `ls <pkg>` used `&v.sif_sha256[..12]` and panicked
/// on sha shorter than 12 chars (corrupt meta.json / hand-edited).
/// Fix: `.get(..12).unwrap_or(&v.sif_sha256)`.
#[test]
fn repo_search_short_sha_does_not_panic() {
    let _g = lock_env();
    let mock = IpfsMock::new();
    mock.set_env();
    std::env::set_var("MOCK_IPFS_INDEX_CID", "bafkreishort");
    // build_index_json uses "deadbeef" (8 chars) — this triggers the old panic.
    mock.place_cat("bafkreishort", &build_index_json());

    appspec()
        .args(["repo", "search", "demo", "--repo", "ipns://k51t"])
        .assert()
        .success()
        .stdout(predicate::str::contains("demo"))
        .stdout(predicate::str::contains("deadbeef")); // full short sha shown
}

#[test]
fn repo_ls_pkg_short_sha_does_not_panic() {
    let _g = lock_env();
    let mock = IpfsMock::new();
    mock.set_env();
    std::env::set_var("MOCK_IPFS_INDEX_CID", "bafkreishort2");
    mock.place_cat("bafkreishort2", &build_index_json());

    appspec()
        .args(["repo", "ls", "demo", "--repo", "ipns://k51t"])
        .assert()
        .success()
        .stdout(predicate::str::contains("0.0.1-1"));
}
