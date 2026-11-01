//! `repo export-oci` (E8) и `repo pull --from oci://` (E9)
//! через `oras` CLI.
//!
//! Схема (§6.13 ТЗ):
//!   * `<base>/<target>:latest` — artifact manifest, `artifactType =
//!     application/vnd.appspec.index.v1+json`, единственный blob — index.json.
//!   * `<base>/<name>:<version>-<release>` — artifact manifest,
//!     `artifactType = application/vnd.appspec.sif.v1+json`, единственный
//!     blob — SIF.
//!
//! Pull (E9):
//!   * `oci://<registry>/<index-repo>:<tag>` — резолвит index.json через
//!     `<registry>/<index-repo>:<tag>`, затем SIF через
//!     `<registry>/<name>:<ver>`.
//!
//! Бинарь: `$APPSPEC_ORAS` (для тестов) или `oras`.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

use super::layout::RepoRoot;
use super::meta::IndexV1;
use super::util::{blob_path, sha256_file, sha256_hex};

pub const INDEX_ARTIFACT_TYPE: &str = "application/vnd.appspec.index.v1+json";
pub const SIF_ARTIFACT_TYPE: &str = "application/vnd.appspec.sif.v1+json";
pub const INDEX_MEDIA_TYPE: &str = "application/json";
pub const SIF_MEDIA_TYPE: &str = "application/vnd.sylabs.sif";

pub fn oras_bin() -> String {
    std::env::var("APPSPEC_ORAS").unwrap_or_else(|_| "oras".to_string())
}

pub fn which_oras() -> bool {
    let bin = oras_bin();
    if bin.contains('/') {
        return Path::new(&bin).is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(&bin).is_file()))
}

// ===========================================================================
// export_oci (E8)
// ===========================================================================

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
    let index: IndexV1 = serde_json::from_str(&index_text).context("parsing local index.json")?;

    let (base, index_repo) = match opts.registry.as_deref() {
        Some(r) => {
            let b = r.trim_end_matches('/').to_string();
            (b.clone(), format!("{}/{}", b, target))
        }
        None => (target.clone(), format!("{}/appspec-index", target)),
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
    push_annotation(
        &mut index_args,
        "org.opencontainers.image.title",
        "appspec-index",
    );
    push_annotation(
        &mut index_args,
        "org.opencontainers.image.description",
        &format!("appspec index for {}", index.repo),
    );
    push_annotation(&mut index_args, "org.appspec.repo", &index.repo);
    push_annotation(&mut index_args, "org.appspec.updated", &index.updated);
    push_annotation(&mut index_args, "org.appspec.generator", &index.generator);
    push_annotation(
        &mut index_args,
        "org.appspec.package_count",
        &format!("{}", total_versions),
    );
    index_args.push(format!(
        "{}:{}",
        root.index_path().display(),
        INDEX_MEDIA_TYPE
    ));

    run_oras(&index_args, opts.dry_run)?;
    pushed.push(index_ref.clone());

    // 2. SIF'ы
    for (name, pkg) in &index.packages {
        for (ver_key, v) in &pkg.versions {
            let blob = blob_path(&root.path, &v.sif_sha256);
            if !blob.is_file() {
                eprintln!(
                    "warning: skip {}:{} — blob {} missing",
                    name,
                    ver_key,
                    blob.display()
                );
                continue;
            }

            // Проверка целостности перед push: не публикуем битое.
            let actual = sha256_file(&blob)?;
            if actual != v.sif_sha256 {
                bail!(
                    "blob {} sha256 mismatch: expected {}, got {}",
                    blob.display(),
                    v.sif_sha256,
                    actual
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

// ===========================================================================
// pull_from_oci (E9)
// ===========================================================================

/// `oci://<repo>[:<tag>]` → `(repo, tag)`. Тег по умолчанию — `latest`.
/// Порт в registry (`registry.local:5000/x`) не путается с тегом:
/// `:` внутри `repo` означает порт, если после него идёт `/`.
pub fn parse_oci_ref(url: &str) -> Result<(String, String)> {
    let rest = url
        .strip_prefix("oci://")
        .ok_or_else(|| anyhow::anyhow!("not an oci:// URL: {}", url))?;
    let (repo, tag) = match rest.rfind(':') {
        Some(i) if !rest[i + 1..].contains('/') => (&rest[..i], &rest[i + 1..]),
        _ => (rest, "latest"),
    };
    if repo.is_empty() {
        bail!("oci:// URL: empty repository in `{}`", url);
    }
    Ok((repo.to_string(), tag.to_string()))
}

/// Разделить `oci://<registry_base>/<index-repo>:<tag>` на `registry_base`
/// и `index-repo`. `registry_base` может содержать `/` (ghcr.io/user),
/// `index-repo` — последний сегмент до `:`.
pub fn split_oci_index_ref(url: &str) -> Result<(String, String)> {
    let (repo, _tag) = parse_oci_ref(url)?;
    let (base, index_name) = repo.rsplit_once('/').ok_or_else(|| {
        anyhow::anyhow!(
            "oci:// URL must be `oci://<registry>/<index-repo>:<tag>`, got `{}`",
            url
        )
    })?;
    Ok((base.to_string(), index_name.to_string()))
}

pub struct OciFetchIndexOpts {
    pub url: String,
    pub trusted_key: Option<PathBuf>,
    pub allow_unsigned: bool,
}

/// `oras pull <ref> -o <dir>` → содержимое блоба (самый большой файл
/// в `<dir>`; artifact-config `{}` = 2 байта отбрасывается).
/// sha256-проверка — в вызывающем коде: только там известно ожидаемое
/// значение, и можно дать осмысленное сообщение об ошибке.
fn oras_pull_blob(ref_str: &str, workdir: &Path) -> Result<Vec<u8>> {
    let bin = oras_bin();
    if !which_oras() {
        bail!("`{}` not found in PATH; required for oci:// pulls", bin);
    }
    std::fs::create_dir_all(workdir)?;
    let st = Command::new(&bin)
        .args(["pull", ref_str, "-o"])
        .arg(workdir)
        .status()
        .with_context(|| format!("{} pull {}", bin, ref_str))?;
    if !st.success() {
        bail!("{} pull {} failed (exit {})", bin, ref_str, st);
    }
    let mut best: Option<(u64, PathBuf)> = None;
    for entry in std::fs::read_dir(workdir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let size = entry.metadata()?.len();
        if size <= 2 {
            continue;
        } // artifact-config `{}`
        match &best {
            Some((n, _)) if size <= *n => continue,
            _ => best = Some((size, entry.path())),
        }
    }
    let (_, path) = best.ok_or_else(|| {
        anyhow::anyhow!(
            "oras pull {} produced no blob in {}",
            ref_str,
            workdir.display()
        )
    })?;
    Ok(std::fs::read(path)?)
}

/// Скачать index.json из OCI registry и проверить подпись (если есть
/// trusted keys в allowlist). Fail-closed по §10.2.
pub fn fetch_index_from_oci(opts: OciFetchIndexOpts) -> Result<(Vec<u8>, IndexV1)> {
    let (repo, tag) = parse_oci_ref(&opts.url)?;
    let ref_str = format!("{}:{}", repo, tag);
    let workdir = std::env::temp_dir().join(format!(
        "appspec-oci-pull-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let result = (|| -> Result<(Vec<u8>, IndexV1)> {
        let bytes = oras_pull_blob(&ref_str, &workdir)?;
        let text = std::str::from_utf8(&bytes).context("OCI index.json is not valid UTF-8")?;
        let trusted = super::sign::load_trusted_keys(opts.trusted_key.as_deref())?;
        super::sign::verify_remote_index(text, &trusted, opts.allow_unsigned)?;
        let index: IndexV1 = serde_json::from_str(text).context("parsing OCI index.json")?;
        Ok((bytes, index))
    })();
    let _ = std::fs::remove_dir_all(&workdir);
    result
}

/// Скачать SIF-пакет из OCI registry. Проверяет sha256 и падает с
/// `sha256 mismatch from OCI: expected …, got …` при расхождении.
pub fn fetch_sif_from_oci(
    registry_base: &str,
    name: &str,
    ver: &str,
    expected_sha: &str,
) -> Result<Vec<u8>> {
    let ref_str = format!("{}/{}:{}", registry_base.trim_end_matches('/'), name, ver);
    let workdir = std::env::temp_dir().join(format!(
        "appspec-oci-sif-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let result = (|| -> Result<Vec<u8>> {
        let bytes = oras_pull_blob(&ref_str, &workdir)?;
        let actual = sha256_hex(&bytes);
        if actual != expected_sha {
            bail!(
                "sha256 mismatch from OCI: expected {}, got {}",
                expected_sha,
                actual
            );
        }
        Ok(bytes)
    })();
    let _ = std::fs::remove_dir_all(&workdir);
    result
}
