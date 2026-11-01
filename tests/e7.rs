//! E7 conformance tests: [dependencies], base_digest, новые лейблы.
//!
//! Тесты проверяют только `appspec gen` — он рендерит Dockerfile без
//! внешних движков и Apptainer, поэтому моки не нужны.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn appspec() -> Command {
    Command::cargo_bin("appspec").expect("appspec binary built")
}

fn write_spec(dir: &Path, body: &str) -> PathBuf {
    let p = dir.join("test.appspec");
    fs::write(&p, body).unwrap();
    p
}

fn minimal_spec() -> &'static str {
    r#"
[package]
name    = "test"
version = "0.0.1"

[build]
base     = "alpine:3.21"
template = "alpine"

[build.apt]
extra = ["ca-certificates"]

[runtime]
"#
}

// ===========================================================================
// [dependencies]
// ===========================================================================

#[test]
fn gen_with_dependencies_emits_labels() {
    let tmp = TempDir::new().unwrap();
    let mut body = minimal_spec().to_string();
    body.push_str(
        r#"
[dependencies]
requires  = ["gstreamer >= 1.22", "qt6-base"]
provides  = ["music-player"]
conflicts = ["strawberry-nightly"]
"#,
    );
    let spec = write_spec(tmp.path(), &body);

    appspec()
        .args(["gen", spec.to_str().unwrap()])
        .assert()
        .success();

    let df = fs::read_to_string(tmp.path().join("Dockerfile.test")).unwrap();

    // serde_json::to_string — компактная форма: пробела после запятой нет.
    assert!(
        df.contains(
            r#"LABEL org.appspec.dependencies.requires="[\"gstreamer >= 1.22\",\"qt6-base\"]""#
        ),
        "missing requires label in:\n{df}"
    );
    assert!(
        df.contains(r#"LABEL org.appspec.dependencies.provides="[\"music-player\"]""#),
        "missing provides label in:\n{df}"
    );
    assert!(
        df.contains(r#"LABEL org.appspec.dependencies.conflicts="[\"strawberry-nightly\"]""#),
        "missing conflicts label in:\n{df}"
    );
}

#[test]
fn gen_without_dependencies_skips_labels() {
    let tmp = TempDir::new().unwrap();
    let spec = write_spec(tmp.path(), minimal_spec());

    appspec()
        .args(["gen", spec.to_str().unwrap()])
        .assert()
        .success();

    let df = fs::read_to_string(tmp.path().join("Dockerfile.test")).unwrap();
    assert!(
        !df.contains("org.appspec.dependencies."),
        "unexpected dependencies label in:\n{df}"
    );
    assert!(
        !df.contains("org.appspec.build.base.digest"),
        "unexpected base_digest label in:\n{df}"
    );
}

#[test]
fn comma_in_requires_rejected() {
    let tmp = TempDir::new().unwrap();
    let mut body = minimal_spec().to_string();
    body.push_str(
        r#"
[dependencies]
requires = ["foo,bar"]
"#,
    );
    let spec = write_spec(tmp.path(), &body);

    appspec()
        .args(["gen", spec.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("must not contain ','"));
}

// ===========================================================================
// [build].base_digest
// ===========================================================================

#[test]
fn base_digest_substitutes_from() {
    let tmp = TempDir::new().unwrap();
    let digest = "sha256:74df01f140d8945e4ea8d3616bb5b299ebfc1afd90d09c2e15ca7e24bed263a0";
    let body = format!(
        r#"
[package]
name    = "test"
version = "0.0.1"

[build]
base        = "alpine:3.21"
base_digest = "{digest}"
template    = "alpine"

[build.apt]
extra = ["ca-certificates"]

[runtime]
"#
    );
    let spec = write_spec(tmp.path(), &body);

    appspec()
        .args(["gen", spec.to_str().unwrap()])
        .assert()
        .success();

    let df = fs::read_to_string(tmp.path().join("Dockerfile.test")).unwrap();

    // qualify_base_image: strip tag → qualify_image → append @digest.
    // Ожидаем `FROM docker.io/library/alpine@sha256:...`.
    let expected = format!("FROM docker.io/library/alpine@{digest}");
    assert!(df.contains(&expected), "expected `{expected}` in:\n{df}");
    assert!(
        !df.contains("alpine:3.21"),
        "tag must be stripped, got:\n{df}"
    );
    assert!(
        df.contains(&format!(
            r#"LABEL org.appspec.build.base.digest="{}""#,
            digest
        )),
        "missing base.digest label in:\n{df}"
    );
}

#[test]
fn invalid_base_digest_rejected() {
    let tmp = TempDir::new().unwrap();
    let body = minimal_spec().replace(
        r#"base     = "alpine:3.21""#,
        r#"base        = "alpine:3.21"
base_digest = "sha256:zzz""#,
    );
    let spec = write_spec(tmp.path(), &body);

    appspec()
        .args(["gen", spec.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("base_digest"));
}

#[test]
fn uppercase_hex_digest_rejected() {
    let tmp = TempDir::new().unwrap();
    let body = minimal_spec().replace(
        r#"base     = "alpine:3.21""#,
        r#"base        = "alpine:3.21"
base_digest = "sha256:74DF01F140D8945E4EA8D3616BB5B299EBFC1AFD90D09C2E15CA7E24BED263A0""#,
    );
    let spec = write_spec(tmp.path(), &body);

    appspec()
        .args(["gen", spec.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("lowercase"));
}
