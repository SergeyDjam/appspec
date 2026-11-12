//! Configuration: server (`repo-server.toml`) и client (`repos.toml`).
//!
//! Server config живёт вне repository root. Repository root — data-only
//! (`index.json`, `blobs/`, `packages/`, `refs/`, `.lock`), без `config.toml`.
//! Legacy `<root>/config.toml` поддерживается в read-режиме с warning.
//!
//! Client config — набор именованных профилей источников (path/url/ipfs),
//! плюс per-profile токен, trusted_keys, proxy.
//!
//! Discovery:
//!   server: --server-config → $APPSPEC_REPO_SERVER_CONFIG
//!           → $XDG_CONFIG_HOME/appspec/repo-server.toml
//!           → /etc/appspec/repo-server.toml
//!           → (legacy) <root>/config.toml с warning
//!   client: --client-config → $APPSPEC_REPO_CLIENT_CONFIG
//!           → $XDG_CONFIG_HOME/appspec/repos.toml
//!           → /etc/appspec/repos.toml

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::layout;

pub const CONFIG_SCHEMA: u32 = 1;

// ===========================================================================
// Server config
// ===========================================================================

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerConfig {
    pub schema: u32,
    pub repo: ServerRepo,
    #[serde(default)]
    pub policy: layout::Policy,
    #[serde(default)]
    pub signing: ServerSigning,
    #[serde(default)]
    pub server: layout::Server,
    #[serde(default)]
    pub ipfs: layout::Ipfs,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerRepo {
    /// Абсолютный путь к данным. `~` раскрывается при загрузке.
    pub root: PathBuf,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub generator: String,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct ServerSigning {
    /// Приватный ключ. Если None — fallback:
    /// `$APPSPEC_SIGNING_KEY` → `~/.config/appspec/signing.key`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<PathBuf>,
    #[serde(default)]
    pub pubkey: String,
}

impl ServerConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut cfg: Self =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        cfg.normalize()
            .with_context(|| format!("validating {}", path.display()))?;
        Ok(cfg)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let text = toml::to_string_pretty(self).context("serializing server config")?;
        std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("chmod 0600 {}", path.display()))?;
        }
        Ok(())
    }

    pub fn normalize(&mut self) -> Result<()> {
        self.repo.root = expand_user_path(&self.repo.root);
        self.signing.key = self.signing.key.as_ref().map(|p| expand_user_path(p));
        self.validate()
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != CONFIG_SCHEMA {
            bail!(
                "server config: unsupported schema {} (this client supports {})",
                self.schema,
                CONFIG_SCHEMA
            );
        }
        if self.repo.root.as_os_str().is_empty() {
            bail!("server config: [repo].root is required");
        }
        if !self.repo.root.is_absolute() {
            bail!(
                "server config: [repo].root must be absolute, got `{}`",
                self.repo.root.display()
            );
        }
        if self.repo.name.trim().is_empty() {
            bail!("server config: [repo].name is required");
        }
        if let Some(key) = &self.ipfs.ipns_key {
            if key.trim().is_empty() || key.chars().any(|c| c.is_whitespace()) {
                bail!("server config: [ipfs].ipns_key must be non-empty and contain no whitespace");
            }
        }
        if let Some(name) = &self.ipfs.ipns_name {
            if name.trim().is_empty() || name.chars().any(|c| c.is_whitespace()) {
                bail!(
                    "server config: [ipfs].ipns_name must be non-empty and contain no whitespace"
                );
            }
        }
        Ok(())
    }

    pub fn from_legacy(root: PathBuf, legacy: layout::Config) -> Self {
        Self {
            schema: CONFIG_SCHEMA,
            repo: ServerRepo {
                root,
                name: legacy.repo.name,
                description: legacy.repo.description,
                created_at: legacy.repo.created_at,
                generator: legacy.repo.generator,
            },
            policy: legacy.policy,
            signing: ServerSigning {
                key: None,
                pubkey: legacy.signing.pubkey,
            },
            server: legacy.server,
            ipfs: legacy.ipfs,
        }
    }

    /// Обратное преобразование в `layout::Config`. Нужно потому, что
    /// `RepoRoot.config: layout::Config` — стабильный публичный API
    /// (используется `add`, `check`, `gc`, `sign`, `server`).
    pub fn into_layout(self) -> layout::Config {
        layout::Config {
            repo: layout::RepoMeta {
                name: self.repo.name,
                description: self.repo.description,
                created_at: self.repo.created_at,
                generator: self.repo.generator,
                layout: layout::LAYOUT_VERSION,
            },
            policy: self.policy,
            signing: layout::Signing {
                pubkey: self.signing.pubkey,
            },
            server: self.server,
            ipfs: self.ipfs,
        }
    }
}

// ===========================================================================
// Client config
// ===========================================================================

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ClientConfig {
    pub schema: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default)]
    pub repos: Vec<RepoProfile>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RepoProfile {
    pub name: String,
    pub source: RepoSourceTable,
    /// `env:VAR` | `file:PATH` | `keyring:SERVICE` | `literal:…`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trusted_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_unsigned: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RepoSourceTable {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// IPNS name (`k51...` для ed25519 или `12D3Koo...` для RSA).
    /// Может комбинироваться с `url` как fallback chain: `ipns -> url`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipns: Option<String>,
    /// Legacy-поле из v1 (сырой index-CID). При чтении выдаёт warning.
    /// Новые конфиги должны использовать `ipns`. Удаляется в v2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipfs: Option<String>,
}

impl RepoSourceTable {
    #[allow(dead_code)]
    pub fn kind_count(&self) -> usize {
        [
            self.path.is_some(),
            self.url.is_some(),
            self.ipns.is_some(),
            self.ipfs.is_some(),
        ]
        .into_iter()
        .filter(|b| *b)
        .count()
    }

    pub fn validate(&self, ctx: &str) -> Result<()> {
        if self.path.is_some() && (self.url.is_some() || self.ipns.is_some() || self.ipfs.is_some())
        {
            bail!(
                "{ctx}: source.path cannot be combined with url/ipns/ipfs                  (path is a local repository)"
            );
        }
        if self.path.is_none() && self.url.is_none() && self.ipns.is_none() && self.ipfs.is_none() {
            bail!(
                "{ctx}: source must contain `path`, or at least one of                  `url` | `ipns` (found none)"
            );
        }
        if let Some(u) = &self.url {
            if !u.starts_with("http://") && !u.starts_with("https://") {
                bail!("{ctx}: source.url must start with http:// or https://, got `{u}`");
            }
        }
        if let Some(i) = &self.ipns {
            validate_ipns_name(i).with_context(|| format!("{ctx}: source.ipns"))?;
        }
        if let Some(i) = &self.ipfs {
            crate::log(format!(
                "warning: {ctx}: source.ipfs is deprecated; use source.ipns"
            ));
            if i.trim().is_empty() {
                bail!("{ctx}: source.ipfs must not be empty (legacy field)");
            }
        }
        Ok(())
    }

    pub fn origin(&self) -> String {
        if let Some(p) = &self.path {
            return format!("path:{}", p.display());
        }
        if let Some(i) = &self.ipns {
            return format!("ipns:{}", i);
        }
        if let Some(u) = &self.url {
            return format!("url:{}", u);
        }
        if let Some(i) = &self.ipfs {
            return format!("ipfs:{} (legacy)", i);
        }
        "?".into()
    }
}

fn validate_ipns_name(s: &str) -> Result<()> {
    let t = s.trim();
    if t.is_empty() {
        bail!("empty IPNS name");
    }
    if t.chars().any(|c| c.is_whitespace() || c.is_control()) {
        bail!("IPNS name contains whitespace/control chars: `{}`", s);
    }
    if !(t.starts_with("k51") || t.starts_with("12D3Koo")) {
        bail!(
            "IPNS name must start with `k51` (ed25519) or `12D3Koo` (RSA), got `{}`",
            s
        );
    }
    Ok(())
}
impl ClientConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut cfg: Self =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        cfg.normalize();
        cfg.validate()
            .with_context(|| format!("validating {}", path.display()))?;
        Ok(cfg)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let text = toml::to_string_pretty(self).context("serializing client config")?;
        std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("chmod 0600 {}", path.display()))?;
        }
        Ok(())
    }

    fn normalize(&mut self) {
        for r in &mut self.repos {
            if let Some(p) = &r.source.path {
                r.source.path = Some(expand_user_path(p));
            }
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != CONFIG_SCHEMA {
            bail!(
                "client config: unsupported schema {} (this client supports {})",
                self.schema,
                CONFIG_SCHEMA
            );
        }
        let mut seen = std::collections::HashSet::new();
        for r in &self.repos {
            if r.name.trim().is_empty() {
                bail!("client config: profile with empty `name`");
            }
            if !seen.insert(r.name.as_str()) {
                bail!("client config: duplicate profile `{}`", r.name);
            }
            r.source.validate(&format!("profile `{}`", r.name))?;
            if let Some(t) = &r.token {
                validate_token_source(t).with_context(|| format!("profile `{}` token", r.name))?;
            }
        }
        if let Some(d) = &self.default {
            if !seen.contains(d.as_str()) {
                bail!(
                    "client config: default `{}` is not a known profile; \
                     known: {:?}",
                    d,
                    seen.iter().collect::<Vec<_>>()
                );
            }
        }
        Ok(())
    }

    pub fn profile(&self, name: &str) -> Option<&RepoProfile> {
        self.repos.iter().find(|r| r.name == name)
    }
}

fn validate_token_source(t: &str) -> Result<()> {
    let (kind, rest) = t.split_once(':').ok_or_else(|| {
        anyhow!("token must be `env:VAR` | `file:PATH` | `keyring:SERVICE` | `literal:…`")
    })?;
    if rest.is_empty() {
        bail!("token `{}`: empty payload after `{}:`", t, kind);
    }
    match kind {
        "env" | "file" | "keyring" | "literal" => Ok(()),
        other => bail!(
            "token `{}`: unknown source `{}` (expected env | file | keyring | literal)",
            t,
            other
        ),
    }
}

// ===========================================================================
// Token resolution (из client profile)
// ===========================================================================

pub fn resolve_token(t: &str) -> Result<String> {
    let (kind, rest) = t
        .split_once(':')
        .ok_or_else(|| anyhow!("token must be `env:VAR` | `file:PATH` | ..."))?;
    match kind {
        "env" => {
            std::env::var(rest).with_context(|| format!("token env var `${}` is not set", rest))
        }
        "file" => {
            let p = expand_user_path(Path::new(rest));
            let s = std::fs::read_to_string(&p)
                .with_context(|| format!("reading token file {}", p.display()))?;
            Ok(s.trim().to_string())
        }
        "literal" => {
            eprintln!("warning: token literal in client config; prefer env: or file:");
            Ok(rest.to_string())
        }
        "keyring" => bail!("token keyring backend not implemented in v1"),
        other => bail!("token source `{}` not supported", other),
    }
}

// ===========================================================================
// Discovery
// ===========================================================================

pub fn xdg_config_home() -> Option<PathBuf> {
    if let Some(v) = std::env::var_os("XDG_CONFIG_HOME") {
        let p = PathBuf::from(v);
        if p.is_absolute() {
            return Some(p);
        }
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))
}

pub fn server_config_paths() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(p) = std::env::var_os("APPSPEC_REPO_SERVER_CONFIG") {
        let p = PathBuf::from(p);
        if !p.as_os_str().is_empty() {
            v.push(p);
        }
    }
    if let Some(x) = xdg_config_home() {
        v.push(x.join("appspec/repo-server.toml"));
    }
    v.push(PathBuf::from("/etc/appspec/repo-server.toml"));
    v
}

pub fn find_server_config(explicit: Option<&Path>) -> Result<(PathBuf, ServerConfig)> {
    if let Some(p) = explicit {
        let p = expand_user_path(p);
        let cfg = ServerConfig::load(&p)?;
        return Ok((p, cfg));
    }
    let paths = server_config_paths();
    for p in &paths {
        if p.is_file() {
            let cfg = ServerConfig::load(p)?;
            return Ok((p.clone(), cfg));
        }
    }
    bail!(
        "server config not found; looked in: {}",
        paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Server config + legacy `<root>/config.toml`.
///
/// Возвращает:
///   * `Ok(Some((path, cfg)))` — конфиг найден (discovery или legacy).
///     Для legacy печатает warning о deprecation.
///   * `Ok(None)` — ни discovery, ни legacy не дали результата.
///   * `Err` — конфиг найден, но невалиден (например, layout ≠ 1).
///
/// Явный `--server-config` (`explicit = Some`) выключает legacy:
/// если указали — ищем строго там и не смотрим в `<root>/config.toml`.
pub fn find_server_config_with_legacy(
    explicit: Option<&Path>,
    root_hint: Option<&Path>,
) -> Result<Option<(PathBuf, ServerConfig)>> {
    if let Some(p) = explicit {
        let p = expand_user_path(p);
        let cfg = ServerConfig::load(&p)?;
        return Ok(Some((p, cfg)));
    }

    // 1. Discovery: ищем существующий файл.
    for p in server_config_paths() {
        if p.is_file() {
            let cfg = ServerConfig::load(&p)?;
            return Ok(Some((p, cfg)));
        }
    }

    // 2. Legacy `<root>/config.toml`.
    if let Some(root) = root_hint {
        let legacy_path = root.join("config.toml");
        if legacy_path.is_file() {
            eprintln!(
                "warning: {} is deprecated; run \
                 `appspec repo migrate-config {}`",
                legacy_path.display(),
                root.display()
            );
            let text = std::fs::read_to_string(&legacy_path)
                .with_context(|| format!("reading {}", legacy_path.display()))?;
            let legacy: layout::Config = toml::from_str(&text)
                .with_context(|| format!("parsing {}", legacy_path.display()))?;
            if legacy.repo.layout != layout::LAYOUT_VERSION {
                bail!(
                    "unsupported repository layout {} \
                     (this client supports {})",
                    legacy.repo.layout,
                    layout::LAYOUT_VERSION
                );
            }
            let cfg = ServerConfig::from_legacy(root.to_path_buf(), legacy);
            cfg.validate()?;
            return Ok(Some((legacy_path, cfg)));
        }
    }

    Ok(None)
}

pub fn client_config_paths() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(p) = std::env::var_os("APPSPEC_REPO_CLIENT_CONFIG") {
        let p = PathBuf::from(p);
        if !p.as_os_str().is_empty() {
            v.push(p);
        }
    }
    if let Some(x) = xdg_config_home() {
        v.push(x.join("appspec/repos.toml"));
    }
    v.push(PathBuf::from("/etc/appspec/repos.toml"));
    v
}

pub fn find_client_config(explicit: Option<&Path>) -> Result<(PathBuf, ClientConfig)> {
    if let Some(p) = explicit {
        let p = expand_user_path(p);
        let cfg = ClientConfig::load(&p)?;
        return Ok((p, cfg));
    }
    let paths = client_config_paths();
    for p in &paths {
        if p.is_file() {
            let cfg = ClientConfig::load(p)?;
            return Ok((p.clone(), cfg));
        }
    }
    bail!(
        "client config not found; looked in: {}",
        paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Куда писать новый client config, если ни один не найден.
pub fn default_client_config_path() -> Result<PathBuf> {
    let x = xdg_config_home().ok_or_else(|| {
        anyhow!("cannot determine client config path (set $XDG_CONFIG_HOME or $HOME)")
    })?;
    Ok(x.join("appspec/repos.toml"))
}

/// Загрузить client config для записи. Если файла нет — создать в памяти.
pub fn load_or_default_client_config() -> Result<(PathBuf, ClientConfig)> {
    match find_client_config(None) {
        Ok((p, c)) => Ok((p, c)),
        Err(_) => {
            let p = default_client_config_path()?;
            Ok((
                p,
                ClientConfig {
                    schema: CONFIG_SCHEMA,
                    default: None,
                    repos: Vec::new(),
                },
            ))
        }
    }
}

// ===========================================================================
// Path expansion & secrets
// ===========================================================================

pub fn expand_user_path(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    if s == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home);
        }
    }
    p.to_path_buf()
}

/// Записать секрет (токен) в файл с mode по умолчанию.
pub fn write_secret_file(path: &Path, data: &str, mode: u32) -> Result<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true).mode(mode);
        let mut f = opts
            .open(path)
            .with_context(|| format!("creating {}", path.display()))?;
        f.write_all(data.as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, format!("{}\n", data))
            .with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parse_minimal_server_config() {
        let text = r#"
schema = 1
[repo]
root = "/tmp/repo"
name = "test/repo"
"#;
        let mut cfg: ServerConfig = toml::from_str(text).unwrap();
        cfg.normalize().unwrap();
        assert_eq!(cfg.schema, 1);
        assert_eq!(cfg.repo.root, PathBuf::from("/tmp/repo"));
    }

    #[test]
    fn reject_relative_root() {
        let mut cfg: ServerConfig = toml::from_str(
            r#"schema = 1
[repo]
root = "repo"
name = "x"
"#,
        )
        .unwrap();
        assert!(cfg.normalize().is_err());
    }

    #[test]
    fn reject_bad_schema() {
        let cfg: ServerConfig = toml::from_str(
            r#"schema = 2
[repo]
root = "/tmp/x"
name = "x"
"#,
        )
        .unwrap();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn from_legacy_roundtrip() {
        let legacy = layout::Config {
            repo: layout::RepoMeta {
                name: "old/repo".into(),
                description: "d".into(),
                created_at: "2026-01-01T00:00:00Z".into(),
                generator: "appspec 0.3.4".into(),
                layout: 1,
            },
            policy: Default::default(),
            signing: layout::Signing {
                pubkey: "ed25519:abc".into(),
            },
            server: Default::default(),
            ipfs: Default::default(),
        };
        let cfg = ServerConfig::from_legacy(PathBuf::from("/srv/repo"), legacy);
        assert_eq!(cfg.repo.root, PathBuf::from("/srv/repo"));
        assert_eq!(cfg.signing.pubkey, "ed25519:abc");
        assert!(cfg.signing.key.is_none());
        cfg.validate().unwrap();
    }

    #[test]
    fn into_layout_roundtrip() {
        let sc = ServerConfig {
            schema: CONFIG_SCHEMA,
            repo: ServerRepo {
                root: PathBuf::from("/srv/repo"),
                name: "n".into(),
                description: String::new(),
                created_at: String::new(),
                generator: String::new(),
            },
            policy: Default::default(),
            signing: ServerSigning {
                key: None,
                pubkey: "ed25519:p".into(),
            },
            server: Default::default(),
            ipfs: Default::default(),
        };
        let lc = sc.into_layout();
        assert_eq!(lc.repo.name, "n");
        assert_eq!(lc.signing.pubkey, "ed25519:p");
        assert_eq!(lc.repo.layout, layout::LAYOUT_VERSION);
    }

    #[test]
    fn parse_client_config() {
        let text = r#"
schema = 1
default = "corp"

[[repos]]
name = "corp"
source = { url = "http://host:8080" }
token = "env:APPSPEC_REPO_TOKEN"

[[repos]]
name = "local"
source = { path = "/srv/repo" }
"#;
        let cfg: ClientConfig = toml::from_str(text).unwrap();
        cfg.validate().unwrap();
        assert_eq!(cfg.repos.len(), 2);
        assert!(cfg.profile("corp").is_some());
    }

    #[test]
    fn client_duplicate_profile_rejected() {
        let text = r#"schema = 1
[[repos]]
name = "x"
source = { path = "/a" }
[[repos]]
name = "x"
source = { path = "/b" }
"#;
        let cfg: ClientConfig = toml::from_str(text).unwrap();
        assert!(format!("{:#}", cfg.validate().unwrap_err()).contains("duplicate profile"));
    }

    #[test]
    fn client_source_path_is_exclusive() {
        let text = r#"schema = 1
[[repos]]
name = "x"
source = { path = "/a", url = "http://h" }
"#;
        let cfg: ClientConfig = toml::from_str(text).unwrap();
        assert!(format!("{:#}", cfg.validate().unwrap_err())
            .contains("cannot be combined with url/ipns"));
    }

    #[test]
    fn client_source_url_and_ipns_allowed() {
        let text = r#"schema = 1
[[repos]]
name = "x"
source = { url = "http://h", ipns = "k51qzi5uqu5dtest" }
"#;
        let cfg: ClientConfig = toml::from_str(text).unwrap();
        cfg.validate().unwrap();
    }

    #[test]
    fn client_source_empty_rejected() {
        let text = r#"schema = 1
[[repos]]
name = "x"
source = {}
"#;
        let cfg: ClientConfig = toml::from_str(text).unwrap();
        assert!(format!("{:#}", cfg.validate().unwrap_err()).contains("at least one"));
    }

    #[test]
    fn client_source_bad_ipns_rejected() {
        let text = r#"schema = 1
[[repos]]
name = "x"
source = { ipns = "not-an-ipns-name" }
"#;
        let cfg: ClientConfig = toml::from_str(text).unwrap();
        assert!(format!("{:#}", cfg.validate().unwrap_err()).contains("must start with `k51`"));
    }

    #[test]
    fn client_default_must_exist() {
        let text = r#"schema = 1
default = "missing"
[[repos]]
name = "x"
source = { path = "/a" }
"#;
        let cfg: ClientConfig = toml::from_str(text).unwrap();
        assert!(format!("{:#}", cfg.validate().unwrap_err()).contains("default `missing`"));
    }

    #[test]
    fn token_source_validation() {
        for good in &["env:VAR", "file:/x", "literal:abc", "keyring:svc"] {
            assert!(validate_token_source(good).is_ok(), "{good}");
        }
        for bad in &["VAR", "env:", "unknown:x", ""] {
            assert!(validate_token_source(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn resolve_token_env() {
        std::env::set_var("APPSPEC_TEST_TOKEN_XYZ", "secret");
        assert_eq!(
            resolve_token("env:APPSPEC_TEST_TOKEN_XYZ").unwrap(),
            "secret"
        );
        std::env::remove_var("APPSPEC_TEST_TOKEN_XYZ");
    }

    #[test]
    fn resolve_token_file_trims_whitespace() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("t");
        std::fs::write(&p, "  hunter2\n").unwrap();
        let s = resolve_token(&format!("file:{}", p.display())).unwrap();
        assert_eq!(s, "hunter2");
    }

    #[test]
    fn resolve_token_keyring_not_implemented() {
        let e = resolve_token("keyring:foo").unwrap_err();
        assert!(format!("{:#}", e).contains("keyring"));
    }

    #[test]
    fn expand_user_path_tilde() {
        std::env::set_var("HOME", "/home/test");
        assert_eq!(
            expand_user_path(Path::new("~/x")),
            PathBuf::from("/home/test/x")
        );
        assert_eq!(
            expand_user_path(Path::new("~")),
            PathBuf::from("/home/test")
        );
        assert_eq!(expand_user_path(Path::new("/abs")), PathBuf::from("/abs"));
    }

    #[test]
    fn write_secret_file_creates_0600() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("token");
        write_secret_file(&p, "abc", 0o600).unwrap();
        let s = std::fs::read_to_string(&p).unwrap();
        assert_eq!(s.trim(), "abc");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn save_load_server_config_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("repo-server.toml");
        let cfg = ServerConfig {
            schema: CONFIG_SCHEMA,
            repo: ServerRepo {
                root: PathBuf::from("/srv/repo"),
                name: "test/repo".into(),
                description: String::new(),
                created_at: "2026-11-09T00:00:00Z".into(),
                generator: "appspec test".into(),
            },
            policy: Default::default(),
            signing: ServerSigning::default(),
            server: Default::default(),
            ipfs: Default::default(),
        };
        cfg.save(&p).unwrap();
        let loaded = ServerConfig::load(&p).unwrap();
        assert_eq!(loaded.repo.root, cfg.repo.root);
    }

    #[test]
    fn save_load_client_config_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("repos.toml");
        let cfg = ClientConfig {
            schema: CONFIG_SCHEMA,
            default: Some("local".into()),
            repos: vec![
                RepoProfile {
                    name: "local".into(),
                    source: RepoSourceTable {
                        path: Some(PathBuf::from("/srv/repo")),
                        ..Default::default()
                    },
                    token: None,
                    proxy: None,
                    trusted_keys: Vec::new(),
                    allow_unsigned: None,
                },
                RepoProfile {
                    name: "corp".into(),
                    source: RepoSourceTable {
                        url: Some("http://host:8080".into()),
                        ..Default::default()
                    },
                    token: Some("env:APPSPEC_REPO_TOKEN".into()),
                    proxy: None,
                    trusted_keys: Vec::new(),
                    allow_unsigned: None,
                },
            ],
        };
        cfg.save(&p).unwrap();
        let loaded = ClientConfig::load(&p).unwrap();
        assert_eq!(loaded.repos.len(), 2);
    }

    #[test]
    fn find_server_config_explicit_wins() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("repo-server.toml");
        ServerConfig {
            schema: CONFIG_SCHEMA,
            repo: ServerRepo {
                root: PathBuf::from("/srv/repo"),
                name: "test/repo".into(),
                description: String::new(),
                created_at: String::new(),
                generator: String::new(),
            },
            policy: Default::default(),
            signing: ServerSigning::default(),
            server: Default::default(),
            ipfs: Default::default(),
        }
        .save(&p)
        .unwrap();

        let (got_path, got_cfg) = find_server_config(Some(&p)).unwrap();
        assert_eq!(got_path, p);
        assert_eq!(got_cfg.repo.name, "test/repo");
    }

    #[test]
    fn find_server_config_with_legacy_falls_back() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();

        let legacy = layout::Config {
            repo: layout::RepoMeta {
                name: "legacy/repo".into(),
                description: String::new(),
                created_at: String::new(),
                generator: "appspec 0.3.4".into(),
                layout: 1,
            },
            policy: Default::default(),
            signing: Default::default(),
            server: Default::default(),
            ipfs: Default::default(),
        };
        std::fs::write(
            root.join("config.toml"),
            toml::to_string_pretty(&legacy).unwrap(),
        )
        .unwrap();

        std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("xdg"));
        std::env::remove_var("APPSPEC_REPO_SERVER_CONFIG");

        let (path, cfg) = find_server_config_with_legacy(None, Some(&root))
            .unwrap()
            .expect("legacy config should be found");
        assert!(path.ends_with("config.toml"));
        assert_eq!(cfg.repo.root, root);
        assert_eq!(cfg.repo.name, "legacy/repo");

        std::env::remove_var("XDG_CONFIG_HOME");
    }
}
