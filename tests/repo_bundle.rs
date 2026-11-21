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

[[extensions]]
name = "publisher.ext"
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

    for _ in 0..2 {
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
        .stderr(predicate::str::contains("--version` is required"));
}

#[test]
fn add_bundle_rejects_local_sources() {
    let (_tmp, root) = make_repo();
    let bundle = write_bundle(_tmp.path(), "testbundle", "./base.appspec", "git:0.0.1-1");
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
        .failure()
        .stderr(predicate::str::contains("local path"));

    let bundle = write_bundle(_tmp.path(), "testbundle2", "base:0.0.1-1", "./git.appspec");
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
        .failure()
        .stderr(predicate::str::contains("local path"));
}
