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
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(&bin).is_file()))
}

/// `ipfs add --pin=<pin> --quieter <path>` → CID (trim).
pub fn add_file(path: &Path, pin: bool) -> Result<String> {
    let bin = ipfs_bin();
    if !which_ipfs() {
        bail!(
            "`{}` not found in PATH; required for IPFS ops (set APPSPEC_IPFS to override)",
            bin
        );
    }
    let out = Command::new(&bin)
        .args([
            "add",
            &format!("--pin={}", pin),
            "--quieter",
            "--raw-leaves",
            "--cid-version=1",
            "--chunker=size-1073741824",
        ])
        .arg(path)
        .output()
        .with_context(|| format!("spawning {} add", bin))?;
    if !out.status.success() {
        bail!(
            "{} add failed for {}: {}",
            bin,
            path.display(),
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
    // Must be raw CID (bafkrei...) — otherwise file was chunked into dag-pb
    // (e.g. >1 GB) and sha256-derived CID would not match.
    if !cid.starts_with("bafkrei") {
        bail!(
            "ipfs add returned non-raw CID `{}` for {}; file is likely >1 GB              and would be chunked into dag-pb. Use a smaller SIF or a larger              --chunker in a fork.",
            cid,
            path.display()
        );
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
            bin,
            cid,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

/// `ipfs cat <cid>` и записать в `out`.
#[allow(dead_code)] // reserved: pull --sif-out via ipfs (v2)
pub fn get_to(cid: &str, out: &Path) -> Result<()> {
    let bytes = cat(cid)?;
    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p)?;
    }
    let mut f =
        std::fs::File::create(out).with_context(|| format!("creating {}", out.display()))?;
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

// ===========================================================================
// IPNS: publish / resolve
// ===========================================================================
//
// IPNS-ключ живёт в keystore локального `ipfs` daemon. Создаётся через
// `ipfs key gen <name>` или `ipfs key import <name> <key-file>`. Клиент
// передаёт имя ключа (`[ipfs].ipns_key` в server config), а не сам ключ.
//
// `ipfs name publish --key=<name> /ipfs/<cid>` публикует маппинг
//   <ipns-name> -> /ipfs/<cid>
// в DHT. Латентность — секунды на локальном узле, 1–3 минуты на первом
// резолве с другой машины (DHT walk), потом кеш.

/// `ipfs name publish --key=<key> /ipfs/<cid>` → IPNS name (`k51…` или
/// `12D3Koo…` для RSA-ключей).
///
/// Парсит stdout `Published to <name>: /ipfs/<cid>`.
pub fn name_publish(key: &str, cid: &str) -> Result<String> {
    let bin = ipfs_bin();
    if !which_ipfs() {
        bail!(
            "`{}` not found in PATH; required for `repo publish --ipns` \
             (set APPSPEC_IPFS to override)",
            bin
        );
    }
    let path = format!("/ipfs/{}", cid);
    let out = Command::new(&bin)
        .args(["name", "publish", "--key", key, &path])
        .output()
        .with_context(|| format!("spawning {} name publish", bin))?;
    if !out.status.success() {
        bail!(
            "{} name publish failed for key `{}`: {}",
            bin,
            key,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let stdout = String::from_utf8(out.stdout).context("ipfs name publish stdout is not UTF-8")?;
    let tokens: Vec<&str> = stdout.split_whitespace().collect();
    let name = tokens
        .iter()
        .position(|t| *t == "Published")
        .and_then(|i| tokens.get(i + 2))
        .map(|s| s.trim_end_matches(':').to_string())
        .ok_or_else(|| {
            anyhow!(
                "unexpected `ipfs name publish` output (want `Published to <name>: …`): {}",
                stdout.trim()
            )
        })?;
    if name.is_empty() {
        bail!("`ipfs name publish` returned empty name");
    }
    Ok(name)
}

/// `ipfs name resolve <name>` → index CID.
///
/// stdout: `/ipfs/<cid>`. Если `timeout` есть в PATH — обёртка с
/// таймаутом (`timeout <secs> ipfs name resolve <name>`), иначе
/// запускаем без таймаута, полагаясь на daemon.
pub fn name_resolve(name: &str, timeout_secs: u64) -> Result<String> {
    let bin = ipfs_bin();
    if !which_ipfs() {
        bail!("`{}` not found in PATH; required for IPNS sources", bin);
    }
    let out = if which_tool("timeout") {
        Command::new("timeout")
            .arg(timeout_secs.to_string())
            .arg(&bin)
            .args(["name", "resolve", name])
            .output()
            .with_context(|| format!("spawning timeout {} name resolve", bin))?
    } else {
        Command::new(&bin)
            .args(["name", "resolve", name])
            .output()
            .with_context(|| format!("spawning {} name resolve", bin))?
    };
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let hint = if out.status.code() == Some(124) {
            format!(" (timeout after {}s)", timeout_secs)
        } else {
            String::new()
        };
        bail!("ipfs name resolve `{}` failed{}: {}", name, hint, err);
    }
    let stdout = String::from_utf8(out.stdout).context("ipfs name resolve stdout is not UTF-8")?;
    let cid = stdout
        .trim()
        .strip_prefix("/ipfs/")
        .ok_or_else(|| {
            anyhow!(
                "unexpected `ipfs name resolve` output (want `/ipfs/<cid>`): {}",
                stdout.trim()
            )
        })?
        .to_string();
    validate_cid(&cid)?;
    Ok(cid)
}

fn which_tool(name: &str) -> bool {
    if name.contains('/') {
        return Path::new(name).is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
}
