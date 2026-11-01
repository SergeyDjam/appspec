use anyhow::{bail, Context, Result};
use std::path::Path;

use super::http::http_request;
use super::util::sha256_file;

#[derive(Debug)]
pub struct PushOutcome {
    pub sha256: String,
    pub package: String,
    pub version: String,
    pub already_present: bool,
}

pub fn push_one(sif: &Path, server_url: &str, token: &str) -> Result<PushOutcome> {
    let body = std::fs::read(sif)
        .with_context(|| format!("reading {}", sif.display()))?;
    let sha = sha256_file(sif)?;

    let url = format!("{}/api/v1/push", server_url.trim_end_matches('/'));
    let auth = format!("Bearer {}", token);
    let (status, _h, resp_body) = http_request(
        &url,
        "POST",
        &[
            ("Authorization", auth.as_str()),
            ("Content-Type", "application/octet-stream"),
            ("X-Sif-Sha256", sha.as_str()),
        ],
        Some(&body),
    )?;

    let v: serde_json::Value = serde_json::from_slice(&resp_body).unwrap_or_default();
    match status {
        200 | 201 => Ok(PushOutcome {
            sha256: v["sha256"].as_str().unwrap_or(&sha).to_string(),
            package: v["package"].as_str().unwrap_or("?").to_string(),
            version: v["version"].as_str().unwrap_or("?").to_string(),
            already_present: v["already_present"].as_bool().unwrap_or(false),
        }),
        401 => bail!("server rejected token (401)"),
        409 => bail!("conflict (409): {}", v["reason"].as_str().unwrap_or("?")),
        413 => bail!("payload too large (413)"),
        other => bail!(
            "push failed: status={} body={}",
            other, String::from_utf8_lossy(&resp_body)),
    }
}
