//! bundle.lock.toml — фиксация sha256 компонентов bundle (§9.2 ТЗ).
//!
//! Lock не переопределяет индекс: при расхождении `bundle build`
//! печатает warning и идёт по свежему индексу (§9.2: «warning + выбор
//! индекса»). Lock фиксирует ожидаемое состояние на момент `bundle lock`
//! и позволяет обнаружить дрейф.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::model::{BundleFile, SourceSpec};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct LockFile {
    pub bundle: LockBundle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<BaseLock>,
    #[serde(default, rename = "components")]
    pub components: Vec<LockComponent>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct LockBundle {
    pub name: String,
    pub locked_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaseLock {
    pub repo: String,
    pub sif_sha256: String,
    pub spec_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_digest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockComponent {
    pub name: String,
    pub repo: String,
    pub sif_sha256: String,
    pub spec_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_digest: Option<String>,
}

impl LockFile {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Ok(toml::from_str(&text)?)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    #[allow(dead_code)] // reserved: bundle build --locked (E4)
    pub fn by_name(&self) -> BTreeMap<&str, &LockComponent> {
        self.components
            .iter()
            .map(|c| (c.name.as_str(), c))
            .collect()
    }
}

/// `<spec_dir>/bundle.lock.toml`.
pub fn default_path(spec_path: &Path) -> PathBuf {
    spec_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("bundle.lock.toml")
}

/// Repo ref from `spec` (0.6.0) or deprecated `repo`.
/// `foo.appspec` is locked only when the local file is absent and it falls
/// back to `foo:latest`.
fn source_ref(src: &SourceSpec, base_dir: &Path) -> Option<String> {
    if let Some(r) = crate::bundle::loader::source_repo_ref(src) {
        return Some(r.to_string());
    }
    if let Some(r) = &src.repo {
        return Some(r.clone());
    }
    let s = src.spec.as_deref()?;
    match crate::bundle::loader::classify_spec_source(s) {
        crate::bundle::loader::SpecSource::Ambiguous(_) => {
            let p = crate::bundle::loader::resolve_relative(s, base_dir);
            if p.is_file() {
                None
            } else {
                let pkg = s.strip_suffix(".appspec").unwrap_or(s);
                Some(format!("{}:latest", pkg))
            }
        }
        _ => None,
    }
}

/// Resolve `foo`, `foo:latest`, `foo:1.0`, `foo:1.0-1` to a canonical
/// `name:version-release` key plus the index version.
fn lock_ref<'a>(
    index: &'a crate::repo::meta::IndexV1,
    r: &str,
    ctx: &str,
) -> Result<(String, &'a crate::repo::meta::IndexVersion)> {
    let (name, ver) = r.split_once(':').unwrap_or((r, "latest"));
    let name = name.to_ascii_lowercase();
    let (ver_key, v) = crate::repo::run_package::pick_version(index, &name, Some(ver))
        .map_err(|e| anyhow!("{ctx}: {e:#}"))?;
    Ok((format!("{name}:{ver_key}"), v))
}

/// Сгенерировать lock по текущему индексу для всех `repo`-компонентов.
/// Компоненты без `repo` пропускаются (у них другая природа: spec/sif/cid/manifest).
pub fn generate(
    file: &BundleFile,
    spec_path: &Path,
    root: Option<&crate::repo::layout::RepoRoot>,
    trusted: &[crate::repo::sign::TrustedKey],
    allow_unsigned: bool,
) -> Result<LockFile> {
    let base_dir = spec_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let has_repo = source_ref(&file.bundle.base.source, base_dir).is_some()
        || file
            .components
            .iter()
            .any(|c| source_ref(&c.source, base_dir).is_some());
    if !has_repo {
        return Ok(LockFile {
            bundle: LockBundle {
                name: file.bundle.name.clone(),
                locked_at: crate::repo::util::now_iso8601(),
            },
            base: None,
            components: Vec::new(),
        });
    }

    let root = root.ok_or_else(|| {
        anyhow!(
            "bundle has `repo:` sources but no repo root; \
             pass `--repo`, `$APPSPEC_REPO`, or configure server config `[repo].root`"
        )
    })?;

    let index_text = std::fs::read_to_string(root.index_path())
        .with_context(|| format!("reading {}", root.index_path().display()))?;
    crate::repo::sign::verify_remote_index(&index_text, trusted, allow_unsigned)
        .with_context(|| format!("verifying {}", root.index_path().display()))?;
    let index: crate::repo::meta::IndexV1 =
        serde_json::from_str(&index_text).context("parsing repo index.json")?;

    let base_lock = if let Some(r) = source_ref(&file.bundle.base.source, base_dir) {
        let (repo, v) = lock_ref(&index, &r, "base")?;
        Some(BaseLock {
            repo,
            sif_sha256: v.sif_sha256.clone(),
            spec_sha256: v.spec_sha256.clone(),
            base_digest: v.base_digest.clone(),
        })
    } else {
        None
    };

    let mut components = Vec::new();
    for c in &file.components {
        let Some(r) = source_ref(&c.source, base_dir) else {
            continue;
        };
        let (repo, v) = lock_ref(&index, &r, &format!("component `{}`", c.name))?;
        components.push(LockComponent {
            name: c.name.clone(),
            repo,
            sif_sha256: v.sif_sha256.clone(),
            spec_sha256: v.spec_sha256.clone(),
            base_digest: v.base_digest.clone(),
        });
    }

    Ok(LockFile {
        bundle: LockBundle {
            name: file.bundle.name.clone(),
            locked_at: crate::repo::util::now_iso8601(),
        },
        base: base_lock,
        components,
    })
}

/// Сверить lock с текущим индексом. Возвращает список warning'ов
/// (дрейф). Не ошибка: `bundle build` идёт по индексу.
pub fn check_drift(
    lock: &LockFile,
    root: Option<&crate::repo::layout::RepoRoot>,
    trusted: &[crate::repo::sign::TrustedKey],
    allow_unsigned: bool,
) -> Result<Vec<String>> {
    if lock.base.is_none() && lock.components.is_empty() {
        return Ok(Vec::new());
    }

    let root = root.ok_or_else(|| {
        anyhow!(
            "lock has repo refs but no repo root; \
             pass `--repo`, `$APPSPEC_REPO`, or configure server config `[repo].root`"
        )
    })?;

    let index_text = std::fs::read_to_string(root.index_path())
        .with_context(|| format!("reading {}", root.index_path().display()))?;
    crate::repo::sign::verify_remote_index(&index_text, trusted, allow_unsigned)
        .with_context(|| format!("verifying {}", root.index_path().display()))?;
    let index: crate::repo::meta::IndexV1 =
        serde_json::from_str(&index_text).context("parsing repo index.json")?;

    let mut warns = Vec::new();

    if let Some(b) = &lock.base {
        match find_index_version(&index, &b.repo) {
            Some(iv) if iv.sif_sha256 == b.sif_sha256 => {}
            Some(iv) => warns.push(format!(
                "lock drift: base `{}` sif_sha256 {} != index {}; using index",
                b.repo, b.sif_sha256, iv.sif_sha256
            )),
            None => warns.push(format!(
                "lock drift: base `{}` not in current index",
                b.repo
            )),
        }
    }

    for c in &lock.components {
        match find_index_version(&index, &c.repo) {
            Some(iv) if iv.sif_sha256 == c.sif_sha256 => {}
            Some(iv) => warns.push(format!(
                "lock drift: {} sif_sha256 {} != index {}; using index",
                c.name, c.sif_sha256, iv.sif_sha256
            )),
            None => warns.push(format!(
                "lock drift: {} repo `{}` not in current index",
                c.name, c.repo
            )),
        }
    }
    Ok(warns)
}

/// Look up a lock ref (`foo`, `foo:latest`, `foo:1.0`, `foo:1.0-1`) in the
/// current index. Old locks used exact keys; new locks store canonical
/// `name:version-release`.
fn find_index_version<'a>(
    index: &'a crate::repo::meta::IndexV1,
    r: &str,
) -> Option<&'a crate::repo::meta::IndexVersion> {
    let (name, ver) = r.split_once(':').unwrap_or((r, "latest"));
    let name = name.to_ascii_lowercase();
    crate::repo::run_package::pick_version(index, &name, Some(ver))
        .ok()
        .map(|(_, v)| v)
}
