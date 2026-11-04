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
    if let Some(r) = &src.repo {
        return resolve_repo_source(r, label);
    }
    unreachable!()
}

fn resolve_repo_source(pv: &str, label: &str) -> Result<ResolvedSource> {
    use crate::repo::{default_root, layout::RepoRoot};
    let (name, ver) = pv.split_once(':').ok_or_else(|| {
        anyhow!(
            "{label}: repo source must be `NAME:VERSION-RELEASE`, got `{}`",
            pv
        )
    })?;
    let name = name.to_ascii_lowercase();
    let root = RepoRoot::open(&default_root())
        .map_err(|e| anyhow!("{label}: repo not available: {e:#}"))?;
    let index_text = std::fs::read_to_string(root.index_path())
        .with_context(|| format!("reading {}", root.index_path().display()))?;
    let index: crate::repo::meta::IndexV1 =
        serde_json::from_str(&index_text).with_context(|| "parsing repo index.json".to_string())?;
    let pkg = index
        .packages
        .get(&name)
        .ok_or_else(|| anyhow!("{label}: package `{name}` not found in repo index"))?;
    let v = pkg
        .versions
        .get(ver)
        .ok_or_else(|| anyhow!("{label}: {name}:{ver} not found in repo index"))?;
    let sif = crate::repo::util::blob_path(&root.path, &v.sif_sha256);
    if !sif.is_file() {
        bail!(
            "{label}: SIF blob missing: {} (run `appspec repo add` или `appspec repo pull`)",
            sif.display()
        );
    }
    let spec_path = root
        .packages_dir()
        .join(&name)
        .join(ver)
        .join("spec.appspec");
    let setup = if spec_path.is_file() {
        crate::load_spec(&spec_path)
            .ok()
            .and_then(|s| s.runtime.setup)
    } else {
        None
    };
    Ok(ResolvedSource {
        sif,
        setup,
        origin: format!("repo:{pv}"),
    })
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
    let kind = ext.kind();
    let (src_path, fp, source_label) =
        resolve_extension_source(ext, &ext_dir_root, base_dir, allow_fetch)?;
    match kind {
        ExtensionKind::File => prepare_as_file(ext, &src_path, &fp, &ext_dir_root, source_label),
        ExtensionKind::Vsix => {
            prepare_as_archive(ext, &src_path, &fp, &ext_dir_root, source_label, true)
        }
        ExtensionKind::Archive => {
            prepare_as_archive(ext, &src_path, &fp, &ext_dir_root, source_label, false)
        }
    }
}

fn resolve_extension_source(
    ext: &Extension,
    ext_dir_root: &Path,
    base_dir: &Path,
    allow_fetch: bool,
) -> Result<(PathBuf, String, String)> {
    if let Some(f) = &ext.file {
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
        return Ok((p, fp, format!("file:{}", f)));
    }
    if let Some(cid) = &ext.cid {
        let cached = ext_dir_root.join(format!("{}.bin", sanitize_filename(cid)));
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
        return Ok((cached, format!("cid:{}", cid), format!("cid:{}", cid)));
    }
    if let Some(url) = &ext.url {
        let base = url.rsplit('/').next().unwrap_or("extension.bin");
        let cached = ext_dir_root.join(sanitize_filename(base));
        if !cached.is_file() {
            if !allow_fetch {
                bail!("extension `{}`: url не скачан", ext.name);
            }
            crate::log(format!("bundle: download {} → {}", url, cached.display()));
            download_url(url, &cached)?;
        }
        return Ok((cached, format!("url:{}", url), format!("url:{}", url)));
    }
    let (dl_url, ver) = openvsx_lookup(&ext.name, allow_fetch)?;
    let cached = ext_dir_root.join(format!("{}-{}.vsix", ext.name, ver));
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
    Ok((
        cached,
        format!("openvsx:{}@{}", ext.name, ver),
        format!("openvsx:{}", ext.name),
    ))
}

/// `type = "file"`: скопировать файл в `<extensions>/`.
/// `<mount_dir>/<basename>` — то, что получит install-команда в `{file}`.
///
/// Если `src` уже лежит в `ext_dir_root` (URL/CID кэшируются туда же),
/// копировать «в самого себя» нельзя — иначе `remove_file(target)`
/// удалит `src`, и `copy(src, target)` упадёт с ENOENT.
fn prepare_as_file(
    ext: &Extension,
    src: &Path,
    fp: &str,
    ext_dir_root: &Path,
    source_label: String,
) -> Result<PreparedExtension> {
    let basename = src
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| sanitize_filename(&ext.name));
    let target = ext_dir_root.join(&basename);
    let fp_file = ext_dir_root.join(format!(".{}.fingerprint", basename));

    if target.is_file()
        && fp_file.is_file()
        && std::fs::read_to_string(&fp_file).ok().as_deref() == Some(fp)
    {
        return Ok(PreparedExtension {
            id: ext.name.clone(),
            version: "1".into(),
            path: target,
            kind: ExtensionKind::File,
            source: source_label,
            dependencies: Vec::new(),
            extension_pack: Vec::new(),
            engines_vscode: None,
            requires: ext.requires.clone(),
        });
    }

    if src != target {
        if target.is_file() {
            let _ = std::fs::remove_file(&target);
        }
        std::fs::copy(src, &target)
            .with_context(|| format!("copy {} → {}", src.display(), target.display()))?;
    }
    std::fs::write(&fp_file, fp)?;

    crate::log(format!(
        "bundle: extension prepared (file): {} → {}",
        ext.name,
        target.display()
    ));

    Ok(PreparedExtension {
        id: ext.name.clone(),
        version: "1".into(),
        path: target,
        kind: ExtensionKind::File,
        source: source_label,
        dependencies: Vec::new(),
        extension_pack: Vec::new(),
        engines_vscode: None,
        requires: ext.requires.clone(),
    })
}

/// `type = "vsix"` — распаковать `.vsix` в каталог
/// `<publisher>.<name>-<version>/` в `$BUNDLE_DIR/extensions/`. Этот каталог
/// монтируется в целевой `--extensions-dir` VSCodium — VSCodium подхватывает
/// расширения из подкаталогов без запуска `codium --install-extension`
/// (без X-сервера, без GUI).
///
/// `type = "archive"` — распаковать в каталог `<name>-archive/` в `$BUNDLE_DIR/extensions/`.
fn prepare_as_archive(
    ext: &Extension,
    src: &Path,
    fp: &str,
    ext_dir_root: &Path,
    source_label: String,
    parse_vsix: bool,
) -> Result<PreparedExtension> {
    let src_lower = src.to_string_lossy().to_ascii_lowercase();
    let is_zip = src_lower.ends_with(".zip") || src_lower.ends_with(".vsix");

    let tmp_root = ext_dir_root.join(format!(".tmp-extract-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp_root);
    std::fs::create_dir_all(&tmp_root)?;

    let unpack_ok = if is_zip {
        if !which("unzip") {
            let _ = std::fs::remove_dir_all(&tmp_root);
            bail!("не найден `unzip` в PATH (нужен для .vsix / .zip)");
        }
        Command::new("unzip")
            .arg("-q")
            .arg(src)
            .arg("-d")
            .arg(&tmp_root)
            .status()
            .with_context(|| format!("spawn unzip {}", src.display()))?
            .success()
    } else {
        if !which("tar") {
            let _ = std::fs::remove_dir_all(&tmp_root);
            bail!("не найден `tar` в PATH (нужен для .tar.gz / .tgz)");
        }
        Command::new("tar")
            .arg("-xf")
            .arg(src)
            .arg("-C")
            .arg(&tmp_root)
            .status()
            .with_context(|| format!("spawn tar -xf {}", src.display()))?
            .success()
    };
    if !unpack_ok {
        let _ = std::fs::remove_dir_all(&tmp_root);
        bail!("распаковка {} завершилась с ошибкой", src.display());
    }

    let (full_id, version, dependencies, extension_pack, engines_vscode) = if parse_vsix {
        let pkg_path = tmp_root.join("extension/package.json");
        if !pkg_path.is_file() {
            let _ = std::fs::remove_dir_all(&tmp_root);
            bail!(
                "{}: нет extension/package.json — не похоже на .vsix \
                 (для другого формата используйте `type = \"file\"` или `type = \"archive\"`)",
                src.display()
            );
        }
        let pkg: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&pkg_path)?)?;
        let publisher = pkg
            .get("publisher")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("{}: нет publisher", src.display()))?;
        let name_pkg = pkg
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("{}: нет name", src.display()))?;
        let version = pkg
            .get("version")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("{}: нет version", src.display()))?;
        let full_id = format!("{}.{}", publisher, name_pkg);
        let deps: Vec<String> = pkg
            .get("extensionDependencies")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let pack: Vec<String> = pkg
            .get("extensionPack")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let engines = pkg
            .pointer("/engines/vscode")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        (full_id, version.to_string(), deps, pack, engines)
    } else {
        (ext.name.clone(), "1".into(), Vec::new(), Vec::new(), None)
    };

    // Для vsix target — КАТАЛОГ `<id>-<version>/`, чтобы VSCodium подхватил
    // его из `--extensions-dir`. Для archive — `<name>-archive/`.
    let target_name = if parse_vsix {
        format!("{}-{}", full_id, version)
    } else {
        format!("{}-archive", sanitize_filename(&ext.name))
    };
    let target = ext_dir_root.join(&target_name);
    let fp_file = ext_dir_root.join(format!(".{}.fingerprint", target_name));

    if target.is_dir()
        && fp_file.is_file()
        && std::fs::read_to_string(&fp_file).ok().as_deref() == Some(fp)
    {
        let _ = std::fs::remove_dir_all(&tmp_root);
        return Ok(PreparedExtension {
            id: full_id,
            version,
            path: target,
            kind: if parse_vsix {
                ExtensionKind::Vsix
            } else {
                ExtensionKind::Archive
            },
            source: source_label,
            dependencies,
            extension_pack,
            engines_vscode,
            requires: ext.requires.clone(),
        });
    }

    let content_src = if parse_vsix {
        tmp_root.join("extension")
    } else {
        tmp_root.clone()
    };
    if !content_src.is_dir() {
        let _ = std::fs::remove_dir_all(&tmp_root);
        bail!("{}: нет каталога с содержимым", src.display());
    }
    if target.exists() {
        let _ = std::fs::remove_dir_all(&target);
    }
    if std::fs::rename(&content_src, &target).is_err() {
        copy_dir_recursive(&content_src, &target)?;
    }
    std::fs::write(&fp_file, fp)?;
    let _ = std::fs::remove_dir_all(&tmp_root);

    crate::log(format!(
        "bundle: extension prepared ({}): {} v{} → {}",
        if parse_vsix { "vsix" } else { "archive" },
        full_id,
        version,
        target.display()
    ));

    Ok(PreparedExtension {
        id: full_id,
        version,
        path: target,
        kind: if parse_vsix {
            ExtensionKind::Vsix
        } else {
            ExtensionKind::Archive
        },
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn has(tool: &str) -> bool {
        std::env::var_os("PATH")
            .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(tool).is_file()))
    }

    fn has_zip_tools() -> bool {
        has("zip") && has("unzip")
    }

    /// Собирает минимальный `.vsix` с `extension/package.json`.
    fn make_vsix(dst: &Path, publisher: &str, name: &str, version: &str) -> Result<()> {
        let build = dst
            .parent()
            .ok_or_else(|| anyhow!("no parent for {}", dst.display()))?
            .join(format!(".vsix-build-{}", std::process::id()));
        let inner = build.join("extension");
        let _ = std::fs::remove_dir_all(&build);
        std::fs::create_dir_all(&inner)?;
        let pkg = format!(r#"{{"publisher":"{publisher}","name":"{name}","version":"{version}"}}"#);
        std::fs::write(inner.join("package.json"), pkg)?;
        let st = Command::new("zip")
            .current_dir(&build)
            .args(["-qr"])
            .arg(dst)
            .arg("extension")
            .status()
            .map_err(|e| anyhow!("spawn zip: {e}"))?;
        let _ = std::fs::remove_dir_all(&build);
        if !st.success() {
            bail!("zip failed");
        }
        Ok(())
    }

    fn dummy_ext(name: &str) -> Extension {
        Extension {
            name: name.into(),
            r#type: None,
            file: None,
            cid: None,
            url: None,
            requires: Vec::new(),
        }
    }

    // -----------------------------------------------------------------------
    // prepare_as_file
    // -----------------------------------------------------------------------
    //
    // `type = "file"`: URL/CID кэшируются прямо в `<ext_dir>/<basename>`,
    // что совпадает с target. Старый код делал `remove_file(target)` —
    // это удаляло src, `copy(src, target)` падал с ENOENT. Сейчас проверяем
    // ветку `if src != target` для обоих сценариев.

    #[test]
    fn prepare_file_skips_copy_when_src_equals_target() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ext_dir = tmp.path().join("extensions");
        std::fs::create_dir_all(&ext_dir).unwrap();
        let src = ext_dir.join("demo.oxt");
        std::fs::write(&src, b"fake-oxt").unwrap();

        let ext = dummy_ext("demo");
        let prep = prepare_as_file(&ext, &src, "fp-1", &ext_dir, "t".into())
            .expect("prepare_as_file must not fail when src == target");
        assert!(prep.path.is_file());
        assert_eq!(prep.path, src);
        assert_eq!(prep.kind, ExtensionKind::File);
        assert_eq!(std::fs::read(&prep.path).unwrap(), b"fake-oxt");

        let fp = ext_dir.join(".demo.oxt.fingerprint");
        assert!(fp.is_file());
        assert_eq!(std::fs::read_to_string(&fp).unwrap(), "fp-1");
    }

    #[test]
    fn prepare_file_copies_when_src_differs_from_target() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ext_dir = tmp.path().join("extensions");
        std::fs::create_dir_all(&ext_dir).unwrap();
        let src = tmp.path().join("custom-name.oxt");
        std::fs::write(&src, b"oxt").unwrap();

        let ext = dummy_ext("grammalecte");
        let prep =
            prepare_as_file(&ext, &src, "fp-2", &ext_dir, "t".into()).expect("prepare_as_file");

        assert_ne!(prep.path, src);
        assert_eq!(prep.path, ext_dir.join("custom-name.oxt"));
        assert!(prep.path.is_file());
        assert!(src.is_file());
    }

    // -----------------------------------------------------------------------
    // prepare_as_archive (vsix) — распаковка в каталог `<id>-<ver>/`
    // -----------------------------------------------------------------------
    //
    // VSCodium сам подхватывает каталоги `<id>-<ver>/` из `--extensions-dir`,
    // `codium --install-extension` не нужен (и без X он зависает в GUI-режиме).
    // Поэтому target — КАТАЛОГ, а не `.vsix`-файл.
    // Исходный `.vsix` остаётся в `<ext_dir>/` как источник для инкрементальной
    // пересборки (fingerprint по size+mtime).

    #[test]
    fn prepare_vsix_extracts_to_dir() {
        if !has_zip_tools() {
            eprintln!("SKIP: zip/unzip не в PATH");
            return;
        }
        let tmp = tempfile::TempDir::new().unwrap();
        let ext_dir = tmp.path().join("extensions");
        std::fs::create_dir_all(&ext_dir).unwrap();

        // Имитация online (OpenVSX): src = <ext_dir>/<id>-<ver>.vsix.
        let src = ext_dir.join("demo.widget-1.0.0.vsix");
        make_vsix(&src, "demo", "widget", "1.0.0").unwrap();

        let ext = dummy_ext("demo.widget");
        let prep = prepare_as_archive(&ext, &src, "fp", &ext_dir, "t".into(), true)
            .expect("prepare_as_archive");

        // target — каталог, а не .vsix.
        assert!(
            prep.path.is_dir(),
            "path must be a directory: {:?}",
            prep.path
        );
        assert_eq!(prep.path, ext_dir.join("demo.widget-1.0.0"));
        assert_eq!(prep.path.extension().and_then(|s| s.to_str()), None);
        // Содержимое `extension/` из .vsix теперь корень каталога.
        assert!(prep.path.join("package.json").is_file());
        // Исходный .vsix остаётся на месте (кэш для инкрементальности).
        assert!(src.is_file());
        // Fingerprint — рядом с каталогом, не внутри.
        let fp = ext_dir.join(".demo.widget-1.0.0.fingerprint");
        assert!(fp.is_file());
        assert_eq!(std::fs::read_to_string(&fp).unwrap(), "fp");
        // Метаданные из package.json.
        assert_eq!(prep.id, "demo.widget");
        assert_eq!(prep.version, "1.0.0");
        assert_eq!(prep.kind, ExtensionKind::Vsix);
    }

    #[test]
    fn prepare_vsix_idempotent_with_same_fingerprint() {
        if !has_zip_tools() {
            eprintln!("SKIP: zip/unzip не в PATH");
            return;
        }
        let tmp = tempfile::TempDir::new().unwrap();
        let ext_dir = tmp.path().join("extensions");
        std::fs::create_dir_all(&ext_dir).unwrap();
        let src = ext_dir.join("demo.widget-1.0.0.vsix");
        make_vsix(&src, "demo", "widget", "1.0.0").unwrap();

        let ext = dummy_ext("demo.widget");
        let p1 = prepare_as_archive(&ext, &src, "same", &ext_dir, "t".into(), true).unwrap();
        let p2 = prepare_as_archive(&ext, &src, "same", &ext_dir, "t".into(), true).unwrap();
        assert_eq!(p1.path, p2.path);
        assert!(p2.path.is_dir());
        assert!(p2.path.join("package.json").is_file());
    }

    #[test]
    fn prepare_vsix_refreshes_dir_when_fingerprint_differs() {
        if !has_zip_tools() {
            eprintln!("SKIP: zip/unzip не в PATH");
            return;
        }
        let tmp = tempfile::TempDir::new().unwrap();
        let ext_dir = tmp.path().join("extensions");
        std::fs::create_dir_all(&ext_dir).unwrap();
        let src = ext_dir.join("demo.widget-1.0.0.vsix");
        make_vsix(&src, "demo", "widget", "1.0.0").unwrap();

        let ext = dummy_ext("demo.widget");
        let _ = prepare_as_archive(&ext, &src, "old-fp", &ext_dir, "t".into(), true).unwrap();

        // Ставим «мусор» внутри — при смене fingerprint он должен быть удалён
        // вместе с каталогом и заменён свежим содержимым.
        std::fs::write(ext_dir.join("demo.widget-1.0.0/MARKER"), b"stale").unwrap();

        let p = prepare_as_archive(&ext, &src, "new-fp", &ext_dir, "t".into(), true).unwrap();
        assert!(!p.path.join("MARKER").exists(), "stale dir must be removed");
        assert!(p.path.join("package.json").is_file());
    }
}
