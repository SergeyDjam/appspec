//! 0.6.6: `repo add package.appspec` builds/locates the SIF and then adds it.

use assert_cmd::Command;
use base64::Engine as _;
use predicates::prelude::*;
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use tempfile::TempDir;

fn appspec() -> Command {
    Command::cargo_bin("appspec").expect("appspec built")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

struct Fixture {
    _tmp: TempDir,
    bin: PathBuf,
    dir: PathBuf,
    spec: PathBuf,
    repo: PathBuf,
}

fn fixture(with_sif: bool) -> Fixture {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("app");
    fs::create_dir_all(&dir).unwrap();
    let spec = dir.join("app.appspec");
    let spec_bytes = b"[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[build]\nbase = \"alpine:3.21\"\ntemplate = \"alpine\"\n\n[runtime]\nsif_file = \"app.sif\"\n";
    fs::write(&spec, spec_bytes).unwrap();
    // SIF must not be older than the spec, otherwise `repo add` rebuilds it.
    if with_sif {
        fs::write(dir.join("app.sif"), b"fake app sif").unwrap();
    }

    let bin = tmp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let labels = serde_json::json!({
        "data": { "attributes": { "labels": {
            "org.appspec.package.name": "app",
            "org.appspec.package.version": "0.1.0",
            "org.appspec.package.release": "1",
            "org.appspec.package.epoch": "0",
            "org.appspec.package.arch": "amd64",
            "org.appspec.spec.content.base64":
                base64::engine::general_purpose::STANDARD.encode(spec_bytes),
            "org.appspec.spec.sha256": sha256_hex(spec_bytes),
            "org.appspec.spec.filename": "app.appspec",
            "org.appspec.generator": "appspec test"
        }}}
    });
    let labels_path = tmp.path().join("labels.json");
    fs::write(&labels_path, serde_json::to_vec(&labels).unwrap()).unwrap();
    let apptainer = bin.join("apptainer");
    fs::write(
        &apptainer,
        format!("#!/bin/sh\ncat '{}'\n", labels_path.display()),
    )
    .unwrap();
    fs::set_permissions(&apptainer, fs::Permissions::from_mode(0o755)).unwrap();

    let repo = tmp.path().join("repo");
    appspec()
        .args(["repo", "init", repo.to_str().unwrap()])
        .assert()
        .success();

    Fixture {
        _tmp: tmp,
        bin,
        dir: dir.clone(),
        spec,
        repo,
    }
}

impl Fixture {
    fn cmd(&self) -> Command {
        let mut c = appspec();
        c.env(
            "PATH",
            format!(
                "{}:{}",
                self.bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        );
        c.env("APPSPEC_APPTAINER", self.bin.join("apptainer"));
        c.env("APPSPEC_REPO", &self.repo);
        c
    }
}

fn run_add(f: &Fixture, extra: &[&str]) -> assert_cmd::assert::Assert {
    let mut args = vec!["repo", "add", f.spec.to_str().unwrap()];
    args.extend_from_slice(extra);
    f.cmd().args(args).assert()
}

#[test]
fn add_appspec_uses_existing_sif() {
    let f = fixture(true);
    run_add(&f, &[])
        .success()
        .stdout(predicate::str::contains("app:0.1.0-1"));

    assert!(f.repo.join("packages/app/0.1.0-1/meta.json").is_file());
    let meta: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(f.repo.join("packages/app/0.1.0-1/meta.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(meta["kind"], "sif");
    assert_eq!(meta["name"], "app");
    assert_eq!(meta["spec_filename"], "app.appspec");

    let index: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(f.repo.join("index.json")).unwrap()).unwrap();
    assert!(index["packages"]["app"]["versions"]["0.1.0-1"].is_object());
}

#[test]
fn add_appspec_without_sif_fails_with_no_build() {
    let f = fixture(false);
    run_add(&f, &["--no-build"])
        .failure()
        .stderr(predicate::str::contains("SIF не собран"));
    assert!(!f.dir.join("app.sif").exists());
}

#[test]
fn add_appspec_does_not_need_version_flag() {
    let f = fixture(true);
    // `--version` refers to bundles; app specs must ignore it with a warning
    // and still add the SIF from labels.
    run_add(&f, &["--version", "9.9.9"])
        .success()
        .stderr(predicate::str::contains("apply only to bundles"));
}

#[test]
fn install_package_from_repo() {
    let f = fixture(true);
    run_add(&f, &[]).success();

    let bin = f._tmp.path().join("bin");
    let data = f._tmp.path().join("data");
    f.cmd()
        .args([
            "install",
            "app",
            "--root",
            f.repo.to_str().unwrap(),
            "--no-desktop",
        ])
        .env("XDG_BIN_HOME", &bin)
        .env("XDG_DATA_HOME", &data)
        .assert()
        .success();

    assert!(bin.join("app").is_file(), "launcher not installed");
    assert!(
        data.join("apptainer/app.sif").is_file(),
        "SIF not installed"
    );

    f.cmd()
        .args(["uninstall", "app"])
        .env("XDG_BIN_HOME", &bin)
        .env("XDG_DATA_HOME", &data)
        .assert()
        .success();
    assert!(!bin.join("app").exists(), "launcher not removed");
    assert!(!data.join("apptainer/app.sif").exists(), "SIF not removed");
}

#[test]
fn add_bundle_auto_publishes_local_dependencies() {
    let f = fixture(true);
    let bundle = f.dir.join("bundle.appspec");
    fs::write(
        &bundle,
        r#"[bundle]
name    = "appbundle"
version = "1.0"

[bundle.base]
spec  = "./app.appspec"
entry = "exec /bin/sh"
"#,
    )
    .unwrap();

    f.cmd()
        .args(["repo", "add", bundle.to_str().unwrap()])
        .assert()
        .success();

    assert!(f.repo.join("packages/app/0.1.0-1/meta.json").is_file());
    assert!(f.repo.join("packages/appbundle/1.0-1/meta.json").is_file());
}

#[test]
fn add_appspec_and_sif_together() {
    let f = fixture(true);
    let sif = f.dir.join("app.sif");
    f.cmd()
        .args([
            "repo",
            "add",
            f.spec.to_str().unwrap(),
            sif.to_str().unwrap(),
        ])
        .assert()
        .success();
    assert!(f.repo.join("packages/app/0.1.0-1/meta.json").is_file());
}
