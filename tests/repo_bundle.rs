//! 0.6.1 B: `repo add <bundle.appspec>` publishes a bundle package.
//!
//! Repo fixture contains two SIF packages (`base`, `git`); the bundle
//! references them by repo refs. No apptainer required (blobs are fake).

use assert_cmd::Command;
use predicates::prelude::*;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;

fn appspec() -> Command {
    Command::cargo_bin("appspec").expect("appspec built")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

fn write_pkg(root: &Path, name: &str, version: &str, release: &str) -> serde_json::Value {
    let ver_key = format!("{version}-{release}");
    let spec = format!(
        "[package]\nname = \"{name}\"\nversion = \"{version}\"\n\n[build]\nbase = \"alpine:3.21\"\ntemplate = \"alpine\"\n\n[runtime]\nsif_file = \"{name}.sif\"\n"
    );
    let spec_sha = sha256_hex(spec.as_bytes());
    let sif = format!("fake sif {name} {ver_key}").into_bytes();
    let sif_sha = sha256_hex(&sif);
    let blob = root
        .join("blobs/sha256")
        .join(&sif_sha[..2])
        .join(&sif_sha[2..]);
    fs::create_dir_all(blob.parent().unwrap()).unwrap();
    fs::write(&blob, &sif).unwrap();

    let pkg = root.join("packages").join(name).join(&ver_key);
    fs::create_dir_all(&pkg).unwrap();
    fs::write(pkg.join("spec.appspec"), spec.as_bytes()).unwrap();
    fs::write(pkg.join("sha256"), sif_sha.as_bytes()).unwrap();
    let meta = serde_json::json!({
        "schema": 1,
        "name": name,
        "version": version,
        "release": release,
        "epoch": 0,
        "arch": "amd64",
        "sif_sha256": sif_sha,
        "sif_size": sif.len(),
        "spec_sha256": spec_sha,
        "spec_size": spec.len(),
        "spec_filename": "spec.appspec",
        "generator": "appspec test",
        "built_at": "2026-11-21T00:00:00Z",
        "requires": [],
        "provides": [],
        "conflicts": [],
        "raw_labels": {}
    });
    fs::write(
        pkg.join("meta.json"),
        serde_json::to_vec_pretty(&meta).unwrap(),
    )
    .unwrap();

    serde_json::json!({
        "epoch": 0,
        "version": version,
        "release": release,
        "arch": "amd64",
        "sif_sha256": sif_sha,
        "sif_size": sif.len(),
        "spec_sha256": spec_sha,
        "spec_size": spec.len(),
        "built_at": "2026-11-21T00:00:00Z",
        "generator": "appspec test",
        "tags": [],
        "requires": [],
        "provides": []
    })
}

fn make_repo() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    fs::create_dir_all(root.join("blobs/sha256")).unwrap();
    fs::create_dir_all(root.join("packages")).unwrap();
    fs::create_dir_all(root.join("refs")).unwrap();
    fs::create_dir_all(root.join("index")).unwrap();
    fs::write(root.join("index/.keep"), b"").unwrap();

    let mut packages = BTreeMap::new();
    let mut versions = BTreeMap::new();
    versions.insert(
        "0.0.1-1".to_string(),
        write_pkg(&root, "base", "0.0.1", "1"),
    );
    packages.insert(
        "base".to_string(),
        serde_json::json!({ "versions": versions }),
    );
    let mut versions = BTreeMap::new();
    versions.insert("0.0.1-1".to_string(), write_pkg(&root, "git", "0.0.1", "1"));
    packages.insert(
        "git".to_string(),
        serde_json::json!({ "versions": versions }),
    );

    let index = serde_json::json!({
        "schema": 1,
        "repo": "test/bundle",
        "updated": "2026-11-21T00:00:00Z",
        "generator": "appspec test",
        "signature": "",
        "packages": packages
    });
    fs::write(
        root.join("index.json"),
        serde_json::to_vec_pretty(&index).unwrap(),
    )
    .unwrap();
    (tmp, root)
}

fn write_bundle(dir: &Path, name: &str, base: &str, git: &str) -> PathBuf {
    let p = dir.join("bundle.appspec");
    fs::write(
        &p,
        format!(
            r#"[bundle]
name        = "{name}"
description = "test bundle"

[bundle.base]
spec  = "{base}"
entry = "exec /bin/sh"

[[components]]
name    = "git"
spec    = "{git}"
provide = "git"
"#
        ),
    )
    .unwrap();
    p
}

#[test]
fn add_bundle_writes_manifest_meta_and_index() {
    let (_tmp, root) = make_repo();
    let bundle = write_bundle(_tmp.path(), "testbundle", "base:0.0.1-1", "git:0.0.1-1");

    appspec()
        .args([
            "repo",
            "add",
            bundle.to_str().unwrap(),
            "--version",
            "1.0",
            "--root",
            root.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("(bundle"));

    let pkg = root.join("packages/testbundle/1.0-1");
    for f in ["spec.appspec", "manifest.json", "meta.json", "sha256"] {
        assert!(pkg.join(f).is_file(), "missing {f}");
    }

    let meta: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(pkg.join("meta.json")).unwrap()).unwrap();
    assert_eq!(meta["kind"], "bundle");
    assert_eq!(meta["sif_sha256"], "");
    assert_eq!(meta["sif_size"], 0);
    assert_eq!(meta["bundle_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(meta["manifest_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(meta["spec_sha256"], meta["bundle_sha256"]);
    assert!(meta["provides"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v.as_str() == Some("git")));

    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(pkg.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["schema"], 1);
    assert_eq!(manifest["components"][0]["name"], "git");
    assert_eq!(
        manifest["components"][0]["source"]["resolved"],
        "repo:git:0.0.1-1"
    );
    assert_eq!(manifest["base"]["resolved"], "repo:base:0.0.1-1");

    let index: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join("index.json")).unwrap()).unwrap();
    let v = &index["packages"]["testbundle"]["versions"]["1.0-1"];
    assert_eq!(v["kind"], "bundle");
    assert_eq!(v["sif_sha256"], "");
    assert_eq!(v["bundle_sha256"], meta["bundle_sha256"]);
    assert_eq!(v["manifest_sha256"], meta["manifest_sha256"]);

    // Blobs for bundle spec + manifest must exist.
    for sha in [
        meta["bundle_sha256"].as_str().unwrap().to_string(),
        meta["manifest_sha256"].as_str().unwrap().to_string(),
    ] {
        let blob = root.join("blobs/sha256").join(&sha[..2]).join(&sha[2..]);
        assert!(blob.is_file(), "missing blob {}", blob.display());
    }
}

#[test]
fn add_bundle_is_idempotent_and_check_passes() {
    let (_tmp, root) = make_repo();
    let bundle = write_bundle(_tmp.path(), "testbundle", "base:0.0.1-1", "git:0.0.1-1");

    for i in 0..2 {
        appspec()
            .args([
                "repo",
                "add",
                bundle.to_str().unwrap(),
                "--version",
                "1.0",
                "--root",
                root.to_str().unwrap(),
            ])
            .assert()
            .success();
        if i == 0 {
            // Regression: manifest must not include timestamps, otherwise
            // a second add in a different second looks like a conflict.
            std::thread::sleep(Duration::from_millis(1100));
        }
    }
    let out = appspec()
        .args([
            "repo",
            "add",
            bundle.to_str().unwrap(),
            "--version",
            "1.0",
            "--root",
            root.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("no-op"));

    appspec()
        .args(["repo", "check", "--root", root.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("check: ok"));

    appspec()
        .args(["repo", "ls", "testbundle", "--root", root.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("kind=bundle"));

    appspec()
        .args([
            "repo",
            "show",
            "testbundle:1.0-1",
            "--json",
            "--root",
            root.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"kind\": \"bundle\""));
}

#[test]
fn add_bundle_alias_and_gc_keep_blobs() {
    let (_tmp, root) = make_repo();
    let bundle = write_bundle(_tmp.path(), "testbundle", "base:0.0.1-1", "git:0.0.1-1");

    appspec()
        .args([
            "repo",
            "add-bundle",
            bundle.to_str().unwrap(),
            "--version",
            "1.0",
            "--root",
            root.to_str().unwrap(),
        ])
        .assert()
        .success();

    let meta: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(root.join("packages/testbundle/1.0-1/meta.json")).unwrap(),
    )
    .unwrap();
    appspec()
        .args([
            "repo",
            "gc",
            "--keep",
            "1",
            "--root",
            root.to_str().unwrap(),
        ])
        .assert()
        .success();

    for sha in [
        meta["bundle_sha256"].as_str().unwrap().to_string(),
        meta["manifest_sha256"].as_str().unwrap().to_string(),
    ] {
        let blob = root.join("blobs/sha256").join(&sha[..2]).join(&sha[2..]);
        assert!(blob.is_file(), "gc removed bundle blob {}", blob.display());
    }
}

#[test]
fn pull_bundle_from_path_recurses_into_components() {
    let (tmp, src) = make_repo();
    let bundle = write_bundle(tmp.path(), "testbundle", "base:0.0.1-1", "git:0.0.1-1");
    appspec()
        .args([
            "repo",
            "add",
            bundle.to_str().unwrap(),
            "--version",
            "1.0",
            "--root",
            src.to_str().unwrap(),
        ])
        .assert()
        .success();

    let dst = tmp.path().join("dst");
    appspec()
        .args(["repo", "init", dst.to_str().unwrap()])
        .assert()
        .success();

    appspec()
        .args([
            "repo",
            "pull",
            "testbundle:1.0-1",
            "--from",
            src.to_str().unwrap(),
            "--root",
            dst.to_str().unwrap(),
        ])
        .assert()
        .success();

    for rel in [
        "testbundle/1.0-1/manifest.json",
        "testbundle/1.0-1/spec.appspec",
        "testbundle/1.0-1/meta.json",
        "base/0.0.1-1/meta.json",
        "git/0.0.1-1/meta.json",
    ] {
        assert!(dst.join("packages").join(rel).is_file(), "missing {rel}");
    }

    appspec()
        .args(["repo", "check", "--root", dst.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("check: ok"));
}

#[test]
fn run_bundle_package_dry_run_builds_launcher() {
    let (tmp, root) = make_repo();
    let bundle = write_bundle(tmp.path(), "testbundle", "base:0.0.1-1", "git:0.0.1-1");
    appspec()
        .args([
            "repo",
            "add",
            bundle.to_str().unwrap(),
            "--version",
            "1.0",
            "--root",
            root.to_str().unwrap(),
        ])
        .assert()
        .success();

    let cache = tmp.path().join("cache");
    appspec()
        .args([
            "run",
            "testbundle:1.0-1",
            "--root",
            root.to_str().unwrap(),
            "--allow-unsigned",
            "--dry-run",
        ])
        .env("XDG_CACHE_HOME", &cache)
        .assert()
        .success()
        .stderr(predicate::str::contains("dry-run: not executing"));

    let launcher = cache.join("appspec/run/bundle.testbundle.1.0-1/run.testbundle.sh");
    assert!(
        launcher.is_file(),
        "launcher missing: {}",
        launcher.display()
    );
}

#[cfg(unix)]
#[test]
fn pin_bundle_pins_spec_and_manifest() {
    use std::os::unix::fs::PermissionsExt;

    let (tmp, root) = make_repo();
    let bundle = write_bundle(tmp.path(), "testbundle", "base:0.0.1-1", "git:0.0.1-1");
    appspec()
        .args([
            "repo",
            "add",
            bundle.to_str().unwrap(),
            "--version",
            "1.0",
            "--root",
            root.to_str().unwrap(),
        ])
        .assert()
        .success();

    let meta_path = root.join("packages/testbundle/1.0-1/meta.json");
    let mut meta: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&meta_path).unwrap()).unwrap();
    meta["ipfs_cid"] = serde_json::Value::Null;
    meta["manifest_ipfs_cid"] = serde_json::Value::Null;
    fs::write(&meta_path, serde_json::to_vec_pretty(&meta).unwrap()).unwrap();

    let bin = tmp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let log = tmp.path().join("ipfs.log");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\ncase \"$1\" in\n  add) echo 'bafkreipinnedbundle' ;;\n  pin) exit 0 ;;\n  *) exit 0 ;;\nesac\n",
        log.display()
    );
    let ipfs = bin.join("ipfs");
    fs::write(&ipfs, script).unwrap();
    fs::set_permissions(&ipfs, fs::Permissions::from_mode(0o755)).unwrap();

    appspec()
        .args(["repo", "pin", "--root", root.to_str().unwrap()])
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .assert()
        .success();

    let meta: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&meta_path).unwrap()).unwrap();
    assert_eq!(meta["ipfs_cid"], "bafkreipinnedbundle");
    assert_eq!(meta["manifest_ipfs_cid"], "bafkreipinnedbundle");
    let log_text = fs::read_to_string(&log).unwrap();
    assert!(
        log_text.lines().filter(|l| l.starts_with("add ")).count() >= 2,
        "expected ipfs add calls for bundle + manifest:\n{log_text}"
    );
}

#[test]
fn export_oci_dry_run_includes_bundle_artifact() {
    let (tmp, root) = make_repo();
    let bundle = write_bundle(tmp.path(), "testbundle", "base:0.0.1-1", "git:0.0.1-1");
    appspec()
        .args([
            "repo",
            "add",
            bundle.to_str().unwrap(),
            "--version",
            "1.0",
            "--root",
            root.to_str().unwrap(),
        ])
        .assert()
        .success();

    appspec()
        .args([
            "repo",
            "export-oci",
            "appspec-index",
            "--registry",
            "ghcr.io/test",
            "--dry-run",
            "--root",
            root.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "application/vnd.appspec.bundle.v1+json",
        ))
        .stdout(predicate::str::contains("org.appspec.kind=bundle"));
}

#[test]
fn bundle_version_from_toml_is_used() {
    let (tmp, root) = make_repo();
    let p = tmp.path().join("bundle.appspec");
    fs::write(
        &p,
        r#"[bundle]
name        = "testbundle"
version     = "0.1.0"
description = "version in TOML"

[bundle.base]
spec  = "base:0.0.1-1"
entry = "exec /bin/sh"

[[components]]
name    = "git"
spec    = "git:0.0.1-1"
provide = "git"
"#,
    )
    .unwrap();

    appspec()
        .args([
            "repo",
            "add",
            p.to_str().unwrap(),
            "--root",
            root.to_str().unwrap(),
        ])
        .assert()
        .success();

    assert!(root.join("packages/testbundle/0.1.0-1/meta.json").is_file());
}

#[test]
fn add_bundle_requires_version() {
    let (_tmp, root) = make_repo();
    let bundle = write_bundle(_tmp.path(), "testbundle", "base:0.0.1-1", "git:0.0.1-1");
    appspec()
        .args([
            "repo",
            "add",
            bundle.to_str().unwrap(),
            "--root",
            root.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("version is required"));
}

fn write_local_appspec(dir: &Path, name: &str, version: &str) {
    fs::write(
        dir.join(format!("{name}.appspec")),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"{version}\"\n\n[build]\nbase = \"alpine:3.21\"\ntemplate = \"alpine\"\n\n[runtime]\n"
        ),
    )
    .unwrap();
}

#[test]
fn add_bundle_maps_local_specs_to_repo_packages() {
    let (tmp, root) = make_repo();
    write_local_appspec(tmp.path(), "base", "0.0.1");
    write_local_appspec(tmp.path(), "git", "0.0.1");
    let bundle = write_bundle(tmp.path(), "testbundle", "./base.appspec", "./git.appspec");
    appspec()
        .args([
            "repo",
            "add",
            bundle.to_str().unwrap(),
            "--version",
            "1.0",
            "--root",
            root.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("(bundle"));

    let manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(root.join("packages/testbundle/1.0-1/manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["base"]["resolved"], "repo:base:0.0.1-1");
    assert_eq!(
        manifest["components"][0]["source"]["resolved"],
        "repo:git:0.0.1-1"
    );
}

#[test]
fn add_bundle_rejects_local_spec_not_in_repo() {
    let (tmp, root) = make_repo();
    write_local_appspec(tmp.path(), "missing", "1.0");
    let bundle = write_bundle(tmp.path(), "testbundle", "./missing.appspec", "git:0.0.1-1");
    appspec()
        .args([
            "repo",
            "add",
            bundle.to_str().unwrap(),
            "--version",
            "1.0",
            "--root",
            root.to_str().unwrap(),
            "--no-deps",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not in the repo"));
}
