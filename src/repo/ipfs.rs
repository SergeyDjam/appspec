//! Обёртка над `ipfs` CLI.
//!
//! Бинарь: `$APPSPEC_IPFS` (для тестов) или `ipfs`.
//! Все операции — через CLI, без libp2p/HTTP API в v1.

use anyhow::{anyhow, bail, Context, Result};
use std::io::Write;
use std::path::Path;
use std::process::Command;

pub fn ipfs_bin() -> String {
    std::env::var("APPSPEC_IPFS").unwrap_or_else(|_| "ipfs".to_string())
}

pub fn which_ipfs() -> bool {
    let bin = ipfs_bin();
    if bin.contains('/') {
        return Path::new(&bin).is_file();
    }
    std::env::var_os("PATH").is_some_and(|p| {
        std::env::split_paths(&p).any(|d| d.join(&bin).is_file())
    })
}

/// `ipfs add --pin=<pin> --quieter <path>` → CID (trim).
pub fn add_file(path: &Path, pin: bool) -> Result<String> {
    let bin = ipfs_bin();
    if !which_ipfs() {
        bail!("`{}` not found in PATH; required for IPFS ops (set APPSPEC_IPFS to override)", bin);
    }
    let out = Command::new(&bin)
        .args(["add", &format!("--pin={}", pin), "--quieter"])
        .arg(path)
        .output()
        .with_context(|| format!("spawning {} add", bin))?;
    if !out.status.success() {
        bail!(
            "{} add failed for {}: {}",
            bin, path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let cid = String::from_utf8(out.stdout)
        .context("ipfs add stdout is not UTF-8")?
        .trim()
        .to_string();
    if cid.is_empty() {
        bail!("{} add returned empty CID for {}", bin, path.display());
    }
    Ok(cid)
}

/// `ipfs cat <cid>` → байты (в памяти).
pub fn cat(cid: &str) -> Result<Vec<u8>> {
    let bin = ipfs_bin();
    if !which_ipfs() {
        bail!("`{}` not found in PATH; required for IPFS ops", bin);
    }
    let out = Command::new(&bin)
        .args(["cat", cid])
        .output()
        .with_context(|| format!("spawning {} cat", bin))?;
    if !out.status.success() {
        bail!(
            "{} cat {} failed: {}",
            bin, cid,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

/// `ipfs cat <cid>` и записать в `out`.
#[allow(dead_code)] // reserved: pull --sif-out via ipfs (v2)
pub fn get_to(cid: &str, out: &Path) -> Result<()> {
    let bytes = cat(cid)?;
    if let Some(p) = out.parent() { std::fs::create_dir_all(p)?; }
    let mut f = std::fs::File::create(out)
        .with_context(|| format!("creating {}", out.display()))?;
    f.write_all(&bytes)?;
    f.sync_all()?;
    Ok(())
}

/// `ipfs pin add <cid>` — явный пин (когда файл уже в IPFS без --pin).
#[allow(dead_code)] // зарезервировано: repo pin <cid> (v2)
pub fn pin(cid: &str) -> Result<()> {
    let bin = ipfs_bin();
    if !which_ipfs() {
        bail!("`{}` not found in PATH", bin);
    }
    let st = Command::new(&bin)
        .args(["pin", "add", cid])
        .status()
        .with_context(|| format!("spawning {} pin add", bin))?;
    if !st.success() {
        bail!("{} pin add {} failed", bin, cid);
    }
    Ok(())
}

/// Проверить, что CID разумной формы. Не валидируем алгоритм — только
/// базовый sanity: непустой, без пробелов/управляющих символов.
pub fn validate_cid(cid: &str) -> Result<()> {
    if cid.is_empty() {
        return Err(anyhow!("empty CID"));
    }
    if cid.chars().any(|c| c.is_whitespace() || c.is_control()) {
        bail!("invalid CID: contains whitespace/control chars");
    }
    Ok(())
}
