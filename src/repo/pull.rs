use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};

use super::http::http_request;
use super::layout::RepoRoot;
use super::meta::{IndexV1, IndexVersion, MetaV1};
use super::util::{atomic_write, sha256_hex};

pub struct PullOpts {
    pub from: Option<String>,
    pub via: Option<String>, // "http" | "ipfs" | "path"
    pub spec_only: bool,
    pub sif_out: Option<PathBuf>,
    pub trusted_key: Option<PathBuf>,
    pub allow_unsigned: bool,
}

pub fn pull(local_root: &RepoRoot, name_ver: &str, opts: PullOpts) -> Result<()> {
    let (name, ver) = name_ver
        .split_once(':')
        .ok_or_else(|| anyhow!("expected NAME:VERSION-RELEASE, got `{}`", name_ver))?;
    let name = name.to_ascii_lowercase();

    // Ровно один источник: --from или --via.
    if opts.from.is_some() && opts.via.is_some() {
        bail!("--from and --via are mutually exclusive");
    }

    if let Some(from) = opts.from.as_deref() {
        if let Some(url) = from.strip_prefix("oci://") {
            let full = format!("oci://{}", url);
            return pull_from_oci(local_root, &name, ver, &full, &opts);
        }
        if from.starts_with("https://") {
            bail!("https:// not supported in v1 (use http:// or reverse proxy)");
        }
        if let Some(cid) = from.strip_prefix("ipfs://") {
            return pull_from_ipfs_index(local_root, &name, ver, cid, &opts);
        }
        if from.starts_with("http://") {
            return pull_from_http(local_root, &name, ver, from, &opts);
        }
        return pull_from_path(local_root, &name, ver, Path::new(from), &opts);
    }

    if opts.via.as_deref() == Some("ipfs") {
        return pull_via_ipfs(local_root, &name, ver, &opts);
    }

    bail!(
        "repo pull requires --from URL|PATH or --via ipfs \
         (local pull is no-op; use `repo show --sif`)"
    )
}

// ===========================================================================
// HTTP
// ===========================================================================

fn pull_from_http(
    local_root: &RepoRoot,
    name: &str,
    ver: &str,
    base_url: &str,
    opts: &PullOpts,
) -> Result<()> {
    let base = base_url.trim_end_matches('/');

    let url = format!("{}/index.json", base);
    let (status, _h, body) = http_request(&url, "GET", &[], None)?;
    if status != 200 {
        bail!("GET {} returned {}", url, status);
    }

    let index_text = std::str::from_utf8(&body).context("remote index.json is not valid UTF-8")?;
    let trusted = super::sign::load_trusted_keys(opts.trusted_key.as_deref())?;
    super::sign::verify_remote_index(index_text, &trusted, opts.allow_unsigned)?;

    let index: IndexV1 = serde_json::from_str(index_text).context("parsing remote index.json")?;

    let pkg = index
        .packages
        .get(name)
        .ok_or_else(|| anyhow!("package `{}` not in remote index", name))?;
    let v = pkg
        .versions
        .get(ver)
        .ok_or_else(|| anyhow!("{}:{} not in remote index", name, ver))?;

    let spec_url = format!("{}/packages/{}/{}/spec.appspec", base, name, ver);
    let (s, _h, spec_body) = http_request(&spec_url, "GET", &[], None)?;
    if s != 200 {
        bail!("GET {} returned {}", spec_url, s);
    }
    let actual_spec_sha = sha256_hex(&spec_body);
    if actual_spec_sha != v.spec_sha256 {
        bail!(
            "spec sha256 mismatch: expected {}, got {}",
            v.spec_sha256,
            actual_spec_sha
        );
    }

    let meta = build_meta(name, v);

    if opts.spec_only {
        write_version(local_root, name, ver, &spec_body, &meta, None)?;
        let _ = super::index_build::rebuild_all(local_root);
        println!("pulled spec: {}:{}", name, ver);
        return Ok(());
    }

    let ab = &v.sif_sha256[..2];
    let rest = &v.sif_sha256[2..];
    let blob_url = format!("{}/blobs/sha256/{}/{}", base, ab, rest);
    let (bs, _h, blob_body) = http_request(&blob_url, "GET", &[], None)?;
    if bs != 200 {
        bail!("GET {} returned {}", blob_url, bs);
    }
    let actual_sha = sha256_hex(&blob_body);
    if actual_sha != v.sif_sha256 {
        bail!(
            "blob sha256 mismatch: expected {}, got {}",
            v.sif_sha256,
            actual_sha
        );
    }

    if let Some(out) = &opts.sif_out {
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p)?;
        }
        atomic_write(out, &blob_body)?;
        println!("pulled: {}:{} → {}", name, ver, out.display());
        return Ok(());
    }

    write_version(local_root, name, ver, &spec_body, &meta, Some(&blob_body))?;
    let _ = super::index_build::rebuild_all(local_root);
    println!("pulled: {}:{}", name, ver);
    Ok(())
}

// ===========================================================================
// Локальный путь
// ===========================================================================

fn pull_from_path(
    local_root: &RepoRoot,
    name: &str,
    ver: &str,
    from: &Path,
    opts: &PullOpts,
) -> Result<()> {
    let src_root = RepoRoot::open(from)
        .with_context(|| format!("opening source repo at {}", from.display()))?;
    let meta_path = src_root
        .packages_dir()
        .join(name)
        .join(ver)
        .join("meta.json");
    if !meta_path.is_file() {
        bail!("not found in {}: {}:{}", from.display(), name, ver);
    }
    let meta: MetaV1 = serde_json::from_str(&std::fs::read_to_string(&meta_path)?)?;
    let spec_src = meta_path.parent().unwrap().join("spec.appspec");
    let blob_src = super::util::blob_path(&src_root.path, &meta.sif_sha256);

    if opts.spec_only {
        let spec = std::fs::read(&spec_src)?;
        write_version(local_root, name, ver, &spec, &meta, None)?;
        let _ = super::index_build::rebuild_all(local_root);
        println!("pulled spec: {}:{}", name, ver);
        return Ok(());
    }

    if let Some(out) = &opts.sif_out {
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::copy(&blob_src, out)?;
        println!("pulled: {}:{} → {}", name, ver, out.display());
        return Ok(());
    }

    let spec = std::fs::read(&spec_src)?;
    let blob = std::fs::read(&blob_src)?;
    write_version(local_root, name, ver, &spec, &meta, Some(&blob))?;
    let _ = super::index_build::rebuild_all(local_root);
    println!("pulled: {}:{}", name, ver);
    Ok(())
}

// ===========================================================================
// IPFS: локальный index → ipfs_cid → ipfs cat
// ===========================================================================

fn pull_via_ipfs(local_root: &RepoRoot, name: &str, ver: &str, opts: &PullOpts) -> Result<()> {
    if !super::ipfs::which_ipfs() {
        bail!(
            "`{}` not found in PATH; required for --via ipfs",
            super::ipfs::ipfs_bin()
        );
    }

    let index_text = std::fs::read_to_string(local_root.index_path())
        .context("local index.json missing; pull --via ipfs needs a local index")?;
    let index: IndexV1 = serde_json::from_str(&index_text)?;

    let pkg = index
        .packages
        .get(name)
        .ok_or_else(|| anyhow!("package `{}` not in local index", name))?;
    let v = pkg
        .versions
        .get(ver)
        .ok_or_else(|| anyhow!("{}:{} not in local index", name, ver))?;

    let cid = v.ipfs_cid.as_deref().ok_or_else(|| {
        anyhow!(
            "no ipfs_cid for {}:{} — first `appspec repo add --pin-ipfs` \
         or `appspec repo publish-index` and re-add",
            name,
            ver
        )
    })?;

    pull_blob_from_ipfs(local_root, name, ver, cid, v, opts)
}

// ===========================================================================
// IPFS: index-cid → index.json → ipfs_cid → ipfs cat
// ===========================================================================

fn pull_from_ipfs_index(
    local_root: &RepoRoot,
    name: &str,
    ver: &str,
    index_cid: &str,
    opts: &PullOpts,
) -> Result<()> {
    super::ipfs::validate_cid(index_cid)?;
    if !super::ipfs::which_ipfs() {
        bail!(
            "`{}` not found in PATH; required for ipfs:// URLs",
            super::ipfs::ipfs_bin()
        );
    }

    let index_bytes = super::ipfs::cat(index_cid)
        .with_context(|| format!("ipfs cat {} (index.json)", index_cid))?;
    let index_text =
        std::str::from_utf8(&index_bytes).context("parsing IPFS index.json: not valid UTF-8")?;

    let trusted = super::sign::load_trusted_keys(opts.trusted_key.as_deref())?;
    super::sign::verify_remote_index(index_text, &trusted, opts.allow_unsigned)?;

    let index: IndexV1 = serde_json::from_str(index_text).context("parsing IPFS index.json")?;

    let pkg = index
        .packages
        .get(name)
        .ok_or_else(|| anyhow!("package `{}` not in IPFS index", name))?;
    let v = pkg
        .versions
        .get(ver)
        .ok_or_else(|| anyhow!("{}:{} not in IPFS index", name, ver))?;

    let cid = v.ipfs_cid.as_deref().ok_or_else(|| {
        anyhow!(
            "no ipfs_cid for {}:{} in IPFS index — publisher must run \
         `appspec repo add --pin-ipfs`",
            name,
            ver
        )
    })?;

    pull_blob_from_ipfs(local_root, name, ver, cid, v, opts)
}

// ===========================================================================
// общая часть IPFS-pull
// ===========================================================================

fn pull_blob_from_ipfs(
    local_root: &RepoRoot,
    name: &str,
    ver: &str,
    cid: &str,
    v: &IndexVersion,
    opts: &PullOpts,
) -> Result<()> {
    super::ipfs::validate_cid(cid)?;

    let blob_bytes = super::ipfs::cat(cid).with_context(|| format!("ipfs cat {}", cid))?;
    let actual_sha = sha256_hex(&blob_bytes);
    if actual_sha != v.sif_sha256 {
        bail!(
            "blob sha256 mismatch from IPFS: expected {}, got {} (cid {})",
            v.sif_sha256,
            actual_sha,
            cid
        );
    }

    if let Some(out) = &opts.sif_out {
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p)?;
        }
        atomic_write(out, &blob_bytes)?;
        println!("pulled: {}:{} → {} (cid {})", name, ver, out.display(), cid);
        return Ok(());
    }

    if opts.spec_only {
        bail!("--spec-only с IPFS не реализовано в v1; используйте HTTP или PATH");
    }

    let meta = build_meta(name, v);
    write_version(local_root, name, ver, b"", &meta, Some(&blob_bytes))?;

    let spec_path = local_root
        .packages_dir()
        .join(name)
        .join(ver)
        .join("spec.appspec");
    if !spec_path.is_file() || std::fs::metadata(&spec_path)?.len() == 0 {
        eprintln!(
            "warning: spec.appspec для {}:{} не восстановлен из IPFS; \
             используйте `pull --from http://... --spec-only`",
            name, ver
        );
        let _ = std::fs::remove_file(&spec_path);
        let _ = std::fs::remove_file(spec_path.with_file_name("meta.json"));
    }

    let _ = super::index_build::rebuild_all(local_root);
    println!("pulled: {}:{} (cid {})", name, ver, cid);
    Ok(())
}

// ===========================================================================
// helpers
// ===========================================================================

fn write_version(
    local_root: &RepoRoot,
    name: &str,
    ver: &str,
    spec: &[u8],
    meta: &MetaV1,
    blob: Option<&[u8]>,
) -> Result<()> {
    let dir = local_root.packages_dir().join(name).join(ver);
    std::fs::create_dir_all(&dir)?;
    if !spec.is_empty() {
        atomic_write(&dir.join("spec.appspec"), spec)?;
    }
    atomic_write(&dir.join("meta.json"), &serde_json::to_vec_pretty(meta)?)?;
    if let Some(b) = blob {
        let blob_path = super::util::blob_path(&local_root.path, &meta.sif_sha256);
        if let Some(p) = blob_path.parent() {
            std::fs::create_dir_all(p)?;
        }
        atomic_write(&blob_path, b)?;
    }
    Ok(())
}

fn build_meta(name: &str, v: &IndexVersion) -> MetaV1 {
    MetaV1 {
        schema: 1,
        name: name.to_string(),
        version: v.version.clone(),
        release: v.release.clone(),
        epoch: v.epoch,
        arch: v.arch.clone(),
        summary: None,
        description: None,
        license: None,
        url: None,
        vendor: None,
        packager: None,
        sif_sha256: v.sif_sha256.clone(),
        sif_size: v.sif_size,
        spec_sha256: v.spec_sha256.clone(),
        spec_size: v.spec_size,
        spec_filename: "spec.appspec".into(),
        generator: v.generator.clone(),
        built_at: v.built_at.clone(),
        base_digest: v.base_digest.clone(),
        requires: v.requires.clone(),
        provides: v.provides.clone(),
        conflicts: Vec::new(),
        ipfs_cid: v.ipfs_cid.clone(),
        raw_labels: Default::default(),
    }
}

// ===========================================================================
// OCI (E9)
// ===========================================================================

fn pull_from_oci(
    local_root: &RepoRoot,
    name: &str,
    ver: &str,
    url: &str, // oci://<registry>/<index-repo>:<tag>
    opts: &PullOpts,
) -> Result<()> {
    if !super::oci::which_oras() {
        bail!(
            "`{}` not found in PATH; required for oci:// pulls",
            super::oci::oras_bin()
        );
    }

    let (registry_base, _index_name) = super::oci::split_oci_index_ref(url)?;

    // 1. index.json
    let (_index_bytes, index) = super::oci::fetch_index_from_oci(super::oci::OciFetchIndexOpts {
        url: url.to_string(),
        trusted_key: opts.trusted_key.clone(),
        allow_unsigned: opts.allow_unsigned,
    })?;

    let pkg = index
        .packages
        .get(name)
        .ok_or_else(|| anyhow!("package `{}` not in OCI index", name))?;
    let v = pkg
        .versions
        .get(ver)
        .ok_or_else(|| anyhow!("{}:{} not in OCI index", name, ver))?;

    let meta = build_meta(name, v);

    if opts.spec_only {
        bail!("--spec-only с oci:// не реализовано: spec.appspec не публикуется как artifact в v1");
    }

    // 2. SIF
    let blob_bytes = super::oci::fetch_sif_from_oci(&registry_base, name, ver, &v.sif_sha256)?;

    if let Some(out) = &opts.sif_out {
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p)?;
        }
        atomic_write(out, &blob_bytes)?;
        println!("pulled: {}:{} → {} (oci {})", name, ver, out.display(), url);
        return Ok(());
    }

    // spec.appspec недоступен из OCI (только index.json + SIF).
    // Кладём meta.json и SIF, spec.appspec остаётся пустым.
    // Пользователь может восстановить spec из SIF через `apptainer inspect`.
    write_version(local_root, name, ver, b"", &meta, Some(&blob_bytes))?;
    let spec_path = local_root
        .packages_dir()
        .join(name)
        .join(ver)
        .join("spec.appspec");
    let _ = std::fs::remove_file(&spec_path);

    let _ = super::index_build::rebuild_all(local_root);
    println!("pulled: {}:{} (oci {})", name, ver, url);
    eprintln!(
        "warning: spec.appspec для {}:{} не публикуется в OCI; \
         восстановить: apptainer inspect --json --labels <blob> | jq -r \
         '.\"org.appspec.spec.content.base64\"' | base64 -d",
        name, ver
    );
    Ok(())
}
