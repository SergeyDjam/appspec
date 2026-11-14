//! `appspec repo pin` — pin blobs to IPFS, backfill ipfs_cid, reindex.
//!
//! Parallel workers (default 8) pull from a shared queue. Per package:
//!   - `ipfs_cid = None` + blob file exists  → `ipfs add --pin` (local)
//!   - `ipfs_cid = Some` + already pinned     → skip
//!   - `ipfs_cid = Some` + local block exists → `pin add` (local)
//!   - `ipfs_cid = Some` + no local block     → skip (unless `--dht`)
//!
//! `--dht`: attempt DHT fetch for non-local blobs (timeout per blob).

use anyhow::{anyhow, Result};
use std::collections::VecDeque;
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

fn pin_one(
    root_path: &Path,
    meta_path: &Path,
    meta: &MetaV1,
    opts: &PinOpts,
    idx: usize,
    total: usize,
    elapsed_ms: u128,
) -> PerEntry {
    let mut s = PerEntry::default();
    let ver_key = format!("{}-{}", meta.version, meta.release);
    let label = format!("{}:{}", meta.name, ver_key);
    let eta = format_eta(idx, total, elapsed_ms);

    if meta.ipfs_cid.is_none() {
        let blob = util::blob_path(root_path, &meta.sif_sha256);
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
                eprintln!("[{}/{}] {} — would add+pin (local)", idx, total, label);
            }
            return s;
        }
        if opts.progress {
            eprintln!("[{}/{}] {} — add+pin (local){}", idx, total, label, eta);
        }
        let t = Instant::now();
        match ipfs::add_file(&blob, true) {
            Ok(cid) => {
                let mut new_meta = meta.clone();
                new_meta.ipfs_cid = Some(cid.clone());
                match serde_json::to_vec_pretty(&new_meta) {
                    Ok(bytes) => match util::atomic_write(meta_path, &bytes) {
                        Ok(()) => {
                            s.added = 1;
                            if opts.progress {
                                eprintln!(
                                    "[{}/{}] {} — OK {} ({}ms)",
                                    idx,
                                    total,
                                    label,
                                    cid,
                                    t.elapsed().as_millis()
                                );
                            }
                        }
                        Err(e) => {
                            s.failed = 1;
                            if opts.progress {
                                eprintln!("[{}/{}] {} — write failed: {:#}", idx, total, label, e);
                            }
                        }
                    },
                    Err(e) => {
                        s.failed = 1;
                        if opts.progress {
                            eprintln!("[{}/{}] {} — serialize failed: {:#}", idx, total, label, e);
                        }
                    }
                }
            }
            Err(e) => {
                s.failed = 1;
                if opts.progress {
                    eprintln!("[{}/{}] {} — FAILED: {:#}", idx, total, label, e);
                }
            }
        }
        return s;
    }

    let cid = meta.ipfs_cid.clone().unwrap();
    let short: String = if cid.len() >= 12 {
        cid[..12].to_string()
    } else {
        cid.clone()
    };

    if ipfs::is_pinned(&cid).unwrap_or(false) {
        s.already_pinned = 1;
        if opts.progress {
            eprintln!("[{}/{}] {} — already pinned ({})", idx, total, label, short);
        }
        return s;
    }

    let is_local = ipfs::is_local_block(&cid).unwrap_or(false);
    let hint = if is_local { "local" } else { "DHT" };

    if !is_local && !opts.dht {
        s.skipped_not_local = 1;
        if opts.progress {
            eprintln!(
                "[{}/{}] {} — SKIP not local (use --dht to fetch), cid {}",
                idx, total, label, short
            );
        }
        return s;
    }

    if opts.dry_run {
        if opts.progress {
            eprintln!(
                "[{}/{}] {} — would pin ({}), timeout {}s, cid {}",
                idx, total, label, hint, opts.pin_timeout_secs, short
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
                let one = pin_one(&rp, &meta_path, &meta, &o, i, total, elapsed_ms);
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
             - run `appspec repo pin --dht --jobs 8 --timeout 600` to fetch from network, or\n  \
             - run `appspec build` to regenerate SIFs locally (they are not in IPFS yet).",
            result.skipped_not_local
        );
    }

    Ok(result)
}
