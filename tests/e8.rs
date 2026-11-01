//! E8 conformance tests: `repo export-oci` через мок `oras` CLI.
//!
//! Мок-скрипт подменяется через `APPSPEC_ORAS`. Env-мутации
//! сериализуются через `ENV_LOCK`.

#![allow(dead_code)]

#[path = "../src/repo/mod.rs"]
mod repo;

pub fn log(_msg: impl AsRef<str>) {}

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use tempfile::TempDir;

use repo::{layout, meta, oci, util};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

// ============================================================================
// Мок `oras`
// ============================================================================

struct OrasMock {
    _tmp: TempDir,
    bin: PathBuf,
    log: PathBuf,
}

impl OrasMock {
    /// Мок: `oras push REF --artifact-type T --annotation k=v ... PATH:media`
    /// записывает `"$@"` в лог; возвращает 0.
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let bin = tmp.path().join("oras");
        let log = tmp.path().join("oras.log");
        let script = r#"#!/bin/sh
set -eu
if [ "$1" = push ]; then
    printf '%s\n' "$@" >> "$MOCK_ORAS_LOG"
    echo "Pushed $2"
    exit 0
fi
echo "mock oras: unexpected: $*" >&2
exit 1
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
        OrasMock {
            _tmp: tmp,
            bin,
            log,
        }
    }

    fn set_env(&self) {
        std::env::set_var("APPSPEC_ORAS", &self.bin);
        std::env::set_var("MOCK_ORAS_LOG", &self.log);
    }

    fn read_log(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }
}

// ============================================================================
// helpers
// ============================================================================

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

fn make_repo_with_packages(packages: &[(&str, &str, &str, &[u8])]) -> (TempDir, PathBuf) {
    // packages: [(name, version_release, arch, sif_bytes)]
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    fs::create_dir_all(root.join("blobs/sha256")).unwrap();
    fs::create_dir_all(root.join("packages")).unwrap();
    fs::create_dir_all(root.join("refs")).unwrap();
    fs::create_dir_all(root.join("index")).unwrap();
    fs::write(root.join("index/.keep"), b"").unwrap();

    let mut pkg_map: BTreeMap<String, meta::IndexPackage> = BTreeMap::new();

    for (name, ver_key, arch, sif_bytes) in packages {
        let sha = sha256_hex(sif_bytes);
        let blob = util::blob_path(&root, &sha);
        fs::create_dir_all(blob.parent().unwrap()).unwrap();
        fs::write(&blob, sif_bytes).unwrap();

        let (ver, rel) = ver_key.split_once('-').unwrap_or((ver_key, "1"));
        let v = meta::IndexVersion {
            epoch: 0,
            version: ver.to_string(),
            release: rel.to_string(),
            arch: arch.to_string(),
            sif_sha256: sha.clone(),
            sif_size: sif_bytes.len() as u64,
            spec_sha256: sha256_hex(b"spec"),
            spec_size: 4,
            built_at: "2026-10-28T00:00:00Z".into(),
            generator: "appspec test".into(),
            base_digest: Some("sha256:base".into()),
            tags: vec![],
            requires: vec![],
            provides: vec![],
            ipfs_cid: None,
        };
        pkg_map
            .entry(name.to_string())
            .or_insert_with(|| meta::IndexPackage {
                summary: Some(format!("summary of {}", name)),
                license: Some("MIT".into()),
                vendor: None,
                url: None,
                versions: BTreeMap::new(),
            })
            .versions
            .insert(ver_key.to_string(), v);

        // packages/<name>/<ver_key>/{meta.json,spec.appspec}
        let pd = root.join("packages").join(name).join(ver_key);
        fs::create_dir_all(&pd).unwrap();
        fs::write(pd.join("spec.appspec"), b"spec").unwrap();
        let meta_v1 = meta::MetaV1 {
            schema: 1,
            name: name.to_string(),
            version: ver.to_string(),
            release: rel.to_string(),
            epoch: 0,
            arch: arch.to_string(),
            summary: Some(format!("summary of {}", name)),
            description: None,
            license: Some("MIT".into()),
            url: None,
            vendor: None,
            packager: None,
            sif_sha256: sha,
            sif_size: sif_bytes.len() as u64,
            spec_sha256: sha256_hex(b"spec"),
            spec_size: 4,
            spec_filename: "spec.appspec".into(),
            generator: "appspec test".into(),
            built_at: "2026-10-28T00:00:00Z".into(),
            base_digest: Some("sha256:base".into()),
            requires: vec![],
            provides: vec![],
            conflicts: vec![],
            ipfs_cid: None,
            raw_labels: Default::default(),
        };
        fs::write(
            pd.join("meta.json"),
            serde_json::to_vec_pretty(&meta_v1).unwrap(),
        )
        .unwrap();
    }

    let cfg = layout::Config {
        repo: layout::RepoMeta {
            name: "test/e8".into(),
            description: String::new(),
            created_at: util::now_iso8601(),
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

    let idx = meta::IndexV1 {
        schema: 1,
        repo: "test/e8".into(),
        updated: "2026-10-28T00:00:00Z".into(),
        generator: "appspec test".into(),
        signature: String::new(),
        packages: pkg_map,
    };
    fs::write(
        root.join("index.json"),
        serde_json::to_vec_pretty(&idx).unwrap(),
    )
    .unwrap();

    (tmp, root)
}

// ============================================================================
// Тесты
// ============================================================================

#[test]
fn export_oci_dry_run_prints_commands_and_does_not_invoke_oras() {
    let _g = lock_env();
    let mock = OrasMock::new();
    mock.set_env();
    let (_tmp, root) = make_repo_with_packages(&[("demo", "0.0.1-1", "amd64", b"sif-demo-1")]);

    let root_obj = layout::RepoRoot::open(&root).unwrap();
    let out = oci::export_oci(
        &root_obj,
        oci::OciExportOpts {
            target: "appspec-index".into(),
            registry: Some("ghcr.io/user".into()),
            dry_run: true,
        },
    )
    .unwrap();

    assert!(out.dry_run);
    assert_eq!(out.index_ref, "ghcr.io/user/appspec-index:latest");
    // index + 1 package
    assert_eq!(out.pushed.len(), 2);
    // мок не должен вызываться
    assert_eq!(mock.read_log(), "");
}

#[test]
fn export_oci_pushes_index_and_packages() {
    let _g = lock_env();
    let mock = OrasMock::new();
    mock.set_env();
    let (_tmp, root) = make_repo_with_packages(&[
        ("demo", "0.0.1-1", "amd64", b"sif-demo-1"),
        ("demo", "0.0.2-1", "amd64", b"sif-demo-2"),
        ("other", "1.0.0-1", "amd64", b"sif-other-1"),
    ]);

    let root_obj = layout::RepoRoot::open(&root).unwrap();
    let out = oci::export_oci(
        &root_obj,
        oci::OciExportOpts {
            target: "appspec-index".into(),
            registry: Some("ghcr.io/user".into()),
            dry_run: false,
        },
    )
    .unwrap();

    assert_eq!(out.index_ref, "ghcr.io/user/appspec-index:latest");
    // index + 3 (demo-1, demo-2, other-1)
    assert_eq!(out.pushed.len(), 4);

    let log = mock.read_log();
    assert!(
        log.contains("ghcr.io/user/appspec-index:latest"),
        "missing index push in log:\n{log}"
    );
    assert!(
        log.contains("ghcr.io/user/demo:0.0.1-1"),
        "missing demo:0.0.1-1 in log:\n{log}"
    );
    assert!(
        log.contains("ghcr.io/user/demo:0.0.2-1"),
        "missing demo:0.0.2-1 in log:\n{log}"
    );
    assert!(
        log.contains("ghcr.io/user/other:1.0.0-1"),
        "missing other:1.0.0-1 in log:\n{log}"
    );
    assert!(
        log.contains("application/vnd.appspec.index.v1+json"),
        "index must be artifact-typed:\n{log}"
    );
    assert!(
        log.contains("application/vnd.appspec.sif.v1+json"),
        "sif must be artifact-typed:\n{log}"
    );
    assert!(
        log.contains("org.appspec.sif_sha256="),
        "sif push must have org.appspec.sif_sha256 annotation:\n{log}"
    );
}

#[test]
fn export_oci_without_registry_uses_target_as_prefix() {
    let _g = lock_env();
    let mock = OrasMock::new();
    mock.set_env();
    let (_tmp, root) = make_repo_with_packages(&[("demo", "0.0.1-1", "amd64", b"sif-demo-1")]);

    let root_obj = layout::RepoRoot::open(&root).unwrap();
    let out = oci::export_oci(
        &root_obj,
        oci::OciExportOpts {
            target: "myregistry.example.com".into(),
            registry: None,
            dry_run: false,
        },
    )
    .unwrap();

    assert_eq!(out.index_ref, "myregistry.example.com/appspec-index:latest");
    let log = mock.read_log();
    assert!(log.contains("myregistry.example.com/appspec-index:latest"));
    assert!(log.contains("myregistry.example.com/demo:0.0.1-1"));
}

#[test]
fn export_oci_skips_missing_blob_with_warning() {
    let _g = lock_env();
    let mock = OrasMock::new();
    mock.set_env();
    let (_tmp, root) = make_repo_with_packages(&[("demo", "0.0.1-1", "amd64", b"sif-demo-1")]);

    // Удалим blob — должен быть skip, не ошибка.
    let idx: meta::IndexV1 =
        serde_json::from_str(&fs::read_to_string(root.join("index.json")).unwrap()).unwrap();
    let sha = &idx.packages["demo"].versions["0.0.1-1"].sif_sha256;
    fs::remove_file(util::blob_path(&root, sha)).unwrap();

    let root_obj = layout::RepoRoot::open(&root).unwrap();
    let out = oci::export_oci(
        &root_obj,
        oci::OciExportOpts {
            target: "appspec-index".into(),
            registry: Some("r.example".into()),
            dry_run: false,
        },
    )
    .unwrap();

    // index всё равно пушится
    assert_eq!(out.pushed.len(), 1);
    assert!(out.pushed[0].contains("appspec-index:latest"));
}

#[test]
fn export_oci_fails_on_sha_mismatch() {
    let _g = lock_env();
    let mock = OrasMock::new();
    mock.set_env();
    let (_tmp, root) = make_repo_with_packages(&[("demo", "0.0.1-1", "amd64", b"sif-demo-1")]);

    // Испортим blob, сохранив путь и имя (sha не сойдётся).
    let idx: meta::IndexV1 =
        serde_json::from_str(&fs::read_to_string(root.join("index.json")).unwrap()).unwrap();
    let sha = &idx.packages["demo"].versions["0.0.1-1"].sif_sha256;
    fs::write(util::blob_path(&root, sha), b"corrupted").unwrap();

    let root_obj = layout::RepoRoot::open(&root).unwrap();
    let err = oci::export_oci(
        &root_obj,
        oci::OciExportOpts {
            target: "appspec-index".into(),
            registry: Some("r.example".into()),
            dry_run: false,
        },
    )
    .unwrap_err();

    assert!(
        format!("{err:#}").contains("sha256 mismatch"),
        "expected sha256 mismatch, got: {err:#}"
    );
}

#[test]
fn export_oci_fails_when_oras_missing_and_not_dry_run() {
    let _g = lock_env();
    // APPSPEC_ORAS указывает на несуществующий файл → which_oras() == false
    std::env::set_var("APPSPEC_ORAS", "/nonexistent/oras-for-test");
    let (_tmp, root) = make_repo_with_packages(&[("demo", "0.0.1-1", "amd64", b"sif-demo-1")]);

    let root_obj = layout::RepoRoot::open(&root).unwrap();
    let err = oci::export_oci(
        &root_obj,
        oci::OciExportOpts {
            target: "appspec-index".into(),
            registry: Some("r.example".into()),
            dry_run: false,
        },
    )
    .unwrap_err();
    assert!(
        format!("{err:#}").contains("oras"),
        "expected oras-not-found, got: {err:#}"
    );
}
