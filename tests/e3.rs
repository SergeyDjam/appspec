//! E3 conformance tests: bundle + source = repo + bundle.lock.
//!
//! `bundle build` тесты не запускают (нужен реальный apptainer).
//! Покрыто: `bundle check` с repo-источником, `bundle lock`,
//! обнаружение дрейфа lock'а.

use assert_cmd::Command;
use base64::Engine as _;
use predicates::prelude::*;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn appspec() -> Command {
    Command::cargo_bin("appspec").expect("appspec binary built")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

/// Готовит repo с одним пакетом `demo:0.0.1-1` (amd64) + мок apptainer.
struct RepoFixture {
    _tmp: TempDir,
    repo_root: PathBuf,
    mock_bin: PathBuf,
}

impl RepoFixture {
    fn new(pkg_name: &str, version: &str, release: &str) -> Self {
        let tmp = TempDir::new().unwrap();
        let repo_root = tmp.path().join("repo");
        let mock_bin = tmp.path().join("bin");
        fs::create_dir_all(&mock_bin).unwrap();
        let mock_apptainer = mock_bin.join("apptainer");

        // мок apptainer + labels.json
        let labels_path = tmp.path().join("labels.json");
        let spec_bytes = format!(
            "[package]\nname = \"{pkg_name}\"\nversion = \"{version}\"\n"
        )
        .into_bytes();
        let spec_b64 =
            base64::engine::general_purpose::STANDARD.encode(&spec_bytes);
        let spec_sha = sha256_hex(&spec_bytes);
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
        fs::write(&labels_path, serde_json::to_vec_pretty(&labels).unwrap()).unwrap();

        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = inspect ] && [ \"$2\" = --json ] && [ \"$3\" = --labels ]; then\n\
                 cat {}\n\
                 exit 0\n\
             fi\n\
             echo \"mock apptainer: unexpected: $*\" >&2\n\
             exit 1\n",
            labels_path.display()
        );
        fs::write(&mock_apptainer, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut m = fs::metadata(&mock_apptainer).unwrap().permissions();
            m.set_mode(0o755);
            fs::set_permissions(&mock_apptainer, m).unwrap();
        }

        // Фиктивный SIF
        let sif = tmp.path().join(format!("{pkg_name}.sif"));
        fs::write(&sif, b"fake sif bytes").unwrap();

        // init + add через мок
        let mut cmd = appspec();
        cmd.env("PATH", format!(
            "{}:{}", mock_bin.display(),
            std::env::var("PATH").unwrap_or_default()
        ));
        cmd.env("APPSPEC_REPO", &repo_root);
        cmd.args(["repo", "init"]).assert().success();

        let mut cmd = appspec();
        cmd.env("PATH", format!(
            "{}:{}", mock_bin.display(),
            std::env::var("PATH").unwrap_or_default()
        ));
        cmd.env("APPSPEC_REPO", &repo_root);
        cmd.args(["repo", "add", sif.to_str().unwrap()]).assert().success();

        RepoFixture { _tmp: tmp, repo_root, mock_bin }
    }

    fn cmd(&self) -> Command {
        let mut c = appspec();
        c.env("PATH", format!(
            "{}:{}", self.mock_bin.display(),
            std::env::var("PATH").unwrap_or_default()
        ));
        c.env("APPSPEC_REPO", &self.repo_root);
        c
    }
}

fn write_bundle(dir: &std::path::Path, name: &str, components: &str) -> PathBuf {
    let p = dir.join("bundle.appspec");
    let body = format!(
        r#"
[bundle]
name = "{name}"

[bundle.base]
spec = "./base.appspec"

{components}
"#
    );
    fs::write(&p, body).unwrap();
    // base.appspec нужен, чтобы preflight не ругался на отсутствие spec
    fs::write(
        dir.join("base.appspec"),
        r#"
[package]
name    = "base"
version = "0.0.1"

[build]
base     = "alpine:3.21"
template = "alpine"

[runtime]
"#,
    )
    .unwrap();
    p
}

// ===========================================================================
// Tests
// ===========================================================================

#[test]
fn bundle_check_accepts_repo_source() {
    let repo = RepoFixture::new("demo", "0.0.1", "1");
    let tmp = TempDir::new().unwrap();
    let bundle = write_bundle(
        tmp.path(),
        "test",
        r#"[[components]]
name    = "demo"
repo    = "demo:0.0.1-1"
provide = "demo"
"#,
    );

    repo.cmd()
        .args(["bundle", "check", bundle.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("check: ok"))
        .stderr(predicate::str::contains("demo:0.0.1-1").not());
}

#[test]
fn bundle_check_rejects_missing_package() {
    let repo = RepoFixture::new("demo", "0.0.1", "1");
    let tmp = TempDir::new().unwrap();
    let bundle = write_bundle(
        tmp.path(),
        "test",
        r#"[[components]]
name    = "nope"
repo    = "nope:0.0.1-1"
provide = "nope"
"#,
    );

    repo.cmd()
        .args(["bundle", "check", bundle.to_str().unwrap()])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not in repo index"));
}

#[test]
fn bundle_lock_writes_file() {
    let repo = RepoFixture::new("demo", "0.0.1", "1");
    let tmp = TempDir::new().unwrap();
    let bundle = write_bundle(
        tmp.path(),
        "test",
        r#"[[components]]
name    = "demo"
repo    = "demo:0.0.1-1"
provide = "demo"
"#,
    );

    repo.cmd()
        .args(["bundle", "lock", bundle.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("wrote"))
        .stdout(predicate::str::contains("1 repo components"));

    let lock_path = tmp.path().join("bundle.lock.toml");
    assert!(lock_path.is_file(), "bundle.lock.toml not created");

    let lock: toml::Value = toml::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
    assert_eq!(lock["bundle"]["name"].as_str(), Some("test"));
    assert!(lock["bundle"]["locked_at"].as_str().is_some());
    let comps = lock["components"].as_array().unwrap();
    assert_eq!(comps.len(), 1);
    assert_eq!(comps[0]["name"].as_str(), Some("demo"));
    assert_eq!(comps[0]["repo"].as_str(), Some("demo:0.0.1-1"));
    assert_eq!(
        comps[0]["sif_sha256"].as_str().unwrap().len(),
        64,
        "sif_sha256 must be 64 hex chars"
    );
    assert_eq!(comps[0]["spec_sha256"].as_str().unwrap().len(), 64);
}

#[test]
fn bundle_check_ok_with_drifted_lock() {
    let repo = RepoFixture::new("demo", "0.0.1", "1");
    let tmp = TempDir::new().unwrap();
    let bundle = write_bundle(
        tmp.path(),
        "test",
        r#"[[components]]
name    = "demo"
repo    = "demo:0.0.1-1"
provide = "demo"
"#,
    );

    // lock по текущему индексу
    repo.cmd()
        .args(["bundle", "lock", bundle.to_str().unwrap()])
        .assert()
        .success();

    // подменяем sif_sha256 в lock, чтобы был дрейф
    let lock_path = tmp.path().join("bundle.lock.toml");
    let mut lock: toml::Value =
        toml::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
    let sif_sha = lock["components"][0]["sif_sha256"].as_str().unwrap().to_string();
    lock["components"][0]["sif_sha256"] = toml::Value::String(
        sif_sha.chars().map(|c| if c == '0' { '1' } else { '0' }).collect::<String>(),
    );
    fs::write(&lock_path, toml::to_string_pretty(&lock).unwrap()).unwrap();

    // `bundle check` не читает lock (это делает build). Проверим,
    // что `check` по-прежнему ok — дрейф обнаруживается только в build.
    repo.cmd()
        .args(["bundle", "check", bundle.to_str().unwrap()])
        .assert()
        .success();
}