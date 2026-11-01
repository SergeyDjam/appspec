use anyhow::Result;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::layout::RepoRoot;
use super::meta::MetaV1;
use super::util::{blob_path, sha256_file};

pub struct CheckOutcome { pub errors: usize, pub warnings: usize }

pub fn run(root: &RepoRoot, deep: bool) -> Result<CheckOutcome> {
    let mut errors = 0usize;
    let warnings = 0usize;

    let pkg_root = root.packages_dir();
    if !pkg_root.exists() {
        eprintln!("warning: no packages yet");
        return Ok(CheckOutcome { errors, warnings });
    }

    // собираем все (meta_path, blob, meta)
    let mut items = Vec::new();
    for name_entry in std::fs::read_dir(&pkg_root)? {
        let name_entry = name_entry?;
        if !name_entry.file_type()?.is_dir() { continue; }
        for ver_entry in std::fs::read_dir(name_entry.path())? {
            let ver_entry = ver_entry?;
            if !ver_entry.file_type()?.is_dir() { continue; }
            let meta_path = ver_entry.path().join("meta.json");
            if !meta_path.is_file() { continue; }
            let m: MetaV1 = serde_json::from_str(&std::fs::read_to_string(&meta_path)?)?;
            let blob = blob_path(&root.path, &m.sif_sha256);
            items.push((meta_path, blob, m));
        }
    }

    // 1) blob sha256
    for (meta_path, blob, m) in &items {
        if !blob.is_file() {
            eprintln!("error: blob missing for {}: {}", meta_path.display(), blob.display());
            errors += 1;
            continue;
        }
        let actual = sha256_file(blob)?;
        if actual != m.sif_sha256 {
            eprintln!("error: blob sha256 mismatch for {}: expected {}, got {}",
                blob.display(), m.sif_sha256, actual);
            errors += 1;
        }
    }

    // 2) spec sha256
    for (meta_path, _blob, m) in &items {
        let spec_path = meta_path.parent().unwrap().join("spec.appspec");
        if !spec_path.is_file() {
            eprintln!("error: spec.appspec missing next to {}", meta_path.display());
            errors += 1;
            continue;
        }
        let actual = sha256_file(&spec_path)?;
        if actual != m.spec_sha256 {
            eprintln!("error: spec sha256 mismatch for {}: expected {}, got {}",
                spec_path.display(), m.spec_sha256, actual);
            errors += 1;
        }
    }

    // 3) --deep: параллельный apptainer inspect
    if deep && !items.is_empty() {
        let results: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let idx = Arc::new(AtomicUsize::new(0));
        let items = Arc::new(items);
        let n_workers = std::thread::available_parallelism()
            .map(|n| n.get()).unwrap_or(4).min(8);
        let mut handles = Vec::new();
        for _ in 0..n_workers {
            let idx = idx.clone();
            let items = items.clone();
            let results = results.clone();
            handles.push(std::thread::spawn(move || loop {
                let i = idx.fetch_add(1, Ordering::SeqCst);
                if i >= items.len() { break; }
                let (_meta_path, blob, m) = &items[i];
                match super::inspect::inspect(Path::new(blob)) {
                    Ok(labels) => {
                        let actual = labels.get("org.appspec.spec.sha256").unwrap_or("");
                        if actual != m.spec_sha256 {
                            results.lock().unwrap().push(format!(
                                "{}: spec sha mismatch (inspect {} vs meta {})",
                                blob.display(), actual, m.spec_sha256));
                        }
                    }
                    Err(e) => results.lock().unwrap().push(format!(
                        "{}: inspect failed: {:#}", blob.display(), e)),
                }
            }));
        }
        for h in handles { let _ = h.join(); }
        for msg in results.lock().unwrap().iter() {
            eprintln!("error: {}", msg);
            errors += 1;
        }
    }

    Ok(CheckOutcome { errors, warnings })
}
