use anyhow::{anyhow, bail, Context, Result};
use clap::Args;
use std::path::PathBuf;

use super::{add, check as check_mod, default_root, gc as gc_mod, index_build, layout, meta, util};

// ---- helpers ---------------------------------------------------------------

fn open_root(a: Option<PathBuf>) -> Result<layout::RepoRoot> {
    layout::RepoRoot::open(&a.unwrap_or_else(default_root))
}

fn read_index(root: &layout::RepoRoot) -> Result<meta::IndexV1> {
    Ok(serde_json::from_str(&std::fs::read_to_string(
        root.index_path(),
    )?)?)
}

fn split_pv(s: &str) -> Result<(String, String)> {
    let (n, v) = s
        .split_once(':')
        .ok_or_else(|| anyhow!("expected NAME:VERSION-RELEASE, got `{}`", s))?;
    Ok((n.to_ascii_lowercase(), v.to_string()))
}

// ---- init ------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct InitArgs {
    pub path: Option<PathBuf>,
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long)]
    pub description: Option<String>,
    /// Сгенерировать серверный токен и сохранить его sha256 в config.toml.
    /// Токен печатается в stdout один раз.
    #[arg(long)]
    pub token: Option<String>,
}

pub fn init(a: InitArgs) -> Result<i32> {
    let root = a.path.unwrap_or_else(default_root);
    let name = a.name.unwrap_or_else(|| "local/appspec-repo".to_string());

    if root.join("config.toml").exists() {
        let cfg = layout::RepoRoot::open(&root)?;
        if cfg.config.repo.name != name {
            bail!(
                "repository at {} already initialized as `{}` (requested `{}`)",
                root.display(),
                cfg.config.repo.name,
                name
            );
        }
        println!("already initialized: {}", root.display());
        return Ok(0);
    }

    std::fs::create_dir_all(root.join("blobs/sha256"))?;
    std::fs::create_dir_all(root.join("packages"))?;
    std::fs::create_dir_all(root.join("refs"))?;
    std::fs::create_dir_all(root.join("index"))?;
    std::fs::write(root.join("index/.keep"), b"")?;

    let mut tokens: Vec<layout::ServerToken> = Vec::new();
    let mut generated_token: Option<(String, String)> = None;
    if let Some(tname) = &a.token {
        let raw = util::hex_encode(&util::random_bytes(32)?);
        let hash = format!("sha256:{}", util::sha256_hex(raw.as_bytes()));
        tokens.push(layout::ServerToken {
            name: tname.clone(),
            hash,
        });
        generated_token = Some((tname.clone(), raw));
    }

    let mut cfg = layout::Config {
        repo: layout::RepoMeta {
            name: name.clone(),
            description: a.description.unwrap_or_default(),
            created_at: util::now_iso8601(),
            generator: format!("appspec {}", env!("CARGO_PKG_VERSION")),
            layout: layout::LAYOUT_VERSION,
        },
        policy: Default::default(),
        signing: Default::default(),
        server: Default::default(),
        ipfs: Default::default(),
    };
    cfg.server.tokens = tokens;
    std::fs::write(root.join("config.toml"), toml::to_string_pretty(&cfg)?)?;

    let index = meta::IndexV1 {
        schema: 1,
        repo: name,
        updated: util::now_iso8601(),
        generator: format!("appspec {}", env!("CARGO_PKG_VERSION")),
        signature: String::new(),
        packages: Default::default(),
    };
    std::fs::write(root.join("index.json"), serde_json::to_vec_pretty(&index)?)?;

    println!("initialized repository at {}", root.display());
    if let Some((tname, raw)) = generated_token {
        println!("token {}: {}", tname, raw);
        println!("(save it now; only sha256 is stored)");
    }
    Ok(0)
}

// ---- add -------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct AddArgs {
    pub sif: Vec<PathBuf>,
    #[arg(long)]
    pub arch: Option<String>,
    #[arg(long)]
    pub force: bool,
    #[arg(long)]
    pub release_suffix: Option<String>,
    #[arg(long)]
    pub pin_ipfs: bool,
    #[arg(long)]
    pub tag: Option<String>,
    #[arg(long)]
    pub root: Option<PathBuf>,
}

pub fn add(a: AddArgs) -> Result<i32> {
    if a.sif.is_empty() {
        bail!("no SIF arguments");
    }
    let root = open_root(a.root)?;
    let _lock = util::FileLock::acquire(&root.lock_path())?;

    if a.pin_ipfs {
        eprintln!("warning: --pin-ipfs ignored (IPFS support arrives in E6)");
    }
    if a.tag.is_some() {
        eprintln!("warning: --tag ignored for now; use `refs/<name>.toml` (repo tag in E2)");
    }

    let mut changed = false;
    for sif in &a.sif {
        let out = add::add_one(
            &root,
            add::AddParams {
                sif,
                arch: a.arch.as_deref(),
                release_suffix: a.release_suffix.as_deref(),
                force: a.force,
                pin_ipfs: a.pin_ipfs,
                tag: a.tag.as_deref(),
            },
        )?;
        if out.already_present {
            println!(
                "no-op: {}:{}-{} (epoch {}, arch {}) (sha {})",
                out.name,
                out.version,
                out.release,
                out.epoch,
                out.arch,
                &out.sif_sha256[..12]
            );
            if let Some(cid) = &out.ipfs_cid {
                println!("  ipfs: {}", cid);
            }
        } else {
            println!(
                "added: {}:{}-{} (epoch {}, arch {}) (sha {})",
                out.name,
                out.version,
                out.release,
                out.epoch,
                out.arch,
                &out.sif_sha256[..12]
            );
            if let Some(cid) = &out.ipfs_cid {
                println!("  ipfs: {}", cid);
            }
            changed = true;
        }
    }
    if changed {
        index_build::rebuild_all(&root)?;
    }
    Ok(0)
}

// ---- ls --------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct LsArgs {
    pub package: Option<String>,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub root: Option<PathBuf>,
}

pub fn ls(a: LsArgs) -> Result<i32> {
    let root = open_root(a.root)?;
    let index = read_index(&root)?;
    match a.package {
        Some(p) => {
            let key = p.to_ascii_lowercase();
            let pkg = index
                .packages
                .get(&key)
                .ok_or_else(|| anyhow!("package not found: {}", p))?;
            if a.json {
                println!("{}", serde_json::to_string_pretty(&pkg.versions)?);
            } else {
                for (k, v) in &pkg.versions {
                    println!(
                        "{:<24}  {:>12}  tags=[{}]  arch={}  sha={}",
                        k,
                        v.sif_size,
                        v.tags.join(","),
                        v.arch,
                        &v.sif_sha256[..12]
                    );
                }
            }
        }
        None => {
            if a.json {
                println!("{}", serde_json::to_string_pretty(&index.packages)?);
            } else {
                for (name, pkg) in &index.packages {
                    // §5.2: latest — по `(version, release, epoch)`, а не
                    // по лексикографическому порядку ключа BTreeMap.
                    // `"2.2.0-1" < "2.2.6-1"` лексикографически, но
                    // semver-максимум — "2.2.6-1"; `.keys().next()` даёт
                    // именно старейшую версию. Используем тот же `pick_latest`,
                    // что и `rebuild_refs` для `refs/latest`.
                    let latest_owned = index_build::pick_latest(&pkg.versions);
                    let latest = latest_owned.as_deref().unwrap_or("-");
                    println!(
                        "{:<24}  {} versions  latest={}",
                        name,
                        pkg.versions.len(),
                        latest
                    );
                }
            }
        }
    }
    Ok(0)
}

// ---- show ------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct ShowArgs {
    pub package_version: String,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub spec: bool,
    #[arg(long)]
    pub sif: Option<PathBuf>,
    #[arg(long)]
    pub no_verify: bool,
    #[arg(long)]
    pub root: Option<PathBuf>,
}

pub fn show(a: ShowArgs) -> Result<i32> {
    let root = open_root(a.root)?;
    let (pkg, ver) = split_pv(&a.package_version)?;
    let meta_path = root.packages_dir().join(&pkg).join(&ver).join("meta.json");
    if !meta_path.is_file() {
        bail!("not found: {}:{}", pkg, ver);
    }

    if a.spec {
        let spec = meta_path.parent().unwrap().join("spec.appspec");
        print!("{}", std::fs::read_to_string(&spec)?);
        return Ok(0);
    }

    if let Some(out) = a.sif {
        let m: meta::MetaV1 = serde_json::from_str(&std::fs::read_to_string(&meta_path)?)?;
        let blob = util::blob_path(&root.path, &m.sif_sha256);
        if !a.no_verify {
            let actual = util::sha256_file(&blob)?;
            if actual != m.sif_sha256 {
                bail!("blob {} sha256 mismatch", blob.display());
            }
        }
        std::fs::copy(&blob, &out)
            .with_context(|| format!("copy {} -> {}", blob.display(), out.display()))?;
        println!("wrote {}", out.display());
        return Ok(0);
    }

    let m: meta::MetaV1 = serde_json::from_str(&std::fs::read_to_string(&meta_path)?)?;
    if a.json {
        println!("{}", serde_json::to_string_pretty(&m)?);
        return Ok(0);
    }
    println!("Name:        {}", m.name);
    println!(
        "Version:     {}-{} (epoch {})",
        m.version, m.release, m.epoch
    );
    println!("Arch:        {}", m.arch);
    if let Some(s) = &m.summary {
        println!("Summary:     {}", s);
    }
    if let Some(s) = &m.license {
        println!("License:     {}", s);
    }
    if let Some(s) = &m.url {
        println!("URL:         {}", s);
    }
    if let Some(s) = &m.vendor {
        println!("Vendor:      {}", s);
    }
    if let Some(s) = &m.base_digest {
        println!("Base digest: {}", s);
    }
    println!("SIF sha256:  {}", m.sif_sha256);
    println!("SIF size:    {}", m.sif_size);
    println!("Spec sha256: {}", m.spec_sha256);
    println!("Built at:    {}", m.built_at);
    if !m.requires.is_empty() {
        println!("Requires:    {}", m.requires.join(", "));
    }
    if !m.provides.is_empty() {
        println!("Provides:    {}", m.provides.join(", "));
    }
    if !m.conflicts.is_empty() {
        println!("Conflicts:   {}", m.conflicts.join(", "));
    }
    Ok(0)
}

// ---- reindex ---------------------------------------------------------------

#[derive(Debug, Args)]
pub struct ReindexArgs {
    #[arg(long)]
    pub root: Option<PathBuf>,
}

pub fn reindex(a: ReindexArgs) -> Result<i32> {
    let root = open_root(a.root)?;
    let _lock = util::FileLock::acquire(&root.lock_path())?;
    let idx = index_build::rebuild_all(&root)?;
    println!(
        "reindexed: {} packages, updated={}",
        idx.packages.len(),
        idx.updated
    );
    Ok(0)
}

// ---- check -----------------------------------------------------------------

#[derive(Debug, Args)]
pub struct CheckArgs {
    #[arg(long)]
    pub deep: bool,
    #[arg(long)]
    pub root: Option<PathBuf>,
}

pub fn check(a: CheckArgs) -> Result<i32> {
    let root = open_root(a.root)?;
    let out = check_mod::run(&root, a.deep)?;
    if out.errors > 0 {
        eprintln!(
            "check: FAILED ({} errors, {} warnings)",
            out.errors, out.warnings
        );
        Ok(2)
    } else {
        println!("check: ok ({} warnings)", out.warnings);
        Ok(0)
    }
}

// ---- gc --------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct GcArgs {
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub keep: Option<usize>,
    #[arg(long)]
    pub root: Option<PathBuf>,
}

pub fn gc(a: GcArgs) -> Result<i32> {
    let root = open_root(a.root)?;
    let _lock = util::FileLock::acquire(&root.lock_path())?;
    let out = gc_mod::run(&root, a.dry_run, a.keep)?;
    let verb = if a.dry_run { "would remove" } else { "removed" };
    println!(
        "{}: {} blobs, {} referenced, {} bytes",
        verb,
        out.removed.len(),
        out.referenced,
        out.bytes_reclaimed
    );
    if a.dry_run {
        for p in &out.removed {
            println!("  {}", p.display());
        }
    }
    Ok(0)
}

// ---- search ----------------------------------------------------------------

#[derive(Debug, Args)]
pub struct SearchArgs {
    pub query: Option<String>,
    #[arg(long)]
    pub license: Option<String>,
    #[arg(long)]
    pub arch: Option<String>,
    #[arg(long)]
    pub vendor: Option<String>,
    #[arg(long)]
    pub spec_sha256: Option<String>,
    #[arg(long)]
    pub requires: Option<String>,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub limit: Option<usize>,
    #[arg(long)]
    pub root: Option<PathBuf>,
}

pub fn search(a: SearchArgs) -> Result<i32> {
    let root = open_root(a.root)?;
    let index = read_index(&root)?;
    let q = a.query.as_deref().map(str::to_lowercase);
    let limit = a.limit.unwrap_or(usize::MAX);

    let mut results = Vec::new();
    for (name, pkg) in &index.packages {
        if let Some(q) = &q {
            let in_name = name.to_lowercase().contains(q);
            let in_summary = pkg
                .summary
                .as_deref()
                .map(|s| s.to_lowercase().contains(q))
                .unwrap_or(false);
            if !(in_name || in_summary) {
                continue;
            }
        }
        if let Some(l) = &a.license {
            if pkg.license.as_deref() != Some(l.as_str()) {
                continue;
            }
        }
        if let Some(v) = &a.vendor {
            if pkg.vendor.as_deref() != Some(v.as_str()) {
                continue;
            }
        }
        let mut versions: Vec<&meta::IndexVersion> = Vec::new();
        for v in pkg.versions.values() {
            if let Some(ar) = &a.arch {
                if v.arch != *ar {
                    continue;
                }
            }
            if let Some(p) = &a.requires {
                if !v.provides.iter().any(|x| x == p) {
                    continue;
                }
            }
            if let Some(s) = &a.spec_sha256 {
                if !v.spec_sha256.starts_with(s) {
                    continue;
                }
            }
            versions.push(v);
        }
        if versions.is_empty() {
            continue;
        }
        results.push((name, pkg, versions));
        if results.len() >= limit {
            break;
        }
    }

    if a.json {
        let json: Vec<serde_json::Value> = results
            .iter()
            .map(|(n, p, vs)| {
                serde_json::json!({
                    "name": n,
                    "summary": p.summary,
                    "license": p.license,
                    "vendor": p.vendor,
                    "versions": vs.iter().map(|v| serde_json::json!({
                        "version": v.version,
                        "release": v.release,
                        "arch": v.arch,
                        "sif_sha256": v.sif_sha256,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else {
        for (name, pkg, versions) in &results {
            println!("{}", name);
            if let Some(s) = &pkg.summary {
                println!("  {}", s);
            }
            for v in versions {
                println!(
                    "  {}-{}  arch={}  sha={}",
                    v.version,
                    v.release,
                    v.arch,
                    &v.sif_sha256[..12]
                );
            }
        }
    }
    Ok(0)
}

// ---- serve -----------------------------------------------------------------

#[derive(Debug, Args)]
pub struct ServeArgs {
    #[arg(long)]
    pub listen: Option<String>,
    #[arg(long)]
    pub root: Option<PathBuf>,
}

pub fn serve(a: ServeArgs) -> Result<i32> {
    let mut root = open_root(a.root)?;
    if let Some(l) = a.listen {
        root.config.server.listen = l;
    }
    super::server::serve(&root)?;
    Ok(0)
}

// ---- push ------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct PushArgs {
    pub sif: Vec<PathBuf>,
    #[arg(long)]
    pub to: String,
    #[arg(long)]
    pub token_file: Option<PathBuf>,
    #[arg(long)]
    pub token: Option<String>,
    #[arg(long)]
    pub force: bool,
}

pub fn push(a: PushArgs) -> Result<i32> {
    if a.sif.is_empty() {
        bail!("no SIF arguments");
    }
    let _ = a.force; // сервер вернёт 409, --force на клиенте не реализован в v1

    let token = if let Some(tf) = &a.token_file {
        std::fs::read_to_string(tf)
            .with_context(|| format!("reading token file {}", tf.display()))?
            .trim()
            .to_string()
    } else if let Ok(t) = std::env::var("APPSPEC_REPO_TOKEN") {
        t
    } else if let Some(t) = &a.token {
        eprintln!("warning: --token exposes secrets in shell history; use --token-file or APPSPEC_REPO_TOKEN");
        t.clone()
    } else {
        bail!("no token: pass --token-file, APPSPEC_REPO_TOKEN, or --token");
    };

    let mut ok = 0;
    let mut failed = 0;
    for sif in &a.sif {
        match super::push::push_one(sif, &a.to, &token) {
            Ok(out) => {
                let tag = if out.already_present {
                    "no-op"
                } else {
                    "pushed"
                };
                println!(
                    "{}: {}:{} (sha {})",
                    tag,
                    out.package,
                    out.version,
                    &out.sha256[..12.min(out.sha256.len())]
                );
                ok += 1;
            }
            Err(e) => {
                eprintln!("error: {}: {:#}", sif.display(), e);
                failed += 1;
            }
        }
    }
    if failed > 0 {
        Ok(1)
    } else {
        let _ = ok;
        Ok(0)
    }
}

// ---- pull ------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct PullArgs {
    pub package_version: String,
    #[arg(long)]
    pub from: Option<String>,
    #[arg(long)]
    pub spec_only: bool,
    #[arg(long)]
    pub sif_out: Option<PathBuf>,
    #[arg(long)]
    pub trusted_key: Option<PathBuf>,
    #[arg(long)]
    pub allow_unsigned: bool,
    #[arg(long)]
    pub via: Option<String>,
    #[arg(long)]
    pub root: Option<PathBuf>,
}

pub fn pull(a: PullArgs) -> Result<i32> {
    let root = open_root(a.root)?;
    let _lock = util::FileLock::acquire(&root.lock_path())?;

    super::pull::pull(
        &root,
        &a.package_version,
        super::pull::PullOpts {
            from: a.from,
            via: a.via,
            spec_only: a.spec_only,
            sif_out: a.sif_out,
            trusted_key: a.trusted_key,
            allow_unsigned: a.allow_unsigned,
        },
    )?;
    Ok(0)
}

// ---- sign / verify (E5) ----------------------------------------------------

#[derive(Debug, Args)]
pub struct SignArgs {
    /// Путь к приватному ключу. Если файла нет — будет сгенерирован.
    /// По умолчанию: $APPSPEC_SIGNING_KEY, $XDG_CONFIG_HOME/appspec/signing.key,
    /// $HOME/.config/appspec/signing.key.
    #[arg(long)]
    pub key: Option<PathBuf>,
    #[arg(long)]
    pub root: Option<PathBuf>,
}

pub fn sign(a: SignArgs) -> Result<i32> {
    let root = open_root(a.root)?;
    let _lock = util::FileLock::acquire(&root.lock_path())?;

    let key_path = a.key.unwrap_or_else(super::sign::default_key_path);
    let key = if key_path.is_file() {
        super::sign::load_key(&key_path)?
    } else {
        let k = super::sign::keygen();
        super::sign::save_key(&key_path, &k)?;
        eprintln!("generated new signing key: {}", key_path.display());
        k
    };

    let index_text = std::fs::read_to_string(root.index_path())?;
    let sig = super::sign::sign_index(&index_text, &key)?;

    let mut v: serde_json::Value = serde_json::from_str(&index_text)?;
    v.as_object_mut()
        .ok_or_else(|| anyhow!("index.json is not an object"))?
        .insert("signature".into(), serde_json::Value::String(sig));
    std::fs::write(root.index_path(), serde_json::to_vec_pretty(&v)?)?;

    let pubkey = super::sign::pubkey_to_string(&key.verifying_key());
    println!("signed: {}", root.index_path().display());
    println!("pubkey: {}", pubkey);
    Ok(0)
}

#[derive(Debug, Args)]
pub struct VerifyArgs {
    /// Публичный ключ (`ed25519:<hex>`) или файл с ним. Если не задан —
    /// берётся `[signing].pubkey` из config.toml.
    #[arg(long)]
    pub pubkey: Option<PathBuf>,
    #[arg(long)]
    pub root: Option<PathBuf>,
}

pub fn verify(a: VerifyArgs) -> Result<i32> {
    let root = open_root(a.root)?;
    let index_text = std::fs::read_to_string(root.index_path())?;

    let pubkey = if let Some(p) = &a.pubkey {
        let s = std::fs::read_to_string(p)
            .with_context(|| format!("reading pubkey {}", p.display()))?;
        super::sign::parse_pubkey(s.trim())?
    } else {
        let s = root.config.signing.pubkey.clone();
        if s.is_empty() {
            bail!("no public key: pass --pubkey PATH or set [signing].pubkey in config.toml");
        }
        super::sign::parse_pubkey(&s)?
    };

    super::sign::verify_index(&index_text, &pubkey)?;
    println!("verify: OK");
    Ok(0)
}

// ---- publish-index (E6) ---------------------------------------------------

#[derive(Debug, Args)]
pub struct PublishIndexArgs {
    #[arg(long)]
    pub pin: bool,
    #[arg(long)]
    pub root: Option<PathBuf>,
}

pub fn publish_index(a: PublishIndexArgs) -> Result<i32> {
    let root = open_root(a.root)?;
    let index_path = root.index_path();
    if !index_path.is_file() {
        bail!("index.json missing at {}", index_path.display());
    }
    let cid = super::ipfs::add_file(&index_path, a.pin)?;
    super::ipfs::validate_cid(&cid)?;
    // Кэшируем CID рядом с index.json — удобно для последующего --from ipfs://
    let cid_path = root.path.join("ipfs-index.cid");
    std::fs::write(&cid_path, format!("{}\n", cid))?;
    println!("{}", cid);
    Ok(0)
}

// ---- export-oci (E8) ------------------------------------------------------

#[derive(Debug, Args)]
pub struct ExportOciArgs {
    /// Имя репозитория индекса в registry (например `appspec-index`).
    /// При `--registry` — используется как имя; без `--registry` — как
    /// registry-префикс для индекса и пакетов.
    pub target: String,
    /// Префикс registry (например `ghcr.io/user`). Если задан, TARGET —
    /// это имя индекса: `<registry>/<target>:latest`, пакеты — `<registry>/<name>:<ver>`.
    #[arg(long)]
    pub registry: Option<String>,
    /// Только напечатать команды `oras push`, ничего не выполнять.
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub root: Option<PathBuf>,
}

pub fn export_oci(a: ExportOciArgs) -> Result<i32> {
    let root = open_root(a.root)?;
    let out = super::oci::export_oci(
        &root,
        super::oci::OciExportOpts {
            target: a.target,
            registry: a.registry,
            dry_run: a.dry_run,
        },
    )?;
    if out.dry_run {
        println!("# dry-run: {} push command(s)", out.pushed.len());
        println!("# index: {}", out.index_ref);
    } else {
        println!(
            "exported: {} artifact(s), index: {}",
            out.pushed.len(),
            out.index_ref
        );
    }
    Ok(0)
}
