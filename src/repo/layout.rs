use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const LAYOUT_VERSION: u32 = 1;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    pub repo: RepoMeta,
    #[serde(default)]
    pub policy: Policy,
    #[serde(default)]
    pub signing: Signing,
    #[serde(default)]
    pub server: Server,
    #[serde(default)]
    pub ipfs: Ipfs,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RepoMeta {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub created_at: String,
    pub generator: String,
    pub layout: u32,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Policy {
    #[serde(default = "default_true")]
    pub immutable_versions: bool,
    #[serde(default)]
    pub allow_force: bool,
    #[serde(default = "default_auto_tag")]
    pub auto_tag_latest: String,
}

fn default_true() -> bool {
    true
}
fn default_auto_tag() -> String {
    "semver".to_string()
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            immutable_versions: true,
            allow_force: false,
            auto_tag_latest: default_auto_tag(),
        }
    }
}

#[derive(Debug, Default, Deserialize, Serialize, Clone)]
pub struct Signing {
    #[serde(default)]
    pub pubkey: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Server {
    #[serde(default = "default_listen")]
    pub listen: String,
    #[serde(default = "default_max_push")]
    pub max_push_bytes: u64,
    #[serde(default = "default_timeout")]
    pub read_timeout_secs: u64,
    #[serde(default = "default_timeout")]
    pub write_timeout_secs: u64,
    #[serde(default)]
    pub tokens: Vec<ServerToken>,
    #[serde(default)]
    pub tls: TlsConfig,
}

fn default_listen() -> String {
    "127.0.0.1:8080".into()
}
fn default_max_push() -> u64 {
    2 * 1024 * 1024 * 1024
}
fn default_timeout() -> u64 {
    600
}

impl Default for Server {
    fn default() -> Self {
        Server {
            listen: default_listen(),
            max_push_bytes: default_max_push(),
            read_timeout_secs: default_timeout(),
            write_timeout_secs: default_timeout(),
            tokens: Vec::new(),
            tls: TlsConfig::default(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ServerToken {
    pub name: String,
    pub hash: String,
}

#[derive(Debug, Default, Deserialize, Serialize, Clone)]
pub struct TlsConfig {
    #[serde(default)]
    pub cert: Option<String>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub client_ca: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize, Clone)]
pub struct Ipfs {
    #[serde(default)]
    pub auto_pin: bool,
    #[serde(default = "default_ipfs_api")]
    pub api_url: String,
    #[serde(default = "default_ipfs_gateway")]
    pub gateway: String,
}

fn default_ipfs_api() -> String {
    "http://127.0.0.1:5001".into()
}
fn default_ipfs_gateway() -> String {
    "https://ipfs.io/ipfs".into()
}

pub struct RepoRoot {
    pub path: PathBuf,
    pub config: Config,
}

impl RepoRoot {
    pub fn open(root: &Path) -> Result<Self> {
        let cfg_path = root.join("config.toml");
        if !cfg_path.is_file() {
            bail!(
                "repository not initialized at {} (missing config.toml); \
                 run `appspec repo init {}`",
                root.display(),
                root.display()
            );
        }
        let text = std::fs::read_to_string(&cfg_path)
            .with_context(|| format!("reading {}", cfg_path.display()))?;
        let config: Config =
            toml::from_str(&text).with_context(|| format!("parsing {}", cfg_path.display()))?;

        eprintln!("\x1b[2m[appspec] repo: {}\x1b[0m", root.display());

        if config.repo.layout != LAYOUT_VERSION {
            bail!(
                "unsupported repository layout {} (this client supports {}); \
                 upgrade appspec or use a compatible repository",
                config.repo.layout,
                LAYOUT_VERSION
            );
        }
        Ok(RepoRoot {
            path: root.to_path_buf(),
            config,
        })
    }

    pub fn blobs_dir(&self) -> PathBuf {
        self.path.join("blobs/sha256")
    }
    pub fn packages_dir(&self) -> PathBuf {
        self.path.join("packages")
    }
    pub fn refs_dir(&self) -> PathBuf {
        self.path.join("refs")
    }
    pub fn index_path(&self) -> PathBuf {
        self.path.join("index.json")
    }
    pub fn lock_path(&self) -> PathBuf {
        self.path.join(".lock")
    }
}
