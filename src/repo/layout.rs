use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

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

// ===========================================================================
// RepoRoot
// ===========================================================================

pub struct RepoRoot {
    pub path: PathBuf,
    pub config: Config,
    /// Путь к server config, если он был загружен из файла. `None` —
    /// default-конфиг (data-only репо, config не найден) или legacy.
    /// Используется `repo sign` (сохраняет `pubkey` обратно в файл) и
    /// `repo serve` (warn, если config не найден).
    pub server_config_path: Option<PathBuf>,
    /// Путь к приватному ключу из `[signing].key` (если был в конфиге).
    /// Fallback `repo sign`: `--key` → `signing_key_path` → `$APPSPEC_SIGNING_KEY`
    /// → `~/.config/appspec/signing.key`.
    pub signing_key_path: Option<PathBuf>,
}

impl RepoRoot {
    /// Открыть data-директорию по пути.
    ///
    /// Порядок:
    ///   1. `<root>/config.toml` (legacy) → warning + чтение.
    ///   2. discovery (XDG/env/etc); если `[repo].root` совпадает с `root` — использовать.
    ///   3. discovery нашёл, но `[repo].root` ≠ `root` — warning + defaults.
    ///   4. discovery ничего не нашёл — defaults (data-only репо).
    ///
    /// Всегда требует наличия `<root>/index.json`.
    pub fn open(root: &Path) -> Result<Self> {
        if !root.join("index.json").is_file() {
            bail!(
                "repository not initialized at {} (missing index.json); \
                 run `appspec repo init {}` or `appspec repo server init --root {}`",
                root.display(),
                root.display(),
                root.display()
            );
        }

        // `find_server_config_with_legacy` инкапсулирует порядок:
        //   1. discovery (env / XDG / /etc) — `Some((path, cfg))`;
        //   2. legacy `<root>/config.toml` — warning + `Some((path, cfg))`;
        //   3. ничего нет — `None`;
        //   4. legacy есть, но layout ≠ LAYOUT_VERSION — `Err`.
        // `?` пробрасывает «невалидный конфиг» наружу — `RepoRoot::open`
        // не должен молча падать в defaults, если конфиг битый.
        match crate::repo::config::find_server_config_with_legacy(None, Some(root))? {
            Some((sc_path, sc)) => {
                if norm_path(&sc.repo.root) != norm_path(root) {
                    eprintln!(
                        "warning: server config {} points to {}, not {}; \
                         using defaults",
                        sc_path.display(),
                        sc.repo.root.display(),
                        root.display()
                    );
                    return Ok(Self::with_default_config(root));
                }
                let signing_key_path = sc.signing.key.clone();
                let layout_cfg = sc.into_layout();
                Ok(RepoRoot {
                    path: root.to_path_buf(),
                    config: layout_cfg,
                    server_config_path: Some(sc_path),
                    signing_key_path,
                })
            }
            None => Ok(Self::with_default_config(root)),
        }
    }

    /// Data-only репо без конфига: `add`, `check`, `gc`, `reindex`
    /// работают с разумными дефолтами. `serve` без конфига — работает,
    /// но `push` не пройдёт (нет токенов).
    pub fn with_default_config(root: &Path) -> Self {
        RepoRoot {
            path: root.to_path_buf(),
            config: default_layout_config(),
            server_config_path: None,
            signing_key_path: None,
        }
    }

    /// Собрать `RepoRoot` из явного `ServerConfig` (используется, когда
    /// пользователь передал `--server-config`).
    pub fn from_server_config(
        data_root: PathBuf,
        config_path: PathBuf,
        sc: crate::repo::config::ServerConfig,
    ) -> Self {
        let signing_key_path = sc.signing.key.clone();
        RepoRoot {
            path: data_root,
            config: sc.into_layout(),
            server_config_path: Some(config_path),
            signing_key_path,
        }
    }

    /// `RepoRoot` из уже загруженного `Config` — для `server.rs::handle_push`,
    /// который держит `ServerState.config` в памяти и не хочет повторно
    /// гонять discovery.
    pub fn open_with_config(root: &Path, config: Config) -> Self {
        RepoRoot {
            path: root.to_path_buf(),
            config,
            server_config_path: None,
            signing_key_path: None,
        }
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

/// Лексическая нормализация. Для сравнения `[repo].root` с `--root`
/// в `RepoRoot::open`. Не разрешает симлинки.
pub(crate) fn norm_path(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

fn default_layout_config() -> Config {
    Config {
        repo: RepoMeta {
            name: "local/appspec-repo".into(),
            description: String::new(),
            created_at: String::new(),
            generator: format!("appspec {}", env!("CARGO_PKG_VERSION")),
            layout: LAYOUT_VERSION,
        },
        policy: Default::default(),
        signing: Default::default(),
        server: Default::default(),
        ipfs: Default::default(),
    }
}
