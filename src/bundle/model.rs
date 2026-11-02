#![allow(dead_code)] // reserved: bundle ls --json, [bundle.extensions], публичный API
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct SourceSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sif: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
}

impl SourceSpec {
    pub fn kind_count(&self) -> usize {
        [
            self.spec.is_some(),
            self.sif.is_some(),
            self.cid.is_some(),
            self.manifest.is_some(),
            self.repo.is_some(),
        ]
        .into_iter()
        .filter(|b| *b)
        .count()
    }
    pub fn validate(&self, ctx: &str) -> anyhow::Result<()> {
        let n = self.kind_count();
        if n != 1 {
            anyhow::bail!(
                "{ctx}: source должен содержать ровно одно из \
                `spec` | `sif` | `cid` | `manifest` | `repo` (найдено {n})"
            );
        }
        Ok(())
    }
    pub fn origin(&self) -> String {
        if let Some(s) = &self.spec {
            return format!("spec:{s}");
        }
        if let Some(s) = &self.sif {
            return format!("sif:{s}");
        }
        if let Some(s) = &self.cid {
            return format!("cid:{s}");
        }
        if let Some(s) = &self.manifest {
            return format!("manifest:{s}");
        }
        if let Some(s) = &self.repo {
            return format!("repo:{s}");
        }
        "?".into()
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedSource {
    pub sif: PathBuf,
    pub setup: Option<String>,
    pub origin: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BundleFile {
    pub bundle: BundleMeta,
    #[serde(default)]
    pub components: Vec<Component>,
    #[serde(default)]
    pub extensions: Vec<Extension>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BundleMeta {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub strict: bool,
    pub base: BaseSection,
    #[serde(default)]
    pub extensions: Option<ExtensionsSection>,
    #[serde(default = "default_true")]
    pub resolve_dependencies: bool,
    #[serde(default)]
    pub expand_packs: bool,
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct BaseSection {
    #[serde(flatten)]
    pub source: SourceSpec,
    #[serde(default)]
    pub setup: Option<String>,
    #[serde(default)]
    pub entry: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ComponentMode {
    Chroot,
    Instance,
    Both,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Component {
    pub name: String,
    #[serde(flatten)]
    pub source: SourceSpec,

    #[serde(default)]
    pub mount: Option<String>,
    #[serde(default)]
    pub provide: Option<String>,
    #[serde(default)]
    pub binary: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub bridge: Option<String>,

    #[serde(default)]
    pub mounts: Vec<String>,
    #[serde(default)]
    pub forward: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, toml::Value>,
    #[serde(default)]
    pub selftest: Option<SelfTest>,
    #[serde(default)]
    pub files: Vec<FileRef>,

    /// "chroot" (по умолчанию) | "instance" | "both"
    #[serde(default)]
    pub mode: Option<String>,

    #[serde(default)]
    pub socket_dir: Option<String>,
    #[serde(default)]
    pub state_dir: Option<String>,
    #[serde(default)]
    pub daemon: Option<String>,
    #[serde(default)]
    pub daemon_socket: Option<String>,

    #[serde(default)]
    pub contain: Option<bool>,
}

impl Component {
    pub fn mount(&self) -> String {
        self.mount
            .clone()
            .unwrap_or_else(|| format!("/opt/utils/{}", self.name))
    }
    pub fn provide(&self) -> String {
        self.provide.clone().unwrap_or_else(|| self.name.clone())
    }
    pub fn binary(&self) -> String {
        self.binary
            .clone()
            .unwrap_or_else(|| format!("/usr/bin/{}", self.name))
    }
    pub fn target(&self) -> String {
        self.target
            .clone()
            .unwrap_or_else(|| format!("/usr/local/bin/{}", self.name))
    }
    pub fn bridge_kind(&self) -> &str {
        self.bridge.as_deref().unwrap_or("chroot")
    }
    pub fn mode(&self) -> ComponentMode {
        match self.mode.as_deref() {
            Some("instance") => ComponentMode::Instance,
            Some("both") => ComponentMode::Both,
            _ => ComponentMode::Chroot,
        }
    }
    pub fn has_sandbox(&self) -> bool {
        matches!(self.mode(), ComponentMode::Chroot | ComponentMode::Both)
    }
    pub fn has_instance(&self) -> bool {
        matches!(self.mode(), ComponentMode::Instance | ComponentMode::Both)
    }
    pub fn daemon(&self) -> Option<&str> {
        self.daemon.as_deref()
    }
    pub fn daemon_socket(&self) -> Option<&str> {
        self.daemon_socket.as_deref()
    }
    pub fn socket_dir(&self) -> Option<&str> {
        self.socket_dir.as_deref()
    }
    pub fn state_dir(&self) -> Option<&str> {
        self.state_dir.as_deref()
    }
    pub fn mounts(&self) -> Vec<String> {
        if self.mounts.is_empty() {
            vec!["PWD".into(), "HOME".into()]
        } else {
            self.mounts.clone()
        }
    }
    pub fn forward(&self) -> Vec<String> {
        if self.forward.is_empty() {
            vec!["TERM".into(), "LANG".into(), "LC_ALL".into()]
        } else {
            self.forward.clone()
        }
    }
    pub fn contain(&self) -> bool {
        self.contain.unwrap_or(true)
    }
    pub fn env_str(&self) -> BTreeMap<String, String> {
        self.env
            .iter()
            .map(|(k, v)| {
                let sv = match v {
                    toml::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                (k.clone(), sv)
            })
            .collect()
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SelfTest {
    pub cmd: Option<Vec<String>>,
    pub expect_contains: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FileScope {
    Base,
    Sandbox,
    Instance,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileRef {
    pub to: String,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub mode: Option<u32>,
    /// "base" (по умолчанию) | "sandbox" | "instance";
    /// можно несколько через запятую: "base,sandbox".
    #[serde(default)]
    pub scope: Option<String>,
}

impl FileRef {
    pub fn scopes(&self) -> Vec<FileScope> {
        let s = self.scope.as_deref().unwrap_or("base");
        let mut out = Vec::new();
        for tok in s.split(',') {
            match tok.trim() {
                "base" => out.push(FileScope::Base),
                "sandbox" => out.push(FileScope::Sandbox),
                "instance" => out.push(FileScope::Instance),
                _ => {}
            }
        }
        if out.is_empty() {
            out.push(FileScope::Base);
        }
        out
    }
    pub fn validate(&self, ctx: &str) -> anyhow::Result<()> {
        if !self.to.starts_with('/') {
            anyhow::bail!("{ctx}: `to` должен быть абсолютным: {}", self.to);
        }
        let n = [self.from.is_some(), self.content.is_some()]
            .iter()
            .filter(|b| **b)
            .count();
        if n != 1 {
            anyhow::bail!(
                "{ctx}: для `{}` укажите ровно одно из `from` | `content`",
                self.to
            );
        }
        if let Some(s) = &self.scope {
            for tok in s.split(',') {
                match tok.trim() {
                    "base" | "sandbox" | "instance" => {}
                    other => anyhow::bail!(
                        "{ctx}: неизвестный scope `{other}` \
                         (ожидается base | sandbox | instance, через запятую)"
                    ),
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedFile {
    pub to: String,
    pub content: String,
    pub mode: u32,
    pub source: String,
    pub scopes: Vec<FileScope>,
    pub component: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExtensionsSection {
    #[serde(default)]
    pub mount_dir: String,
    #[serde(default)]
    pub install: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Extension {
    pub name: String,
    /// Тип расширения: `vsix` | `file` | `archive` | `auto`.
    /// Если не задан — определяется по расширению файла/URL
    /// (`.vsix` → vsix, `.oxt`/`.deb`/`.rpm`/… → file, `.zip`/`.tar.gz` → archive).
    /// Для online-расширений (без file/cid/url) — vsix (Open VSX).
    #[serde(default, rename = "type")]
    pub r#type: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub cid: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub requires: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum ExtensionKind {
    /// VS Code extension (.vsix): распаковать, прочитать `extension/package.json`,
    /// положить распакованный каталог в `<id>-<version>/`.
    Vsix,
    /// Одиночный файл (.oxt, .deb, .rpm, …): положить как есть,
    /// `{file}` в install-команде указывает на файл.
    File,
    /// Архив (.zip / .tar.gz / …): распаковать в каталог без разбора package.json.
    Archive,
}

impl ExtensionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExtensionKind::Vsix => "vsix",
            ExtensionKind::File => "file",
            ExtensionKind::Archive => "archive",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PreparedExtension {
    pub id: String,
    pub version: String,
    /// Путь внутри `$BUNDLE_CACHE/extensions/`.
    /// Для `Vsix`/`Archive` — каталог; для `File` — сам файл.
    pub path: PathBuf,
    pub kind: ExtensionKind,
    pub source: String,
    pub dependencies: Vec<String>,
    pub extension_pack: Vec<String>,
    pub engines_vscode: Option<String>,
    pub requires: Vec<String>,
}

impl PreparedExtension {
    /// Относительный путь внутри каталога `<mount_dir>` —
    /// то, что подставляется в `{file}` install-команды.
    pub fn rel_path(&self) -> String {
        self.path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.id.clone())
    }
}

impl Extension {
    /// Сколько из `file | cid | url` задано. 0 — online (Open VSX),
    /// 1 — корректно, ≥2 — ошибка (ловится в `validate`).
    pub fn kind_count(&self) -> usize {
        [self.file.is_some(), self.cid.is_some(), self.url.is_some()]
            .into_iter()
            .filter(|b| *b)
            .count()
    }

    pub fn is_online(&self) -> bool {
        self.kind_count() == 0
    }

    /// Конструктор для авто-добавляемых расширений (extensionDependencies,
    /// extensionPack): `file`/`cid`/`url` пустые → online через Open VSX.
    /// `type` не задаём — `kind()` вернёт `Vsix` (нет URL/файла).
    pub fn online(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            r#type: None,
            file: None,
            cid: None,
            url: None,
            requires: Vec::new(),
        }
    }

    pub fn validate(&self, ctx: &str) -> anyhow::Result<()> {
        let n = self.kind_count();
        if n > 1 {
            anyhow::bail!("{ctx}: укажите не более одного из `file` | `cid` | `url`");
        }
        if let Some(t) = &self.r#type {
            if !matches!(t.as_str(), "vsix" | "file" | "archive" | "auto") {
                anyhow::bail!(
                    "{ctx}: неизвестный type `{t}` (ожидается vsix | file | archive | auto)"
                );
            }
        }
        Ok(())
    }

    /// Реальный тип расширения: явный `type`, либо автоопределение по
    /// расширению файла/URL. Для online (нет file/cid/url) — `Vsix`.
    pub fn kind(&self) -> ExtensionKind {
        match self.r#type.as_deref() {
            Some("vsix") => return ExtensionKind::Vsix,
            Some("file") => return ExtensionKind::File,
            Some("archive") => return ExtensionKind::Archive,
            Some("auto") | None => {}
            Some(other) => crate::log(format!(
                "bundle: extension `{}`: неизвестный type `{}`, авто-определение",
                self.name, other
            )),
        }
        let candidate = self.file.as_deref().or(self.url.as_deref()).unwrap_or("");
        let lower = candidate.to_ascii_lowercase();
        if lower.ends_with(".vsix") {
            return ExtensionKind::Vsix;
        }
        if lower.ends_with(".zip")
            || lower.ends_with(".tar.gz")
            || lower.ends_with(".tgz")
            || lower.ends_with(".tar.xz")
            || lower.ends_with(".tar.bz2")
            || lower.ends_with(".tar")
        {
            return ExtensionKind::Archive;
        }
        if lower.ends_with(".oxt")
            || lower.ends_with(".deb")
            || lower.ends_with(".rpm")
            || lower.ends_with(".sh")
            || lower.ends_with(".bin")
            || lower.ends_with(".exe")
        {
            return ExtensionKind::File;
        }
        // Ничего не опознали: file/url → file (безопаснее),
        // cid/online → vsix (backwards compat).
        if self.file.is_some() || self.url.is_some() {
            ExtensionKind::File
        } else {
            ExtensionKind::Vsix
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedBase {
    pub sif: PathBuf,
    pub pre: String,
    pub entry: String,
    pub origin: String,
}

#[derive(Debug, Serialize)]
pub struct LsReport<'a> {
    pub bundle: LsBundle<'a>,
    pub base: LsBase,
    pub components: Vec<LsComponent<'a>>,
    pub extensions: Vec<LsExtension<'a>>,
    pub issues: &'a [crate::bundle::validate::Issue],
}
#[derive(Debug, Serialize)]
pub struct LsBundle<'a> {
    pub name: &'a str,
    pub description: &'a str,
    pub strict: bool,
    pub resolve_dependencies: bool,
    pub expand_packs: bool,
    pub spec: String,
}
#[derive(Debug, Serialize)]
pub struct LsBase {
    pub origin: String,
    pub sif: String,
    pub present: bool,
}
#[derive(Debug, Serialize)]
pub struct LsComponent<'a> {
    pub name: &'a str,
    pub mode: String,
    pub origin: String,
    pub sif: String,
    pub present: bool,
    pub provide: String,
    pub binary: String,
    pub target: String,
    pub mounts: Vec<String>,
    pub forward: Vec<String>,
    pub env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub socket_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daemon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daemon_socket: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct LsExtension<'a> {
    pub name: &'a str,
    pub source: String,
    pub id: Option<String>,
    pub version: Option<String>,
    pub dir: Option<String>,
    pub prepared: bool,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub depends_on: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub pack_contains: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub requires: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engines_vscode: Option<String>,
    #[serde(default)]
    pub auto_added: bool,
}
