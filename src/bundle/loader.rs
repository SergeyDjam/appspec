use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

use super::model::*;

pub fn load_bundle(path: &Path) -> Result<BundleFile> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("bundle spec не читается: {}", path.display()))?;
    let file: BundleFile = toml::from_str(&text)
        .with_context(|| format!("bundle spec не парсится: {}", path.display()))?;

    if file.bundle.name.is_empty() {
        bail!("[bundle].name обязателен");
    }
    file.bundle.base.source.validate("[bundle.base].source")?;

    let mut seen = std::collections::HashSet::new();
    for c in &file.components {
        if c.name.is_empty() {
            bail!("[[components]].name обязателен");
        }
        if !seen.insert(c.name.clone()) {
            bail!("[[components]]: дубликат имени `{}`", c.name);
        }
        c.source.validate(&format!("component `{}`", c.name))?;

        for f in &c.files {
            f.validate(&format!("component `{}`", c.name))?;
        }

        match c.mode() {
            ComponentMode::Chroot => {
                if let Some(b) = &c.bridge {
                    match b.as_str() {
                        "chroot" | "apptainer" => {}
                        other => bail!(
                            "component `{}`: неизвестная стратегия bridge = `{}`",
                            c.name,
                            other
                        ),
                    }
                }
                if c.socket_dir().is_some()
                    || c.state_dir().is_some()
                    || c.daemon().is_some()
                    || c.daemon_socket().is_some()
                {
                    bail!(
                        "component `{}`: socket_dir/state_dir/daemon применимы только \
                           при mode = \"instance\" или \"both\"",
                        c.name
                    );
                }
            }
            ComponentMode::Instance | ComponentMode::Both => {
                if c.bridge.is_some() {
                    bail!(
                        "component `{}`: bridge несовместим с mode = instance/both",
                        c.name
                    );
                }
                if c.daemon().is_none() {
                    bail!(
                        "component `{}`: mode = instance/both требует `daemon`",
                        c.name
                    );
                }
                if c.daemon_socket().is_none() {
                    bail!(
                        "component `{}`: mode = instance/both требует `daemon_socket`",
                        c.name
                    );
                }
                if c.socket_dir().is_none() {
                    bail!(
                        "component `{}`: mode = instance/both требует `socket_dir`",
                        c.name
                    );
                }
            }
        }

        // daemon без daemon_socket и наоборот — ошибка в любом режиме.
        match (&c.daemon, &c.daemon_socket) {
            (Some(d), Some(_)) => {
                if d.trim().is_empty() {
                    bail!("component `{}`: `daemon` не может быть пустым", c.name);
                }
            }
            (Some(_), None) => bail!("component `{}`: `daemon` без `daemon_socket`", c.name),
            (None, Some(_)) => bail!("component `{}`: `daemon_socket` без `daemon`", c.name),
            (None, None) => {}
        }
    }

    let mut seen_ext = std::collections::HashSet::new();
    for e in &file.extensions {
        e.validate(&format!("extension `{}`", e.name))?;
        if !seen_ext.insert(e.name.clone()) {
            bail!("[[extensions]]: дубликат имени `{}`", e.name);
        }
    }

    Ok(file)
}

pub fn resolve_source(
    src: &SourceSpec,
    cache_dir: &Path,
    base_dir: &Path,
    allow_build: bool,
    label: &str,
) -> Result<ResolvedSource> {
    src.validate(label)?;

    if let Some(s) = &src.sif {
        let p = resolve_relative(s, base_dir);
        if !p.is_file() {
            bail!("{label}: SIF не найден: {}", p.display());
        }
        return Ok(ResolvedSource {
            sif: p,
            setup: None,
            origin: format!("sif:{s}"),
        });
    }

    if let Some(s) = &src.spec {
        let sp = resolve_relative(s, base_dir);
        if !sp.is_file() {
            bail!("{label}: spec не найден: {}", sp.display());
        }
        let spec = crate::load_spec(&sp)?;
        let sif = sp
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(spec.sif_file());

        if !sif.is_file() {
            if !allow_build {
                bail!(
                    "{label}: SIF не собран: {}\n  appspec build {}",
                    sif.display(),
                    sp.display()
                );
            }
            build_spec_via_self(&sp)?;
        }
        if !sif.is_file() {
            bail!("{label}: после сборки SIF не найден: {}", sif.display());
        }
        return Ok(ResolvedSource {
            sif,
            setup: spec.runtime.setup.clone(),
            origin: format!("spec:{s}"),
        });
    }

    if let Some(c) = &src.cid {
        std::fs::create_dir_all(cache_dir)?;
        let cached = cache_dir.join(format!("{}.sif", sanitize_filename(c)));
        if cached.is_file() {
            return Ok(ResolvedSource {
                sif: cached,
                setup: None,
                origin: format!("cid:{c}"),
            });
        }
        if !allow_build {
            bail!("{label}: CID {c} не выкачан");
        }
        crate::log(format!("bundle: ipfs get {c} → {}", cached.display()));
        let status = Command::new("ipfs")
            .args(["get", c, "-o"])
            .arg(&cached)
            .status()
            .map_err(|e| anyhow!("не могу запустить `ipfs`: {e}"))?;
        if !status.success() || !cached.is_file() {
            bail!("{label}: `ipfs get {c}` не удался");
        }
        return Ok(ResolvedSource {
            sif: cached,
            setup: None,
            origin: format!("cid:{c}"),
        });
    }

    if let Some(m) = &src.manifest {
        let d = resolve_relative(m, base_dir);
        let mf = d.join("manifest.json");
        if !mf.is_file() {
            bail!("{label}: manifest.json не найден: {}", mf.display());
        }
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&mf)?)?;
        let sif = v
            .get("sif")
            .and_then(|x| x.as_str())
            .ok_or_else(|| anyhow!("{label}: manifest.json без `sif`"))?;
        let sif = PathBuf::from(sif);
        if !sif.is_file() {
            bail!("{label}: SIF из манифеста не найден");
        }
        return Ok(ResolvedSource {
            sif,
            setup: None,
            origin: format!("manifest:{m}"),
        });
    }

    unreachable!()
}

pub fn resolve_base(
    base: &BaseSection,
    cache_dir: &Path,
    base_dir: &Path,
    allow_build: bool,
) -> Result<ResolvedBase> {
    let r = resolve_source(&base.source, cache_dir, base_dir, allow_build, "base")?;
    let raw_setup = r.setup.as_deref().or(base.setup.as_deref()).unwrap_or("");
    let split = split_setup_at_final_exec(raw_setup);
    let entry = split.entry.or_else(|| base.entry.clone()).ok_or_else(|| {
        anyhow!("не удалось определить entry для base: задайте [bundle.base].entry = \"exec …\"")
    })?;
    Ok(ResolvedBase {
        sif: r.sif,
        pre: split.pre,
        entry,
        origin: r.origin,
    })
}

pub fn split_setup_at_final_exec(setup: &str) -> SplitSetup {
    let lines: Vec<&str> = setup.lines().collect();
    let mut exec_idx: Option<usize> = None;
    for (i, ln) in lines.iter().enumerate() {
        if ln.trim_start().starts_with("exec ") && !ln.starts_with(' ') && !ln.starts_with('\t') {
            exec_idx = Some(i);
        }
    }
    let Some(i) = exec_idx else {
        return SplitSetup {
            pre: setup.trim_end().to_string(),
            entry: None,
        };
    };
    let mut end = i + 1;
    while end < lines.len() {
        let prev = lines[end - 1].trim_end();
        if prev.ends_with('\\') {
            end += 1;
        } else {
            break;
        }
    }
    SplitSetup {
        pre: lines[..i].join("\n").trim_end().to_string(),
        entry: Some(lines[i..end].join("\n")),
    }
}

#[derive(Debug, Clone)]
pub struct SplitSetup {
    pub pre: String,
    pub entry: Option<String>,
}

pub fn resolve_file(
    fr: &FileRef,
    component: &str,
    base_dir: &Path,
    label: &str,
) -> Result<ResolvedFile> {
    fr.validate(label)?;
    let mode = fr.mode.unwrap_or(0o644);
    let (content, source) = if let Some(f) = &fr.from {
        let p = resolve_relative(f, base_dir);
        if !p.is_file() {
            bail!("{label}: файл не найден: {}", p.display());
        }
        (std::fs::read_to_string(&p)?, format!("file:{}", f))
    } else {
        (fr.content.clone().unwrap_or_default(), "inline".into())
    };
    Ok(ResolvedFile {
        to: fr.to.clone(),
        content,
        mode,
        source,
        scopes: fr.scopes(),
        component: component.to_string(),
    })
}

pub fn prepare_extension(
    ext: &Extension,
    cache_dir: &Path,
    base_dir: &Path,
    allow_fetch: bool,
) -> Result<PreparedExtension> {
    ext.validate(&format!("extension `{}`", ext.name))?;

    let ext_dir_root = cache_dir.join("extensions");
    std::fs::create_dir_all(&ext_dir_root)?;

    let (vsix_path, fp, source_label) = if let Some(f) = &ext.file {
        let p = resolve_relative(f, base_dir);
        if !p.is_file() {
            bail!("extension `{}`: файл не найден: {}", ext.name, p.display());
        }
        let meta = std::fs::metadata(&p)?;
        let fp = format!(
            "file:{}|{}|{}",
            p.display(),
            meta.len(),
            meta.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0)
        );
        (p, fp, format!("file:{}", f))
    } else if let Some(cid) = &ext.cid {
        let cached = cache_dir.join(format!("{}.vsix", sanitize_filename(cid)));
        if !cached.is_file() {
            if !allow_fetch {
                bail!("extension `{}`: CID не выкачан", ext.name);
            }
            crate::log(format!("bundle: ipfs get {} → {}", cid, cached.display()));
            let st = Command::new("ipfs")
                .args(["get", cid, "-o"])
                .arg(&cached)
                .status()
                .map_err(|e| anyhow!("не могу запустить `ipfs`: {e}"))?;
            if !st.success() || !cached.is_file() {
                bail!("extension `{}`: `ipfs get {}` не удался", ext.name, cid);
            }
        }
        (cached, format!("cid:{}", cid), format!("cid:{}", cid))
    } else if let Some(url) = &ext.url {
        let cached = cache_dir.join(format!("{}.vsix", sanitize_filename(&ext.name)));
        if !cached.is_file() {
            if !allow_fetch {
                bail!("extension `{}`: url не скачан", ext.name);
            }
            crate::log(format!("bundle: download {} → {}", url, cached.display()));
            download_url(url, &cached)?;
        }
        (cached, format!("url:{}", url), format!("url:{}", url))
    } else {
        let (dl_url, ver) = openvsx_lookup(&ext.name, allow_fetch)?;
        let cached = cache_dir.join(format!("{}-{}.vsix", ext.name, ver));
        if !cached.is_file() {
            if !allow_fetch {
                bail!("extension `{}`: v{} не скачан", ext.name, ver);
            }
            crate::log(format!(
                "bundle: download {} v{} → {}",
                ext.name,
                ver,
                cached.display()
            ));
            download_url(&dl_url, &cached)?;
        }
        (
            cached,
            format!("openvsx:{}@{}", ext.name, ver),
            format!("openvsx:{}", ext.name),
        )
    };

    if !which("unzip") {
        bail!("не найден `unzip` в PATH");
    }
    let tmp_root = cache_dir.join(format!(".tmp-extract-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp_root);
    std::fs::create_dir_all(&tmp_root)?;

    let status = Command::new("unzip")
        .arg("-q")
        .arg(&vsix_path)
        .arg("-d")
        .arg(&tmp_root)
        .status()
        .with_context(|| format!("spawn unzip {}", vsix_path.display()))?;
    if !status.success() {
        let _ = std::fs::remove_dir_all(&tmp_root);
        bail!("unzip {} завершился с ошибкой", vsix_path.display());
    }

    let pkg_path = tmp_root.join("extension/package.json");
    if !pkg_path.is_file() {
        let _ = std::fs::remove_dir_all(&tmp_root);
        bail!("{}: нет extension/package.json", vsix_path.display());
    }
    let pkg: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&pkg_path)?)?;
    let publisher = pkg
        .get("publisher")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("{}: нет publisher", vsix_path.display()))?;
    let name_pkg = pkg
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("{}: нет name", vsix_path.display()))?;
    let version = pkg
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("{}: нет version", vsix_path.display()))?;

    let full_id = format!("{}.{}", publisher, name_pkg);
    let target = ext_dir_root.join(format!("{}-{}", full_id, version));

    let dependencies: Vec<String> = pkg
        .get("extensionDependencies")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let extension_pack: Vec<String> = pkg
        .get("extensionPack")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let engines_vscode = pkg
        .pointer("/engines/vscode")
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    let fp_file = target.join(".appspec-fingerprint");
    if target.is_dir()
        && fp_file.is_file()
        && std::fs::read_to_string(&fp_file).ok().as_deref() == Some(fp.as_str())
    {
        let _ = std::fs::remove_dir_all(&tmp_root);
        return Ok(PreparedExtension {
            id: full_id,
            version: version.to_string(),
            dir: target,
            source: source_label,
            dependencies,
            extension_pack,
            engines_vscode,
            requires: ext.requires.clone(),
        });
    }

    let src = tmp_root.join("extension");
    if !src.is_dir() {
        let _ = std::fs::remove_dir_all(&tmp_root);
        bail!("{}: нет каталога extension/", vsix_path.display());
    }
    if target.exists() {
        let _ = std::fs::remove_dir_all(&target);
    }
    if std::fs::rename(&src, &target).is_err() {
        copy_dir_recursive(&src, &target)?;
    }
    std::fs::write(target.join(".appspec-fingerprint"), &fp)?;
    let _ = std::fs::remove_dir_all(&tmp_root);

    crate::log(format!(
        "bundle: extension prepared: {} v{} → {}",
        full_id,
        version,
        target.display()
    ));
    Ok(PreparedExtension {
        id: full_id,
        version: version.to_string(),
        dir: target,
        source: source_label,
        dependencies,
        extension_pack,
        engines_vscode,
        requires: ext.requires.clone(),
    })
}

pub fn reset_extensions_index(cache_dir: &Path) {
    let idx = cache_dir.join("extensions/extensions.json");
    if idx.is_file() {
        let _ = std::fs::remove_file(&idx);
    }
}

fn openvsx_lookup(ext_id: &str, allow_fetch: bool) -> Result<(String, String)> {
    let (ns, name) = ext_id
        .split_once('.')
        .ok_or_else(|| anyhow!("ID должен быть publisher.name, получено `{ext_id}`"))?;
    if !allow_fetch {
        bail!("online extension `{ext_id}`: нужен bundle build без --no-build");
    }
    let api = format!("https://open-vsx.org/api/{}/{}/latest", ns, name);
    let body = http_get(&api).with_context(|| format!("Open VSX API: {api}"))?;
    let v: serde_json::Value = serde_json::from_str(&body)?;
    let dl = v
        .pointer("/files/download")
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow!("{}: нет files.download", ext_id))?
        .to_string();
    let ver = v
        .get("version")
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow!("{}: нет version", ext_id))?
        .to_string();
    Ok((dl, ver))
}

fn http_get(url: &str) -> Result<String> {
    if which("curl") {
        let out = Command::new("curl")
            .args(["-fsSL", "--max-time", "60", url])
            .output()?;
        if !out.status.success() {
            bail!("curl {} вернул {}", url, out.status);
        }
        return Ok(String::from_utf8(out.stdout)?);
    }
    if which("wget") {
        let out = Command::new("wget").args(["-q", "-O", "-", url]).output()?;
        if !out.status.success() {
            bail!("wget {} вернул {}", url, out.status);
        }
        return Ok(String::from_utf8(out.stdout)?);
    }
    bail!("ни curl, ни wget не найдены")
}

fn download_url(url: &str, dst: &Path) -> Result<()> {
    if let Some(p) = dst.parent() {
        std::fs::create_dir_all(p)?;
    }
    let tools: &[(&str, &[&str])] = &[
        ("curl", &["-fsSL", "--max-time", "300", "-o"]),
        ("wget", &["-q", "-O"]),
    ];
    for (tool, args) in tools {
        if !which(tool) {
            continue;
        }
        let status = Command::new(tool).args(*args).arg(dst).arg(url).status()?;
        if !status.success() || !dst.is_file() {
            let _ = std::fs::remove_file(dst);
            bail!("{tool} {url} не удался");
        }
        return Ok(());
    }
    bail!("ни curl, ни wget не найдены")
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn build_spec_via_self(spec_path: &Path) -> Result<()> {
    let self_exe = std::env::current_exe().context("не могу определить путь к бинарю")?;
    crate::log(format!(
        "bundle: building {} (SIF отсутствует)",
        spec_path.display()
    ));
    let status = Command::new(&self_exe)
        .arg("build")
        .arg(spec_path)
        .status()?;
    if !status.success() {
        bail!(
            "`appspec build {}` завершился с ошибкой",
            spec_path.display()
        );
    }
    Ok(())
}

pub fn resolve_relative(s: &str, base_dir: &Path) -> PathBuf {
    let expanded = expand_env(&tilde(s));
    let p = PathBuf::from(&expanded);
    if p.is_absolute() {
        return lexical_normalize(&p);
    }
    lexical_normalize(&base_dir.join(&p))
}

fn lexical_normalize(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

pub fn expand_env(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('{') => {
                chars.next();
                let mut name = String::new();
                while let Some(&c) = chars.peek() {
                    if c == '}' {
                        chars.next();
                        break;
                    }
                    name.push(c);
                    chars.next();
                }
                match std::env::var(&name) {
                    Ok(v) => out.push_str(&v),
                    Err(_) => {
                        out.push_str("${");
                        out.push_str(&name);
                        out.push('}');
                    }
                }
            }
            Some(c0) if c0.is_ascii_alphabetic() || c0 == '_' => {
                let mut name = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_alphanumeric() || c == '_' {
                        name.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                match std::env::var(&name) {
                    Ok(v) => out.push_str(&v),
                    Err(_) => {
                        out.push('$');
                        out.push_str(&name);
                    }
                }
            }
            _ => out.push('$'),
        }
    }
    out
}

fn tilde(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{}/{}", home, rest);
        }
    }
    if s == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return home;
        }
    }
    s.to_string()
}

pub fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn bundle_cache_dir(bundle_name: &str) -> PathBuf {
    if let Ok(v) = std::env::var("APPSPEC_BUNDLE_CACHE") {
        if !v.is_empty() {
            return PathBuf::from(v).join(bundle_name);
        }
    }
    if let Ok(v) = std::env::var("XDG_CACHE_HOME") {
        if !v.is_empty() {
            return PathBuf::from(v).join("appspec/bundles").join(bundle_name);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".cache/appspec/bundles")
            .join(bundle_name);
    }
    PathBuf::from("/tmp/appspec-bundles").join(bundle_name)
}

fn which(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|p| p.join(name).is_file()))
}
