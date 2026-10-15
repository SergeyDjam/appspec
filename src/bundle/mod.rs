//! Композитные окружения: base SIF + автономные компоненты + плагины.
//!
//! Bundle не ищет чужие spec'и. Каждый компонент указывается одним из
//! четырёх источников (spec | sif | cid | manifest). Если source = spec
//! и SIF отсутствует — bundle собирает его, как docker-compose build.
//!
//! Extensions готовятся на хосте (скачивание/распаковка .vsix) и в
//! контейнер монтируются уже готовым каталогом.

pub mod codegen;
pub mod commands;
pub mod loader;
pub mod model;
pub mod validate;

use anyhow::Result;
use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum BundleCommand {
    /// Собрать/пересобрать launcher по bundle.toml
    Build(commands::BuildArgs),
    /// Показать состав bundle
    Ls(commands::LsArgs),
    /// Только валидация (без сборки); exit 2 при ошибках
    Check(commands::CheckArgs),
    /// Добавить [[components]] в bundle.toml
    Add(commands::AddArgs),
    /// Удалить [[components]] из bundle.toml
    Remove(commands::RemoveArgs),
}

pub fn run(cmd: BundleCommand) -> Result<i32> {
    match cmd {
        BundleCommand::Build(a) => commands::build(a),
        BundleCommand::Ls(a) => commands::ls(a),
        BundleCommand::Check(a) => commands::check(a),
        BundleCommand::Add(a) => commands::add(a),
        BundleCommand::Remove(a) => commands::remove(a),
    }
}
