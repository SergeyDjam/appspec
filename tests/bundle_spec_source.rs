//! 0.6.0: `spec = "..."` classification (3-tier), `repo` alias,
//! repo refs (`latest` / version-only) and `bundle.lock.toml` canonical refs.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn appspec() -> Command {
    Command::cargo_bin("appspec").expect("appspec built")
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

/// Minimal repository with `foo:1.0-1` (fake SIF + spec + meta + index).
fn make_repo_with_foo() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    fs::create_dir_all(root.join("blobs/sha256")).unwrap();
    fs::create_dir_all(root.join("packages/foo/1.0-1")).unwrap();
    fs::create_dir_all(root.join("refs")).unwrap();
    fs::create_dir_all(root.join("index")).unwrap();
    fs::write(root.join("index/.keep"), b"").unwrap();

    let blob_bytes = b"fake-sif-foo";
    let sha = sha256_hex(blob_bytes);
    let blob = root.join("blobs/sha256").join(&sha[..2]).join(&sha[2..]);
    fs::create_dir_all(blob.parent().unwrap()).unwrap();
    fs::write(&blob, blob_bytes).unwrap();

    let spec = b"[package]\nname = \"foo\"\nversion = \"1.0\"\n\n[build]\nbase = \"alpine:3.21\"\ntemplate = \"alpine\"\n\n[runtime]\nsif_file = \"foo.sif\"\n";
    let spec_sha = sha256_hex(spec);
    fs::write(root.join("packages/foo/1.0-1/spec.appspec"), spec).unwrap();

    let meta = serde_json::json!({
        "schema": 1, "name": "foo", "version": "1.0", "release": "1", "epoch": 0,
        "arch": "amd64",
        "sif_sha256": sha.clone(), "sif_size": blob_bytes.len(),
        "spec_sha256": spec_sha.clone(), "spec_size": spec.len(),
        "spec_filename": "spec.appspec",
        "generator": "appspec test", "built_at": "2026-11-20T00:00:00Z",
        "requires": [], "provides": [], "conflicts": [],
        "ipfs_cid": null, "raw_labels": {}
    });
    fs::write(
        root.join("packages/foo/1.0-1/meta.json"),
        serde_json::to_vec_pretty(&meta).unwrap(),
    )
    .unwrap();

    let idx = serde_json::json!({
        "schema": 1, "repo": "test/a1", "updated": "2026-11-20T00:00:00Z",
        "generator": "appspec test", "signature": "",
        "packages": {
            "foo": {
                "versions": {
                    "1.0-1": {
                        "epoch": 0, "version": "1.0", "release": "1", "arch": "amd64",
                        "sif_sha256": sha, "sif_size": blob_bytes.len(),
                        "spec_sha256": spec_sha, "spec_size": spec.len(),
                        "built_at": "2026-11-20T00:00:00Z",
                        "generator": "appspec test",
                        "tags": [], "requires": [], "provides": []
                    }
                }
            }
        }
    });
    fs::write(
        root.join("index.json"),
        serde_json::to_vec_pretty(&idx).unwrap(),
    )
    .unwrap();
    (tmp, root)
}

fn make_empty_repo() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    fs::create_dir_all(root.join("blobs/sha256")).unwrap();
    fs::create_dir_all(root.join("packages")).unwrap();
    fs::create_dir_all(root.join("refs")).unwrap();
    fs::create_dir_all(root.join("index")).unwrap();
    fs::write(root.join("index/.keep"), b"").unwrap();
    let idx = serde_json::json!({
        "schema": 1, "repo": "test/empty", "updated": "2026-11-20T00:00:00Z",
        "generator": "appspec test", "signature": "", "packages": {}
    });
    fs::write(
        root.join("index.json"),
        serde_json::to_vec_pretty(&idx).unwrap(),
    )
    .unwrap();
    (tmp, root)
}

fn write_local_spec(dir: &Path, name: &str) {
    fs::write(
        dir.join(name),
        "[package]\nname = \"base\"\nversion = \"1.0\"\n\n[build]\nbase = \"alpine:3.21\"\ntemplate = \"alpine\"\n\n[runtime]\n",
    )
    .unwrap();
}

fn write_bundle(dir: &Path, base_source: &str) -> PathBuf {
    let p = dir.join("bundle.appspec");
    fs::write(
        &p,
        format!(
            "[bundle]\nname = \"test-bundle\"\n\n[bundle.base]\nspec  = \"{}\"\nentry = \"exec /bin/sh\"\n",
            base_source
        ),
    )
    .unwrap();
    p
}

#[test]
fn explicit_path_missing_fails() {
    let tmp = TempDir::new().unwrap();
    let bundle = write_bundle(tmp.path(), "./missing.appspec");
    appspec()
        .args(["bundle", "check", bundle.to_str().unwrap()])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("spec not found"));
}

#[test]
fn explicit_path_exists_ok() {
    let tmp = TempDir::new().unwrap();
    write_local_spec(tmp.path(), "base.appspec");
    let bundle = write_bundle(tmp.path(), "./base.appspec");
    appspec()
        .args(["bundle", "check", bundle.to_str().unwrap()])
        .assert()
        .success();
}

#[test]
fn ambiguous_local_exists_ok() {
    let tmp = TempDir::new().unwrap();
    write_local_spec(tmp.path(), "base.appspec");
    let bundle = write_bundle(tmp.path(), "base.appspec");
    appspec()
        .args(["bundle", "check", bundle.to_str().unwrap()])
        .assert()
        .success();
}

#[test]
fn ambiguous_missing_fallback_to_repo() {
    let (_tmp_repo, repo_path) = make_repo_with_foo();
    let tmp = TempDir::new().unwrap();
    let bundle = write_bundle(tmp.path(), "foo.appspec");
    appspec()
        .args([
            "bundle",
            "check",
            bundle.to_str().unwrap(),
            "--repo",
            repo_path.to_str().unwrap(),
            "--allow-unsigned",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "resolving as `foo:latest` from repo",
        ));
}

#[test]
fn ambiguous_missing_everywhere_fails() {
    let (_tmp_repo, repo_path) = make_empty_repo();
    let tmp = TempDir::new().unwrap();
    let bundle = write_bundle(tmp.path(), "nope.appspec");
    appspec()
        .args([
            "bundle",
            "check",
            bundle.to_str().unwrap(),
            "--repo",
            repo_path.to_str().unwrap(),
            "--allow-unsigned",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("nope:latest not in repo index"));
}

#[test]
fn repo_alias_normalized() {
    let (_tmp_repo, repo_path) = make_repo_with_foo();
    let tmp = TempDir::new().unwrap();
    let p = tmp.path().join("bundle.appspec");
    fs::write(
        &p,
        "[bundle]\nname = \"test-bundle\"\n\n[bundle.base]\nrepo  = \"foo:1.0-1\"\nentry = \"exec /bin/sh\"\n",
    )
    .unwrap();
    appspec()
        .args([
            "bundle",
            "check",
            p.to_str().unwrap(),
            "--repo",
            repo_path.to_str().unwrap(),
            "--allow-unsigned",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "`repo = \"foo:1.0-1\"` is deprecated",
        ));
}

#[test]
fn spec_and_repo_both_fails() {
    let tmp = TempDir::new().unwrap();
    let p = tmp.path().join("bundle.appspec");
    fs::write(
        &p,
        "[bundle]\nname = \"test-bundle\"\n\n[bundle.base]\nspec  = \"./foo.appspec\"\nrepo  = \"foo:1.0-1\"\nentry = \"exec /bin/sh\"\n",
    )
    .unwrap();
    appspec()
        .args(["bundle", "check", p.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "use either `spec` or `repo`, not both",
        ));
}

#[test]
fn repo_ref_latest_and_version_only() {
    let (_tmp_repo, repo_path) = make_repo_with_foo();
    for source in ["foo", "foo:latest", "foo:1.0"] {
        let tmp = TempDir::new().unwrap();
        let bundle = write_bundle(tmp.path(), source);
        appspec()
            .args([
                "bundle",
                "check",
                bundle.to_str().unwrap(),
                "--repo",
                repo_path.to_str().unwrap(),
                "--allow-unsigned",
            ])
            .assert()
            .success();
    }
}

#[test]
fn lock_stores_canonical_ref_and_has_no_drift() {
    let (_tmp_repo, repo_path) = make_repo_with_foo();
    let tmp = TempDir::new().unwrap();
    let p = tmp.path().join("bundle.appspec");
    fs::write(
        &p,
        "[bundle]\nname = \"test-bundle\"\n\n[bundle.base]\nspec  = \"foo:1.0\"\nentry = \"exec /bin/sh\"\n",
    )
    .unwrap();

    appspec()
        .args([
            "bundle",
            "lock",
            p.to_str().unwrap(),
            "--repo",
            repo_path.to_str().unwrap(),
            "--allow-unsigned",
        ])
        .assert()
        .success();

    let lock = fs::read_to_string(tmp.path().join("bundle.lock.toml")).unwrap();
    assert!(
        lock.contains("foo:1.0-1"),
        "lock must store canonical ref:\n{lock}"
    );

    appspec()
        .args([
            "bundle",
            "check",
            p.to_str().unwrap(),
            "--repo",
            repo_path.to_str().unwrap(),
            "--allow-unsigned",
            "--locked",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("lock drift").not());
}
