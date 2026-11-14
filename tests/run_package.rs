//! 0.5.0: `appspec run <package>` — version resolution + dry-run.

#![allow(dead_code)]

#[path = "../src/repo/mod.rs"]
mod repo;
pub fn log(_msg: impl AsRef<str>) {}

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::Path;
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

fn build_repo_with_pkg(root: &Path, name: &str, ver: &str, rel: &str) -> String {
    fs::create_dir_all(root.join("blobs/sha256")).unwrap();
    fs::create_dir_all(root.join("refs")).unwrap();
    fs::create_dir_all(root.join("index")).unwrap();
    fs::write(root.join("index/.keep"), b"").unwrap();

    let blob_bytes = format!("fake-sif-{}-{}-{}", name, ver, rel).into_bytes();
    let sha = sha256_hex(&blob_bytes);
    let blob_path = root.join("blobs/sha256").join(&sha[..2]).join(&sha[2..]);
    fs::create_dir_all(blob_path.parent().unwrap()).unwrap();
    fs::write(&blob_path, &blob_bytes).unwrap();

    let spec = format!(
        "[package]\nname = \"{}\"\nversion = \"{}\"\n\n[build]\nbase = \"alpine:3.21\"\ntemplate = \"alpine\"\n\n[runtime]\nsif_file = \"{}.sif\"\n",
        name, ver, name
    );
    let spec_sha = sha256_hex(spec.as_bytes());

    let ver_key = format!("{}-{}", ver, rel);
    let pkg_dir = root.join("packages").join(name).join(&ver_key);
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::write(pkg_dir.join("spec.appspec"), &spec).unwrap();

    let meta = serde_json::json!({
        "schema": 1, "name": name, "version": ver, "release": rel, "epoch": 0,
        "arch": "amd64",
        "sif_sha256": sha, "sif_size": blob_bytes.len(),
        "spec_sha256": spec_sha, "spec_size": spec.len(),
        "spec_filename": "spec.appspec",
        "generator": "appspec test", "built_at": "2026-11-14T00:00:00Z",
        "requires": [], "provides": [], "conflicts": [],
        "ipfs_cid": null, "raw_labels": {}
    });
    fs::write(
        pkg_dir.join("meta.json"),
        serde_json::to_vec_pretty(&meta).unwrap(),
    )
    .unwrap();
    ver_key
}

fn write_index(root: &Path, packages: serde_json::Value) {
    fs::create_dir_all(root).unwrap();
    let idx = serde_json::json!({
        "schema": 1, "repo": "test", "updated": "2026-11-14T00:00:00Z",
        "generator": "appspec test", "signature": "",
        "packages": packages
    });
    fs::write(
        root.join("index.json"),
        serde_json::to_vec_pretty(&idx).unwrap(),
    )
    .unwrap();
}

fn iv_json(ver: &str, rel: &str, sha: &str, size: u64) -> serde_json::Value {
    serde_json::json!({
        "epoch": 0, "version": ver, "release": rel, "arch": "amd64",
        "sif_sha256": sha, "sif_size": size,
        "spec_sha256": sha, "spec_size": 1,
        "built_at": "2026-11-14T00:00:00Z", "generator": "appspec test",
        "tags": [], "requires": [], "provides": []
    })
}

#[test]
fn run_dry_local_cache() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    let ver_key = build_repo_with_pkg(&root, "foo", "1.0.0", "1");

    let blob_bytes = "fake-sif-foo-1.0.0-1".to_string().into_bytes();
    let sha = sha256_hex(&blob_bytes);

    write_index(
        &root,
        serde_json::json!({
            "foo": {
                "versions": {
                    ver_key: iv_json("1.0.0", "1", &sha, blob_bytes.len() as u64)
                }
            }
        }),
    );

    // Pretend apptainer is a no-op (we only need render_launcher_abs, no exec).
    // Since --dry-run skips exec, no mock needed.
    appspec()
        .env("XDG_CACHE_HOME", tmp.path().join("cache"))
        .args(["run", "foo", "--root", root.to_str().unwrap(), "--dry-run"])
        .assert()
        .success()
        .stderr(predicate::str::contains("dry-run"));
}

#[test]
fn run_missing_package_errors() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    write_index(&root, serde_json::json!({}));

    appspec()
        .env("XDG_CACHE_HOME", tmp.path().join("cache"))
        .args([
            "run",
            "nonexistent",
            "--root",
            root.to_str().unwrap(),
            "--dry-run",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}
