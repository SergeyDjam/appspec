use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};

use super::layout::RepoRoot;
use super::meta::MetaV1;

pub struct GcOutcome {
    pub removed: Vec<std::path::PathBuf>,
    pub referenced: usize,
    pub bytes_reclaimed: u64,
}

pub fn run(root: &RepoRoot, dry_run: bool, keep: Option<usize>) -> Result<GcOutcome> {
    // все blob'ы на диске
    let mut all_blobs: BTreeMap<String, std::path::PathBuf> = BTreeMap::new();
    let blobs_root = root.blobs_dir();
    if blobs_root.exists() {
        for ab in std::fs::read_dir(&blobs_root)? {
            let ab = ab?;
            if !ab.file_type()?.is_dir() {
                continue;
            }
            let ab_s = ab.file_name().to_string_lossy().to_string();
            for f in std::fs::read_dir(ab.path())? {
                let f = f?;
                if !f.file_type()?.is_file() {
                    continue;
                }
                let sha = format!("{}{}", ab_s, f.file_name().to_string_lossy());
                all_blobs.insert(sha, f.path());
            }
        }
    }

    // пакеты: name -> [MetaV1], сортировка по (version, release, epoch) desc
    let mut per_pkg: BTreeMap<String, Vec<MetaV1>> = BTreeMap::new();
    let pkg_root = root.packages_dir();
    if pkg_root.exists() {
        for name_entry in std::fs::read_dir(&pkg_root)? {
            let name_entry = name_entry?;
            if !name_entry.file_type()?.is_dir() {
                continue;
            }
            let name = name_entry.file_name().to_string_lossy().to_string();
            let mut v = Vec::new();
            for ver_entry in std::fs::read_dir(name_entry.path())? {
                let ver_entry = ver_entry?;
                if !ver_entry.file_type()?.is_dir() {
                    continue;
                }
                let meta_path = ver_entry.path().join("meta.json");
                if !meta_path.is_file() {
                    continue;
                }
                v.push(serde_json::from_str::<MetaV1>(&std::fs::read_to_string(
                    &meta_path,
                )?)?);
            }
            v.sort_by(|a, b| ver_key(b).cmp(&ver_key(a)));
            per_pkg.insert(name, v);
        }
    }

    // referenced = keep-N последних per package (arch считается отдельной
    // версией, см. §6.5 ТЗ)
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    for versions in per_pkg.values() {
        let n = keep.unwrap_or(usize::MAX).min(versions.len());
        for m in versions.iter().take(n) {
            referenced.insert(m.sif_sha256.clone());
        }
    }

    let mut removed = Vec::new();
    let mut bytes = 0u64;
    for (sha, path) in &all_blobs {
        if referenced.contains(sha) {
            continue;
        }
        bytes += std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if !dry_run {
            std::fs::remove_file(path)?;
            if let Some(p) = path.parent() {
                let _ = std::fs::remove_dir(p);
            }
        }
        removed.push(path.clone());
    }

    Ok(GcOutcome {
        removed,
        referenced: referenced.len(),
        bytes_reclaimed: bytes,
    })
}

fn ver_key(m: &MetaV1) -> (Vec<i64>, String, i64) {
    let nums: Vec<i64> = m
        .version
        .split('.')
        .map(|s| s.parse().unwrap_or(0))
        .collect();
    (nums, m.release.clone(), m.epoch as i64)
}
