use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

pub struct SifLabels {
    pub raw: BTreeMap<String, String>,
}

impl SifLabels {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.raw.get(k).map(String::as_str)
    }
    pub fn require(&self, k: &str) -> Result<&str> {
        self.get(k).ok_or_else(|| {
            anyhow!(
                "SIF is missing required label `{}`; \
             rebuild with appspec 0.3.4+ (labels are injected at build time)",
                k
            )
        })
    }
}

/// Бинарь apptainer: `$APPSPEC_APPTAINER` (для тестов) или `apptainer`.
fn apptainer_bin() -> String {
    std::env::var("APPSPEC_APPTAINER").unwrap_or_else(|_| "apptainer".to_string())
}

/// ETXTBSY (os error 26) — «Text file busy». На некоторых ФС/в тестах
/// `apptainer` (или мок-скрипт) может быть ещё открыт на запись в момент
/// exec. Это транзиентная ошибка: несколько коротких повторов безопасны.
fn run_inspect(bin: &str, sif: &Path) -> std::io::Result<std::process::Output> {
    let mut last: Option<std::io::Error> = None;
    for attempt in 0..8 {
        match Command::new(bin)
            .args(["inspect", "--json", "--labels"])
            .arg(sif)
            .output()
        {
            Ok(out) => return Ok(out),
            Err(e) if e.raw_os_error() == Some(26) => {
                last = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(10 * (attempt + 1)));
            }
            Err(e) => return Err(e),
        }
    }
    Err(last.expect("at least one ETXTBSY attempt"))
}

pub fn inspect(sif: &Path) -> Result<SifLabels> {
    if !sif.is_file() {
        bail!("SIF not found: {}", sif.display());
    }
    let bin = apptainer_bin();
    if !which(&bin) {
        bail!(
            "`{}` not found in PATH; required for `repo add` (set APPSPEC_APPTAINER to override)",
            bin
        );
    }
    let out = run_inspect(&bin, sif)
        .with_context(|| format!("running {} inspect on {}", bin, sif.display()))?;
    if !out.status.success() {
        bail!(
            "{} inspect failed for {}: {}",
            bin,
            sif.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let v: Value = serde_json::from_slice(&out.stdout)
        .with_context(|| format!("parsing inspect JSON for {}", sif.display()))?;

    let labels_val = v
        .pointer("/data/attributes/labels")
        .or_else(|| v.get("labels"))
        .ok_or_else(|| {
            anyhow!(
                "unexpected `{} inspect --json --labels` output shape; \
             got top-level keys: {:?}",
                bin,
                v.as_object().map(|o| o.keys().collect::<Vec<_>>())
            )
        })?;

    let mut raw = BTreeMap::new();
    if let Some(obj) = labels_val.as_object() {
        for (k, v) in obj {
            if let Some(s) = v.as_str() {
                raw.insert(k.clone(), s.to_string());
            }
        }
    }
    Ok(SifLabels { raw })
}

fn which(name: &str) -> bool {
    if name.contains('/') {
        return Path::new(name).is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
}
