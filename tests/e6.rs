//! E6 conformance tests: IPFS через мок `ipfs` CLI.
//!
//! Env-переменные (`APPSPEC_IPFS`, `MOCK_IPFS_*`) — process-global.
//! Тесты, мутирующие env, сериализуются через `ENV_LOCK`.

#![allow(dead_code)]

#[path = "../src/repo/mod.rs"]
mod repo;

pub fn log(_msg: impl AsRef<str>) {}

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use tempfile::TempDir;

use repo::{ipfs, layout, meta, pull, util};

// Сериализация env-мутирующих тестов.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

// ============================================================================
// Мок `ipfs`
// ============================================================================

struct IpfsMock {
    _tmp: TempDir,
    bin: PathBuf,
}

impl IpfsMock {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let bin = tmp.path().join("ipfs");
        let script = r#"#!/bin/sh
set -eu
cmd="$1"; shift
case "$cmd" in
  add)
    if [ -n "${MOCK_IPFS_ADD_CID:-}" ]; then echo "$MOCK_IPFS_ADD_CID"; else echo "QmFakeAddCID"; fi
    ;;
  cat)
    if [ -n "${MOCK_IPFS_CAT_FILE:-}" ] && [ -f "$MOCK_IPFS_CAT_FILE" ]; then
      cat "$MOCK_IPFS_CAT_FILE"
    else
      echo "mock ipfs: no MOCK_IPFS_CAT_FILE" >&2
      exit 1
    fi
    ;;
  pin)
    exit 0
    ;;
  *)
    echo "mock ipfs: unknown cmd $cmd" >&2
    exit 1
    ;;
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
        IpfsMock { _tmp: tmp, bin }
    }

    fn set_env(&self, add_cid: Option<&str>, cat_file: Option<&std::path::Path>) {
        std::env::set_var("APPSPEC_IPFS", &self.bin);
        std::env::set_var("MOCK_IPFS_ADD_CID", add_cid.unwrap_or(""));
        std::env::set_var(
            "MOCK_IPFS_CAT_FILE",
            cat_file
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
        );
    }
}

// ============================================================================
// helpers
// ============================================================================

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

fn make_min_repo() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    fs::create_dir_all(root.join("blobs/sha256")).unwrap();
    fs::create_dir_all(root.join("packages")).unwrap();
    fs::create_dir_all(root.join("refs")).unwrap();
    fs::create_dir_all(root.join("index")).unwrap();
    fs::write(root.join("index/.keep"), b"").unwrap();

    let cfg = layout::Config {
        repo: layout::RepoMeta {
            name: "test/e6".into(),
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

    let idx = meta::IndexV1 {
        schema: 1,
        repo: "test/e6".into(),
        updated: util::now_iso8601(),
        generator: "appspec test".into(),
        signature: String::new(),
        packages: BTreeMap::new(),
    };
    fs::write(
        root.join("index.json"),
        serde_json::to_vec_pretty(&idx).unwrap(),
    )
    .unwrap();

    (tmp, root)
}

fn make_index_with_ipfs_cid(
    pkg: &str,
    ver: &str,
    sif_sha: &str,
    cid: Option<&str>,
) -> meta::IndexV1 {
    let mut versions = BTreeMap::new();
    versions.insert(
        ver.to_string(),
        meta::IndexVersion {
            epoch: 0,
            version: ver.split('-').next().unwrap().to_string(),
            release: ver.split('-').nth(1).unwrap_or("1").to_string(),
            arch: "amd64".into(),
            sif_sha256: sif_sha.to_string(),
            sif_size: 10,
            spec_sha256: sha256_hex(b"spec"),
            spec_size: 4,
            built_at: "2026-10-28T00:00:00Z".into(),
            generator: "appspec test".into(),
            base_digest: None,
            tags: vec![],
            requires: vec![],
            provides: vec![],
            ipfs_cid: cid.map(str::to_string),
        },
    );
    let mut packages = BTreeMap::new();
    packages.insert(
        pkg.to_string(),
        meta::IndexPackage {
            summary: None,
            license: None,
            vendor: None,
            url: None,
            versions,
        },
    );
    meta::IndexV1 {
        schema: 1,
        repo: "test/e6".into(),
        updated: "2026-10-28T00:00:00Z".into(),
        generator: "appspec test".into(),
        signature: String::new(),
        packages,
    }
}

fn default_opts() -> pull::PullOpts {
    pull::PullOpts {
        from: None,
        via: None,
        spec_only: false,
        sif_out: None,
        trusted_key: None,
        allow_unsigned: true,
    }
}

// ============================================================================
// Тесты: ipfs cat / add через мок
// ============================================================================

#[test]
fn ipfs_add_returns_cid_from_mock() {
    let _g = lock_env();
    let mock = IpfsMock::new();
    let tmp = TempDir::new().unwrap();
    let f = tmp.path().join("blob.bin");
    fs::write(&f, b"hello").unwrap();
    mock.set_env(Some("QmTestAdd123"), Some(&f));

    let cid = ipfs::add_file(&f, true).unwrap();
    assert_eq!(cid, "QmTestAdd123");
}

#[test]
fn ipfs_add_without_pin_flag_via_mock() {
    let _g = lock_env();
    let mock = IpfsMock::new();
    let tmp = TempDir::new().unwrap();
    let f = tmp.path().join("x.bin");
    fs::write(&f, b"x").unwrap();
    mock.set_env(Some("QmNoPin"), Some(&f));

    let cid = ipfs::add_file(&f, false).unwrap();
    assert_eq!(cid, "QmNoPin");
}

#[test]
fn ipfs_cat_returns_file_contents() {
    let _g = lock_env();
    let mock = IpfsMock::new();
    let tmp = TempDir::new().unwrap();
    let cat_file = tmp.path().join("payload.bin");
    fs::write(&cat_file, b"payload-bytes").unwrap();
    mock.set_env(None, Some(&cat_file));

    let out = ipfs::cat("QmAnyCID").unwrap();
    assert_eq!(out, b"payload-bytes");
}

// ============================================================================
// Тесты: publish_index (через ipfs::add_file)
// ============================================================================

#[test]
fn publish_index_writes_cid_file() {
    let _g = lock_env();
    let mock = IpfsMock::new();
    let (_tmp, root) = make_min_repo();

    let idx_path = root.join("index.json");
    mock.set_env(Some("QmIndexABC"), Some(&idx_path));

    let cid = ipfs::add_file(&idx_path, true).unwrap();
    assert_eq!(cid, "QmIndexABC");

    let cid_file = root.join("ipfs-index.cid");
    fs::write(&cid_file, format!("{}\n", cid)).unwrap();
    assert!(cid_file.is_file());
    assert_eq!(fs::read_to_string(&cid_file).unwrap().trim(), "QmIndexABC");
}

// ============================================================================
// Тесты: pull --via ipfs
// ============================================================================

#[test]
fn pull_via_ipfs_uses_local_index() {
    let _g = lock_env();
    let mock = IpfsMock::new();
    let (_tmp, root) = make_min_repo();

    let blob_bytes = b"fake-sif-from-ipfs";
    let sha = sha256_hex(blob_bytes);
    let index = make_index_with_ipfs_cid("demo", "0.0.1-1", &sha, Some("QmBlobCID"));
    fs::write(
        root.join("index.json"),
        serde_json::to_vec_pretty(&index).unwrap(),
    )
    .unwrap();

    let cat_file = mock._tmp.path().join("blob.bin");
    fs::write(&cat_file, blob_bytes).unwrap();
    mock.set_env(None, Some(&cat_file));

    let root_obj = layout::RepoRoot::open(&root).unwrap();
    let mut opts = default_opts();
    opts.via = Some("ipfs".into());
    pull::pull(&root_obj, "demo:0.0.1-1", opts).unwrap();

    let blob_path = util::blob_path(&root, &sha);
    assert!(blob_path.is_file(), "blob must be written");
    assert_eq!(fs::read(&blob_path).unwrap(), blob_bytes);
}

#[test]
fn pull_via_ipfs_no_cid_errors() {
    let _g = lock_env();
    let mock = IpfsMock::new();
    let (_tmp, root) = make_min_repo();
    let index = make_index_with_ipfs_cid("demo", "0.0.1-1", "deadbeef", None);
    fs::write(
        root.join("index.json"),
        serde_json::to_vec_pretty(&index).unwrap(),
    )
    .unwrap();
    mock.set_env(None, None);

    let root_obj = layout::RepoRoot::open(&root).unwrap();
    let mut opts = default_opts();
    opts.via = Some("ipfs".into());
    let err = pull::pull(&root_obj, "demo:0.0.1-1", opts).unwrap_err();
    assert!(
        format!("{err:#}").contains("no ipfs_cid"),
        "expected no-ipfs-cid, got: {err:#}"
    );
}

// ============================================================================
// Тесты: pull --from ipfs://
// ============================================================================

#[test]
fn pull_from_ipfs_index_with_invalid_json_fails_parsing() {
    let _g = lock_env();
    let mock = IpfsMock::new();
    let (_tmp, root) = make_min_repo();

    let cat_file = mock._tmp.path().join("bad.json");
    fs::write(&cat_file, b"not-json").unwrap();
    mock.set_env(None, Some(&cat_file));

    let root_obj = layout::RepoRoot::open(&root).unwrap();
    let mut opts = default_opts();
    opts.from = Some("ipfs://QmIndexCID".into());
    let err = pull::pull(&root_obj, "demo:0.0.1-1", opts).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("parsing"), "expected parse error, got: {msg}");
}

#[test]
fn pull_from_and_via_mutually_exclusive() {
    let _g = lock_env();
    let mock = IpfsMock::new();
    let (_tmp, root) = make_min_repo();
    mock.set_env(None, None);

    let root_obj = layout::RepoRoot::open(&root).unwrap();
    let mut opts = default_opts();
    opts.from = Some("ipfs://QmX".into());
    opts.via = Some("ipfs".into());
    let err = pull::pull(&root_obj, "demo:0.0.1-1", opts).unwrap_err();
    assert!(
        format!("{err:#}").contains("mutually exclusive"),
        "expected mutual-exclusion error, got: {err:#}"
    );
}
