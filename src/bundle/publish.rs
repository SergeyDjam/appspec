//! Publish a bundle spec as a repository package (`kind = "bundle"`).
//!
//! Kept outside `repo::add` because integration tests compile `src/repo`
//! standalone; bundle publishing is orchestrated from `main`.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::repo::layout::RepoRoot;
use crate::repo::meta::{IndexV1, MetaV1, PackageKind};
use crate::repo::util::{
    atomic_write, blob_path, cid_v1_from_sha256, create_exclusive, now_iso8601, sha256_file,
    sha256_hex,
};

use super::loader::{classify_spec_source, load_bundle, SpecSource};
use super::manifest::{
    BundleManifestV1, ManifestComponent, ManifestExtension, ManifestSource, MANIFEST_SCHEMA,
};
use super::model::{BundleFile, ComponentMode, Extension, SourceSpec};

pub struct AddOpts<'a> {
    pub version: Option<&'a str>,
    pub release: Option<&'a str>,
    pub epoch: Option<u32>,
    pub force: bool,
    pub pin_ipfs: bool,
}

pub struct AddOutcome {
    pub name: String,
    pub version: String,
    pub release: String,
    pub epoch: u32,
    pub bundle_sha256: String,
    pub manifest_sha256: String,
    pub already_present: bool,
}

/// True if the file is a bundle spec (contains `[bundle]`).
pub fn is_bundle_spec(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(v) = toml::from_str::<toml::Value>(&text) else {
        return false;
    };
    v.get("bundle").is_some()
}

fn read_index(root: &RepoRoot) -> Result<IndexV1> {
    let text = std::fs::read_to_string(root.index_path())
        .with_context(|| format!("reading {}", root.index_path().display()))?;
    serde_json::from_str(&text).with_context(|| "parsing repo index.json".to_string())
}

fn manifest_from_local_spec(
    scope: &str,
    orig: &str,
    spec_path: &Path,
    root: &RepoRoot,
) -> Result<ManifestSource> {
    let spec = crate::load_spec(spec_path)
        .with_context(|| format!("{scope}: parsing {}", spec_path.display()))?;
    let pkg = normalize_name(&spec.package.name);
    let ver_spec = match &spec.package.release {
        Some(r) => format!("{}-{}", spec.package.version, r),
        None => spec.package.version.clone(),
    };
    let index = read_index(root)?;
    let (ver_key, v) = crate::repo::run_package::pick_version(&index, &pkg, Some(&ver_spec))
        .map_err(|_| {
            anyhow!(
                "{scope}: local `{orig}` is {pkg}:{ver_spec}, which is not in the repo; \
                 first publish its SIF with `appspec repo add <{pkg}.sif>`"
            )
        })?;
    let canonical = format!("{pkg}:{ver_key}");
    let blob = blob_path(&root.path, &v.sif_sha256);
    if !blob.is_file() {
        bail!(
            "{scope}: repo blob missing: {} (run `appspec repo pull {canonical}` first)",
            blob.display()
        );
    }
    Ok(ManifestSource {
        spec: orig.to_string(),
        resolved: format!("repo:{canonical}"),
        sif_sha256: Some(v.sif_sha256.clone()),
        spec_sha256: Some(v.spec_sha256.clone()),
        ipfs_cid: v.ipfs_cid.clone(),
    })
}

/// Convert a bundle source to a portable manifest source.
/// Local `spec` paths are mapped to the matching repo package (same package
/// name/version); `sif`/`manifest` remain rejected.
fn manifest_source(
    scope: &str,
    src: &SourceSpec,
    root: &RepoRoot,
    base_dir: &Path,
) -> Result<ManifestSource> {
    if let Some(s) = &src.spec {
        return match classify_spec_source(s) {
            SpecSource::RepoRef(_) => {
                let index = read_index(root)?;
                let (name, ver) = s.split_once(':').unwrap_or((s, "latest"));
                let name = name.to_ascii_lowercase();
                let (ver_key, v) = crate::repo::run_package::pick_version(&index, &name, Some(ver))
                    .map_err(|e| anyhow!("{scope}: {e:#}"))?;
                let canonical = format!("{name}:{ver_key}");
                let blob = blob_path(&root.path, &v.sif_sha256);
                if !blob.is_file() {
                    bail!(
                        "{scope}: repo blob missing: {} (run `appspec repo pull {canonical}` first)",
                        blob.display()
                    );
                }
                Ok(ManifestSource {
                    spec: s.clone(),
                    resolved: format!("repo:{canonical}"),
                    sif_sha256: Some(v.sif_sha256.clone()),
                    spec_sha256: Some(v.spec_sha256.clone()),
                    ipfs_cid: v.ipfs_cid.clone(),
                })
            }
            SpecSource::LocalPath(_) => {
                let sp = super::loader::resolve_relative(s, base_dir);
                if !sp.is_file() {
                    bail!("{scope}: local spec not found: {}", sp.display());
                }
                manifest_from_local_spec(scope, s, &sp, root)
            }
            SpecSource::Ambiguous(_) => {
                let sp = super::loader::resolve_relative(s, base_dir);
                if sp.is_file() {
                    manifest_from_local_spec(scope, s, &sp, root)
                } else {
                    bail!(
                        "{scope}: `{s}` not found locally; use an explicit repo ref (`name[:ver]`)"
                    )
                }
            }
        };
    }
    if let Some(cid) = &src.cid {
        return Ok(ManifestSource {
            spec: format!("cid:{cid}"),
            resolved: format!("cid:{cid}"),
            sif_sha256: None,
            spec_sha256: None,
            ipfs_cid: Some(cid.clone()),
        });
    }
    if src.sif.is_some() {
        bail!("{scope}: `sif` is a local file; use `name[:ver]` repo ref or `cid`");
    }
    if src.manifest.is_some() {
        bail!("{scope}: `manifest` is a local install dir; use `name[:ver]` repo ref or `cid`");
    }
    bail!("{scope}: missing source")
}

fn manifest_extension(e: &Extension, base_dir: &Path) -> Result<ManifestExtension> {
    if let Some(file) = &e.file {
        let path = super::loader::resolve_relative(file, base_dir);
        if !path.is_file() {
            bail!("extension `{}`: file not found: {}", e.name, path.display());
        }
        if !crate::repo::ipfs::which_ipfs() {
            bail!(
                "extension `{}`: local file `{}` requires `ipfs` to publish as CID; \
                 install ipfs or replace `file` with `cid`/`url`",
                e.name,
                path.display()
            );
        }
        let cid = crate::repo::ipfs::add_file(&path, true)
            .with_context(|| format!("pinning extension `{}` ({})", e.name, path.display()))?;
        eprintln!("pinned extension `{}` → {}", e.name, cid);
        return Ok(ManifestExtension {
            name: e.name.clone(),
            source: Some(format!("cid:{cid}")),
            cid: Some(cid),
            url: None,
            requires: e.requires.clone(),
        });
    }
    let (source, cid, url) = if let Some(c) = &e.cid {
        (Some(format!("cid:{c}")), Some(c.clone()), None)
    } else if let Some(u) = &e.url {
        (Some(format!("url:{u}")), None, Some(u.clone()))
    } else {
        (None, None, None)
    };
    Ok(ManifestExtension {
        name: e.name.clone(),
        source,
        cid,
        url,
        requires: e.requires.clone(),
    })
}

fn component_mode(c: &super::model::Component) -> &'static str {
    match c.mode() {
        ComponentMode::Chroot => "chroot",
        ComponentMode::Instance => "instance",
        ComponentMode::Both => "both",
    }
}

/// Publish one bundle spec. Does not rebuild the index.
pub fn add_bundle(root: &RepoRoot, path: &Path, opts: &AddOpts) -> Result<AddOutcome> {
    let raw =
        std::fs::read(path).with_context(|| format!("reading bundle spec {}", path.display()))?;
    let text = std::str::from_utf8(&raw).context("bundle spec is not valid UTF-8")?;
    let value: toml::Value =
        toml::from_str(text).with_context(|| format!("parsing bundle spec {}", path.display()))?;
    if value.get("package").is_some() {
        bail!(
            "{}: contains both [bundle] and [package] — not a bundle spec",
            path.display()
        );
    }

    let file: BundleFile = load_bundle(path)?;
    let version = opts
        .version
        .or(file.bundle.version.as_deref())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            anyhow!(
                "bundle version is required: set `[bundle] version = \"...\"` \
                 or pass `--version`"
            )
        })?;
    let release = opts
        .release
        .or(file.bundle.release.as_deref())
        .filter(|s| !s.is_empty())
        .unwrap_or("1");
    let epoch = opts.epoch.or(file.bundle.epoch).unwrap_or(0);
    if version.contains('/') || release.contains('/') {
        bail!("version/release must not contain `/`");
    }

    let name = normalize_name(&file.bundle.name);
    if name.is_empty() {
        bail!("[bundle].name is empty after normalization");
    }
    let base_dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let base = manifest_source("base", &file.bundle.base.source, root, base_dir)?;
    let mut components = Vec::with_capacity(file.components.len());
    for c in &file.components {
        components.push(ManifestComponent {
            name: c.name.clone(),
            provide: c.provide(),
            mode: component_mode(c).to_string(),
            source: manifest_source(
                &format!("component `{}`", c.name),
                &c.source,
                root,
                base_dir,
            )?,
        });
    }
    let mut extensions = Vec::with_capacity(file.extensions.len());
    for e in &file.extensions {
        extensions.push(manifest_extension(e, base_dir)?);
    }

    let bundle_sha256 = sha256_hex(&raw);
    let bundle_size = raw.len() as u64;
    let manifest = BundleManifestV1 {
        schema: MANIFEST_SCHEMA,
        name: name.clone(),
        version: version.to_string(),
        release: release.to_string(),
        epoch,
        arch: "noarch".into(),
        bundle_sha256: bundle_sha256.clone(),
        base,
        components,
        extensions,
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    let manifest_sha256 = sha256_hex(&manifest_bytes);
    let manifest_size = manifest_bytes.len() as u64;

    let pkg_id = format!("{}-{}", version, release);
    let pkg_dir = root.packages_dir().join(&name).join(&pkg_id);
    let meta_path = pkg_dir.join("meta.json");

    if meta_path.is_file() {
        let existing: MetaV1 = serde_json::from_str(&std::fs::read_to_string(&meta_path)?)?;
        if existing.kind == PackageKind::Bundle
            && existing.bundle_sha256.as_deref() == Some(bundle_sha256.as_str())
            && existing.manifest_sha256.as_deref() == Some(manifest_sha256.as_str())
        {
            return Ok(AddOutcome {
                name,
                version: version.to_string(),
                release: release.to_string(),
                epoch,
                bundle_sha256,
                manifest_sha256,
                already_present: true,
            });
        }
        if !opts.force {
            bail!(
                "conflict: bundle {}:{}-{} already exists with different sha256; \
                 use --force (requires policy.allow_force = true)",
                name,
                version,
                release
            );
        }
        if !root.config.policy.allow_force {
            bail!(
                "conflict: bundle {}:{}-{}; --force rejected by policy.allow_force = false",
                name,
                version,
                release
            );
        }
    }

    // blobs for the bundle spec and manifest
    let blob = blob_path(&root.path, &bundle_sha256);
    let created = create_exclusive(&blob, &raw)?;
    if !created && sha256_file(&blob)? != bundle_sha256 {
        bail!("existing blob {} has wrong sha256", blob.display());
    }
    let manifest_blob = blob_path(&root.path, &manifest_sha256);
    let created = create_exclusive(&manifest_blob, &manifest_bytes)?;
    if !created && sha256_file(&manifest_blob)? != manifest_sha256 {
        bail!(
            "existing manifest blob {} has wrong sha256",
            manifest_blob.display()
        );
    }

    std::fs::create_dir_all(&pkg_dir)?;
    atomic_write(&pkg_dir.join("spec.appspec"), &raw)?;
    atomic_write(&pkg_dir.join("manifest.json"), &manifest_bytes)?;
    atomic_write(&pkg_dir.join("sha256"), bundle_sha256.as_bytes())?;

    let mut ipfs_cid = cid_v1_from_sha256(&bundle_sha256).ok();
    let mut manifest_ipfs_cid = cid_v1_from_sha256(&manifest_sha256).ok();
    if opts.pin_ipfs {
        ipfs_cid = Some(
            crate::repo::ipfs::add_file(&blob, true)
                .with_context(|| format!("pinning bundle blob {}", blob.display()))?,
        );
        manifest_ipfs_cid = Some(
            crate::repo::ipfs::add_file(&manifest_blob, true)
                .with_context(|| format!("pinning manifest {}", manifest_blob.display()))?,
        );
    }

    let mut provides: Vec<String> = file.components.iter().map(|c| c.provide()).collect();
    provides.sort();
    provides.dedup();
    let mut requires: Vec<String> = file
        .extensions
        .iter()
        .flat_map(|e| e.requires.iter().cloned())
        .collect();
    requires.sort();
    requires.dedup();

    let meta = MetaV1 {
        schema: 1,
        kind: PackageKind::Bundle,
        name: name.clone(),
        version: version.to_string(),
        release: release.to_string(),
        epoch,
        arch: "noarch".into(),
        summary: if file.bundle.description.is_empty() {
            None
        } else {
            Some(file.bundle.description.clone())
        },
        description: None,
        license: None,
        url: None,
        vendor: None,
        packager: None,
        sif_sha256: String::new(),
        sif_size: 0,
        bundle_sha256: Some(bundle_sha256.clone()),
        bundle_size: Some(bundle_size),
        manifest_sha256: Some(manifest_sha256.clone()),
        manifest_size: Some(manifest_size),
        manifest_ipfs_cid,
        spec_sha256: bundle_sha256.clone(),
        spec_size: bundle_size,
        spec_filename: "spec.appspec".into(),
        generator: format!("appspec {}", env!("CARGO_PKG_VERSION")),
        built_at: now_iso8601(),
        base_digest: None,
        requires,
        provides,
        conflicts: Vec::new(),
        ipfs_cid,
        raw_labels: BTreeMap::new(),
        icon_sha256: None,
        icon_mimetype: None,
        icon_size: None,
        icon_filename: None,
    };
    atomic_write(&meta_path, &serde_json::to_vec_pretty(&meta)?)?;

    Ok(AddOutcome {
        name,
        version: version.to_string(),
        release: release.to_string(),
        epoch,
        bundle_sha256,
        manifest_sha256,
        already_present: false,
    })
}

/// Publish several bundle specs; returns whether anything changed.
pub fn add_bundles(root: &RepoRoot, paths: &[PathBuf], opts: &AddOpts) -> Result<bool> {
    let mut changed = false;
    for path in paths {
        let out = add_bundle(root, path, opts)?;
        let tag = if out.already_present {
            "no-op"
        } else {
            "added"
        };
        println!(
            "{}: {}:{}-{} (bundle, epoch {}, sha {}, manifest {})",
            tag,
            out.name,
            out.version,
            out.release,
            out.epoch,
            &out.bundle_sha256[..12],
            &out.manifest_sha256[..12]
        );
        changed |= !out.already_present;
    }
    Ok(changed)
}

fn normalize_name(s: &str) -> String {
    s.to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_' || *c == '-')
        .collect()
}
