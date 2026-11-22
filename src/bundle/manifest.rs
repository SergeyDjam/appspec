#![allow(dead_code)] // reserved: repo add-bundle / run <bundle-name> (B/C)

//! Bundle manifest (0.6.1).
//!
//! A published bundle package (`kind = "bundle"`) stores its original bundle
//! spec as `spec.appspec` plus a resolved `manifest.json`. The manifest pins
//! every source by `name:version-release` and hash, so a client can rebuild
//! the launcher without re-resolving `latest`.

use serde::{Deserialize, Serialize};

pub const MANIFEST_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleManifestV1 {
    pub schema: u32,
    pub name: String,
    pub version: String,
    pub release: String,
    pub epoch: u32,
    pub arch: String,
    /// sha256 of the bundle spec (`spec.appspec` in the package).
    pub bundle_sha256: String,
    pub base: ManifestSource,
    #[serde(default)]
    pub components: Vec<ManifestComponent>,
    #[serde(default)]
    pub extensions: Vec<ManifestExtension>,
    // No timestamps/generator here: the manifest must be deterministic so
    // `repo add` is idempotent for the same bundle + index. Provenance lives
    // in meta.json / index.json (`built_at`, `generator`).
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestSource {
    /// Original value of `spec = "..."`.
    pub spec: String,
    /// Resolution result: `repo:name:ver`, `local:path`, `cid:...`, ...
    pub resolved: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sif_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipfs_cid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestComponent {
    pub name: String,
    pub provide: String,
    pub mode: String,
    pub source: ManifestSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestExtension {
    pub name: String,
    /// `cid:...` / `url:...`, or `None` for Open VSX online.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_roundtrip() {
        let m = BundleManifestV1 {
            schema: MANIFEST_SCHEMA,
            name: "dev".into(),
            version: "1.0".into(),
            release: "1".into(),
            epoch: 0,
            arch: "noarch".into(),
            bundle_sha256: "ab".repeat(32),
            base: ManifestSource {
                spec: "base:1.0".into(),
                resolved: "repo:base:1.0-1".into(),
                sif_sha256: Some("cd".repeat(32)),
                spec_sha256: Some("ef".repeat(32)),
                ipfs_cid: None,
            },
            components: vec![ManifestComponent {
                name: "git".into(),
                provide: "git".into(),
                mode: "chroot".into(),
                source: ManifestSource {
                    spec: "git".into(),
                    resolved: "repo:git:2.47.2-1".into(),
                    sif_sha256: Some("11".repeat(32)),
                    spec_sha256: Some("22".repeat(32)),
                    ipfs_cid: Some("bafkrei-test".into()),
                },
            }],
            extensions: vec![ManifestExtension {
                name: "publisher.ext".into(),
                source: None,
                cid: None,
                url: None,
                requires: vec![],
            }],
        };
        let json = serde_json::to_string_pretty(&m).unwrap();
        assert!(json.contains("\"schema\": 1"), "{json}");
        assert!(json.contains("repo:git:2.47.2-1"), "{json}");
        let back: BundleManifestV1 = serde_json::from_str(&json).unwrap();
        assert_eq!(back.components.len(), 1);
        assert_eq!(
            back.components[0].source.sif_sha256,
            m.components[0].source.sif_sha256
        );
    }
}
