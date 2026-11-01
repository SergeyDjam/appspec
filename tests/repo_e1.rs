//! E1 conformance tests: локальный репозиторий SIF.
//!
//! Тесты не требуют реального `apptainer`: создаётся мок-скрипт
//! `apptainer` в tempdir, который отдаёт подготовленный JSON
//! по `inspect --json --labels`. SIF — произвольные байты.
//!
//! Репозиторий автоматически инициализируется в `Fixture::new`.
//! Тесты на сам `init` используют собственный `TempDir`.

use assert_cmd::Command;
use base64::Engine as _;
use predicates::prelude::*;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

// ---- helpers ---------------------------------------------------------------

fn appspec() -> Command {
    Command::cargo_bin("appspec").expect("appspec binary built")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

/// Готовит: tempdir с `<root>/repo` (уже инициализированным), фиктивным
/// SIF, labels.json и мок-`apptainer` в `<tmp>/bin/apptainer`.
struct Fixture {
    _tmp: TempDir,
    root: PathBuf,
    sif: PathBuf,
    mock_bin: PathBuf,
    spec_text: Vec<u8>,
}

impl Fixture {
    fn new(name: &str, version: &str, arch: &str) -> Self {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("repo");
        let sif = tmp.path().join("fake.sif");
        let labels_json = tmp.path().join("labels.json");
        let mock_bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&mock_bin_dir).unwrap();
        let mock_bin = mock_bin_dir.join("apptainer");

        // Спека: валидный TOML, достаточно [package].
        let spec_text = format!(
            "[package]\nname = \"{}\"\nversion = \"{}\"\n",
            name, version
        ).into_bytes();
        let spec_b64 = base64::engine::general_purpose::STANDARD.encode(&spec_text);
        let spec_sha = sha256_hex(&spec_text);

        // Метки — как их отдаёт реальный appspec 0.3.4.
        let labels = serde_json::json!({
            "data": { "attributes": { "labels": {
                "org.appspec.package.name": name,
                "org.appspec.package.version": version,
                "org.appspec.package.release": "1",
                "org.appspec.package.epoch": "0",
                "org.appspec.package.arch": arch,
                "org.appspec.package.summary": "Test package",
                "org.appspec.package.license": "MIT",
                "org.appspec.spec.content.base64": spec_b64,
                "org.appspec.spec.sha256": spec_sha,
                "org.appspec.spec.filename": "fake.appspec",
                "org.appspec.spec.size": spec_text.len().to_string(),
                "org.appspec.generator": "appspec 0.3.4"
            }}}
        });
        fs::write(&labels_json, serde_json::to_vec_pretty(&labels).unwrap()).unwrap();

        // Мок apptainer.
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = inspect ] && [ \"$2\" = --json ] && [ \"$3\" = --labels ]; then\n\
                 cat {}\n\
                 exit 0\n\
             fi\n\
             echo \"mock apptainer: unexpected call: $*\" >&2\n\
             exit 1\n",
            labels_json.display()
        );
        fs::write(&mock_bin, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut m = fs::metadata(&mock_bin).unwrap().permissions();
            m.set_mode(0o755);
            fs::set_permissions(&mock_bin, m).unwrap();
        }

        // Фиктивный SIF.
        fs::write(&sif, b"fake sif bytes for E1 tests").unwrap();

        // Инициализируем репозиторий — нужен всем тестам, кроме init_*.
        // `init` не вызывает apptainer, PATH-мок не требуется.
        appspec()
            .args(["repo", "init", root.to_str().unwrap(), "--name", "test/e1"])
            .assert()
            .success();

        Fixture { _tmp: tmp, root, sif, mock_bin: mock_bin_dir, spec_text }
    }

    fn cmd(&self) -> Command {
        let mut c = appspec();
        c.env("PATH", format!("{}:{}",
            self.mock_bin.display(),
            std::env::var("PATH").unwrap_or_default()));
        c
    }
}

// ---- tests -----------------------------------------------------------------

#[test]
fn init_creates_expected_layout() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");

    appspec()
        .args(["repo", "init", root.to_str().unwrap(), "--name", "test/init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("initialized repository"));

    for rel in ["config.toml", "index.json",
                "blobs/sha256", "packages", "refs",
                "index/.keep"] {
        assert!(root.join(rel).exists(), "missing: {rel}");
    }
}

#[test]
fn init_is_idempotent_with_same_name() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");

    appspec()
        .args(["repo", "init", root.to_str().unwrap(), "--name", "test/x"])
        .assert().success();
    appspec()
        .args(["repo", "init", root.to_str().unwrap(), "--name", "test/x"])
        .assert()
        .success()
        .stdout(predicate::str::contains("already initialized"));
}

#[test]
fn init_rejects_different_name() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");

    appspec()
        .args(["repo", "init", root.to_str().unwrap(), "--name", "test/a"])
        .assert().success();
    appspec()
        .args(["repo", "init", root.to_str().unwrap(), "--name", "test/b"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already initialized as `test/a`"));
}

#[test]
fn add_and_show_roundtrip() {
    let f = Fixture::new("demo", "0.0.1", "amd64");
    let root_str = f.root.to_str().unwrap();
    let sif_str = f.sif.to_str().unwrap();

    // add
    f.cmd()
        .args(["repo", "add", "--root", root_str, sif_str])
        .assert()
        .success()
        .stdout(predicate::str::contains("added: demo:0.0.1-1"));

    // meta.json
    let meta_path = f.root.join("packages/demo/0.0.1-1/meta.json");
    assert!(meta_path.is_file());
    let meta: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&meta_path).unwrap()).unwrap();
    assert_eq!(meta["name"], "demo");
    assert_eq!(meta["version"], "0.0.1");
    assert_eq!(meta["release"], "1");
    assert_eq!(meta["arch"], "amd64");
    assert_eq!(meta["epoch"], 0);                    // integer, не строка
    assert_eq!(meta["requires"], serde_json::json!([]));
    assert_eq!(meta["provides"], serde_json::json!([]));
    assert_eq!(meta["conflicts"], serde_json::json!([]));

    // spec.appspec байт-в-байт
    let spec_on_disk =
        fs::read(f.root.join("packages/demo/0.0.1-1/spec.appspec")).unwrap();
    assert_eq!(spec_on_disk, f.spec_text);

    // blob присутствует, sha256 совпадает с meta
    let sha = meta["sif_sha256"].as_str().unwrap();
    let blob = f.root.join("blobs/sha256").join(&sha[..2]).join(&sha[2..]);
    assert!(blob.is_file());
    assert_eq!(sha256_hex(&fs::read(&blob).unwrap()), sha);

    // show --spec — байт-в-байт
    let out = f.cmd()
        .args(["repo", "show", "demo:0.0.1-1", "--spec", "--root", root_str])
        .assert().success()
        .get_output().stdout.clone();
    assert_eq!(out, f.spec_text);
}

#[test]
fn add_is_idempotent() {
    let f = Fixture::new("demo", "0.0.1", "amd64");
    let root = f.root.to_str().unwrap();
    let sif = f.sif.to_str().unwrap();

    f.cmd().args(["repo", "add", "--root", root, sif]).assert().success();
    f.cmd()
        .args(["repo", "add", "--root", root, sif])
        .assert()
        .success()
        .stdout(predicate::str::contains("no-op"));
}

#[test]
fn release_suffix_creates_new_version() {
    let f = Fixture::new("demo", "0.0.1", "amd64");
    let root = f.root.to_str().unwrap();
    let sif = f.sif.to_str().unwrap();

    f.cmd().args(["repo", "add", "--root", root, sif]).assert().success();
    f.cmd()
        .args(["repo", "add", "--root", root, "--release-suffix", "2", sif])
        .assert().success();

    // A-9: обе записи на месте; sif_sha256 одинаковый (dedup blob'а).
    // §6.2: --release-suffix N → release = "<original>.<N>"
    //       label release="1" + suffix "2" → "1.2" → pkg dir "0.0.1-1.2"
    assert!(f.root.join("packages/demo/0.0.1-1/meta.json").is_file());
    assert!(f.root.join("packages/demo/0.0.1-1.2/meta.json").is_file());

    let m1: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(f.root.join("packages/demo/0.0.1-1/meta.json")).unwrap()
    ).unwrap();
    let m2: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(f.root.join("packages/demo/0.0.1-1.2/meta.json")).unwrap()
    ).unwrap();
    assert_eq!(m1["sif_sha256"], m2["sif_sha256"]);
    assert_eq!(m1["release"], "1");
    assert_eq!(m2["release"], "1.2");
}

#[test]
fn reindex_is_idempotent() {
    let f = Fixture::new("demo", "0.0.1", "amd64");
    let root = f.root.to_str().unwrap();
    let sif = f.sif.to_str().unwrap();

    f.cmd().args(["repo", "add", "--root", root, sif]).assert().success();

    let before = fs::read(f.root.join("index.json")).unwrap();
    f.cmd().args(["repo", "reindex", "--root", root]).assert().success();
    let after1 = fs::read(f.root.join("index.json")).unwrap();
    f.cmd().args(["repo", "reindex", "--root", root]).assert().success();
    let after2 = fs::read(f.root.join("index.json")).unwrap();

    // A-8: без изменений в packages/index.json байт-в-байт
    assert_eq!(before, after1, "first reindex changed index.json");
    assert_eq!(after1, after2, "second reindex changed index.json");
}

#[test]
fn ls_and_search_find_package() {
    let f = Fixture::new("demo", "0.0.1", "amd64");
    let root = f.root.to_str().unwrap();
    let sif = f.sif.to_str().unwrap();
    f.cmd().args(["repo", "add", "--root", root, sif]).assert().success();

    f.cmd()
        .args(["repo", "ls", "--root", root])
        .assert()
        .success()
        .stdout(predicate::str::contains("demo"));

    f.cmd()
        .args(["repo", "ls", "demo", "--root", root])
        .assert()
        .success()
        .stdout(predicate::str::contains("0.0.1-1"));

    f.cmd()
        .args(["repo", "search", "demo", "--root", root])
        .assert()
        .success()
        .stdout(predicate::str::contains("demo"));

    f.cmd()
        .args(["repo", "search", "nonexistent-xyz", "--root", root])
        .assert()
        .success()
        .stdout(predicate::str::is_empty());

    f.cmd()
        .args(["repo", "search", "--arch", "arm64", "--root", root])
        .assert()
        .success()
        .stdout(predicate::str::is_empty());

    f.cmd()
        .args(["repo", "search", "--arch", "amd64", "--root", root])
        .assert()
        .success()
        .stdout(predicate::str::contains("demo"));
}

#[test]
fn check_reports_ok_for_healthy_repo() {
    let f = Fixture::new("demo", "0.0.1", "amd64");
    let root = f.root.to_str().unwrap();
    f.cmd()
        .args(["repo", "add", "--root", root, f.sif.to_str().unwrap()])
        .assert().success();

    f.cmd()
        .args(["repo", "check", "--root", root])
        .assert()
        .success()
        .stdout(predicate::str::contains("check: ok"));
}

#[test]
fn check_detects_corrupted_blob() {
    let f = Fixture::new("demo", "0.0.1", "amd64");
    let root = f.root.to_str().unwrap();
    f.cmd()
        .args(["repo", "add", "--root", root, f.sif.to_str().unwrap()])
        .assert().success();

    // портим blob
    let meta: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(f.root.join("packages/demo/0.0.1-1/meta.json")).unwrap()
    ).unwrap();
    let sha = meta["sif_sha256"].as_str().unwrap();
    let blob = f.root.join("blobs/sha256").join(&sha[..2]).join(&sha[2..]);
    fs::write(&blob, b"corrupted").unwrap();

    f.cmd()
        .args(["repo", "check", "--root", root])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("sha256 mismatch"));
}

#[test]
fn gc_dry_run_does_not_remove() {
    let f = Fixture::new("demo", "0.0.1", "amd64");
    let root = f.root.to_str().unwrap();
    f.cmd()
        .args(["repo", "add", "--root", root, f.sif.to_str().unwrap()])
        .assert().success();

    let meta: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(f.root.join("packages/demo/0.0.1-1/meta.json")).unwrap()
    ).unwrap();
    let sha = meta["sif_sha256"].as_str().unwrap();
    let blob = f.root.join("blobs/sha256").join(&sha[..2]).join(&sha[2..]);
    assert!(blob.is_file());

    f.cmd()
        .args(["repo", "gc", "--dry-run", "--root", root])
        .assert()
        .success()
        .stdout(predicate::str::contains("would remove: 0 blobs"));

    assert!(blob.is_file(), "dry-run must not delete blob");
}

#[test]
fn uninitialized_repo_yields_clear_error() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("nope");
    appspec()
        .args(["repo", "ls", "--root", root.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("repository not initialized"));
}

#[test]
fn unsupported_layout_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    appspec()
        .args(["repo", "init", root.to_str().unwrap()])
        .assert().success();

    // подменяем layout на 2
    let cfg_path = root.join("config.toml");
    let cfg = fs::read_to_string(&cfg_path).unwrap();
    fs::write(&cfg_path, cfg.replace("layout = 1", "layout = 2")).unwrap();

    appspec()
        .args(["repo", "ls", "--root", root.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unsupported repository layout 2"));
}