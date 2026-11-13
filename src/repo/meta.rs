use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MetaV1 {
    pub schema: u32,
    pub name: String,
    pub version: String,
    pub release: String,
    pub epoch: u32, // §5.3: integer
    pub arch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub packager: Option<String>,
    pub sif_sha256: String,
    pub sif_size: u64,
    pub spec_sha256: String,
    pub spec_size: u64,
    pub spec_filename: String,
    pub generator: String,
    pub built_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_digest: Option<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub provides: Vec<String>,
    #[serde(default)]
    pub conflicts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipfs_cid: Option<String>,
    #[serde(default)]
    pub raw_labels: BTreeMap<String, String>,
    // --- иконка (E-icon) ---
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_mimetype: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_size: Option<u64>,
    /// Имя файла иконки в `packages/<name>/<ver>/` (например, `icon.png`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_filename: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexV1 {
    pub schema: u32,
    pub repo: String,
    pub updated: String,
    pub generator: String,
    #[serde(default)]
    pub signature: String, // §5.5: всегда присутствует
    pub packages: BTreeMap<String, IndexPackage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexPackage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub versions: BTreeMap<String, IndexVersion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexVersion {
    pub epoch: u32,
    pub version: String,
    pub release: String,
    pub arch: String,
    pub sif_sha256: String,
    pub sif_size: u64,
    pub spec_sha256: String,
    pub spec_size: u64,
    pub built_at: String,
    pub generator: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_digest: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    // --- иконка (E-icon) ---
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_mimetype: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_size: Option<u64>,
    /// Имя файла иконки в `packages/<name>/<ver>/` (например, `icon.png`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_filename: Option<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub provides: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipfs_cid: Option<String>,
}

// ===========================================================================
// Сравнение версий (§5.2)
// ===========================================================================

/// Сравнение `(version, release, epoch)` — §5.2.
///
/// Не полный semver 2.0.0: `1.2` < `1.2.0` (префикс); для типичных
/// единообразных версий этого достаточно.
///
/// Порядок:
///   1. `version` — покомпонентно численно (`1.10` > `1.9`, не "10" < "9");
///   2. `release` — лексикографически как tiebreak;
///   3. `epoch`   — численно как финальный tiebreak.
///
/// Единый компаратор для `IndexVersion` и `MetaV1`: обе структуры имеют
/// `version`, `release`, `epoch`, поэтому порядок должен быть идентичным
/// во всех местах, где сортируются версии (`repo ls`, `repo gc`,
/// `rebuild_refs`, будущие `repo promote` / `repo ls --sort`).
pub fn ver_key(version: &str, release: &str, epoch: u32) -> (Vec<i64>, String, i64) {
    let nums: Vec<i64> = version.split('.').map(|s| s.parse().unwrap_or(0)).collect();
    (nums, release.to_string(), epoch as i64)
}

impl IndexVersion {
    /// Ключ сортировки — см. [`ver_key`].
    pub fn sort_key(&self) -> (Vec<i64>, String, i64) {
        ver_key(&self.version, &self.release, self.epoch)
    }
}

impl MetaV1 {
    /// Ключ сортировки — см. [`ver_key`].
    pub fn sort_key(&self) -> (Vec<i64>, String, i64) {
        ver_key(&self.version, &self.release, self.epoch)
    }
}

#[cfg(test)]
mod tests {
    use super::ver_key;

    #[test]
    fn ver_key_compares_version_numerically() {
        // Покомпонентно численно: 10 > 9, а не "10" < "9".
        assert!(ver_key("1.10.0", "1", 0) > ver_key("1.9.0", "1", 0));
        assert!(ver_key("2.2.6", "1", 0) > ver_key("2.2.0", "1", 0));
    }

    #[test]
    fn ver_key_uses_release_as_tiebreak() {
        assert!(ver_key("1.2.31", "1.2", 0) > ver_key("1.2.31", "1", 0));
    }

    #[test]
    fn ver_key_uses_epoch_as_final_tiebreak() {
        assert!(ver_key("1.0", "1", 2) > ver_key("1.0", "1", 1));
    }
}
