use anyhow::{Context, Result};
use std::collections::BTreeMap;

use super::layout::RepoRoot;
use super::meta::{IndexPackage, IndexV1, IndexVersion, MetaV1};
use super::util::{atomic_write, now_iso8601};

/// Пересобрать index.json из packages/**/meta.json.
/// §6.3: `updated` не меняется, если `packages` не изменился.
pub fn rebuild_all(root: &RepoRoot) -> Result<IndexV1> {
    let mut index = IndexV1 {
        schema: 1,
        repo: root.config.repo.name.clone(),
        updated: String::new(),
        generator: root.config.repo.generator.clone(),
        signature: String::new(),
        packages: BTreeMap::new(),
    };

    let pkg_root = root.packages_dir();
    if pkg_root.exists() {
        for name_entry in std::fs::read_dir(&pkg_root)? {
            let name_entry = name_entry?;
            if !name_entry.file_type()?.is_dir() {
                continue;
            }
            let name = name_entry.file_name().to_string_lossy().to_string();
            let mut pkg = IndexPackage {
                summary: None,
                license: None,
                vendor: None,
                url: None,
                versions: BTreeMap::new(),
            };
            for ver_entry in std::fs::read_dir(name_entry.path())? {
                let ver_entry = ver_entry?;
                if !ver_entry.file_type()?.is_dir() {
                    continue;
                }
                let meta_path = ver_entry.path().join("meta.json");
                if !meta_path.is_file() {
                    continue;
                }
                let m: MetaV1 = serde_json::from_str(
                    &std::fs::read_to_string(&meta_path)
                        .with_context(|| format!("read {}", meta_path.display()))?,
                )
                .with_context(|| format!("parse {}", meta_path.display()))?;
                if pkg.summary.is_none() {
                    pkg.summary = m.summary.clone();
                }
                if pkg.license.is_none() {
                    pkg.license = m.license.clone();
                }
                if pkg.vendor.is_none() {
                    pkg.vendor = m.vendor.clone();
                }
                if pkg.url.is_none() {
                    pkg.url = m.url.clone();
                }
                let key = format!("{}-{}", m.version, m.release);
                pkg.versions.insert(
                    key,
                    IndexVersion {
                        epoch: m.epoch,
                        version: m.version,
                        release: m.release,
                        arch: m.arch,
                        sif_sha256: m.sif_sha256,
                        sif_size: m.sif_size,
                        spec_sha256: m.spec_sha256,
                        spec_size: m.spec_size,
                        built_at: m.built_at,
                        generator: m.generator,
                        base_digest: m.base_digest,
                        tags: Vec::new(),
                        requires: m.requires,
                        provides: m.provides,
                        ipfs_cid: m.ipfs_cid,
                    },
                );
            }
            if !pkg.versions.is_empty() {
                index.packages.insert(name, pkg);
            }
        }
    }

    rebuild_refs(root, &mut index)?;

    // §6.3: updated меняется только если packages изменился
    let old_updated = read_existing_updated(root).ok().flatten();
    let new_hash = hash_packages(&index.packages)?;
    let old_hash = read_existing_packages_hash(root).ok().flatten();
    index.updated = if old_hash.as_deref() == Some(new_hash.as_str()) {
        old_updated.unwrap_or_else(now_iso8601)
    } else {
        now_iso8601()
    };

    atomic_write(&root.index_path(), &serde_json::to_vec_pretty(&index)?)?;
    Ok(index)
}

fn read_existing_updated(root: &RepoRoot) -> Result<Option<String>> {
    let text = std::fs::read_to_string(root.index_path())?;
    let v: IndexV1 = serde_json::from_str(&text)?;
    Ok(Some(v.updated))
}

fn read_existing_packages_hash(root: &RepoRoot) -> Result<Option<String>> {
    let text = std::fs::read_to_string(root.index_path())?;
    let v: IndexV1 = serde_json::from_str(&text)?;
    Ok(Some(hash_packages(&v.packages)?))
}

fn hash_packages(p: &BTreeMap<String, IndexPackage>) -> Result<String> {
    Ok(super::util::sha256_hex(&serde_json::to_vec(p)?))
}

fn rebuild_refs(root: &RepoRoot, index: &mut IndexV1) -> Result<()> {
    let refs_dir = root.refs_dir();
    std::fs::create_dir_all(&refs_dir)?;

    for (name, pkg) in index.packages.iter_mut() {
        let mut refs: BTreeMap<String, String> = BTreeMap::new();

        // читаем существующие не-latest теги
        let ref_path = refs_dir.join(format!("{}.toml", name));
        if let Ok(text) = std::fs::read_to_string(&ref_path) {
            if let Ok(existing) = toml::from_str::<RefFile>(&text) {
                for (k, v) in existing.refs {
                    if k != "latest" {
                        refs.insert(k, v);
                    }
                }
            }
        }

        // latest по policy.auto_tag_latest (§5.2)
        if matches!(
            root.config.policy.auto_tag_latest.as_str(),
            "always" | "semver"
        ) {
            if let Some(latest_key) = pick_latest(&pkg.versions) {
                refs.insert("latest".to_string(), latest_key);
            }
        }

        // tags в index
        for v in pkg.versions.values_mut() {
            v.tags.clear();
        }
        for (tag, ver_key) in &refs {
            if let Some(v) = pkg.versions.get_mut(ver_key) {
                v.tags.push(tag.clone());
            }
        }
        for v in pkg.versions.values_mut() {
            v.tags.sort();
        }

        let ref_file = RefFile {
            refs: refs.into_iter().collect(),
        };
        atomic_write(&ref_path, toml::to_string_pretty(&ref_file)?.as_bytes())?;
    }
    Ok(())
}

/// Сравнение `(version, release, epoch)` — §5.2.
/// Использует единый компаратор [`super::meta::ver_key`]
/// (через [`IndexVersion::sort_key`]).
///
/// `pub` — используется из `commands::ls` для отображения `latest=…`
/// в сводке пакета. Экспортируем, чтобы не дублировать логику.
pub fn pick_latest(versions: &BTreeMap<String, IndexVersion>) -> Option<String> {
    versions
        .iter()
        .max_by(|a, b| a.1.sort_key().cmp(&b.1.sort_key()))
        .map(|(k, _)| k.clone())
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct RefFile {
    #[serde(default)]
    refs: BTreeMap<String, String>,
}
