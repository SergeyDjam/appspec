//! Package reference resolution for `appspec run <package>`.
//!
//! `index.json` is the source of truth: given a package name and optional
//! version spec, resolve to (version_key, IndexVersion) using max-semver
//! ordering. Pure functions, unit-testable.

use anyhow::{anyhow, bail, Result};

use super::meta::{IndexV1, IndexVersion};

/// Parse `<name>[:<version>[-<release>]]` into `(name, ver_spec)`.
/// Rejects file-like inputs (containing `/`, starting with `.`/`~`, or
/// ending with `.appspec`).
pub fn parse_package_ref(s: &str) -> Result<(String, Option<String>)> {
    if s.is_empty() {
        bail!("empty package reference");
    }
    if s.contains('/') || s.starts_with('.') || s.starts_with('~') || s.ends_with(".appspec") {
        bail!("not a package name: `{}`", s);
    }
    if let Some((name, ver)) = s.split_once(':') {
        if name.is_empty() || ver.is_empty() {
            bail!("invalid package reference `{}`", s);
        }
        Ok((name.to_ascii_lowercase(), Some(ver.to_string())))
    } else {
        Ok((s.to_ascii_lowercase(), None))
    }
}

/// Pick a version from the index.
///
/// `ver_spec`:
/// - `None` / `Some("latest")` — max `sort_key` (max semver, max release);
/// - `Some("2.2.0-1")` — exact key match;
/// - `Some("2.2.0")` — version-only, max release (max `sort_key` filtered
///   by `version == "2.2.0"`).
pub fn pick_version<'a>(
    index: &'a IndexV1,
    name: &str,
    ver_spec: Option<&str>,
) -> Result<(&'a String, &'a IndexVersion)> {
    let pkg = index
        .packages
        .get(name)
        .ok_or_else(|| anyhow!("package `{}` not in index", name))?;

    match ver_spec {
        None | Some("latest") => pkg
            .versions
            .iter()
            .max_by(|a, b| a.1.sort_key().cmp(&b.1.sort_key()))
            .ok_or_else(|| anyhow!("package `{}` has no versions", name)),
        Some(spec) => {
            if let Some((k, v)) = pkg.versions.get_key_value(spec) {
                return Ok((k, v));
            }
            pkg.versions
                .iter()
                .filter(|(_, v)| v.version == spec)
                .max_by(|a, b| a.1.sort_key().cmp(&b.1.sort_key()))
                .ok_or_else(|| {
                    anyhow!(
                        "{}:{} not in index (tried exact key `{}` and version-only)",
                        name,
                        spec,
                        spec
                    )
                })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn iv(version: &str, release: &str) -> IndexVersion {
        IndexVersion {
            epoch: 0,
            version: version.into(),
            release: release.into(),
            arch: "amd64".into(),
            sif_sha256: "00".repeat(32),
            sif_size: 1,
            spec_sha256: "00".repeat(32),
            spec_size: 1,
            built_at: "2026-11-14T00:00:00Z".into(),
            generator: "test".into(),
            base_digest: None,
            tags: vec![],
            icon_sha256: None,
            icon_mimetype: None,
            icon_size: None,
            icon_filename: None,
            requires: vec![],
            provides: vec![],
            ipfs_cid: None,
        }
    }

    fn mk_index(versions: &[(&str, &str)]) -> IndexV1 {
        let mut vs = BTreeMap::new();
        for (v, r) in versions {
            vs.insert(format!("{}-{}", v, r), iv(v, r));
        }
        let mut packages = BTreeMap::new();
        packages.insert(
            "foo".into(),
            super::super::meta::IndexPackage {
                summary: None,
                license: None,
                vendor: None,
                url: None,
                versions: vs,
            },
        );
        IndexV1 {
            schema: 1,
            repo: "test".into(),
            updated: "2026-11-14T00:00:00Z".into(),
            generator: "test".into(),
            signature: String::new(),
            packages,
        }
    }

    #[test]
    fn parse_simple() {
        assert_eq!(parse_package_ref("foo").unwrap(), ("foo".into(), None));
    }
    #[test]
    fn parse_lowercases() {
        assert_eq!(parse_package_ref("Foo").unwrap(), ("foo".into(), None));
    }
    #[test]
    fn parse_with_version() {
        assert_eq!(
            parse_package_ref("foo:2.2.0").unwrap(),
            ("foo".into(), Some("2.2.0".into()))
        );
    }
    #[test]
    fn parse_with_version_release() {
        assert_eq!(
            parse_package_ref("foo:2.2.0-1").unwrap(),
            ("foo".into(), Some("2.2.0-1".into()))
        );
    }
    #[test]
    fn parse_rejects_paths() {
        assert!(parse_package_ref("./foo.appspec").is_err());
        assert!(parse_package_ref("/abs/foo").is_err());
        assert!(parse_package_ref("~user/foo").is_err());
        assert!(parse_package_ref("foo/bar").is_err());
    }

    #[test]
    fn pick_latest() {
        let idx = mk_index(&[("1.0.0", "1"), ("2.2.0", "1"), ("2.2.6", "1")]);
        let (k, _) = pick_version(&idx, "foo", None).unwrap();
        assert_eq!(k, "2.2.6-1");
    }
    #[test]
    fn pick_version_max_release() {
        let idx = mk_index(&[("2.2.0", "1"), ("2.2.0", "3"), ("2.2.0", "2")]);
        let (k, _) = pick_version(&idx, "foo", Some("2.2.0")).unwrap();
        assert_eq!(k, "2.2.0-3");
    }
    #[test]
    fn pick_version_exact() {
        let idx = mk_index(&[("2.2.0", "1"), ("2.2.0", "3")]);
        let (k, _) = pick_version(&idx, "foo", Some("2.2.0-1")).unwrap();
        assert_eq!(k, "2.2.0-1");
    }
    #[test]
    fn pick_missing() {
        let idx = mk_index(&[("1.0.0", "1")]);
        assert!(pick_version(&idx, "foo", Some("2.0.0")).is_err());
        assert!(pick_version(&idx, "bar", None).is_err());
    }
}
