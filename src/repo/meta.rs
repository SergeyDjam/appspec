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
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub provides: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipfs_cid: Option<String>,
}
