//! E9 conformance tests: `pull --from oci://` через мок `oras` CLI.

#![allow(dead_code)]

#[path = "../src/repo/mod.rs"]
mod repo;

pub fn log(_msg: impl AsRef<str>) {}

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use tempfile::TempDir;

use repo::{layout, meta, oci, pull, util};

static ENV_LOCK: Mutex<()> = Mutex::new(());
fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

// ============================================================================
// Мок `oras`: pull по ref
// ============================================================================
//
// Мок читает JSON-карту `<ref> → <path>` из $MOCK_ORAS_MAP и копирует
// указанный файл в `<out>`. Это позволяет тестам подсовывать
// index.json и SIF под конкретные ref'ы.
//
// Мок-скрипт:
//   oras pull <ref> -o <dir>
//     path = jq/cat из $MOCK_ORAS_MAP по <ref>
//     cp "$path" "<dir>/$(basename path)"

struct OrasMock {
    _tmp: TempDir,
    bin: PathBuf,
    map: PathBuf,
}

impl OrasMock {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let bin = tmp.path().join("oras");
        let map = tmp.path().join("map.txt");   // <ref> <path>, по строке

        // Чистый POSIX sh. Без python3, без jq, без bash-специфики.
        // Пути в тестах TempDir не содержат пробелов; `read -r r p`
        // кладёт остаток строки в p без потери пробелов внутри.
        let script = r#"#!/bin/sh
set -eu
if [ "$1" != pull ]; then
    echo "mock oras: expected pull, got $1" >&2
    exit 1
fi
ref="$2"
out=""
shift 2
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o) out="$2"; shift 2 ;;
        *)  shift ;;
    esac
done
[ -n "$out" ] || { echo "mock oras: -o missing" >&2; exit 1; }
mkdir -p "$out"

match=""
if [ -f "$MOCK_ORAS_MAP" ]; then
    while IFS=' ' read -r r p; do
        [ -z "$r" ] && continue
        if [ "$r" = "$ref" ] && [ -n "$p" ]; then
            match="$p"
            break
        fi
    done < "$MOCK_ORAS_MAP"
fi
if [ -z "$match" ]; then
    echo "mock oras: no mapping for $ref" >&2
    exit 1
fi
if [ ! -f "$match" ]; then
    echo "mock oras: mapped file missing: $match" >&2
    exit 1
fi
cp "$match" "$out/$(basename "$match")"
"#;

        fs::write(&bin, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut m = fs::metadata(&bin).unwrap().permissions();
            m.set_mode(0o755);
            fs::set_permissions(&bin, m).unwrap();
        }
        fs::write(&map, b"").unwrap();  // пустая карта
        OrasMock { _tmp: tmp, bin, map }
    }

    fn set_env(&self) {
        std::env::set_var("APPSPEC_ORAS", &self.bin);
        std::env::set_var("MOCK_ORAS_MAP", &self.map);
    }

    fn register(&self, ref_str: &str, path: &std::path::Path) {
        use std::io::Write;
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.map)
            .unwrap();
        // Рефы не содержат пробелов (ghcr.io/user/repo:tag).
        // Пути TempDir тоже, но даже если бы содержали — `read -r r p`
        // съел бы только первый пробел.
        writeln!(f, "{} {}", ref_str, path.display()).unwrap();
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

fn make_min_repo() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    fs::create_dir_all(root.join("blobs/sha256")).unwrap();
    fs::create_dir_all(root.join("packages")).unwrap();
    fs::create_dir_all(root.join("refs")).unwrap();
    fs::create_dir_all(root.join("index")).unwrap();
    fs::write(root.join("index/.keep"), b"").unwrap();

    let cfg = layout::Config {
        repo: layout::RepoMeta {
            name: "test/e9".into(),
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
        repo: "test/e9".into(),
        updated: util::now_iso8601(),
        generator: "appspec test".into(),
        signature: String::new(),
        packages: BTreeMap::new(),
    };
    fs::write(
        root.join("index.json"),
        serde_json::to_vec_pretty(&idx).unwrap(),
    )
    .unwrap();
    (tmp, root)
}

fn make_index(sha: &str) -> String {
    let mut versions = BTreeMap::new();
    versions.insert(
        "0.0.1-1".to_string(),
        meta::IndexVersion {
            epoch: 0,
            version: "0.0.1".into(),
            release: "1".into(),
            arch: "amd64".into(),
            sif_sha256: sha.to_string(),
            sif_size: 100,
            spec_sha256: sha256_hex(b"spec"),
            spec_size: 4,
            built_at: "2026-11-01T00:00:00Z".into(),
            generator: "appspec test".into(),
            base_digest: None,
            tags: vec!["latest".into()],
            requires: vec![],
            provides: vec![],
            ipfs_cid: None,
        },
    );
    let mut packages = BTreeMap::new();
    packages.insert(
        "demo".into(),
        meta::IndexPackage {
            summary: None,
            license: None,
            vendor: None,
            url: None,
            versions,
        },
    );
    serde_json::to_string(&meta::IndexV1 {
        schema: 1,
        repo: "test/e9".into(),
        updated: "2026-11-01T00:00:00Z".into(),
        generator: "appspec test".into(),
        signature: String::new(),
        packages,
    })
    .unwrap()
}

// ============================================================================
// Тесты
// ============================================================================

#[test]
fn parse_oci_ref_basic() {
    let (repo, tag) = oci::parse_oci_ref("oci://ghcr.io/user/appspec-index:latest").unwrap();
    assert_eq!(repo, "ghcr.io/user/appspec-index");
    assert_eq!(tag, "latest");
}

#[test]
fn parse_oci_ref_default_tag() {
    let (repo, tag) = oci::parse_oci_ref("oci://ghcr.io/user/appspec-index").unwrap();
    assert_eq!(repo, "ghcr.io/user/appspec-index");
    assert_eq!(tag, "latest");
}

#[test]
fn parse_oci_ref_with_port() {
    let (repo, tag) = oci::parse_oci_ref("oci://registry.local:5000/appspec-index:v1").unwrap();
    assert_eq!(repo, "registry.local:5000/appspec-index");
    assert_eq!(tag, "v1");
}

#[test]
fn split_oci_index_ref_extracts_base() {
    let (base, idx) = oci::split_oci_index_ref("oci://ghcr.io/user/appspec-index:latest").unwrap();
    assert_eq!(base, "ghcr.io/user");
    assert_eq!(idx, "appspec-index");
}

#[test]
fn pull_from_oci_writes_blob() {
    let _g = lock_env();
    let mock = OrasMock::new();
    mock.set_env();
    let (_tmp, root) = make_min_repo();

    let blob_bytes = b"sif-from-oci-pull";
    let sha = sha256_hex(blob_bytes);

    // index.json и SIF — два отдельных файла в tempdir
    let idx_file = mock._tmp.path().join("index.json");
    fs::write(&idx_file, make_index(&sha)).unwrap();
    let sif_file = mock._tmp.path().join("demo.sif");
    fs::write(&sif_file, blob_bytes).unwrap();

    mock.register("ghcr.io/user/appspec-index:latest", &idx_file);
    mock.register("ghcr.io/user/demo:0.0.1-1", &sif_file);

    let root_obj = layout::RepoRoot::open(&root).unwrap();
    let opts = pull::PullOpts {
        from: Some("oci://ghcr.io/user/appspec-index:latest".into()),
        via: None,
        spec_only: false,
        sif_out: None,
        trusted_key: None,
        allow_unsigned: true,
    };
    pull::pull(&root_obj, "demo:0.0.1-1", opts).unwrap();

    let blob_path = util::blob_path(&root, &sha);
    assert!(blob_path.is_file(), "blob must be written");
    assert_eq!(fs::read(&blob_path).unwrap(), blob_bytes);
}

#[test]
fn pull_from_oci_sha_mismatch_fails() {
    let _g = lock_env();
    let mock = OrasMock::new();
    mock.set_env();
    let (_tmp, root) = make_min_repo();

    let real_bytes = b"real-sif-content";
    let wrong_sha = sha256_hex(b"something-else"); // ≠ real_bytes

    let idx_file = mock._tmp.path().join("index.json");
    fs::write(&idx_file, make_index(&wrong_sha)).unwrap();
    let sif_file = mock._tmp.path().join("demo.sif");
    fs::write(&sif_file, real_bytes).unwrap();

    mock.register("ghcr.io/user/appspec-index:latest", &idx_file);
    mock.register("ghcr.io/user/demo:0.0.1-1", &sif_file);

    let root_obj = layout::RepoRoot::open(&root).unwrap();
    let opts = pull::PullOpts {
        from: Some("oci://ghcr.io/user/appspec-index:latest".into()),
        via: None,
        spec_only: false,
        sif_out: None,
        trusted_key: None,
        allow_unsigned: true,
    };
    let err = pull::pull(&root_obj, "demo:0.0.1-1", opts).unwrap_err();
    assert!(
        format!("{err:#}").contains("sha256 mismatch"),
        "expected sha mismatch, got: {err:#}"
    );
}

#[test]
fn pull_from_oci_missing_package_errors() {
    let _g = lock_env();
    let mock = OrasMock::new();
    mock.set_env();
    let (_tmp, root) = make_min_repo();

    let idx_file = mock._tmp.path().join("index.json");
    fs::write(&idx_file, make_index(&sha256_hex(b"x"))).unwrap();
    mock.register("ghcr.io/user/appspec-index:latest", &idx_file);

    let root_obj = layout::RepoRoot::open(&root).unwrap();
    let opts = pull::PullOpts {
        from: Some("oci://ghcr.io/user/appspec-index:latest".into()),
        via: None,
        spec_only: false,
        sif_out: None,
        trusted_key: None,
        allow_unsigned: true,
    };
    let err = pull::pull(&root_obj, "nope:0.0.1-1", opts).unwrap_err();
    assert!(
        format!("{err:#}").contains("not in OCI index"),
        "expected not-in-index, got: {err:#}"
    );
}

#[test]
fn pull_from_oci_sif_out_copies_without_meta() {
    let _g = lock_env();
    let mock = OrasMock::new();
    mock.set_env();
    let (_tmp, root) = make_min_repo();

    let blob_bytes = b"sif-out-test";
    let sha = sha256_hex(blob_bytes);
    let idx_file = mock._tmp.path().join("index.json");
    fs::write(&idx_file, make_index(&sha)).unwrap();
    let sif_file = mock._tmp.path().join("demo.sif");
    fs::write(&sif_file, blob_bytes).unwrap();

    mock.register("ghcr.io/user/appspec-index:latest", &idx_file);
    mock.register("ghcr.io/user/demo:0.0.1-1", &sif_file);

    let out = mock._tmp.path().join("out.sif");
    let root_obj = layout::RepoRoot::open(&root).unwrap();
    let opts = pull::PullOpts {
        from: Some("oci://ghcr.io/user/appspec-index:latest".into()),
        via: None,
        spec_only: false,
        sif_out: Some(out.clone()),
        trusted_key: None,
        allow_unsigned: true,
    };
    pull::pull(&root_obj, "demo:0.0.1-1", opts).unwrap();

    assert!(out.is_file());
    assert_eq!(fs::read(&out).unwrap(), blob_bytes);
    // в локальный repo не пишем
    assert!(!util::blob_path(&root, &sha).is_file());
}

#[test]
fn pull_from_oci_oras_missing_errors() {
    let _g = lock_env();
    std::env::set_var("APPSPEC_ORAS", "/nonexistent/oras-for-test");
    let (_tmp, root) = make_min_repo();
    let root_obj = layout::RepoRoot::open(&root).unwrap();
    let opts = pull::PullOpts {
        from: Some("oci://ghcr.io/user/appspec-index:latest".into()),
        via: None,
        spec_only: false,
        sif_out: None,
        trusted_key: None,
        allow_unsigned: true,
    };
    let err = pull::pull(&root_obj, "demo:0.0.1-1", opts).unwrap_err();
    assert!(
        format!("{err:#}").contains("oras"),
        "expected oras error, got: {err:#}"
    );
}
