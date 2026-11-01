use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{bail, Result};
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
    #[serde(default)]
    pub dependencies: Dependencies,
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

    /// Валидация [dependencies] и [build].base_digest.
    ///
    /// §8.1: элементы не должны содержать `,`; `requires` — `name [OP ver]`.
    /// §8.2: `base_digest` — `sha256:<64 lowercase hex>`.
    ///
    /// Вызывается из `load_spec` (см. `main.rs`), поэтому действует и на
    /// `gen`, и на `build`, и на bundle-резолве.
    pub fn validate(&self) -> Result<()> {
        if let Some(d) = &self.build.base_digest {
            validate_base_digest(d)?;
        }
        for r in &self.dependencies.requires {
            validate_requires(r)?;
        }
        for p in &self.dependencies.provides {
            validate_bare_name(p, "provides")?;
        }
        for c in &self.dependencies.conflicts {
            validate_bare_name(c, "conflicts")?;
        }
        for r in &self.dependencies.replaces {
            validate_bare_name(r, "replaces")?;
        }
        Ok(())
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Dependencies {
    #[serde(default)] pub requires:  Vec<String>,
    #[serde(default)] pub provides:  Vec<String>,
    #[serde(default)] pub conflicts: Vec<String>,
    #[serde(default)] pub replaces:  Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Package {
    pub name: String,
    pub version: String,

    // --- RPM-style метаданные ---------------------------------------------
    /// Release (RPM-style). Например "1", "2.el9".
    #[serde(default)]
    pub release: Option<String>,

    /// Epoch (RPM-style).
    #[serde(default)]
    pub epoch: Option<u32>,

    /// Краткое описание (одна строка) — аналог RPM Summary.
    #[serde(default)]
    pub summary: Option<String>,

    /// Полное описание (может быть многострочным) — аналог RPM Description.
    #[serde(default)]
    pub description: Option<String>,

    /// Лицензия (SPDX-выражение рекомендуется).
    #[serde(default)]
    pub license: Option<String>,

    /// Домашняя страница проекта.
    #[serde(default)]
    pub url: Option<String>,

    /// Vendor/издатель.
    #[serde(default)]
    pub vendor: Option<String>,

    /// Packager — тот, кто собрал пакет.
    #[serde(default)]
    pub packager: Option<String>,

    /// Имя бинарника внутри контейнера. По умолчанию = name.
    #[serde(default)]
    pub bin: Option<String>,
}

// --- build -----------------------------------------------------------------

#[derive(Debug, Deserialize, Serialize)]
pub struct Build {
    #[serde(default = "default_base")]
    pub base: String,

    /// Digest базового образа (§8.2): `sha256:<64 hex>`.
    /// Если задан — `qualify_base_image` заменяет `base` на
    /// `<qualified_repo>@<digest>` (тег отбрасывается).
    #[serde(default)]
    pub base_digest: Option<String>,

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
            base_digest: None,
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
    /// Приводит base к fully-qualified, опционально с digest.
    ///
    /// §8.2: если `base_digest` задан, `base` заменяется на
    /// `<qualified>@<digest>`, чтобы Dockerfile содержал
    /// `FROM repo@sha256:…` (тег отбрасывается).
    /// Если в `base` уже есть `@` — оставляем как есть (digest в base
    /// считается более специфичным, чем `base_digest`).
    pub fn qualify_base_image(&mut self) {
        let base = self.base.trim().to_string();
        if base.contains('@') {
            self.base = base;
            return;
        }
        self.base = match &self.base_digest {
            Some(d) => {
                let stripped = strip_tag(&base);
                let qualified = qualify_image(stripped);
                format!("{}@{}", qualified, d)
            }
            None => qualify_image(&base),
        };
    }
}

/// Отбрасывает `:tag` из последнего path-компонента. Если тега нет —
/// возвращает `base` как есть. Registry-порт (`:5000`) не трогает.
fn strip_tag(base: &str) -> &str {
    match base.rfind(':') {
        Some(i) if !base[i + 1..].contains('/') => &base[..i],
        _ => base,
    }
}

pub fn qualify_image(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return raw.to_string();
    }

    let first = raw.split('/').next().unwrap_or("");
    let has_registry = first.contains('.') || first.contains(':') || first == "localhost";

    if has_registry {
        return raw.to_string();
    }

    if raw.contains('/') {
        format!("docker.io/{raw}")
    } else {
        format!("docker.io/library/{raw}")
    }
}

fn default_base() -> String {
    "debian:trixie".into()
}
fn default_arch() -> String {
    "amd64".into()
}
fn default_tools() -> Vec<String> {
    [
        "ca-certificates",
        "wget",
        "dpkg-dev",
        "binutils",
        "patchelf",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

// --- [dependencies] validation (§8.1) -------------------------------------

fn validate_base_digest(d: &str) -> Result<()> {
    let Some(hex) = d.strip_prefix("sha256:") else {
        bail!("base_digest must be `sha256:<64 hex>`, got `{}`", d);
    };
    if hex.len() != 64 {
        bail!(
            "base_digest hex must be 64 chars, got {} in `{}`",
            hex.len(),
            d
        );
    }
    if !hex.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)) {
        bail!("base_digest hex must be lowercase [0-9a-f]: `{}`", d);
    }
    Ok(())
}

fn validate_requires(s: &str) -> Result<()> {
    if s.contains(',') {
        bail!("requires element must not contain ',': `{}`", s);
    }
    let (name, op_ver) = split_operator(s.trim());
    validate_pkg_name(name)?;
    if let Some((_op, ver)) = op_ver {
        if ver.is_empty() {
            bail!("requires `{}`: empty version after operator", s);
        }
        if !ver
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".+-_~:".contains(c))
        {
            bail!("requires `{}`: invalid version `{}`", s, ver);
        }
    }
    Ok(())
}

fn validate_bare_name(s: &str, kind: &str) -> Result<()> {
    if s.contains(',') {
        bail!("{} element must not contain ',': `{}`", kind, s);
    }
    let t = s.trim();
    if t.contains(' ') || t.contains('>') || t.contains('<') || t.contains('=') {
        bail!(
            "{} element `{}` must be a bare name (no operator/version)",
            kind,
            s
        );
    }
    validate_pkg_name(t)
}

fn validate_pkg_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("empty package name");
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_alphanumeric() && !first.is_ascii_uppercase()) {
        bail!("invalid package name `{}`: must start with [a-z0-9]", name);
    }
    for c in chars {
        let ok = (c.is_ascii_alphanumeric() && !c.is_ascii_uppercase())
            || "._-".contains(c);
        if !ok {
            bail!("invalid package name `{}`: unexpected char `{}`", name, c);
        }
    }
    Ok(())
}

fn split_operator(s: &str) -> (&str, Option<(&'static str, &str)>) {
    for op in &[">=", "<=", "=", ">", "<"] {
        if let Some(i) = s.find(op) {
            let name = s[..i].trim_end();
            let ver = s[i + op.len()..].trim_start();
            return (name, Some((op, ver)));
        }
    }
    (s, None)
}

// --- apt / source / collect / scratch (без изменений) ---------------------

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

    /// Аргументы, передаваемые целевому бинарнику во время сборки.
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

// --- runtime (без изменений) ----------------------------------------------

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
    #[serde(default)] pub mode: Option<String>,
    #[serde(default)] pub command: Option<String>,
    #[serde(default)] pub workdir: Option<String>,
    #[serde(default)] pub persist: Option<bool>,
    #[serde(default)] pub template: Option<String>,
    #[serde(default)] pub template_file: Option<PathBuf>,
    #[serde(default)] pub template_dir: Option<PathBuf>,
}

// --- install (без изменений) ----------------------------------------------

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