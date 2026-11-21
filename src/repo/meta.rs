use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 0.6.1: repository packages can be either a SIF or a bundle.
/// Old meta/index entries have no `kind` and default to `sif`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageKind {
    #[default]
    Sif,
    Bundle,
}

#[allow(dead_code)] // reserved: repo ls/show output for bundles
impl PackageKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            PackageKind::Sif => "sif",
            PackageKind::Bundle => "bundle",
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct MetaV1 {
    pub schema: u32,
    #[serde(default)]
    pub kind: PackageKind,
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
    /// Bundle artifact (the bundle spec stored as `spec.appspec`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_size: Option<u64>,
    /// Resolved bundle manifest (`manifest.json`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_ipfs_cid: Option<String>,
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

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct IndexVersion {
    #[serde(default)]
    pub kind: PackageKind,
    pub epoch: u32,
    pub version: String,
    pub release: String,
    pub arch: String,
    pub sif_sha256: String,
    pub sif_size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_ipfs_cid: Option<String>,
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

    #[allow(dead_code)] // reserved: B/C
    pub fn is_bundle(&self) -> bool {
        self.kind == PackageKind::Bundle
    }

    /// Hash of the primary artifact: SIF for `kind = sif`, bundle spec for
    /// `kind = bundle`.
    #[allow(dead_code)] // reserved: B/C
    pub fn artifact_sha256(&self) -> Option<&str> {
        match self.kind {
            PackageKind::Sif => Some(self.sif_sha256.as_str()),
            PackageKind::Bundle => self.bundle_sha256.as_deref(),
        }
    }
}

impl MetaV1 {
    /// Ключ сортировки — см. [`ver_key`].
    pub fn sort_key(&self) -> (Vec<i64>, String, i64) {
        ver_key(&self.version, &self.release, self.epoch)
    }

    #[allow(dead_code)] // reserved: B/C
    pub fn is_bundle(&self) -> bool {
        self.kind == PackageKind::Bundle
    }

    /// Hash of the primary artifact: SIF for `kind = sif`, bundle spec for
    /// `kind = bundle`.
    #[allow(dead_code)] // reserved: B/C
    pub fn artifact_sha256(&self) -> Option<&str> {
        match self.kind {
            PackageKind::Sif => Some(self.sif_sha256.as_str()),
            PackageKind::Bundle => self.bundle_sha256.as_deref(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ver_key, IndexVersion, MetaV1, PackageKind};

    #[test]
    fn old_index_version_defaults_to_sif() {
        let v: IndexVersion = serde_json::from_str(
            r#"{
                "epoch": 0, "version": "1.0", "release": "1", "arch": "amd64",
                "sif_sha256": "aa", "sif_size": 1,
                "spec_sha256": "bb", "spec_size": 2,
                "built_at": "2026-11-21T00:00:00Z", "generator": "test"
            }"#,
        )
        .unwrap();
        assert_eq!(v.kind, PackageKind::Sif);
        assert!(v.bundle_sha256.is_none());
        assert_eq!(v.artifact_sha256(), Some("aa"));
    }

    #[test]
    fn bundle_meta_roundtrip_keeps_kind_and_hashes() {
        let m = MetaV1 {
            schema: 1,
            kind: PackageKind::Bundle,
            name: "dev".into(),
            version: "1.0".into(),
            release: "1".into(),
            arch: "noarch".into(),
            sif_sha256: String::new(),
            sif_size: 0,
            bundle_sha256: Some("ab".repeat(32)),
            bundle_size: Some(10),
            manifest_sha256: Some("cd".repeat(32)),
            manifest_size: Some(20),
            manifest_ipfs_cid: Some("bafkrei-test".into()),
            spec_sha256: "ef".repeat(32),
            spec_size: 5,
            spec_filename: "spec.appspec".into(),
            generator: "test".into(),
            built_at: "2026-11-21T00:00:00Z".into(),
            ..Default::default()
        };
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("\"kind\":\"bundle\""), "{json}");
        assert!(json.contains("manifest_sha256"), "{json}");

        let back: MetaV1 = serde_json::from_str(&json).unwrap();
        assert_eq!(back.kind, PackageKind::Bundle);
        assert_eq!(back.manifest_ipfs_cid.as_deref(), Some("bafkrei-test"));
        assert_eq!(back.artifact_sha256(), m.bundle_sha256.as_deref());
    }

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
