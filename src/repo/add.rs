use anyhow::{bail, Context, Result};
use base64::Engine as _;
use std::collections::BTreeMap;
use std::path::Path;

use super::inspect;
use super::layout::RepoRoot;
use super::meta::MetaV1;
use super::util::{
    atomic_write, blob_path, cid_v1_from_sha256, create_exclusive, now_iso8601, sha256_file,
    sha256_hex,
};

pub struct AddParams<'a> {
    pub sif: &'a Path,
    pub arch: Option<&'a str>,
    pub release_suffix: Option<&'a str>,
    pub force: bool,
    pub pin_ipfs: bool,
    #[allow(dead_code)] // reserved: repo tag (v2)
    pub tag: Option<&'a str>,
}

pub struct AddOutcome {
    pub name: String,
    pub version: String,
    pub release: String,
    pub epoch: u32,
    pub arch: String,
    pub sif_sha256: String,
    pub already_present: bool,
    pub ipfs_cid: Option<String>,
}

pub fn add_one(root: &RepoRoot, p: AddParams) -> Result<AddOutcome> {
    // 1. sha256 SIF
    let sif_bytes =
        std::fs::read(p.sif).with_context(|| format!("reading SIF {}", p.sif.display()))?;
    let sif_sha = sha256_hex(&sif_bytes);
    let sif_size = sif_bytes.len() as u64;

    // 2. inspect
    let labels = inspect::inspect(p.sif)?;

    // 3. обязательные лейблы
    let name_orig = labels.require("org.appspec.package.name")?.to_string();
    let version = labels.require("org.appspec.package.version")?.to_string();
    let release_raw = labels
        .get("org.appspec.package.release")
        .map(str::to_string)
        .unwrap_or_else(|| "1".to_string());
    let epoch: u32 = labels
        .get("org.appspec.package.epoch")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let arch = labels
        .get("org.appspec.package.arch")
        .map(str::to_string)
        .unwrap_or_else(|| "unknown".to_string());

    if let Some(a) = p.arch {
        if a != arch.as_str() {
            bail!(
                "--arch {} does not match SIF label org.appspec.package.arch = {}",
                a,
                arch
            );
        }
    }

    let release = match p.release_suffix {
        Some(suf) => format!("{}.{}", release_raw, suf),
        None => release_raw.clone(),
    };

    // 4. spec
    let spec_b64 = labels.require("org.appspec.spec.content.base64")?;
    let spec_sha_label = labels.require("org.appspec.spec.sha256")?;
    let spec_bytes = base64::engine::general_purpose::STANDARD
        .decode(spec_b64)
        .context("decoding org.appspec.spec.content.base64")?;
    let spec_sha = sha256_hex(&spec_bytes);
    if spec_sha != spec_sha_label {
        bail!(
            "spec sha256 mismatch: label says {}, actual {}",
            spec_sha_label,
            spec_sha
        );
    }
    let spec_filename = labels
        .get("org.appspec.spec.filename")
        .unwrap_or("spec.appspec")
        .to_string();

    let name = normalize_name(&name_orig);
    let pkg_id = format!("{}-{}", version, release);
    let pkg_dir = root.packages_dir().join(&name).join(&pkg_id);
    let meta_path = pkg_dir.join("meta.json");

    // 5. конфликт?
    if meta_path.is_file() {
        let existing: MetaV1 = serde_json::from_str(&std::fs::read_to_string(&meta_path)?)?;
        if existing.sif_sha256 == sif_sha {
            return Ok(AddOutcome {
                name,
                version,
                release,
                epoch,
                arch,
                sif_sha256: sif_sha,
                already_present: true,
                ipfs_cid: existing.ipfs_cid,
            });
        }
        if !p.force {
            bail!(
                "conflict: {}:{}-{} (epoch {}) already exists with different sif_sha256; \
                 use --force (requires policy.allow_force = true)",
                name,
                version,
                release,
                epoch
            );
        }
        if !root.config.policy.allow_force {
            bail!(
                "conflict: {}:{}-{}; --force rejected by policy.allow_force = false",
                name,
                version,
                release
            );
        }
    }

    // 6. blob (O_EXCL, dedup)
    let blob = blob_path(&root.path, &sif_sha);
    let created = create_exclusive(&blob, &sif_bytes)?;
    if !created && sha256_file(&blob)? != sif_sha {
        bail!("existing blob {} has wrong sha256", blob.display());
    }

    // 7. packages/
    std::fs::create_dir_all(&pkg_dir)?;
    // --- иконка (E-icon): base64 из SIF → packages/<n>/<v>/icon.<ext> ---
    let icon_sha256_label = labels.get("org.appspec.icon.sha256").map(str::to_string);
    let icon_mimetype_label = labels.get("org.appspec.icon.mimetype").map(str::to_string);
    let icon_size_label = labels
        .get("org.appspec.icon.size")
        .and_then(|s| s.parse::<u64>().ok());
    let icon_orig_filename = labels.get("org.appspec.icon.filename").map(str::to_string);

    let mut icon_filename: Option<String> = None;
    if let Some(b64) = labels.get("org.appspec.icon.content.base64") {
        use base64::Engine as _;
        match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(bytes) => {
                let actual_sha = sha256_hex(&bytes);
                let sha_ok = icon_sha256_label
                    .as_deref()
                    .map(|e| e == actual_sha)
                    .unwrap_or(true);
                if !sha_ok {
                    crate::log(format!(
                        "warning: icon sha256 mismatch in SIF labels (label={:?}, actual={}); пропускаю",
                        icon_sha256_label, actual_sha
                    ));
                } else {
                    let ext = icon_orig_filename
                        .as_deref()
                        .and_then(|n| n.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()))
                        .filter(|e| !e.is_empty() && e.len() <= 8)
                        .unwrap_or_else(|| "png".to_string());
                    let pkg_icon_name = format!("icon.{}", ext);
                    atomic_write(&pkg_dir.join(&pkg_icon_name), &bytes)?;
                    icon_filename = Some(pkg_icon_name);
                }
            }
            Err(e) => {
                crate::log(format!(
                    "warning: cannot decode icon base64 from SIF labels: {}",
                    e
                ));
            }
        }
    }
    atomic_write(&pkg_dir.join("spec.appspec"), &spec_bytes)?;
    atomic_write(&pkg_dir.join("sha256"), sif_sha.as_bytes())?;

    // 8. IPFS CID strategy:
    //   - files <= 256 KB: local raw CID == ipfs add (one block, no chunking)
    //   - files > 256 KB:  ipfs add --only-hash (correct dag-pb root, offline)
    //   - no ipfs + large:  None
    const SMALL_CID_LIMIT: u64 = 262144;
    let expected = cid_v1_from_sha256(&sif_sha)
        .with_context(|| format!("computing local IPFS CID for {}", sif_sha))?;

    let ipfs_cid: Option<String> = if p.pin_ipfs {
        let actual = super::ipfs::add_file(&blob, true)
            .with_context(|| format!("pinning blob {} to IPFS", blob.display()))?;
        if sif_size <= SMALL_CID_LIMIT && actual != expected {
            bail!(
                "ipfs add returned {} but local CID is {} for small file {}; \
                 check IPFS version / config (expected raw-leaves CIDv1)",
                actual,
                expected,
                blob.display()
            );
        }
        Some(actual)
    } else if super::ipfs::which_ipfs() {
        match super::ipfs::add_file_only_hash(&blob) {
            Ok(cid) => Some(cid),
            Err(e) => {
                crate::log(format!(
                    "warning: ipfs add --only-hash failed for {} ({:#}); \
                     falling back to local CID for small files",
                    blob.display(),
                    e
                ));
                if sif_size <= SMALL_CID_LIMIT {
                    Some(expected)
                } else {
                    None
                }
            }
        }
    } else if sif_size <= SMALL_CID_LIMIT {
        Some(expected)
    } else {
        None
    };

    let mut raw_labels = BTreeMap::new();
    for (k, v) in &labels.raw {
        raw_labels.insert(k.clone(), v.clone());
    }

    let meta = MetaV1 {
        schema: 1,
        name: name.clone(),
        version: version.clone(),
        release: release.clone(),
        epoch,
        arch: arch.clone(),
        summary: labels
            .get("org.appspec.package.summary")
            .map(str::to_string),
        description: labels
            .get("org.appspec.package.description")
            .map(str::to_string),
        license: labels
            .get("org.appspec.package.license")
            .map(str::to_string),
        url: labels.get("org.appspec.package.url").map(str::to_string),
        vendor: labels.get("org.appspec.package.vendor").map(str::to_string),
        packager: labels
            .get("org.appspec.package.packager")
            .map(str::to_string),
        sif_sha256: sif_sha.clone(),
        sif_size,
        spec_sha256: spec_sha,
        spec_size: spec_bytes.len() as u64,
        spec_filename,
        generator: labels
            .get("org.appspec.generator")
            .unwrap_or("unknown")
            .to_string(),
        built_at: now_iso8601(),
        base_digest: labels
            .get("org.appspec.build.base.digest")
            .map(str::to_string),
        requires: parse_json_array(labels.get("org.appspec.dependencies.requires")),
        provides: parse_json_array(labels.get("org.appspec.dependencies.provides")),
        conflicts: parse_json_array(labels.get("org.appspec.dependencies.conflicts")),
        icon_sha256: icon_sha256_label.clone(),
        icon_mimetype: icon_mimetype_label,
        icon_size: icon_size_label,
        icon_filename: icon_filename.clone(),
        ipfs_cid: ipfs_cid.clone(),
        raw_labels,
    };
    atomic_write(&meta_path, &serde_json::to_vec_pretty(&meta)?)?;

    Ok(AddOutcome {
        name,
        version,
        release,
        epoch,
        arch,
        sif_sha256: sif_sha,
        already_present: false,
        ipfs_cid,
    })
}

fn normalize_name(s: &str) -> String {
    s.to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_' || *c == '-')
        .collect()
}

fn parse_json_array(s: Option<&str>) -> Vec<String> {
    s.and_then(|x| serde_json::from_str::<Vec<String>>(x).ok())
        .unwrap_or_default()
}
