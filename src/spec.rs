use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct Spec {
    pub package: Package,
    #[serde(default)]
    pub build: Build,
    #[serde(default)]
    pub runtime: Runtime,
    #[serde(default)]
    pub install: Install,
    #[serde(default)] 
    pub macros: BTreeMap<String, String>,
}

impl Spec {
    pub fn sif_file(&self) -> String {
        self.runtime
            .sif_file
            .clone()
            .unwrap_or_else(|| format!("{}.sif", self.package.name))
    }

    pub fn image_name(&self) -> String {
        self.runtime
            .image_name
            .clone()
            .unwrap_or_else(|| format!("{}:latest", self.package.name))
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Package {
    pub name: String,
    pub version: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub description: Option<String>,
    #[serde(default)]
    pub bin: Option<String>,
}

// --- build -----------------------------------------------------------------

#[derive(Debug, Deserialize, Serialize)]
pub struct Build {
    #[serde(default = "default_base")]
    pub base: String,
    #[serde(default = "default_arch")]
    pub arch: String,
    #[serde(default = "default_tools")]
    pub tools: Vec<String>,
    #[serde(default)]
    pub apt: Apt,
    #[serde(default)]
    pub source: Source,
    #[serde(default)]
    pub collect: Collect,
    #[serde(default)]
    pub scratch: Scratch,

    #[serde(default)]
    pub template: Option<String>,
    #[serde(default)]
    pub template_file: Option<PathBuf>,
    #[serde(default)]
    pub template_dir: Option<PathBuf>,

    /// Имя дистрибутива для выбора пакетного менеджера: rosa, alt, fedora, …
    #[serde(default)]
    pub distro: Option<String>,

    /// Мажорная версия дистрибутива (ROSA: >=12 → dnf, <12 → urpmi).
    #[serde(default)]
    pub version: Option<String>,

    /// Явное указание пакетного менеджера: dnf | urpmi | apt-get | apk.
    #[serde(default)]
    pub pkg_manager: Option<String>,
}

impl Default for Build {
    fn default() -> Self {
        Self {
            base: default_base(),
            arch: default_arch(),
            tools: default_tools(),
            apt: Apt::default(),
            source: Source::default(),
            collect: Collect::default(),
            scratch: Scratch::default(),
            template: None,
            template_file: None,
            template_dir: None,
            distro: None,
            version: None,
            pkg_manager: None,
        }
    }
}

impl Build {
    /// Приводит короткие имена образов к fully-qualified:
    /// `alpine:3.21` → `docker.io/library/alpine:3.21`.
    pub fn qualify_base_image(&mut self) {
        self.base = qualify_image(&self.base);
    }
}

pub fn qualify_image(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return raw.to_string();
    }

    let first = raw.split('/').next().unwrap_or("");
    let has_registry = first.contains('.')
        || first.contains(':')
        || first == "localhost";

    if has_registry {
        return raw.to_string();
    }

    if raw.contains('/') {
        format!("docker.io/{raw}")
    } else {
        format!("docker.io/library/{raw}")
    }
}

fn default_base() -> String { "debian:trixie".into() }
fn default_arch() -> String { "amd64".into() }
fn default_tools() -> Vec<String> {
    ["ca-certificates", "wget", "dpkg-dev", "binutils", "patchelf"]
        .iter().map(|s| s.to_string()).collect()
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Apt {
    #[serde(default)]
    pub extra: Vec<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Source {
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub extract_to: Option<String>,
    #[serde(default)]
    pub strip_components: Option<u32>,

    /// Имена пакетов, которые нужно исключить из `Depends` .deb-пакета
    /// перед установкой.
    #[serde(default)]
    pub exclude_deps: Vec<String>,

    /// Целевой путь для `type = "binary"` — куда положить скачанный файл.
    /// По умолчанию `/usr/local/bin/tool` (см. шаблон).
    #[serde(default)]
    pub dest: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Collect {
    #[serde(default)]
    pub binary: Option<String>,
    #[serde(default)]
    pub includes: Vec<String>,
    #[serde(default)]
    pub magicpak_version: Option<String>,
    #[serde(default)]
    pub busybox_url: Option<String>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,

    /// Аргументы, передаваемые целевому бинарнику во время сборки,
    /// чтобы он быстро завершился (например `["-v"]` для angie).
    /// Передаются в magicpak после `binary`.
    #[serde(default)]
    pub target_args: Vec<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Scratch {
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub cmd: Option<Vec<String>>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

// --- runtime ---------------------------------------------------------------

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Runtime {
    #[serde(default)] pub sif_file: Option<String>,
    #[serde(default)] pub image_name: Option<String>,
    #[serde(default)] pub vars: BTreeMap<String, String>,
    #[serde(default)] pub state_dirs: Vec<String>,
    #[serde(default)] pub binds: Vec<String>,
    #[serde(default)] pub optional_binds: Vec<String>,
    #[serde(default)] pub env_passthrough: Option<Vec<String>>,
    #[serde(default)] pub xauthority_auto: Option<bool>,
    #[serde(default)] pub pulse_auto: Option<bool>,
    #[serde(default)] pub setup: Option<String>,
    #[serde(default)] pub entry: Option<String>,

    #[serde(default)] pub create_dirs: Vec<String>,

    /// Режим запуска: "instance" (по умолчанию, для GUI/сервисов) или "cli".
    #[serde(default)] pub mode: Option<String>,

    /// Команда для CLI-режима. Путь к бинарю внутри контейнера.
    #[serde(default)] pub command: Option<String>,

    #[serde(default)] pub workdir: Option<String>,
    #[serde(default)] pub persist: Option<bool>,

    #[serde(default)] pub template: Option<String>,
    #[serde(default)] pub template_file: Option<PathBuf>,
    #[serde(default)] pub template_dir: Option<PathBuf>,
}

// --- install ---------------------------------------------------------------

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Install {
    #[serde(default)] pub scope: Option<String>,
    #[serde(default)] pub sif_dir: Option<PathBuf>,
    #[serde(default)] pub launcher_name: Option<String>,
    #[serde(default)] pub desktop: Option<bool>,
    #[serde(default)] pub icon: Option<PathBuf>,
    #[serde(default)] pub categories: Vec<String>,
    #[serde(default)] pub mime_types: Vec<String>,
    #[serde(default)] pub comment: Option<String>,
    #[serde(default)] pub wm_class: Option<String>,
    #[serde(default)] pub extra_files: Vec<ExtraFile>,
    #[serde(default)] pub user: ScopeOverride,
    #[serde(default)] pub global: ScopeOverride,
    #[serde(default)] pub ipfs: IpfsConfig,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ExtraFile {
    pub from: PathBuf,
    pub to: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ScopeOverride {
    pub sif_dir: Option<PathBuf>,
    pub bin_dir: Option<PathBuf>,
    pub desktop_dir: Option<PathBuf>,
    pub icon_dir: Option<PathBuf>,
    pub state_dir: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct IpfsConfig {
    #[serde(default)] pub enabled: Option<bool>,
    #[serde(default)] pub pin: Option<bool>,
    #[serde(default)] pub api_url: Option<String>,
    #[serde(default)] pub gateway: Option<String>,
    #[serde(default)] pub ipns_key: Option<String>,
}