use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Subcommand};
use std::path::{Path, PathBuf};

use super::{
    add, check as check_mod, config, default_root, gc as gc_mod, index_build, layout, meta, util,
};

// ===========================================================================
// helpers
// ===========================================================================

/// Открыть `RepoRoot`.
///
/// `sc` — явный `--server-config`. Если задан — `[repo].root` из него
/// (при `a.is_some()` — должен совпадать). Иначе — `RepoRoot::open`,
/// который сам смотрит в legacy и discovery.
fn open_root(a: Option<PathBuf>, sc: Option<PathBuf>) -> Result<layout::RepoRoot> {
    // 1. Явный --config: он главнее. --root, если задан, должен совпасть.
    if let Some(sc_path) = sc {
        let (cfg_path, cfg) = config::find_server_config(Some(&sc_path))?;
        let data = if let Some(r) = a {
            if layout::norm_path(&cfg.repo.root) != layout::norm_path(&r) {
                bail!(
                    "--config {} points to {}, not {}",
                    cfg_path.display(),
                    cfg.repo.root.display(),
                    r.display()
                );
            }
            r
        } else {
            cfg.repo.root.clone()
        };
        return Ok(layout::RepoRoot::from_server_config(data, cfg_path, cfg));
    }

    // 2. Явный --root: RepoRoot::open сам сделает discovery, найдёт
    //    matching config по [repo].root, иначе возьмёт defaults.
    if let Some(r) = a {
        return layout::RepoRoot::open(&r);
    }

    // 3. Ни --root, ни --config, но есть $APPSPEC_REPO (path или профиль)
    //    — это явное намерение пользователя, уважаем.
    if let Ok(v) = std::env::var("APPSPEC_REPO") {
        if !v.is_empty() && !is_url_like(&v) {
            let r = default_root(); // раскрывает path или profile.path
            return layout::RepoRoot::open(&r);
        }
    }

    // 4. Ни флагов, ни env — discovery. Если ~/.config/appspec/repo-server.toml
    //    или /etc/appspec/repo-server.toml есть, [repo].root — это и есть
    //    «текущий репозиторий».
    if let Ok((cfg_path, cfg)) = config::find_server_config(None) {
        return Ok(layout::RepoRoot::from_server_config(
            cfg.repo.root.clone(),
            cfg_path,
            cfg,
        ));
    }

    // 5. Fallback: XDG data-директория. Репо может быть не инициализирован —
    //    тогда RepoRoot::open вернёт понятную ошибку с подсказкой init.
    let r = default_root();
    layout::RepoRoot::open(&r)
}

/// Как `open_root`, но с UX-проверкой: для строго локальных операций
/// (`add`, `check`, `gc`, `reindex`, `sign`, `serve`, `publish-index`,
/// `export-oci`) URL в `$APPSPEC_REPO` — ошибка. Иначе `default_root()`
/// молча уводит в XDG и пользователь получает «repository not
/// initialized» вместо «передайте --root PATH».
///
/// `--root` и `--server-config` отключают проверку: явный путь побеждает.
fn require_local_root(
    a: Option<PathBuf>,
    sc: Option<PathBuf>,
    cmd: &str,
) -> Result<layout::RepoRoot> {
    if a.is_none() && sc.is_none() {
        if let Ok(v) = std::env::var("APPSPEC_REPO") {
            if !v.is_empty() && is_url_like(&v) {
                bail!(
                    "`repo {}` — локальная операция, требует путь к каталогу \
                     (получено `{}` из $APPSPEC_REPO); \
                     передайте --root PATH или --server-config PATH",
                    cmd,
                    v
                );
            }
        }
    }
    open_root(a, sc)
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

fn is_http_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

fn is_url_like(s: &str) -> bool {
    s.starts_with("http://")
        || s.starts_with("https://")
        || s.starts_with("ipfs://")
        || s.starts_with("ipns://")
}
// ===========================================================================
// origin resolution (search / ls / show / verify)
// ===========================================================================

/// Вернуть origin как строку: URL, путь или `ipfs://CID`.
///
/// Порядок: `--root` → `$APPSPEC_REPO` → default из client config →
/// `default_root()`. Если значение — не URL и не похоже на путь
/// (`/`, `~`, `.`), пробуем найти профиль с таким именем; если нет —
/// трактуем как относительный путь.
fn resolve_origin(root: &Option<PathBuf>) -> Result<String> {
    if let Some(p) = root {
        let s = p.display().to_string();
        if !s.is_empty() {
            return Ok(expand_origin(&s));
        }
    }
    if let Ok(v) = std::env::var("APPSPEC_REPO") {
        if !v.is_empty() {
            return Ok(expand_origin(&v));
        }
    }
    if let Ok((_, cc)) = config::find_client_config(None) {
        if let Some(def) = &cc.default {
            if let Some(p) = cc.profile(def) {
                return profile_origin(p);
            }
        }
    }
    Ok(default_root().display().to_string())
}

fn expand_origin(s: &str) -> String {
    if is_url_like(s) {
        return s.to_string();
    }
    if s.starts_with('/') || s.starts_with('~') || s.starts_with('.') {
        return s.to_string();
    }
    if let Ok((_, cc)) = config::find_client_config(None) {
        if let Some(p) = cc.profile(s) {
            if let Ok(o) = profile_origin(p) {
                return o;
            }
        }
    }
    s.to_string()
}

fn profile_origin(p: &super::RepoProfile) -> Result<String> {
    if let Some(url) = &p.source.url {
        return Ok(url.clone());
    }
    if let Some(path) = &p.source.path {
        return Ok(path.display().to_string());
    }
    if let Some(ipns) = &p.source.ipns {
        return Ok(format!("ipns://{}", ipns));
    }
    if let Some(ipfs) = &p.source.ipfs {
        return Ok(format!("ipfs://{}", ipfs));
    }
    bail!("profile `{}`: no source", p.name)
}

fn profile_sources(p: &super::RepoProfile) -> Result<Vec<String>> {
    let mut v = Vec::new();
    if let Some(ipns) = &p.source.ipns {
        v.push(format!("ipns://{}", ipns));
    }
    if let Some(url) = &p.source.url {
        v.push(url.clone());
    }
    if let Some(path) = &p.source.path {
        v.push(path.display().to_string());
    }
    if v.is_empty() {
        bail!(
            "profile `{}`: no source (expected path | url | ipns)",
            p.name
        );
    }
    Ok(v)
}
fn read_index_from(origin: &str) -> Result<meta::IndexV1> {
    if is_http_url(origin) {
        eprintln!("\x1b[2m[appspec] repo: {}\x1b[0m", origin);
        let url = format!("{}/index.json", origin.trim_end_matches('/'));
        let (status, _h, body) = super::http::http_request(&url, "GET", &[], None)
            .with_context(|| format!("GET {}", url))?;
        if status != 200 {
            bail!("GET {} returned {}", url, status);
        }
        let index: meta::IndexV1 = serde_json::from_slice(&body)
            .with_context(|| format!("parsing remote index.json from {}", url))?;
        if index.schema != 1 {
            bail!(
                "unsupported remote index schema {} (this client supports 1)",
                index.schema
            );
        }
        return Ok(index);
    }
    if origin.starts_with("ipfs://") || origin.starts_with("ipns://") {
        bail!(
            "ipfs:// and ipns:// origins are not supported in v1 for read commands;              use `repo pull --profile NAME` or `--from URL|PATH`"
        );
    }
    let root = layout::RepoRoot::open(Path::new(origin))?;
    read_index(&root)
}

fn print_meta_fields(m: &meta::MetaV1) {
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
}

// ===========================================================================
// init
// ===========================================================================

#[derive(Debug, Args)]
pub struct InitArgs {
    pub path: Option<PathBuf>,
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long)]
    pub description: Option<String>,
    /// Deprecated: use `appspec repo token gen NAME`.
    #[arg(long)]
    pub token: Option<String>,
    /// Создать server config по этому пути (пишет `[repo].root` = data path).
    /// Путь к `repo-server.toml`. Алиас `--server-config` — deprecated.
    #[arg(short = 'c', long = "config", alias = "server-config")]
    pub server_config: Option<PathBuf>,
}

pub fn init(a: InitArgs) -> Result<i32> {
    let root = config::expand_user_path(&a.path.unwrap_or_else(default_root));
    let name = a.name.unwrap_or_else(|| "local/appspec-repo".to_string());

    if root.join("index.json").is_file() {
        // Phase 1: `<root>/config.toml` больше не создаётся, `[repo].name`
        // хранится в `index.json::repo` — это data, источник истины.
        let existing_text = std::fs::read_to_string(root.join("index.json"))
            .with_context(|| format!("reading {}", root.join("index.json").display()))?;
        let existing: meta::IndexV1 = serde_json::from_str(&existing_text)
            .with_context(|| format!("parsing {}", root.join("index.json").display()))?;
        if existing.repo != name {
            bail!(
                "repository at {} already initialized as `{}` (requested `{}`)",
                root.display(),
                existing.repo,
                name
            );
        }
        if root.join("config.toml").is_file() {
            eprintln!(
                "warning: {} contains legacy config.toml; run `appspec repo migrate-config {}`",
                root.display(),
                root.display()
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

    let index = meta::IndexV1 {
        schema: 1,
        repo: name.clone(),
        updated: util::now_iso8601(),
        generator: format!("appspec {}", env!("CARGO_PKG_VERSION")),
        signature: String::new(),
        packages: Default::default(),
    };
    std::fs::write(root.join("index.json"), serde_json::to_vec_pretty(&index)?)?;

    let sc_path: Option<PathBuf> = a
        .server_config
        .as_ref()
        .map(|p| config::expand_user_path(p.as_path()));
    if let Some(p) = &sc_path {
        let cfg = config::ServerConfig {
            schema: config::CONFIG_SCHEMA,
            repo: config::ServerRepo {
                root: layout::norm_path(&root),
                name: name.clone(),
                description: a.description.clone().unwrap_or_default(),
                created_at: util::now_iso8601(),
                generator: format!("appspec {}", env!("CARGO_PKG_VERSION")),
            },
            policy: Default::default(),
            signing: config::ServerSigning::default(),
            server: Default::default(),
            ipfs: Default::default(),
        };
        cfg.save(p)?;
    }

    println!("initialized repository at {}", root.display());
    if let Some(p) = &sc_path {
        println!("  server config: {}", p.display());
    } else {
        eprintln!(
            "note: run `appspec repo server init --root {}` to create a server config",
            root.display()
        );
    }

    // Deprecated --token: делегируем token gen, если server config доступен.
    if let Some(tname) = a.token {
        eprintln!(
            "warning: --token is deprecated; use `appspec repo token gen {}`",
            tname
        );
        let target = sc_path
            .clone()
            .or_else(|| {
                config::server_config_paths()
                    .into_iter()
                    .find(|p| p.is_file())
            })
            .or_else(|| config::server_config_paths().into_iter().next());
        if let Some(sc_p) = target {
            match config::ServerConfig::load(&sc_p) {
                Ok(mut c) => {
                    if c.server.tokens.iter().any(|t| t.name == tname) {
                        eprintln!("warning: token `{}` already exists", tname);
                    } else {
                        let raw = util::hex_encode(&util::random_bytes(32)?);
                        let hash = format!("sha256:{}", util::sha256_hex(raw.as_bytes()));
                        c.server.tokens.push(layout::ServerToken {
                            name: tname.clone(),
                            hash,
                        });
                        c.save(&sc_p)?;
                        println!("token {}: {}", tname, raw);
                        println!("(save it now; only sha256 is stored)");
                    }
                }
                Err(e) => eprintln!("warning: cannot gen token: {e:#}"),
            }
        } else {
            eprintln!(
                "warning: no server config target; use `appspec repo token gen {}` after `server init`",
                tname
            );
        }
    }

    Ok(0)
}

// ===========================================================================
// add / check / gc / reindex — require local
// ===========================================================================

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
    let root = require_local_root(a.root, None, "add")?;
    let _lock = util::FileLock::acquire(&root.lock_path())?;

    if a.pin_ipfs {
        eprintln!("warning: --pin-ipfs ignored (IPFS support arrives in E6)");
    }
    if a.tag.is_some() {
        eprintln!("warning: --tag ignored for now; use `refs/<name>.toml`");
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
        let tag = if out.already_present {
            "no-op"
        } else {
            "added"
        };
        println!(
            "{}: {}:{}-{} (epoch {}, arch {}) (sha {})",
            tag,
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
        if !out.already_present {
            changed = true;
        }
    }
    if changed {
        index_build::rebuild_all(&root)?;
    }
    Ok(0)
}

#[derive(Debug, Args)]
pub struct ReindexArgs {
    /// Backfill `ipfs_cid` for packages added before 0.4.4.
    #[arg(long)]
    pub backfill_cid: bool,
    #[arg(long)]
    pub root: Option<PathBuf>,
}
pub fn reindex(a: ReindexArgs) -> Result<i32> {
    let root = require_local_root(a.root, None, "reindex")?;
    let _lock = util::FileLock::acquire(&root.lock_path())?;
    if a.backfill_cid {
        let n = backfill_ipfs_cid(&root)?;
        if n > 0 {
            eprintln!("backfilled ipfs_cid for {} package(s)", n);
        }
    }
    let idx = index_build::rebuild_all(&root)?;
    println!(
        "reindexed: {} packages, updated={}",
        idx.packages.len(),
        idx.updated
    );
    Ok(0)
}

fn backfill_ipfs_cid(root: &layout::RepoRoot) -> Result<usize> {
    const SMALL_CID_LIMIT: u64 = 262144;
    let mut count = 0usize;
    let pkg_root = root.packages_dir();
    if !pkg_root.exists() {
        return Ok(0);
    }
    for name_entry in std::fs::read_dir(&pkg_root)? {
        let name_entry = name_entry?;
        if !name_entry.file_type()?.is_dir() {
            continue;
        }
        for ver_entry in std::fs::read_dir(name_entry.path())? {
            let ver_entry = ver_entry?;
            if !ver_entry.file_type()?.is_dir() {
                continue;
            }
            let meta_path = ver_entry.path().join("meta.json");
            if !meta_path.is_file() {
                continue;
            }
            let text = std::fs::read_to_string(&meta_path)?;
            let mut meta: meta::MetaV1 = serde_json::from_str(&text)?;
            if meta.ipfs_cid.is_some() {
                continue;
            }
            let new_cid: Option<String> = if super::ipfs::which_ipfs() {
                let blob = crate::repo::util::blob_path(&root.path, &meta.sif_sha256);
                if blob.is_file() {
                    super::ipfs::add_file_only_hash(&blob).ok()
                } else {
                    None
                }
            } else if meta.sif_size <= SMALL_CID_LIMIT {
                util::cid_v1_from_sha256(&meta.sif_sha256).ok()
            } else {
                None
            };
            let Some(cid) = new_cid else {
                eprintln!(
                    "warning: cannot compute ipfs_cid for {} \
                     (need `ipfs` binary or file <= 256 KB)",
                    meta_path.display()
                );
                continue;
            };
            meta.ipfs_cid = Some(cid);
            util::atomic_write(&meta_path, &serde_json::to_vec_pretty(&meta)?)?;
            count += 1;
        }
    }
    Ok(count)
}
#[derive(Debug, Args)]
pub struct CheckArgs {
    #[arg(long)]
    pub deep: bool,
    #[arg(long)]
    pub root: Option<PathBuf>,
}

pub fn check(a: CheckArgs) -> Result<i32> {
    let root = require_local_root(a.root, None, "check")?;
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
    let root = require_local_root(a.root, None, "gc")?;
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

// ===========================================================================
// ls / search / show / verify
// ===========================================================================

#[derive(Debug, Args)]
pub struct LsArgs {
    pub package: Option<String>,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub root: Option<PathBuf>,
}

pub fn ls(a: LsArgs) -> Result<i32> {
    let origin = resolve_origin(&a.root)?;
    let index = read_index_from(&origin)?;
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
                let mut versions: Vec<(&String, &meta::IndexVersion)> =
                    pkg.versions.iter().collect();
                versions.sort_by_cached_key(|(_, v)| std::cmp::Reverse(v.sort_key()));
                for (k, v) in versions {
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
                    let latest =
                        index_build::pick_latest(&pkg.versions).unwrap_or_else(|| "-".into());
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
    let origin = resolve_origin(&a.root)?;
    let index = read_index_from(&origin)?;
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
        versions.sort_by_cached_key(|v| std::cmp::Reverse(v.sort_key()));
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
    let origin = resolve_origin(&a.root)?;
    let (pkg, ver) = split_pv(&a.package_version)?;

    if is_http_url(&origin) {
        return show_http(&origin, &pkg, &ver, &a);
    }
    if origin.starts_with("ipfs://") {
        bail!("ipfs:// show not supported in v1; use `repo pull --from ipfs://…`");
    }

    let root = layout::RepoRoot::open(Path::new(&origin))?;
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
    print_meta_fields(&m);
    Ok(0)
}

fn show_http(origin: &str, pkg: &str, ver: &str, a: &ShowArgs) -> Result<i32> {
    let base = origin.trim_end_matches('/');
    eprintln!("\x1b[2m[appspec] repo: {}\x1b[0m", origin);

    if a.spec {
        let url = format!("{}/packages/{}/{}/spec.appspec", base, pkg, ver);
        let (status, _h, body) = super::http::http_request(&url, "GET", &[], None)
            .with_context(|| format!("GET {}", url))?;
        if status == 404 {
            bail!("not found: {}:{}", pkg, ver);
        }
        if status != 200 {
            bail!("GET {} returned {}", url, status);
        }
        let text = std::str::from_utf8(&body).context("remote spec.appspec is not valid UTF-8")?;
        print!("{}", text);
        return Ok(0);
    }

    let meta_url = format!("{}/packages/{}/{}/meta.json", base, pkg, ver);
    let (status, _h, meta_body) = super::http::http_request(&meta_url, "GET", &[], None)
        .with_context(|| format!("GET {}", meta_url))?;
    if status == 404 {
        bail!("not found: {}:{}", pkg, ver);
    }
    if status != 200 {
        bail!("GET {} returned {}", meta_url, status);
    }
    let m: meta::MetaV1 = serde_json::from_slice(&meta_body).context("parsing remote meta.json")?;

    if let Some(out) = &a.sif {
        if m.sif_sha256.len() < 2 {
            bail!("malformed sif_sha256: {}", m.sif_sha256);
        }
        let ab = &m.sif_sha256[..2];
        let rest = &m.sif_sha256[2..];
        let blob_url = format!("{}/blobs/sha256/{}/{}", base, ab, rest);
        let (bs, _h, blob_body) = super::http::http_request(&blob_url, "GET", &[], None)
            .with_context(|| format!("GET {}", blob_url))?;
        if bs != 200 {
            bail!("GET {} returned {}", blob_url, bs);
        }
        if !a.no_verify {
            let actual = util::sha256_hex(&blob_body);
            if actual != m.sif_sha256 {
                bail!(
                    "blob sha256 mismatch: expected {}, got {}",
                    m.sif_sha256,
                    actual
                );
            }
        }
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(out, &blob_body).with_context(|| format!("writing {}", out.display()))?;
        println!("wrote {}", out.display());
        return Ok(0);
    }

    if a.json {
        println!("{}", serde_json::to_string_pretty(&m)?);
        return Ok(0);
    }
    print_meta_fields(&m);
    Ok(0)
}

// ===========================================================================
// serve
// ===========================================================================

#[derive(Debug, Args)]
pub struct ServeArgs {
    #[arg(long)]
    pub listen: Option<String>,
    #[arg(long)]
    pub root: Option<PathBuf>,
    /// Путь к `repo-server.toml`. Алиас `--server-config` — deprecated.
    #[arg(short = 'c', long = "config", alias = "server-config")]
    pub server_config: Option<PathBuf>,
}

pub fn serve(a: ServeArgs) -> Result<i32> {
    let mut root = require_local_root(a.root, a.server_config, "serve")?;
    if root.server_config_path.is_none() && root.config.server.tokens.is_empty() {
        eprintln!(
            "warning: no server config found; tokens are not configured. \
             Run `appspec repo server init --root {}` and `appspec repo token gen NAME`.",
            root.path.display()
        );
    }
    if let Some(l) = a.listen {
        root.config.server.listen = l;
    }
    super::server::serve(&root)?;
    Ok(0)
}

// ===========================================================================
// push
// ===========================================================================

#[derive(Debug, Args)]
pub struct PushArgs {
    pub sif: Vec<PathBuf>,
    /// Прямой URL сервера. Если не задан — берётся из `--profile`,
    /// `$APPSPEC_REPO`, или `default` в client config.
    #[arg(long)]
    pub to: Option<String>,
    #[arg(long)]
    pub profile: Option<String>,
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
    let _ = a.force;
    let (to, profile_name) = resolve_push_dest(&a)?;
    let token = resolve_push_token(&a, profile_name.as_deref())?;

    let mut failed = 0;
    for sif in &a.sif {
        match super::push::push_one(sif, &to, &token) {
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
            }
            Err(e) => {
                eprintln!("error: {}: {:#}", sif.display(), e);
                failed += 1;
            }
        }
    }
    Ok(if failed > 0 { 1 } else { 0 })
}

fn resolve_push_dest(a: &PushArgs) -> Result<(String, Option<String>)> {
    if let Some(t) = &a.to {
        return Ok((t.clone(), a.profile.clone()));
    }
    if let Some(pname) = &a.profile {
        let (_, cc) = config::find_client_config(None)
            .with_context(|| format!("loading client config for profile `{}`", pname))?;
        let p = cc
            .profile(pname)
            .ok_or_else(|| anyhow!("unknown profile `{}`", pname))?;
        let url = p
            .source
            .url
            .clone()
            .ok_or_else(|| anyhow!("profile `{}` has no url", pname))?;
        return Ok((url, Some(pname.clone())));
    }
    if let Ok(v) = std::env::var("APPSPEC_REPO") {
        if !v.is_empty() {
            if is_http_url(&v) {
                return Ok((v, None));
            }
            if let Ok((_, cc)) = config::find_client_config(None) {
                if let Some(p) = cc.profile(&v) {
                    if let Some(url) = &p.source.url {
                        return Ok((url.clone(), Some(v)));
                    }
                }
            }
        }
    }
    if let Ok((_, cc)) = config::find_client_config(None) {
        if let Some(def) = &cc.default {
            if let Some(p) = cc.profile(def) {
                if let Some(url) = &p.source.url {
                    return Ok((url.clone(), Some(def.clone())));
                }
            }
        }
    }
    bail!("no destination: pass --to URL, --profile NAME, or set $APPSPEC_REPO to a URL/profile")
}

fn resolve_push_token(a: &PushArgs, profile: Option<&str>) -> Result<String> {
    if let Some(tf) = &a.token_file {
        return Ok(std::fs::read_to_string(tf)
            .with_context(|| format!("reading token file {}", tf.display()))?
            .trim()
            .to_string());
    }
    if let Some(t) = &a.token {
        eprintln!("warning: --token exposes secrets; use --token-file or APPSPEC_REPO_TOKEN");
        return Ok(t.clone());
    }
    if let Some(pname) = profile {
        if let Ok((_, cc)) = config::find_client_config(None) {
            if let Some(p) = cc.profile(pname) {
                if let Some(t) = &p.token {
                    return config::resolve_token(t);
                }
            }
        }
    }
    if let Ok(t) = std::env::var("APPSPEC_REPO_TOKEN") {
        if !t.is_empty() {
            return Ok(t);
        }
    }
    bail!("no token: pass --token-file, APPSPEC_REPO_TOKEN, or set profile.token")
}

// ===========================================================================
// pull
// ===========================================================================

#[derive(Debug, Args)]
pub struct PullArgs {
    pub package_version: String,
    /// Source: PATH | http://... | ipns://NAME | ipfs://CID | oci://...
    /// If set -- fallback chain is not used (explicit choice).
    #[arg(long)]
    pub from: Option<String>,
    #[arg(long)]
    pub profile: Option<String>,
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
    /// Путь к `repo-server.toml`. Алиас `--server-config` — deprecated.
    #[arg(short = 'c', long = "config", alias = "server-config")]
    pub server_config: Option<PathBuf>,
}

pub fn pull(a: PullArgs) -> Result<i32> {
    // Sink: локальный RepoRoot.
    let sink = open_root(a.root.clone(), a.server_config.clone())?;
    let _lock = util::FileLock::acquire(&sink.lock_path())?;

    let sources = resolve_pull_sources(&a)?;
    if sources.is_empty() {
        bail!(
            "no source: pass --from PATH|URL|ipns://, --profile NAME,              or set $APPSPEC_REPO"
        );
    }
    if sources.len() == 1 {
        let f = &sources[0];
        if f.starts_with('/') || f.starts_with('~') || f.starts_with('.') {
            let src = config::expand_user_path(Path::new(f));
            if layout::norm_path(&src) == layout::norm_path(&sink.path) {
                eprintln!(
                    "warning: source and sink are the same directory;                      pull will be a no-op"
                );
            }
        }
    }

    super::pull::pull(
        &sink,
        &a.package_version,
        super::pull::PullOpts {
            sources,
            via: a.via,
            spec_only: a.spec_only,
            sif_out: a.sif_out,
            trusted_key: a.trusted_key,
            allow_unsigned: a.allow_unsigned,
        },
    )?;
    Ok(0)
}
fn resolve_pull_sources(a: &PullArgs) -> Result<Vec<String>> {
    if let Some(f) = &a.from {
        return Ok(vec![f.clone()]);
    }
    if let Some(pname) = &a.profile {
        let (_, cc) = config::find_client_config(None)
            .with_context(|| format!("loading client config for profile `{}`", pname))?;
        let p = cc
            .profile(pname)
            .ok_or_else(|| anyhow!("unknown profile `{}`", pname))?;
        return profile_sources(p);
    }
    if let Ok(v) = std::env::var("APPSPEC_REPO") {
        if !v.is_empty() {
            if is_url_like(&v) {
                return Ok(vec![v]);
            }
            if let Ok((_, cc)) = config::find_client_config(None) {
                if let Some(p) = cc.profile(&v) {
                    return profile_sources(p);
                }
            }
        }
    }
    if let Ok((_, cc)) = config::find_client_config(None) {
        if let Some(def) = &cc.default {
            if let Some(p) = cc.profile(def) {
                return profile_sources(p);
            }
        }
    }
    Ok(Vec::new())
}
// ===========================================================================
// sign / verify
// ===========================================================================

#[derive(Debug, Args)]
pub struct SignArgs {
    /// Явный путь к приватному ключу. Приоритет:
    /// `--key` → `[signing].key` из server config → `$APPSPEC_SIGNING_KEY`
    /// → `~/.config/appspec/signing.key`.
    #[arg(long)]
    pub key: Option<PathBuf>,
    #[arg(long)]
    pub root: Option<PathBuf>,
    /// Путь к `repo-server.toml`. Алиас `--server-config` — deprecated.
    #[arg(short = 'c', long = "config", alias = "server-config")]
    pub server_config: Option<PathBuf>,
}

pub fn sign(a: SignArgs) -> Result<i32> {
    let root = require_local_root(a.root, a.server_config, "sign")?;
    let _lock = util::FileLock::acquire(&root.lock_path())?;

    let key_path = if let Some(k) = a.key {
        config::expand_user_path(&k)
    } else if let Some(k) = &root.signing_key_path {
        k.clone()
    } else {
        super::sign::default_key_path()
    };

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

    // Save pubkey back into server config (if we know the path).
    if let Some(sc_path) = &root.server_config_path {
        if let Ok(mut sc) = config::ServerConfig::load(sc_path) {
            if sc.signing.pubkey != pubkey {
                sc.signing.pubkey = pubkey.clone();
                if let Err(e) = sc.save(sc_path) {
                    eprintln!(
                        "warning: cannot update [signing].pubkey in {}: {e:#}",
                        sc_path.display()
                    );
                }
            }
        }
    }

    println!("signed: {}", root.index_path().display());
    println!("pubkey: {}", pubkey);
    Ok(0)
}

#[derive(Debug, Args)]
pub struct VerifyArgs {
    #[arg(long)]
    pub pubkey: Option<PathBuf>,
    #[arg(long)]
    pub root: Option<PathBuf>,
}

pub fn verify(a: VerifyArgs) -> Result<i32> {
    let origin = resolve_origin(&a.root)?;

    if is_http_url(&origin) {
        eprintln!("\x1b[2m[appspec] repo: {}\x1b[0m", origin);
        let url = format!("{}/index.json", origin.trim_end_matches('/'));
        let (status, _h, body) = super::http::http_request(&url, "GET", &[], None)
            .with_context(|| format!("GET {}", url))?;
        if status != 200 {
            bail!("GET {} returned {}", url, status);
        }
        let index_text = String::from_utf8(body).context("remote index.json is not valid UTF-8")?;

        let pubkey = if let Some(p) = &a.pubkey {
            let s = std::fs::read_to_string(p)
                .with_context(|| format!("reading pubkey {}", p.display()))?;
            super::sign::parse_pubkey(s.trim())?
        } else {
            bail!(
                "no public key: pass --pubkey PATH \
                 (удалённый `{}` не отдаёт [signing].pubkey)",
                origin
            );
        };

        super::sign::verify_index(&index_text, &pubkey)?;
        println!("verify: OK");
        return Ok(0);
    }

    if origin.starts_with("ipfs://") {
        bail!("ipfs:// verify not supported in v1");
    }

    let root = layout::RepoRoot::open(Path::new(&origin))?;
    let index_text = std::fs::read_to_string(root.index_path())?;

    let pubkey = if let Some(p) = &a.pubkey {
        let s = std::fs::read_to_string(p)
            .with_context(|| format!("reading pubkey {}", p.display()))?;
        super::sign::parse_pubkey(s.trim())?
    } else {
        let s = root.config.signing.pubkey.clone();
        if s.is_empty() {
            bail!("no public key: pass --pubkey PATH or set [signing].pubkey in server config");
        }
        super::sign::parse_pubkey(&s)?
    };

    super::sign::verify_index(&index_text, &pubkey)?;
    println!("verify: OK");
    Ok(0)
}

// ===========================================================================
// publish-index / export-oci
// ===========================================================================

#[derive(Debug, Args)]
pub struct PublishIndexArgs {
    #[arg(long)]
    pub pin: bool,
    /// Опубликовать по IPNS (`ipfs name publish`). Требует `[ipfs].ipns_key`
    /// в server config. По умолчанию включается, если `ipns_key` задан.
    #[arg(long, conflicts_with = "no_ipns")]
    pub ipns: bool,
    /// Не публиковать по IPNS, даже если `[ipfs].ipns_key` задан.
    #[arg(long, conflicts_with = "ipns")]
    pub no_ipns: bool,
    #[arg(long)]
    pub root: Option<PathBuf>,
    /// Путь к `repo-server.toml`. Алиас `--server-config` — deprecated.
    #[arg(short = 'c', long = "config", alias = "server-config")]
    pub server_config: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ResolveArgs {
    /// IPNS name (`k51…`, `12D3Koo…` или другой libp2p-key CID).
    pub ipns_name: String,
    /// Таймаут резолва (сек). Использует `timeout` из PATH, если доступен.
    #[arg(long, default_value_t = 30)]
    pub timeout_secs: u64,
}

pub fn publish_index(a: PublishIndexArgs) -> Result<i32> {
    let root = require_local_root(a.root, a.server_config, "publish-index")?;
    let index_path = root.index_path();
    if !index_path.is_file() {
        bail!("index.json missing at {}", index_path.display());
    }

    // IPNS: явно (--ipns), явно выключено (--no-ipns), либо default
    // из конфига (ipns_key.is_some()).
    let want_ipns = if a.ipns {
        true
    } else if a.no_ipns {
        false
    } else {
        root.config.ipfs.ipns_key.is_some()
    };

    // Fail-fast: --ipns требует [ipfs].ipns_key. Проверяем ДО ipfs add,
    // чтобы не делать side-effect (пиннинг, запись <root>/ipfs-index.cid,
    // обновление server config) при заведомо провальной команде.
    // Побочный эффект: тест publish_without_ipns_key_fails_when_forced
    // становится детерминированным — ошибка не зависит от состояния
    // mock ipfs.
    if want_ipns && root.config.ipfs.ipns_key.is_none() {
        bail!(
            "`--ipns` requires `[ipfs].ipns_key` in server config; \
             run `ipfs key gen <name>` and add `[ipfs].ipns_key = \"<name>\"`"
        );
    }

    let cid = super::ipfs::add_file(&index_path, a.pin)?;
    super::ipfs::validate_cid(&cid)?;

    // Локальный кеш CID — для отладки и скриптов.
    let cid_path = root.path.join("ipfs-index.cid");
    std::fs::write(&cid_path, format!("{}\n", cid))?;

    if want_ipns {
        let key = root
            .config
            .ipfs
            .ipns_key
            .as_ref()
            .expect("validated above: want_ipns => ipns_key is Some");
        let ipns_name = super::ipfs::name_publish(key, &cid)
            .with_context(|| format!("ipfs name publish (key={})", key))?;

        // Обновить [ipfs].ipns_name в server config — это кеш для
        // отображения; источник истины — DHT. Если путь к конфигу
        // неизвестен (data-only репо) — предупреждаем, но не падаем.
        match &root.server_config_path {
            Some(sc_path) => match config::ServerConfig::load(sc_path) {
                Ok(mut sc) => {
                    if sc.ipfs.ipns_name.as_deref() != Some(&ipns_name) {
                        sc.ipfs.ipns_name = Some(ipns_name.clone());
                        if let Err(e) = sc.save(sc_path) {
                            eprintln!(
                                "warning: cannot update [ipfs].ipns_name in {}: {e:#}",
                                sc_path.display()
                            );
                        }
                    }
                }
                Err(e) => {
                    eprintln!("warning: cannot reload {}: {e:#}", sc_path.display());
                }
            },
            None => {
                eprintln!(
                    "warning: no server config path; \
                     [ipfs].ipns_name is not persisted (ipns={}, index={})",
                    ipns_name, cid
                );
            }
        }

        eprintln!("published: {} (index {})", ipns_name, cid);
    }

    println!("{}", cid);
    Ok(0)
}

pub fn resolve(a: ResolveArgs) -> Result<i32> {
    let cid = super::ipfs::name_resolve(&a.ipns_name, a.timeout_secs)?;
    println!("{}", cid);
    Ok(0)
}

#[derive(Debug, Args)]
pub struct ExportOciArgs {
    pub target: String,
    #[arg(long)]
    pub registry: Option<String>,
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub root: Option<PathBuf>,
    /// Путь к `repo-server.toml`. Алиас `--server-config` — deprecated.
    #[arg(short = 'c', long = "config", alias = "server-config")]
    pub server_config: Option<PathBuf>,
}

pub fn export_oci(a: ExportOciArgs) -> Result<i32> {
    let root = require_local_root(a.root, a.server_config, "export-oci")?;
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

// ===========================================================================
// token
// ===========================================================================

#[derive(Debug, Subcommand)]
pub enum TokenCommand {
    /// Сгенерировать токен и записать его sha256 в server config.
    Gen(TokenGenArgs),
    /// Список токенов в server config.
    Ls(TokenLsArgs),
    /// Удалить токен по имени.
    Rm(TokenRmArgs),
    /// Проверить, что plaintext-токен совпадает с сохранённым хешем.
    Check(TokenCheckArgs),
}

#[derive(Debug, Args)]
pub struct TokenGenArgs {
    pub name: String,
    /// Путь к `repo-server.toml`. Алиас `--server-config` — deprecated.
    #[arg(short = 'c', long = "config", alias = "server-config")]
    pub server_config: Option<PathBuf>,
    /// Записать plaintext в файл (а не в stdout).
    #[arg(long)]
    pub write_file: Option<PathBuf>,
    /// Режим файла при `--write-file`, восьмеричный, как в `chmod`.
    /// Примеры: `400`, `0400`, `0o400`. По умолчанию `0o400`.
    #[arg(long)]
    pub mode: Option<String>,
}

#[derive(Debug, Args)]
pub struct TokenLsArgs {
    /// Путь к `repo-server.toml`. Алиас `--server-config` — deprecated.
    #[arg(short = 'c', long = "config", alias = "server-config")]
    pub server_config: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct TokenRmArgs {
    pub name: String,
    /// Путь к `repo-server.toml`. Алиас `--server-config` — deprecated.
    #[arg(short = 'c', long = "config", alias = "server-config")]
    pub server_config: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct TokenCheckArgs {
    pub name: String,
    /// Путь к `repo-server.toml`. Алиас `--server-config` — deprecated.
    #[arg(short = 'c', long = "config", alias = "server-config")]
    pub server_config: Option<PathBuf>,
    #[arg(long)]
    pub value: Option<String>,
    #[arg(long)]
    pub file: Option<PathBuf>,
}

pub fn token(cmd: TokenCommand) -> Result<i32> {
    match cmd {
        TokenCommand::Gen(a) => token_gen(a),
        TokenCommand::Ls(a) => token_ls(a),
        TokenCommand::Rm(a) => token_rm(a),
        TokenCommand::Check(a) => token_check(a),
    }
}

fn token_gen(a: TokenGenArgs) -> Result<i32> {
    let (path, mut cfg) = config::find_server_config(a.server_config.as_deref())?;
    if cfg.server.tokens.iter().any(|t| t.name == a.name) {
        bail!("token `{}` already exists in {}", a.name, path.display());
    }
    let raw = util::hex_encode(&util::random_bytes(32)?);
    let hash = format!("sha256:{}", util::sha256_hex(raw.as_bytes()));
    cfg.server.tokens.push(layout::ServerToken {
        name: a.name.clone(),
        hash,
    });
    cfg.save(&path)?;

    if let Some(out) = a.write_file {
        let out = config::expand_user_path(&out);
        // `--mode` — восьмеричный, как в `chmod`: 400, 0400, 0o400.
        // Дефолт 0o400 (r--------).
        let mode = match &a.mode {
            Some(s) => parse_octal_mode(s)?,
            None => 0o400,
        };
        config::write_secret_file(&out, &raw, mode)?;
        eprintln!(
            "token `{}` written to {} (mode {:o})",
            a.name,
            out.display(),
            mode
        );
        eprintln!("(save it now; only sha256 is stored in {})", path.display());
    } else {
        println!("token {}: {}", a.name, raw);
        println!("(save it now; only sha256 is stored in {})", path.display());
    }
    Ok(0)
}

/// Восьмеричный режим: `400`, `0400`, `0o400`.
fn parse_octal_mode(s: &str) -> Result<u32> {
    let t = s.trim();
    let t = t.strip_prefix("0o").unwrap_or(t);
    let v = u32::from_str_radix(t, 8)
        .with_context(|| format!("--mode must be octal (e.g. 400, 0400, 0o400), got `{}`", s))?;
    if v & !0o777 != 0 {
        bail!("--mode out of range: {:o} (allowed 0..=0777)", v);
    }
    Ok(v)
}

fn token_ls(a: TokenLsArgs) -> Result<i32> {
    let (path, cfg) = config::find_server_config(a.server_config.as_deref())?;
    println!("server config: {}", path.display());
    if cfg.server.tokens.is_empty() {
        println!("  (no tokens)");
        return Ok(0);
    }
    for t in &cfg.server.tokens {
        let short = if t.hash.len() > 20 {
            format!("{}…", &t.hash[..17])
        } else {
            t.hash.clone()
        };
        println!("  {:<20}  {}", t.name, short);
    }
    Ok(0)
}

fn token_rm(a: TokenRmArgs) -> Result<i32> {
    let (path, mut cfg) = config::find_server_config(a.server_config.as_deref())?;
    let before = cfg.server.tokens.len();
    cfg.server.tokens.retain(|t| t.name != a.name);
    if cfg.server.tokens.len() == before {
        bail!("token `{}` not found in {}", a.name, path.display());
    }
    cfg.save(&path)?;
    println!("removed token `{}` from {}", a.name, path.display());
    Ok(0)
}

fn token_check(a: TokenCheckArgs) -> Result<i32> {
    let (path, cfg) = config::find_server_config(a.server_config.as_deref())?;
    let provided = if let Some(v) = a.value {
        v
    } else if let Some(f) = a.file {
        std::fs::read_to_string(&f)
            .with_context(|| format!("reading {}", f.display()))?
            .trim()
            .to_string()
    } else {
        use std::io::Read;
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        s.trim().to_string()
    };
    if provided.is_empty() {
        bail!("empty token; pass --value, --file, or pipe to stdin");
    }
    let provided_hash = format!("sha256:{}", util::sha256_hex(provided.as_bytes()));
    match cfg.server.tokens.iter().find(|t| t.name == a.name) {
        None => {
            eprintln!("token `{}` not found in {}", a.name, path.display());
            Ok(1)
        }
        Some(t) if t.hash == provided_hash => {
            println!("ok");
            Ok(0)
        }
        Some(_) => {
            eprintln!("mismatch");
            Ok(1)
        }
    }
}

// ===========================================================================
// migrate-config
// ===========================================================================

#[derive(Debug, Args)]
pub struct MigrateConfigArgs {
    /// Data-директория с legacy `<root>/config.toml`.
    pub root: PathBuf,
    /// Куда писать новый server config. По умолчанию — стандартная
    /// discovery-цепочка (`$APPSPEC_REPO_SERVER_CONFIG`, XDG, /etc).
    /// Путь к `repo-server.toml`. Алиас `--server-config` — deprecated.
    #[arg(short = 'c', long = "config", alias = "server-config")]
    pub server_config: Option<PathBuf>,
}

pub fn migrate_config(a: MigrateConfigArgs) -> Result<i32> {
    let root = config::expand_user_path(&a.root);
    let legacy = root.join("config.toml");
    if !legacy.is_file() {
        bail!("no config.toml at {}; nothing to migrate", root.display());
    }
    let text = std::fs::read_to_string(&legacy)
        .with_context(|| format!("reading {}", legacy.display()))?;
    let legacy_cfg: layout::Config =
        toml::from_str(&text).with_context(|| format!("parsing {}", legacy.display()))?;
    if legacy_cfg.repo.layout != layout::LAYOUT_VERSION {
        bail!(
            "unsupported repository layout {} (this client supports {})",
            legacy_cfg.repo.layout,
            layout::LAYOUT_VERSION
        );
    }

    let target = if let Some(p) = &a.server_config {
        config::expand_user_path(p)
    } else if let Ok(v) = std::env::var("APPSPEC_REPO_SERVER_CONFIG") {
        PathBuf::from(v)
    } else if let Some(x) = config::xdg_config_home() {
        x.join("appspec/repo-server.toml")
    } else {
        bail!("cannot determine server config path; pass --server-config");
    };
    if target.is_file() {
        bail!(
            "server config already exists at {}; pass --server-config to use a different path",
            target.display()
        );
    }

    let sc = config::ServerConfig::from_legacy(layout::norm_path(&root), legacy_cfg);
    sc.save(&target)?;

    let migrated = root.join(".config.toml.migrated");
    std::fs::rename(&legacy, &migrated)?;

    println!("migrated: {}", target.display());
    println!("  [repo].root = {}", root.display());
    println!("  renamed: {} → {}", legacy.display(), migrated.display());
    Ok(0)
}

// ===========================================================================
// server {init}
// ===========================================================================

#[derive(Debug, Subcommand)]
pub enum ServerCommand {
    /// Создать server config для существующего data-only репозитория.
    Init(ServerInitArgs),
}

#[derive(Debug, Args)]
pub struct ServerInitArgs {
    /// Data-директория репозитория (должна содержать `index.json`).
    #[arg(long)]
    pub root: PathBuf,
    /// Куда писать server config. По умолчанию — discovery-цепочка.
    /// Путь к `repo-server.toml`. Алиас `--server-config` — deprecated.
    #[arg(short = 'c', long = "config", alias = "server-config")]
    pub server_config: Option<PathBuf>,
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long)]
    pub description: Option<String>,
    #[arg(long)]
    pub listen: Option<String>,
}

pub fn server(cmd: ServerCommand) -> Result<i32> {
    match cmd {
        ServerCommand::Init(a) => server_init(a),
    }
}

fn server_init(a: ServerInitArgs) -> Result<i32> {
    let root = layout::norm_path(&config::expand_user_path(&a.root));
    if !root.join("index.json").is_file() {
        bail!(
            "{} does not look like a repository (missing index.json); \
             run `appspec repo init {}` first",
            root.display(),
            root.display()
        );
    }
    let target = if let Some(p) = &a.server_config {
        config::expand_user_path(p)
    } else if let Ok(v) = std::env::var("APPSPEC_REPO_SERVER_CONFIG") {
        PathBuf::from(v)
    } else if let Some(x) = config::xdg_config_home() {
        x.join("appspec/repo-server.toml")
    } else {
        bail!("cannot determine server config path; pass --server-config");
    };
    if target.is_file() {
        bail!(
            "server config already exists: {}; \
             use --server-config with a new path, or edit the existing one",
            target.display()
        );
    }

    let cfg = config::ServerConfig {
        schema: config::CONFIG_SCHEMA,
        repo: config::ServerRepo {
            root: root.clone(),
            name: a.name.unwrap_or_else(|| "local/appspec-repo".to_string()),
            description: a.description.unwrap_or_default(),
            created_at: util::now_iso8601(),
            generator: format!("appspec {}", env!("CARGO_PKG_VERSION")),
        },
        policy: Default::default(),
        signing: config::ServerSigning::default(),
        server: layout::Server {
            listen: a.listen.unwrap_or_else(|| "127.0.0.1:8080".into()),
            ..Default::default()
        },
        ipfs: Default::default(),
    };
    cfg.save(&target)?;
    println!("wrote server config: {}", target.display());
    println!("  [repo].root = {}", root.display());
    Ok(0)
}

// ===========================================================================
// client {init, ls}
// ===========================================================================

#[derive(Debug, Subcommand)]
pub enum ClientCommand {
    /// Создать или обновить профиль в client config.
    Init(ClientInitArgs),
    /// Список профилей в client config.
    Ls(ClientLsArgs),
}

#[derive(Debug, Args)]
pub struct ClientInitArgs {
    #[arg(long)]
    pub profile: String,
    #[arg(long, conflicts_with_all = ["source_url", "source_ipns"])]
    pub source_path: Option<PathBuf>,
    #[arg(long, conflicts_with = "source_path")]
    pub source_url: Option<String>,
    /// IPNS name. Combines with `--source-url` for fallback chain: ipns -> url.
    #[arg(long, conflicts_with = "source_path")]
    pub source_ipns: Option<String>,
    #[arg(long)]
    pub token_env: Option<String>,
    #[arg(long)]
    pub token_file: Option<PathBuf>,
    #[arg(long)]
    pub default: bool,
    #[arg(long)]
    pub client_config: Option<PathBuf>,
}
#[derive(Debug, Args)]
pub struct ClientLsArgs {
    #[arg(long)]
    pub client_config: Option<PathBuf>,
    #[arg(long)]
    pub json: bool,
}

pub fn client(cmd: ClientCommand) -> Result<i32> {
    match cmd {
        ClientCommand::Init(a) => client_init(a),
        ClientCommand::Ls(a) => client_ls(a),
    }
}

fn build_source_table(a: &ClientInitArgs) -> Result<config::RepoSourceTable> {
    let path = a.source_path.as_ref();
    let url = a.source_url.as_ref();
    let ipns = a.source_ipns.as_ref();
    if path.is_some() && (url.is_some() || ipns.is_some()) {
        bail!("--source-path cannot be combined with --source-url/--source-ipns");
    }
    if path.is_none() && url.is_none() && ipns.is_none() {
        bail!("pass at least one of --source-path, --source-url, --source-ipns");
    }
    let mut table = config::RepoSourceTable::default();
    if let Some(p) = path {
        table.path = Some(config::expand_user_path(p));
    }
    if let Some(u) = url {
        if !u.starts_with("http://") && !u.starts_with("https://") {
            bail!("--source-url must start with http:// or https://, got `{u}`");
        }
        table.url = Some(u.clone());
    }
    if let Some(i) = ipns {
        table.ipns = Some(i.clone());
    }
    table.validate("--source-*")?;
    Ok(table)
}
fn client_init(a: ClientInitArgs) -> Result<i32> {
    let source = build_source_table(&a)?;
    let token = match (a.token_env, a.token_file) {
        (Some(v), None) => Some(format!("env:{}", v)),
        (None, Some(f)) => Some(format!("file:{}", config::expand_user_path(&f).display())),
        (None, None) => None,
        (Some(_), Some(_)) => {
            bail!("--token-env and --token-file are mutually exclusive")
        }
    };

    let profile = config::RepoProfile {
        name: a.profile.clone(),
        source,
        token,
        proxy: None,
        trusted_keys: Vec::new(),
        allow_unsigned: None,
    };

    let (path, mut cfg) = if let Some(p) = &a.client_config {
        let p = config::expand_user_path(p);
        match config::ClientConfig::load(&p) {
            Ok(c) => (p, c),
            Err(_) => (
                p,
                config::ClientConfig {
                    schema: config::CONFIG_SCHEMA,
                    default: None,
                    repos: Vec::new(),
                },
            ),
        }
    } else {
        config::load_or_default_client_config()?
    };

    if let Some(existing) = cfg.repos.iter_mut().find(|r| r.name == a.profile) {
        *existing = profile;
    } else {
        cfg.repos.push(profile);
    }
    if a.default {
        cfg.default = Some(a.profile.clone());
    }
    cfg.save(&path)?;

    println!("updated client config: {}", path.display());
    println!("  profile `{}`", a.profile);
    Ok(0)
}

fn client_ls(a: ClientLsArgs) -> Result<i32> {
    let (path, cfg) = config::find_client_config(a.client_config.as_deref())?;
    if a.json {
        println!("{}", serde_json::to_string_pretty(&cfg)?);
        return Ok(0);
    }
    println!("client config: {}", path.display());
    if let Some(d) = &cfg.default {
        println!("  default: {}", d);
    }
    for r in &cfg.repos {
        println!(
            "  {:<20}  {}{}",
            r.name,
            r.source.origin(),
            if r.token.is_some() { "  [token]" } else { "" }
        );
    }
    Ok(0)
}
