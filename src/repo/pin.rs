//! `appspec repo pin` — pin blobs to IPFS, backfill ipfs_cid, reindex.
//!
//! Per package:
//!   - `ipfs_cid = None` + blob file exists  → `ipfs add --pin` (local, fast)
//!   - `ipfs_cid = Some` + already pinned     → skip
//!   - `ipfs_cid = Some` + local block exists → `pin add` (local, fast)
//!   - `ipfs_cid = Some` + no local block     → `pin add` (DHT, timeout)
//!
//! Progress on stderr, `--quiet` suppresses.

use anyhow::Result;
use std::time::Instant;

use super::layout::RepoRoot;
use super::meta::MetaV1;
use super::{index_build, ipfs, util};

pub struct PinOpts {
    pub dry_run: bool,
    pub progress: bool,
    pub pin_timeout_secs: u64,
}

#[derive(Default, Debug)]
pub struct PinStats {
    pub total: usize,
    pub added: usize,
    pub pinned_new: usize,
    pub already_pinned: usize,
    pub missing_blob: usize,
    pub failed: usize,
    pub timed_out: usize,
}

pub fn pin_all(root: &RepoRoot, opts: &PinOpts) -> Result<PinStats> {
    let mut stats = PinStats::default();

    if !ipfs::which_ipfs() {
        anyhow::bail!(
            "`{}` not found in PATH; required for `repo pin` (set APPSPEC_IPFS to override)",
            ipfs::ipfs_bin()
        );
    }

    // collect packages
    let mut entries: Vec<(std::path::PathBuf, MetaV1)> = Vec::new();
    let pkg_root = root.packages_dir();
    if pkg_root.exists() {
        for name_entry in std::fs::read_dir(&pkg_root)? {
            let name_entry = name_entry?;
            if !name_entry.file_type()?.is_dir() {
                continue;
            }
            for ver_entry in std::fs::read_dir(name_entry.path())? {
                let ver_entry = ver_entry?;
                if !ver_entry.file_type()?.is_dir() {
                    continue;
                }
                let meta_path = ver_entry.path().join("meta.json");
                if !meta_path.is_file() {
                    continue;
                }
                let text = std::fs::read_to_string(&meta_path)?;
                match serde_json::from_str::<MetaV1>(&text) {
                    Ok(meta) => entries.push((meta_path, meta)),
                    Err(e) => {
                        if opts.progress {
                            eprintln!(
                                "warning: skip {}: parse error: {:#}",
                                meta_path.display(),
                                e
                            );
                        }
                    }
                }
            }
        }
    }

    stats.total = entries.len();
    if entries.is_empty() {
        return Ok(stats);
    }

    for (i, (meta_path, meta)) in entries.iter().enumerate() {
        let ver_key = format!("{}-{}", meta.version, meta.release);
        let label = format!("{}:{}", meta.name, ver_key);
        let idx = format!("[{}/{}]", i + 1, stats.total);

        // 1. ipfs_cid == None → add + pin from local blob
        if meta.ipfs_cid.is_none() {
            let blob = util::blob_path(&root.path, &meta.sif_sha256);
            if !blob.is_file() {
                if opts.progress {
                    eprintln!("{} {} — MISSING blob ({})", idx, label, blob.display());
                }
                stats.missing_blob += 1;
                continue;
            }
            if opts.dry_run {
                if opts.progress {
                    eprintln!("{} {} — would add+pin (local)", idx, label);
                }
                continue;
            }
            if opts.progress {
                eprintln!("{} {} — add+pin (local)", idx, label);
            }
            let start = Instant::now();
            match ipfs::add_file(&blob, true) {
                Ok(cid) => {
                    let mut new_meta = meta.clone();
                    new_meta.ipfs_cid = Some(cid.clone());
                    util::atomic_write(meta_path, &serde_json::to_vec_pretty(&new_meta)?)?;
                    stats.added += 1;
                    if opts.progress {
                        eprintln!(
                            "{} {} — OK {} ({}ms)",
                            idx,
                            label,
                            cid,
                            start.elapsed().as_millis()
                        );
                    }
                }
                Err(e) => {
                    stats.failed += 1;
                    if opts.progress {
                        eprintln!("{} {} — FAILED: {:#}", idx, label, e);
                    }
                }
            }
            continue;
        }

        // 2. ipfs_cid == Some
        let cid = meta.ipfs_cid.as_ref().unwrap().clone();
        let short = if cid.len() >= 12 {
            &cid[..12]
        } else {
            &cid[..]
        };

        match ipfs::is_pinned(&cid) {
            Ok(true) => {
                stats.already_pinned += 1;
                if opts.progress {
                    eprintln!("{} {} — already pinned ({})", idx, label, short);
                }
                continue;
            }
            Ok(false) => {}
            Err(_) => {}
        }

        let is_local = ipfs::is_local_block(&cid).unwrap_or(false);
        let hint = if is_local { "local" } else { "DHT" };

        if opts.dry_run {
            if opts.progress {
                eprintln!(
                    "{} {} — would pin ({}), timeout {}s, cid {}",
                    idx, label, hint, opts.pin_timeout_secs, short
                );
            }
            continue;
        }

        if opts.progress {
            eprintln!(
                "{} {} — pin add ({}), timeout {}s, cid {}",
                idx, label, hint, opts.pin_timeout_secs, short
            );
        }
        let start = Instant::now();
        match ipfs::pin_with_timeout(&cid, opts.pin_timeout_secs) {
            Ok(()) => {
                stats.pinned_new += 1;
                if opts.progress {
                    eprintln!("{} {} — OK ({}s)", idx, label, start.elapsed().as_secs());
                }
            }
            Err(e) => {
                let msg = format!("{:#}", e);
                if msg.contains("timed out") {
                    stats.timed_out += 1;
                    if opts.progress {
                        eprintln!(
                            "{} {} — TIMEOUT after {}s, skip",
                            idx, label, opts.pin_timeout_secs
                        );
                    }
                } else {
                    stats.failed += 1;
                    if opts.progress {
                        eprintln!("{} {} — FAILED: {}", idx, label, msg);
                    }
                }
            }
        }
    }

    if !opts.dry_run && (stats.added > 0 || stats.pinned_new > 0) {
        let _ = index_build::rebuild_all(root)?;
    }

    Ok(stats)
}
