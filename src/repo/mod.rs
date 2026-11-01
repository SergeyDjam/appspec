//! Локальный репозиторий SIF (E1–E5).
//!
//! Модель данных и контракты — по ТЗ `appspec SIF Repository` v1.1.0.
//! Реализовано: init, add, ls, show, reindex, check, gc, search, serve,
//! push, pull (--from PATH|URL, --spec-only, --sif-out, --trusted-key),
//! sign/verify (ed25519 + JCS).
//! Не реализовано: ipfs (E6), export-oci (E8).

pub mod add;
pub mod check;
pub mod commands;
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
pub mod server;
pub mod sign;
pub mod tls;
pub mod util;

use anyhow::Result;
use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum RepoCommand {
    /// Создать новый локальный репозиторий
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
    /// Опубликовать index.json в IPFS и напечатать CID
    PublishIndex(commands::PublishIndexArgs),
    /// Экспортировать index.json и SIF'ы в OCI-совместимый registry
    ExportOci(commands::ExportOciArgs),
    /// Проверить подпись index.json
    Verify(commands::VerifyArgs),
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
        RepoCommand::ExportOci(a) => commands::export_oci(a),
        RepoCommand::Verify(a) => commands::verify(a),
    }
}

pub fn default_root() -> std::path::PathBuf {
    if let Ok(v) = std::env::var("APPSPEC_REPO") {
        if !v.is_empty() {
            return std::path::PathBuf::from(v);
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
