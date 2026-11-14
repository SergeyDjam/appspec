//! `appspec repo pin` — pin blobs to IPFS, backfill ipfs_cid, reindex.
//!
//! Resolution order per package:
//! - `ipfs_cid = None` with local blob file: `ipfs add --pin` (fast, local).
//! - `ipfs_cid = Some` already in pinned set: skip.
//! - `ipfs_cid = Some` with local blob file: `ipfs add --pin` (fast, local).
//! - `ipfs_cid = Some` with local block: `pin add` (fast, local).
//! - `ipfs_cid = Some` without local block: skip (`--dht` to fetch).
//!
//! `pinned_set` is built once via `ipfs pin ls --type=recursive`.
//! Progress on stderr, `--quiet` suppresses.

use anyhow::{anyhow, Result};
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use super::layout::RepoRoot;
use super::meta::MetaV1;
use super::{index_build, ipfs, util};

#[derive(Clone, Debug)]
pub struct PinOpts {
    pub dry_run: bool,
    pub progress: bool,
    pub pin_timeout_secs: u64,
    pub jobs: usize,
    pub dht: bool,
}

impl Default for PinOpts {
    fn default() -> Self {
        Self {
            dry_run: false,
            progress: true,
            pin_timeout_secs: 600,
            jobs: 8,
            dht: false,
        }
    }
}

#[derive(Default, Debug, Clone)]
pub struct PinStats {
    pub total: usize,
    pub added: usize,
    pub pinned_new: usize,
    pub already_pinned: usize,
    pub missing_blob: usize,
    pub skipped_not_local: usize,
    pub failed: usize,
    pub timed_out: usize,
}

fn format_eta(idx: usize, total: usize, elapsed_ms: u128) -> String {
    if idx == 0 || total == 0 || idx >= total {
        return String::new();
    }
    let remaining = total.saturating_sub(idx);
    let avg_ms = elapsed_ms as f64 / idx as f64;
    let eta_s = ((remaining as f64 * avg_ms) / 1000.0) as u64;
    format!(" ETA~{}s", eta_s)
}

#[derive(Default)]
struct PerEntry {
    added: usize,
    pinned_new: usize,
    already_pinned: usize,
    missing_blob: usize,
    skipped_not_local: usize,
    failed: usize,
    timed_out: usize,
}

fn short(cid: &str) -> &str {
    if cid.len() >= 12 {
        &cid[..12]
    } else {
        cid
    }
}

#[allow(clippy::too_many_arguments)]
fn add_from_blob(
    blob: &Path,
    meta_path: &Path,
    meta: &MetaV1,
    expected_cid: Option<&str>,
    opts: &PinOpts,
    idx: usize,
    total: usize,
    label: &str,
    eta: &str,
) -> PerEntry {
    let mut s = PerEntry::default();
    let t = Instant::now();
    match ipfs::add_file(blob, true) {
        Ok(actual) => {
            if let Some(exp) = expected_cid {
                if actual != exp {
                    s.failed = 1;
                    if opts.progress {
                        eprintln!(
                            "[{}/{}] {} — FAILED: ipfs add returned {} but meta.ipfs_cid is {} \
                             (corrupted blob or stale index; run `appspec repo reindex --backfill-cid`?)",
                            idx, total, label, actual, exp
                        );
                    }
                    return s;
                }
            } else {
                let mut new_meta = meta.clone();
                new_meta.ipfs_cid = Some(actual.clone());
                match serde_json::to_vec_pretty(&new_meta) {
                    Ok(bytes) => {
                        if let Err(e) = util::atomic_write(meta_path, &bytes) {
                            s.failed = 1;
                            if opts.progress {
                                eprintln!("[{}/{}] {} — write failed: {:#}", idx, total, label, e);
                            }
                            return s;
                        }
                    }
                    Err(e) => {
                        s.failed = 1;
                        if opts.progress {
                            eprintln!("[{}/{}] {} — serialize failed: {:#}", idx, total, label, e);
                        }
                        return s;
                    }
                }
            }
            s.added = 1;
            if opts.progress {
                eprintln!(
                    "[{}/{}] {} — OK {} ({}ms){}",
                    idx,
                    total,
                    label,
                    actual,
                    t.elapsed().as_millis(),
                    eta
                );
            }
        }
        Err(e) => {
            s.failed = 1;
            if opts.progress {
                eprintln!("[{}/{}] {} — FAILED: {:#}", idx, total, label, e);
            }
        }
    }
    s
}

#[allow(clippy::too_many_arguments)]
fn pin_one(
    root_path: &Path,
    meta_path: &Path,
    meta: &MetaV1,
    opts: &PinOpts,
    idx: usize,
    total: usize,
    elapsed_ms: u128,
    pinned_set: &HashSet<String>,
) -> PerEntry {
    let mut s = PerEntry::default();
    let ver_key = format!("{}-{}", meta.version, meta.release);
    let label = format!("{}:{}", meta.name, ver_key);
    let eta = format_eta(idx, total, elapsed_ms);
    let blob = util::blob_path(root_path, &meta.sif_sha256);

    // 1. cid = None → add from local blob file
    if meta.ipfs_cid.is_none() {
        if !blob.is_file() {
            if opts.progress {
                eprintln!(
                    "[{}/{}] {} — MISSING blob {}",
                    idx,
                    total,
                    label,
                    blob.display()
                );
            }
            s.missing_blob = 1;
            return s;
        }
        if opts.dry_run {
            if opts.progress {
                eprintln!("[{}/{}] {} — would add+pin (local file)", idx, total, label);
            }
            return s;
        }
        if opts.progress {
            eprintln!(
                "[{}/{}] {} — add+pin (local file){}",
                idx, total, label, eta
            );
        }
        return add_from_blob(&blob, meta_path, meta, None, opts, idx, total, &label, &eta);
    }

    // 2. cid = Some
    let cid = meta.ipfs_cid.clone().unwrap();
    let sc = short(&cid).to_string();

    // 2a. batch pinned set
    if pinned_set.contains(&cid) {
        s.already_pinned = 1;
        if opts.progress {
            eprintln!("[{}/{}] {} — already pinned ({})", idx, total, label, sc);
        }
        return s;
    }

    // 2b. blob file exists → add --pin from disk (fast local)
    if blob.is_file() {
        if opts.dry_run {
            if opts.progress {
                eprintln!("[{}/{}] {} — would add+pin (local file)", idx, total, label);
            }
            return s;
        }
        if opts.progress {
            eprintln!(
                "[{}/{}] {} — add+pin (local file){}",
                idx, total, label, eta
            );
        }
        return add_from_blob(
            &blob,
            meta_path,
            meta,
            Some(&cid),
            opts,
            idx,
            total,
            &label,
            &eta,
        );
    }

    // 2c. blob file missing, block in blockstore → pin add (fast)
    let is_local = ipfs::is_local_block(&cid).unwrap_or(false);
    let hint = if is_local { "local" } else { "DHT" };

    if !is_local && !opts.dht {
        s.skipped_not_local = 1;
        if opts.progress {
            eprintln!(
                "[{}/{}] {} — SKIP not local (no blob file, block not in blockstore; \
                 use --dht or `appspec build`), cid {}",
                idx, total, label, sc
            );
        }
        return s;
    }

    if opts.dry_run {
        if opts.progress {
            eprintln!(
                "[{}/{}] {} — would pin ({}), timeout {}s, cid {}",
                idx, total, label, hint, opts.pin_timeout_secs, sc
            );
        }
        return s;
    }

    if opts.progress {
        eprintln!("[{}/{}] {} — pin add ({}){}", idx, total, label, hint, eta);
    }
    let t = Instant::now();
    match ipfs::pin_with_timeout(&cid, opts.pin_timeout_secs) {
        Ok(()) => {
            s.pinned_new = 1;
            if opts.progress {
                eprintln!(
                    "[{}/{}] {} — OK ({}s)",
                    idx,
                    total,
                    label,
                    t.elapsed().as_secs()
                );
            }
        }
        Err(e) => {
            let msg = format!("{:#}", e);
            if msg.contains("timed out") {
                s.timed_out = 1;
                if opts.progress {
                    eprintln!(
                        "[{}/{}] {} — TIMEOUT {}s, skip",
                        idx, total, label, opts.pin_timeout_secs
                    );
                }
            } else {
                s.failed = 1;
                if opts.progress {
                    eprintln!("[{}/{}] {} — FAILED: {}", idx, total, label, msg);
                }
            }
        }
    }
    s
}

pub fn pin_all(root: &RepoRoot, opts: &PinOpts) -> Result<PinStats> {
    if !ipfs::which_ipfs() {
        anyhow::bail!(
            "`{}` not found in PATH; required for `repo pin` (set APPSPEC_IPFS to override)",
            ipfs::ipfs_bin()
        );
    }

    // 1. batch pinned list (1 call, 30s timeout)
    let pinned_set = match ipfs::list_pinned() {
        Ok(s) => s,
        Err(e) => {
            if opts.progress {
                eprintln!("warning: cannot list pinned CIDs: {:#}; assuming empty", e);
            }
            HashSet::new()
        }
    };
    if opts.progress {
        eprintln!("pinned: {} CID(s) already in IPFS", pinned_set.len());
    }

    // 2. collect packages
    let mut entries: Vec<(PathBuf, MetaV1)> = Vec::new();
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
                            eprintln!("warning: skip {}: {:#}", meta_path.display(), e);
                        }
                    }
                }
            }
        }
    }

    let total = entries.len();
    if total == 0 {
        return Ok(PinStats::default());
    }

    let width = opts.jobs.max(1);
    if opts.progress && width > 1 {
        eprintln!("pin: {} package(s), {} workers", total, width);
    }

    let root_path = root.path.clone();
    let pinned_arc = Arc::new(pinned_set);
    let queue = Arc::new(Mutex::new(VecDeque::from(entries)));
    let done = Arc::new(AtomicUsize::new(0));
    let agg = Arc::new(Mutex::new(PinStats {
        total,
        ..Default::default()
    }));
    let opts_arc = Arc::new(opts.clone());
    let start_all = Instant::now();

    let mut handles: Vec<thread::JoinHandle<Result<()>>> = Vec::with_capacity(width);
    for _ in 0..width {
        let q = Arc::clone(&queue);
        let d = Arc::clone(&done);
        let a = Arc::clone(&agg);
        let rp = root_path.clone();
        let o = Arc::clone(&opts_arc);
        let ps = Arc::clone(&pinned_arc);
        handles.push(thread::spawn(move || -> Result<()> {
            loop {
                let item = {
                    let mut guard = q.lock().map_err(|_| anyhow!("queue lock"))?;
                    guard.pop_front()
                };
                let Some((meta_path, meta)) = item else {
                    break;
                };
                let i = d.fetch_add(1, Ordering::SeqCst) + 1;
                let elapsed_ms = start_all.elapsed().as_millis();
                let one = pin_one(&rp, &meta_path, &meta, &o, i, total, elapsed_ms, &ps);
                let mut guard = a.lock().map_err(|_| anyhow!("stats lock"))?;
                guard.added += one.added;
                guard.pinned_new += one.pinned_new;
                guard.already_pinned += one.already_pinned;
                guard.missing_blob += one.missing_blob;
                guard.skipped_not_local += one.skipped_not_local;
                guard.failed += one.failed;
                guard.timed_out += one.timed_out;
            }
            Ok(())
        }));
    }
    for h in handles {
        h.join().map_err(|_| anyhow!("pin worker panicked"))??;
    }

    let result = Arc::try_unwrap(agg)
        .map_err(|_| anyhow!("stats still shared"))?
        .into_inner()
        .map_err(|_| anyhow!("stats lock"))?;

    if !opts.dry_run && (result.added > 0 || result.pinned_new > 0) {
        let _ = index_build::rebuild_all(root)?;
    }

    if opts.progress && !opts.dht && result.skipped_not_local > 0 {
        eprintln!(
            "info: {} blob(s) not local and skipped. Either:\n  \
             - `appspec repo pin --dht --jobs 8 --timeout 60` (fetch from network), or\n  \
             - `appspec build` (regenerate SIFs locally — they are not in IPFS yet).",
            result.skipped_not_local
        );
    }

    Ok(result)
}
