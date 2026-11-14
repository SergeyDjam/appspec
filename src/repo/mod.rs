//! Локальный репозиторий SIF (E1–E5).
//!
//! Модель данных и контракты — по ТЗ `appspec SIF Repository` v1.1.0.
//! Реализовано: init, add, ls, show, reindex, check, gc, search, serve,
//! push, pull, sign/verify (ed25519 + JCS), config-разделение
//! (server / client), token lifecycle, migrate-config.

pub mod add;
pub mod check;
pub mod commands;
pub mod config;
pub mod gc;
pub mod http;
pub mod index_build;
pub mod inspect;
pub mod ipfs;
pub mod layout;
pub mod meta;
pub mod oci;
pub mod pull;
pub mod push;
pub mod run_package;
pub mod server;
pub mod sign;
pub mod tls;
pub mod util;

use anyhow::Result;
use clap::Subcommand;

pub use config::RepoProfile;

#[derive(Debug, Subcommand)]
pub enum RepoCommand {
    /// Создать новый локальный репозиторий (data-only).
    Init(commands::InitArgs),
    /// Добавить SIF в репозиторий
    Add(commands::AddArgs),
    /// Список пакетов или версий пакета
    Ls(commands::LsArgs),
    /// Показать метаданные / спек / SIF
    Show(commands::ShowArgs),
    /// Пересобрать index.json из packages/
    Reindex(commands::ReindexArgs),
    /// Проверить целостность репозитория
    Check(commands::CheckArgs),
    /// Удалить unreferenced blob'ы
    Gc(commands::GcArgs),
    /// Поиск по метаданным
    Search(commands::SearchArgs),
    /// HTTP-сервер репозитория (read + push)
    Serve(commands::ServeArgs),
    /// Отправить SIF на удалённый сервер
    Push(commands::PushArgs),
    /// Скачать пакет из удалённого репозитория
    Pull(commands::PullArgs),
    /// Подписать index.json (ed25519)
    Sign(commands::SignArgs),
    /// Опубликовать index.json в IPFS и (опционально) IPNS
    #[command(alias = "publish")]
    PublishIndex(commands::PublishIndexArgs),
    /// Резолв IPNS name в index CID (для отладки)
    Resolve(commands::ResolveArgs),
    /// Экспортировать index.json и SIF'ы в OCI-совместимый registry
    ExportOci(commands::ExportOciArgs),
    /// Проверить подпись index.json
    Verify(commands::VerifyArgs),
    /// Пиннить блобы в IPFS и заполнить отсутствующие ipfs_cid
    Pin(commands::PinArgs),
    /// Управление токенами сервера
    #[command(subcommand)]
    Token(commands::TokenCommand),
    /// Перенести legacy `<root>/config.toml` в `repo-server.toml`
    MigrateConfig(commands::MigrateConfigArgs),
    /// Создать/обновить server config
    #[command(subcommand)]
    Server(commands::ServerCommand),
    /// Управление client config (профили источников)
    #[command(subcommand)]
    Client(commands::ClientCommand),
}

pub fn run(cmd: RepoCommand) -> Result<i32> {
    match cmd {
        RepoCommand::Init(a) => commands::init(a),
        RepoCommand::Add(a) => commands::add(a),
        RepoCommand::Ls(a) => commands::ls(a),
        RepoCommand::Show(a) => commands::show(a),
        RepoCommand::Reindex(a) => commands::reindex(a),
        RepoCommand::Check(a) => commands::check(a),
        RepoCommand::Gc(a) => commands::gc(a),
        RepoCommand::Search(a) => commands::search(a),
        RepoCommand::Serve(a) => commands::serve(a),
        RepoCommand::Push(a) => commands::push(a),
        RepoCommand::Pull(a) => commands::pull(a),
        RepoCommand::Sign(a) => commands::sign(a),
        RepoCommand::PublishIndex(a) => commands::publish_index(a),
        RepoCommand::Resolve(a) => commands::resolve(a),
        RepoCommand::ExportOci(a) => commands::export_oci(a),
        RepoCommand::Verify(a) => commands::verify(a),
        RepoCommand::Pin(a) => commands::pin_cmd(a),
        RepoCommand::Token(c) => commands::token(c),
        RepoCommand::MigrateConfig(a) => commands::migrate_config(a),
        RepoCommand::Server(c) => commands::server(c),
        RepoCommand::Client(c) => commands::client(c),
    }
}

pub fn default_root() -> std::path::PathBuf {
    if let Ok(v) = std::env::var("APPSPEC_REPO") {
        if !v.is_empty() {
            if v.starts_with("http://") || v.starts_with("https://") || v.starts_with("ipfs://") {
                // URL — не путь; локальные команды используют XDG.
            } else if v.starts_with('/') || v.starts_with('~') || v.starts_with('.') {
                return config::expand_user_path(std::path::Path::new(&v));
            } else {
                // Может быть профилем.
                if let Ok((_, cc)) = config::find_client_config(None) {
                    if let Some(p) = cc.profile(&v) {
                        if let Some(path) = &p.source.path {
                            return path.clone();
                        }
                        // URL/IPFS-профиль — не для локальных команд.
                    }
                }
                return config::expand_user_path(std::path::Path::new(&v));
            }
        }
    }
    if let Ok(v) = std::env::var("XDG_DATA_HOME") {
        if !v.is_empty() {
            return std::path::PathBuf::from(v).join("appspec/repo");
        }
    }
    if let Ok(h) = std::env::var("HOME") {
        if !h.is_empty() {
            return std::path::PathBuf::from(h).join(".local/share/appspec/repo");
        }
    }
    std::path::PathBuf::from("/tmp/appspec-repo")
}
