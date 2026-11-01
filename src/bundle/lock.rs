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

use super::model::BundleFile;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct LockFile {
    pub bundle: LockBundle,
    #[serde(default, rename = "components")]
    pub components: Vec<LockComponent>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct LockBundle {
    pub name: String,
    pub locked_at: String,
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
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
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
        self.components.iter().map(|c| (c.name.as_str(), c)).collect()
    }
}

/// `<spec_dir>/bundle.lock.toml`.
pub fn default_path(spec_path: &Path) -> PathBuf {
    spec_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("bundle.lock.toml")
}

/// Сгенерировать lock по текущему индексу для всех `repo`-компонентов.
/// Компоненты без `repo` пропускаются (у них другая природа: spec/sif/cid/manifest).
pub fn generate(file: &BundleFile, _spec_path: &Path) -> Result<LockFile> {
    use crate::repo::{default_root, layout::RepoRoot};

    let root = RepoRoot::open(&default_root())
        .map_err(|e| anyhow!("repo not available: {e:#}"))?;
    let index: crate::repo::meta::IndexV1 = serde_json::from_str(
        &std::fs::read_to_string(root.index_path())?,
    )
    .context("parsing repo index.json")?;

    let mut components = Vec::new();
    for c in &file.components {
        let Some(r) = &c.source.repo else { continue };
        let (name, ver) = r.split_once(':').ok_or_else(|| {
            anyhow!(
                "component `{}`: repo source must be `NAME:VERSION-RELEASE`, got `{r}`",
                c.name
            )
        })?;
        let pkg_name = name.to_ascii_lowercase();
        let pkg = index
            .packages
            .get(&pkg_name)
            .ok_or_else(|| anyhow!("package `{pkg_name}` not in repo index"))?;
        let v = pkg
            .versions
            .get(ver)
            .ok_or_else(|| anyhow!("{pkg_name}:{ver} not in repo index"))?;
        components.push(LockComponent {
            name: c.name.clone(),
            repo: r.clone(),
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
        components,
    })
}

/// Сверить lock с текущим индексом. Возвращает список warning'ов
/// (дрейф). Не ошибка: `bundle build` идёт по индексу.
pub fn check_drift(lock: &LockFile) -> Result<Vec<String>> {
    use crate::repo::{default_root, layout::RepoRoot};

    let mut warns = Vec::new();
    let root = RepoRoot::open(&default_root())
        .map_err(|e| anyhow!("repo not available: {e:#}"))?;
    let index: crate::repo::meta::IndexV1 = serde_json::from_str(
        &std::fs::read_to_string(root.index_path())?,
    )
    .context("parsing repo index.json")?;

    for c in &lock.components {
        let Some((n, v)) = c.repo.split_once(':') else { continue };
        let n = n.to_ascii_lowercase();
        match index.packages.get(&n).and_then(|p| p.versions.get(v)) {
            Some(iv) if iv.sif_sha256 == c.sif_sha256 => { /* ok */ }
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