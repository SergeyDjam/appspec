//! `repo export-oci`: экспорт index.json и SIF'ов в OCI-совместимый registry
//! через `oras` CLI.
//!
//! Схема (§6.13 ТЗ):
//!   * `<base>/<target>:latest` — artifact manifest, `artifactType =
//!     application/vnd.appspec.index.v1+json`, единственный blob — index.json.
//!   * `<base>/<name>:<version>-<release>` — artifact manifest,
//!     `artifactType = application/vnd.appspec.sif.v1+json`, единственный
//!     blob — SIF.
//!
//! Где `<base>` — `--registry` (если задан) или `TARGET`. Если `--registry`
//! не задан, `TARGET` трактуется как registry-префикс, а индекс
//! публикуется как `<TARGET>/appspec-index:latest`.
//!
//! Бинарь: `$APPSPEC_ORAS` (для тестов) или `oras`.

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

use super::layout::RepoRoot;
use super::meta::IndexV1;
use super::util::{blob_path, sha256_file};

pub const INDEX_ARTIFACT_TYPE: &str = "application/vnd.appspec.index.v1+json";
pub const SIF_ARTIFACT_TYPE: &str   = "application/vnd.appspec.sif.v1+json";
pub const INDEX_MEDIA_TYPE: &str    = "application/json";
pub const SIF_MEDIA_TYPE: &str      = "application/vnd.sylabs.sif";

pub fn oras_bin() -> String {
    std::env::var("APPSPEC_ORAS").unwrap_or_else(|_| "oras".to_string())
}

pub fn which_oras() -> bool {
    let bin = oras_bin();
    if bin.contains('/') {
        return Path::new(&bin).is_file();
    }
    std::env::var_os("PATH").is_some_and(|p| {
        std::env::split_paths(&p).any(|d| d.join(&bin).is_file())
    })
}

pub struct OciExportOpts {
    pub target: String,
    pub registry: Option<String>,
    pub dry_run: bool,
}

#[derive(Debug)]
pub struct OciExportOutcome {
    pub index_ref: String,
    pub pushed: Vec<String>,
    pub dry_run: bool,
}

pub fn export_oci(root: &RepoRoot, opts: OciExportOpts) -> Result<OciExportOutcome> {
    let target = opts.target.trim().to_string();
    if target.is_empty() {
        bail!("TARGET must not be empty");
    }
    if !opts.dry_run && !which_oras() {
        bail!(
            "`{}` not found in PATH; install oras (https://oras.land/) or use --dry-run",
            oras_bin()
        );
    }

    let index_text = std::fs::read_to_string(root.index_path())
        .with_context(|| format!("reading {}", root.index_path().display()))?;
    let index: IndexV1 = serde_json::from_str(&index_text)
        .context("parsing local index.json")?;

    let (base, index_repo) = match opts.registry.as_deref() {
        Some(r) => {
            let b = r.trim_end_matches('/').to_string();
            (b.clone(), format!("{}/{}", b, target))
        }
        None => {
            // TARGET трактуется как registry-префикс, индекс публикуется
            // под `<TARGET>/appspec-index:latest`.
            (target.clone(), format!("{}/appspec-index", target))
        }
    };
    let index_ref = format!("{}:latest", index_repo);

    let mut pushed = Vec::new();

    // 1. index.json как artifact
    let total_versions: usize = index.packages.values().map(|p| p.versions.len()).sum();
    let mut index_args = vec![
        "push".to_string(),
        index_ref.clone(),
        "--artifact-type".to_string(),
        INDEX_ARTIFACT_TYPE.to_string(),
    ];
    push_annotation(&mut index_args, "org.opencontainers.image.title", "appspec-index");
    push_annotation(&mut index_args, "org.opencontainers.image.description",
        &format!("appspec index for {}", index.repo));
    push_annotation(&mut index_args, "org.appspec.repo", &index.repo);
    push_annotation(&mut index_args, "org.appspec.updated", &index.updated);
    push_annotation(&mut index_args, "org.appspec.generator", &index.generator);
    push_annotation(&mut index_args, "org.appspec.package_count",
        &format!("{}", total_versions));
    index_args.push(format!("{}:{}", root.index_path().display(), INDEX_MEDIA_TYPE));

    run_oras(&index_args, opts.dry_run)?;
    pushed.push(index_ref.clone());

    // 2. SIF'ы
    for (name, pkg) in &index.packages {
        for (ver_key, v) in &pkg.versions {
            let blob = blob_path(&root.path, &v.sif_sha256);
            if !blob.is_file() {
                eprintln!(
                    "warning: skip {}:{} — blob {} missing",
                    name, ver_key, blob.display()
                );
                continue;
            }

            // Проверка целостности перед push: экспортируем только то,
            // что сами можем прочитать без ошибок. Это ловит битые
            // локальные репозитории до того, как oras зальёт мусор.
            let actual = sha256_file(&blob)?;
            if actual != v.sif_sha256 {
                bail!(
                    "blob {} sha256 mismatch: expected {}, got {}",
                    blob.display(), v.sif_sha256, actual
                );
            }

            let pkg_ref = format!("{}/{}:{}", base, name, ver_key);
            let mut args = vec![
                "push".to_string(),
                pkg_ref.clone(),
                "--artifact-type".to_string(),
                SIF_ARTIFACT_TYPE.to_string(),
            ];
            push_annotation(&mut args, "org.opencontainers.image.title", name);
            push_annotation(&mut args, "org.opencontainers.image.version", ver_key);
            push_annotation(&mut args, "org.appspec.sif_sha256", &v.sif_sha256);
            push_annotation(&mut args, "org.appspec.spec_sha256", &v.spec_sha256);
            push_annotation(&mut args, "org.appspec.arch", &v.arch);
            push_annotation(&mut args, "org.appspec.epoch", &v.epoch.to_string());
            if let Some(s) = &pkg.summary {
                push_annotation(&mut args, "org.opencontainers.image.description", s);
            }
            if let Some(s) = &pkg.license {
                push_annotation(&mut args, "org.opencontainers.image.licenses", s);
            }
            if let Some(s) = &v.base_digest {
                push_annotation(&mut args, "org.appspec.base.digest", s);
            }
            args.push(format!("{}:{}", blob.display(), SIF_MEDIA_TYPE));

            run_oras(&args, opts.dry_run)?;
            pushed.push(pkg_ref);
        }
    }

    Ok(OciExportOutcome {
        index_ref,
        pushed,
        dry_run: opts.dry_run,
    })
}

fn push_annotation(args: &mut Vec<String>, key: &str, value: &str) {
    args.push("--annotation".to_string());
    args.push(format!("{}={}", key, value));
}

fn run_oras(args: &[String], dry_run: bool) -> Result<()> {
    let bin = oras_bin();
    if dry_run {
        println!("{} {}", bin, args.join(" "));
        return Ok(());
    }
    let st = Command::new(&bin)
        .args(args)
        .status()
        .with_context(|| format!("spawning {} push", bin))?;
    if !st.success() {
        bail!("{} push failed (exit {})", bin, st);
    }
    Ok(())
}
