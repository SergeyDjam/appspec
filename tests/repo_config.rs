//! E2 (Phase 1) conformance tests: server/client config split,
//! token lifecycle, migrate-config, `--profile` в pull/push.
//!
//! Модули `src/repo/*` не импортируются — только CLI через `assert_cmd`.
//! Env-переменные не текут между тестами: каждый `assert_cmd` стартует
//! subprocess с полным env.

use assert_cmd::Command as CliCommand;
use predicates::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn appspec() -> CliCommand {
    CliCommand::cargo_bin("appspec").expect("appspec binary built")
}

/// Настроить изолированный `$XDG_CONFIG_HOME` — чтобы discovery не нашёл
/// настоящий `~/.config/appspec/`.
fn isolated(tmp: &TempDir) -> PathBuf {
    let xdg = tmp.path().join("xdg");
    fs::create_dir_all(&xdg).unwrap();
    xdg
}

fn write_empty_index(root: &Path, repo_name: &str) {
    fs::create_dir_all(root.join("blobs/sha256")).unwrap();
    fs::create_dir_all(root.join("packages")).unwrap();
    fs::create_dir_all(root.join("refs")).unwrap();
    fs::create_dir_all(root.join("index")).unwrap();
    fs::write(root.join("index/.keep"), b"").unwrap();
    let index = serde_json::json!({
        "schema": 1,
        "repo": repo_name,
        "updated": "2026-11-09T00:00:00Z",
        "generator": "appspec test",
        "signature": "",
        "packages": {}
    });
    fs::write(
        root.join("index.json"),
        serde_json::to_vec_pretty(&index).unwrap(),
    )
    .unwrap();
}

// ===========================================================================
// init / server init / token gen / token ls / token check
// ===========================================================================

#[test]
fn init_does_not_create_root_config() {
    let tmp = TempDir::new().unwrap();
    let xdg = isolated(&tmp);
    let root = tmp.path().join("repo");

    appspec()
        .env("XDG_CONFIG_HOME", &xdg)
        .args(["repo", "init", root.to_str().unwrap()])
        .assert()
        .success();

    assert!(root.join("index.json").is_file());
    assert!(
        !root.join("config.toml").exists(),
        "init must not create <root>/config.toml"
    );
}

#[test]
fn init_with_server_config_creates_external_config() {
    let tmp = TempDir::new().unwrap();
    let xdg = isolated(&tmp);
    let root = tmp.path().join("repo");
    let sc = tmp.path().join("repo-server.toml");

    appspec()
        .env("XDG_CONFIG_HOME", &xdg)
        .args([
            "repo",
            "init",
            root.to_str().unwrap(),
            "--name",
            "corp/stable",
            "--server-config",
            sc.to_str().unwrap(),
        ])
        .assert()
        .success();

    assert!(root.join("index.json").is_file());
    assert!(!root.join("config.toml").exists());
    assert!(sc.is_file());

    let text = fs::read_to_string(&sc).unwrap();
    assert!(text.contains(&format!("root = {:?}", root.display().to_string())));
    assert!(text.contains(r#"name = "corp/stable""#));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&sc).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "server config must be 0600");
    }
}

#[test]
fn token_gen_ls_check_rm() {
    let tmp = TempDir::new().unwrap();
    let xdg = isolated(&tmp);
    let root = tmp.path().join("repo");
    let sc = tmp.path().join("repo-server.toml");

    appspec()
        .env("XDG_CONFIG_HOME", &xdg)
        .args([
            "repo",
            "init",
            root.to_str().unwrap(),
            "--server-config",
            sc.to_str().unwrap(),
        ])
        .assert()
        .success();

    // Gen
    let out = appspec()
        .env("XDG_CONFIG_HOME", &xdg)
        .args([
            "repo",
            "token",
            "gen",
            "ci",
            "--server-config",
            sc.to_str().unwrap(),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let s = String::from_utf8_lossy(&out);
    let raw = s
        .lines()
        .find_map(|l| l.strip_prefix("token ci: "))
        .expect("no `token ci:` line")
        .trim()
        .to_string();
    assert_eq!(raw.len(), 64, "token must be 32 bytes hex");

    // ls
    appspec()
        .env("XDG_CONFIG_HOME", &xdg)
        .args([
            "repo",
            "token",
            "ls",
            "--server-config",
            sc.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("ci"));

    // check — правильный токен
    appspec()
        .env("XDG_CONFIG_HOME", &xdg)
        .args([
            "repo",
            "token",
            "check",
            "ci",
            "--server-config",
            sc.to_str().unwrap(),
            "--value",
            &raw,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("ok"));

    // check — неправильный токен → exit 1
    appspec()
        .env("XDG_CONFIG_HOME", &xdg)
        .args([
            "repo",
            "token",
            "check",
            "ci",
            "--server-config",
            sc.to_str().unwrap(),
            "--value",
            "wrong",
        ])
        .assert()
        .code(1);

    // rm
    appspec()
        .env("XDG_CONFIG_HOME", &xdg)
        .args([
            "repo",
            "token",
            "rm",
            "ci",
            "--server-config",
            sc.to_str().unwrap(),
        ])
        .assert()
        .success();

    appspec()
        .env("XDG_CONFIG_HOME", &xdg)
        .args([
            "repo",
            "token",
            "ls",
            "--server-config",
            sc.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("no tokens"));
}

#[test]
fn token_gen_write_file_creates_secret() {
    let tmp = TempDir::new().unwrap();
    let xdg = isolated(&tmp);
    let root = tmp.path().join("repo");
    let sc = tmp.path().join("repo-server.toml");
    let secret = tmp.path().join("ci.token");

    appspec()
        .env("XDG_CONFIG_HOME", &xdg)
        .args([
            "repo",
            "init",
            root.to_str().unwrap(),
            "--server-config",
            sc.to_str().unwrap(),
        ])
        .assert()
        .success();

    // --write-file — токен не печатается в stdout
    let out = appspec()
        .env("XDG_CONFIG_HOME", &xdg)
        .args([
            "repo",
            "token",
            "gen",
            "ci",
            "--server-config",
            sc.to_str().unwrap(),
            "--write-file",
            secret.to_str().unwrap(),
            "--mode",
            "400",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(
        String::from_utf8_lossy(&out).trim().is_empty(),
        "stdout must be empty with --write-file"
    );

    let token = fs::read_to_string(&secret).unwrap();
    assert_eq!(token.trim().len(), 64);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let m = fs::metadata(&secret).unwrap().permissions().mode() & 0o777;
        assert_eq!(m, 0o400);
    }

    // check через --file
    appspec()
        .env("XDG_CONFIG_HOME", &xdg)
        .args([
            "repo",
            "token",
            "check",
            "ci",
            "--server-config",
            sc.to_str().unwrap(),
            "--file",
            secret.to_str().unwrap(),
        ])
        .assert()
        .success();
}

// ===========================================================================
// migrate-config
// ===========================================================================

#[test]
fn migrate_config_renames_legacy() {
    let tmp = TempDir::new().unwrap();
    let xdg = isolated(&tmp);
    let root = tmp.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    write_empty_index(&root, "legacy/repo");

    // Legacy config.toml
    let legacy = r#"[repo]
name = "legacy/repo"
description = "old"
created_at = "2026-10-28T19:36:15Z"
generator = "appspec 0.3.4"
layout = 1

[policy]
immutable_versions = true
allow_force = false
auto_tag_latest = "semver"

[signing]

[server]

[ipfs]
"#;
    fs::write(root.join("config.toml"), legacy).unwrap();

    // Миграция
    appspec()
        .env("XDG_CONFIG_HOME", &xdg)
        .args(["repo", "migrate-config", root.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("migrated:"));

    // Новый config в XDG
    let sc = xdg.join("appspec/repo-server.toml");
    assert!(
        sc.is_file(),
        "server config not created at {}",
        sc.display()
    );
    let text = fs::read_to_string(&sc).unwrap();
    assert!(text.contains(&format!("root = {:?}", root.display().to_string())));
    assert!(text.contains(r#"name = "legacy/repo""#));

    // Legacy переименован
    assert!(!root.join("config.toml").exists());
    assert!(root.join(".config.toml.migrated").exists());

    // repo ls теперь работает без warning'а о legacy
    appspec()
        .env("XDG_CONFIG_HOME", &xdg)
        .args(["repo", "ls", "--root", root.to_str().unwrap()])
        .assert()
        .success();
}

#[test]
fn migrate_config_without_legacy_fails() {
    let tmp = TempDir::new().unwrap();
    let xdg = isolated(&tmp);
    let root = tmp.path().join("repo");
    fs::create_dir_all(&root).unwrap();

    appspec()
        .env("XDG_CONFIG_HOME", &xdg)
        .args(["repo", "migrate-config", root.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("nothing to migrate"));
}

// ===========================================================================
// client init / ls / pull / push через профиль
// ===========================================================================

#[test]
fn client_init_creates_profile() {
    let tmp = TempDir::new().unwrap();
    let xdg = isolated(&tmp);

    appspec()
        .env("XDG_CONFIG_HOME", &xdg)
        .args([
            "repo",
            "client",
            "init",
            "--profile",
            "corp",
            "--source-url",
            "http://example.invalid:8080",
            "--token-env",
            "APPSPEC_REPO_TOKEN",
            "--default",
        ])
        .assert()
        .success();

    let cc = xdg.join("appspec/repos.toml");
    assert!(cc.is_file());
    let text = fs::read_to_string(&cc).unwrap();
    assert!(text.contains(r#"name = "corp""#));
    assert!(text.contains(r#"url = "http://example.invalid:8080""#));
    assert!(text.contains(r#"token = "env:APPSPEC_REPO_TOKEN""#));
    assert!(text.contains(r#"default = "corp""#));

    appspec()
        .env("XDG_CONFIG_HOME", &xdg)
        .args(["repo", "client", "ls"])
        .assert()
        .success()
        .stdout(predicate::str::contains("corp"))
        .stdout(predicate::str::contains("http://example.invalid:8080"));
}

#[test]
fn client_init_rejects_two_sources() {
    let tmp = TempDir::new().unwrap();
    let xdg = isolated(&tmp);
    appspec()
        .env("XDG_CONFIG_HOME", &xdg)
        .args([
            "repo",
            "client",
            "init",
            "--profile",
            "x",
            "--source-path",
            "/a",
            "--source-url",
            "http://h",
        ])
        .assert()
        .failure();
}
