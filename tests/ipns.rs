//! Block A+B: IPNS publish / resolve / pull.
//!
//! Мокает `ipfs` CLI через `APPSPEC_IPFS`. Env-мутации сериализуются
//! через `ENV_LOCK` (env process-global).

#![allow(dead_code)]
#[path = "../src/repo/mod.rs"]
mod repo;
pub fn log(_msg: impl AsRef<str>) {}

use std::collections::BTreeMap;

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
    Command::cargo_bin("appspec").expect("appspec binary built")
}

// ============================================================================
// mock ipfs
// ============================================================================

struct IpfsMock {
    _tmp: TempDir,
    bin: PathBuf,
    log: PathBuf,
}

impl IpfsMock {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let bin = tmp.path().join("ipfs");
        let log = tmp.path().join("ipfs.log");

        // Поведение:
        //   add ...                     → echo $MOCK_IPFS_CID
        //   name publish --key X /ipfs/Y → echo "Published to $MOCK_IPNS_NAME: /ipfs/$MOCK_IPFS_CID"
        //   name resolve X               → echo "/ipfs/$MOCK_IPFS_CID"
        // Логирует каждый вызов: "<args joined by space>" в $MOCK_IPFS_LOG.
        let script = r#"#!/bin/sh
set -eu
log="${MOCK_IPFS_LOG:-/dev/null}"
printf '%s\n' "$*" >> "$log"

fail() { echo "mock ipfs: forced failure" >&2; exit 1; }

case "$1" in
  add)
    [ -n "${MOCK_IPFS_FAIL:-}" ] && fail
    echo "${MOCK_IPFS_CID:-bafkreitestdefault}"
    ;;
  cat)
    [ -n "${MOCK_IPFS_FAIL:-}" ] && fail
    if [ -n "${MOCK_IPFS_CAT_DIR:-}" ] && [ -f "$MOCK_IPFS_CAT_DIR/$2" ]; then
      cat "$MOCK_IPFS_CAT_DIR/$2"; exit 0
    fi
    if [ -n "${MOCK_IPFS_CAT_FILE:-}" ] && [ -f "$MOCK_IPFS_CAT_FILE" ]; then
      cat "$MOCK_IPFS_CAT_FILE"; exit 0
    fi
    echo "mock ipfs: no content for CID $2" >&2; exit 1
    ;;
  name)
    [ -n "${MOCK_IPFS_FAIL:-}" ] && fail
    case "$2" in
      publish) echo "Published to ${MOCK_IPNS_NAME:-k51testmock}: /ipfs/${MOCK_IPFS_CID:-bafkreitestdefault}" ;;
      resolve) echo "/ipfs/${MOCK_IPFS_CID:-bafkreitestdefault}" ;;
      *) echo "mock ipfs: unknown name subcommand $2" >&2; exit 1 ;;
    esac
    ;;
  *) echo "mock ipfs: unexpected $*" >&2; exit 1 ;;
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
        }
    }

    fn set_env(&self) {
        std::env::set_var("APPSPEC_IPFS", &self.bin);
        std::env::set_var("MOCK_IPFS_LOG", &self.log);
    }

    fn read_log(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }
}

// ============================================================================
// helpers
// ============================================================================

fn make_repo(root: &Path) {
    fs::create_dir_all(root.join("blobs/sha256")).unwrap();
    fs::create_dir_all(root.join("packages")).unwrap();
    fs::create_dir_all(root.join("refs")).unwrap();
    fs::create_dir_all(root.join("index")).unwrap();
    fs::write(root.join("index/.keep"), b"").unwrap();

    let cfg = serde_json::json!({
        "schema": 1,
        "repo": "test/ipns",
        "updated": "2026-11-11T00:00:00Z",
        "generator": "appspec test",
        "signature": "",
        "packages": {}
    });
    fs::write(
        root.join("index.json"),
        serde_json::to_vec_pretty(&cfg).unwrap(),
    )
    .unwrap();
}

fn write_server_config(path: &Path, root: &Path, ipns_key: Option<&str>) {
    let mut ipfs = serde_json::json!({});
    if let Some(k) = ipns_key {
        ipfs["ipns_key"] = serde_json::Value::String(k.into());
    }
    let cfg = serde_json::json!({
        "schema": 1,
        "repo": {
            "root": root.display().to_string(),
            "name": "test/ipns",
        },
        "ipfs": ipfs,
    });
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    // ServerConfig — TOML. Пишем через toml.
    let toml_text = toml::to_string_pretty(&cfg).unwrap();
    fs::write(path, toml_text).unwrap();
}

// ============================================================================
// Tests
// ============================================================================

#[test]
fn publish_with_ipns_updates_server_config() {
    let _g = lock_env();
    let mock = IpfsMock::new();
    mock.set_env();
    std::env::set_var("MOCK_IPFS_CID", "bafkreitestidx");
    std::env::set_var("MOCK_IPNS_NAME", "k51testsuccess");

    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    let sc = tmp.path().join("repo-server.toml");
    make_repo(&root);
    write_server_config(&sc, &root, Some("appspec-test"));

    let out = appspec()
        .args([
            "repo",
            "publish",
            "--config",
            sc.to_str().unwrap(),
            "--ipns",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("bafkreitestidx"))
        .stderr(predicate::str::contains("published: k51testsuccess"))
        .get_output()
        .clone();

    let _ = out; // просто assert прошёл

    // ipns_name сохранён в server config.
    let text = fs::read_to_string(&sc).unwrap();
    assert!(
        text.contains("k51testsuccess"),
        "ipns_name not written to config:\n{text}"
    );

    // Лог ipfs содержит name publish.
    let log = mock.read_log();
    assert!(
        log.contains("name publish --key appspec-test /ipfs/bafkreitestidx"),
        "unexpected ipfs calls:\n{log}"
    );
}

#[test]
fn publish_without_ipns_key_fails_when_forced() {
    let _g = lock_env();
    let mock = IpfsMock::new();
    mock.set_env();

    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    let sc = tmp.path().join("repo-server.toml");
    make_repo(&root);
    write_server_config(&sc, &root, None);

    appspec()
        .args([
            "repo",
            "publish",
            "--config",
            sc.to_str().unwrap(),
            "--ipns",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("requires `[ipfs].ipns_key`"));
}

#[test]
fn publish_default_no_ipns_when_key_missing() {
    let _g = lock_env();
    let mock = IpfsMock::new();
    mock.set_env();
    std::env::set_var("MOCK_IPFS_CID", "bafkreiplain");

    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    let sc = tmp.path().join("repo-server.toml");
    make_repo(&root);
    write_server_config(&sc, &root, None);

    appspec()
        .args(["repo", "publish", "--config", sc.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("bafkreiplain"))
        .stderr(predicate::str::contains("published:").not());

    let log = mock.read_log();
    assert!(
        !log.contains("name publish"),
        "name publish must not be called:\n{log}"
    );
}

#[test]
fn publish_no_ipns_flag_overrides_config_key() {
    let _g = lock_env();
    let mock = IpfsMock::new();
    mock.set_env();

    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    let sc = tmp.path().join("repo-server.toml");
    make_repo(&root);
    write_server_config(&sc, &root, Some("appspec-test"));

    appspec()
        .args([
            "repo",
            "publish",
            "--config",
            sc.to_str().unwrap(),
            "--no-ipns",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("published:").not());

    let log = mock.read_log();
    assert!(
        !log.contains("name publish"),
        "--no-ipns must suppress name publish:\n{log}"
    );
}

#[test]
fn publish_ipns_and_no_ipns_conflict() {
    let _g = lock_env();
    let mock = IpfsMock::new();
    mock.set_env();

    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    let sc = tmp.path().join("repo-server.toml");
    make_repo(&root);
    write_server_config(&sc, &root, None);

    appspec()
        .args([
            "repo",
            "publish",
            "--config",
            sc.to_str().unwrap(),
            "--ipns",
            "--no-ipns",
        ])
        .assert()
        .failure();
}

#[test]
fn resolve_prints_cid() {
    let _g = lock_env();
    let mock = IpfsMock::new();
    mock.set_env();
    std::env::set_var("MOCK_IPFS_CID", "bafkreiresolved");

    appspec()
        .args(["repo", "resolve", "k51anything"])
        .assert()
        .success()
        .stdout(predicate::str::contains("bafkreiresolved"));

    let log = mock.read_log();
    assert!(
        log.contains("name resolve k51anything"),
        "unexpected ipfs calls:\n{log}"
    );
}

#[test]
fn resolve_fails_on_ipfs_error() {
    let _g = lock_env();
    // APPSPEC_IPFS → несуществующий файл.
    std::env::set_var("APPSPEC_IPFS", "/nonexistent/ipfs-for-test");

    appspec()
        .args(["repo", "resolve", "k51anything"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("ipfs"));
}

// ============================================================================
// Block B: pull via IPNS
// ============================================================================

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

fn init_empty_sink(root: &Path) {
    fs::create_dir_all(root.join("blobs/sha256")).unwrap();
    fs::create_dir_all(root.join("packages")).unwrap();
    fs::create_dir_all(root.join("refs")).unwrap();
    fs::create_dir_all(root.join("index")).unwrap();
    fs::write(root.join("index/.keep"), b"").unwrap();
    let cfg = repo::layout::Config {
        repo: repo::layout::RepoMeta {
            name: "test/sink".into(),
            description: String::new(),
            created_at: "2026-11-11T00:00:00Z".into(),
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
    fs::write(
        root.join("index.json"),
        b"{\"schema\":1,\"repo\":\"sink\",\"updated\":\"2026-11-11T00:00:00Z\",\"generator\":\"x\",\"signature\":\"\",\"packages\":{}}",
    ).unwrap();
}

fn make_repo_with_blob(root: &Path, name: &str, ver_key: &str, blob_bytes: &[u8]) {
    fs::create_dir_all(root.join("blobs/sha256")).unwrap();
    fs::create_dir_all(root.join("packages")).unwrap();
    fs::create_dir_all(root.join("refs")).unwrap();
    fs::create_dir_all(root.join("index")).unwrap();
    fs::write(root.join("index/.keep"), b"").unwrap();

    let sha = sha256_hex(blob_bytes);
    let blob = repo::util::blob_path(root, &sha);
    fs::create_dir_all(blob.parent().unwrap()).unwrap();
    fs::write(&blob, blob_bytes).unwrap();

    let spec = b"# test spec\n";
    let spec_sha = sha256_hex(spec);
    let (ver, rel) = ver_key.split_once('-').unwrap_or((ver_key, "1"));

    let meta = repo::meta::MetaV1 {
        schema: 1,
        name: name.into(),
        version: ver.into(),
        release: rel.into(),
        epoch: 0,
        arch: "amd64".into(),
        summary: None,
        description: None,
        license: None,
        url: None,
        vendor: None,
        packager: None,
        sif_sha256: sha.clone(),
        sif_size: blob_bytes.len() as u64,
        spec_sha256: spec_sha.clone(),
        spec_size: spec.len() as u64,
        spec_filename: "spec.appspec".into(),
        generator: "appspec test".into(),
        built_at: "2026-11-11T00:00:00Z".into(),
        base_digest: None,
        requires: vec![],
        provides: vec![],
        conflicts: vec![],
        ipfs_cid: None,
        raw_labels: Default::default(),
    };
    let pd = root.join("packages").join(name).join(ver_key);
    fs::create_dir_all(&pd).unwrap();
    fs::write(pd.join("spec.appspec"), spec).unwrap();
    fs::write(
        pd.join("meta.json"),
        serde_json::to_vec_pretty(&meta).unwrap(),
    )
    .unwrap();

    let cfg = repo::layout::Config {
        repo: repo::layout::RepoMeta {
            name: "test/src".into(),
            description: String::new(),
            created_at: "2026-11-11T00:00:00Z".into(),
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

    let mut versions = BTreeMap::new();
    versions.insert(
        ver_key.to_string(),
        repo::meta::IndexVersion {
            epoch: 0,
            version: ver.into(),
            release: rel.into(),
            arch: "amd64".into(),
            sif_sha256: sha,
            sif_size: blob_bytes.len() as u64,
            spec_sha256: spec_sha,
            spec_size: spec.len() as u64,
            built_at: "2026-11-11T00:00:00Z".into(),
            generator: "appspec test".into(),
            base_digest: None,
            tags: vec![],
            requires: vec![],
            provides: vec![],
            ipfs_cid: None,
        },
    );
    let mut packages = BTreeMap::new();
    packages.insert(
        name.to_string(),
        repo::meta::IndexPackage {
            summary: None,
            license: None,
            vendor: None,
            url: None,
            versions,
        },
    );
    let idx = repo::meta::IndexV1 {
        schema: 1,
        repo: "test/src".into(),
        updated: "2026-11-11T00:00:00Z".into(),
        generator: "appspec test".into(),
        signature: String::new(),
        packages,
    };
    fs::write(
        root.join("index.json"),
        serde_json::to_vec_pretty(&idx).unwrap(),
    )
    .unwrap();
}

#[test]
fn pull_falls_back_to_path_on_ipns_failure() {
    let _g = lock_env();
    let mock = IpfsMock::new();
    mock.set_env();
    std::env::set_var("MOCK_IPFS_FAIL", "1");

    let src_tmp = TempDir::new().unwrap();
    let src_root = src_tmp.path().join("src");
    make_repo_with_blob(&src_root, "demo", "0.0.1-1", b"blob-for-fallback");

    let sink_tmp = TempDir::new().unwrap();
    let sink_root = sink_tmp.path().join("sink");
    init_empty_sink(&sink_root);
    let sink = repo::layout::RepoRoot::open(&sink_root).unwrap();

    let opts = repo::pull::PullOpts {
        sources: vec!["ipns://k51fake".to_string(), src_root.display().to_string()],
        via: None,
        spec_only: false,
        sif_out: None,
        trusted_key: None,
        allow_unsigned: true,
    };
    repo::pull::pull(&sink, "demo:0.0.1-1", opts).unwrap();
    let sha = sha256_hex(b"blob-for-fallback");
    assert!(repo::util::blob_path(&sink_root, &sha).is_file());
}

#[test]
fn pull_ipns_only_fails_with_clear_error() {
    let _g = lock_env();
    let mock = IpfsMock::new();
    mock.set_env();
    std::env::set_var("MOCK_IPFS_FAIL", "1");

    let sink_tmp = TempDir::new().unwrap();
    let sink_root = sink_tmp.path().join("sink");
    init_empty_sink(&sink_root);
    let sink = repo::layout::RepoRoot::open(&sink_root).unwrap();

    let opts = repo::pull::PullOpts {
        sources: vec!["ipns://k51fake".to_string()],
        via: None,
        spec_only: false,
        sif_out: None,
        trusted_key: None,
        allow_unsigned: true,
    };
    let err = repo::pull::pull(&sink, "demo:0.0.1-1", opts).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("ipns://k51fake"));
    assert!(msg.contains("failed"));
}

#[test]
fn pull_ipns_succeeds_with_mock() {
    let _g = lock_env();
    let mock = IpfsMock::new();
    mock.set_env();
    std::env::remove_var("MOCK_IPFS_FAIL");

    let blob_bytes = b"ipns-blob-content";
    let sha = sha256_hex(blob_bytes);
    let index_cid = "bafkreiindexcid";
    let blob_cid = "bafkreiblobcid";

    let mut versions = BTreeMap::new();
    versions.insert(
        "0.0.1-1".to_string(),
        repo::meta::IndexVersion {
            epoch: 0,
            version: "0.0.1".into(),
            release: "1".into(),
            arch: "amd64".into(),
            sif_sha256: sha.clone(),
            sif_size: blob_bytes.len() as u64,
            spec_sha256: sha256_hex(b"spec"),
            spec_size: 4,
            built_at: "2026-11-11T00:00:00Z".into(),
            generator: "test".into(),
            base_digest: None,
            tags: vec![],
            requires: vec![],
            provides: vec![],
            ipfs_cid: Some(blob_cid.into()),
        },
    );
    let mut packages = BTreeMap::new();
    packages.insert(
        "demo".to_string(),
        repo::meta::IndexPackage {
            summary: None,
            license: None,
            vendor: None,
            url: None,
            versions,
        },
    );
    let idx = repo::meta::IndexV1 {
        schema: 1,
        repo: "test".into(),
        updated: "2026-11-11T00:00:00Z".into(),
        generator: "test".into(),
        signature: String::new(),
        packages,
    };
    let index_json = serde_json::to_vec_pretty(&idx).unwrap();

    let cat_dir = mock._tmp.path().join("cat");
    fs::create_dir_all(&cat_dir).unwrap();
    fs::write(cat_dir.join(index_cid), &index_json).unwrap();
    fs::write(cat_dir.join(blob_cid), blob_bytes).unwrap();
    std::env::set_var("MOCK_IPFS_CAT_DIR", &cat_dir);
    std::env::set_var("MOCK_IPFS_CID", index_cid);

    let sink_tmp = TempDir::new().unwrap();
    let sink_root = sink_tmp.path().join("sink");
    init_empty_sink(&sink_root);
    let sink = repo::layout::RepoRoot::open(&sink_root).unwrap();

    let opts = repo::pull::PullOpts {
        sources: vec!["ipns://k51test".to_string()],
        via: None,
        spec_only: false,
        sif_out: None,
        trusted_key: None,
        allow_unsigned: true,
    };
    repo::pull::pull(&sink, "demo:0.0.1-1", opts).unwrap();
    assert!(repo::util::blob_path(&sink_root, &sha).is_file());
    let log = mock.read_log();
    assert!(log.contains("name resolve k51test"));
}
