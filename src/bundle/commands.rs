use anyhow::{anyhow, Context, Result};
use clap::Args;
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;

use super::model::{ComponentMode, PreparedExtension, ResolvedFile};
use super::{codegen, loader, model, validate};

#[derive(Debug, Args)]
pub struct BuildArgs {
    pub spec: String,
    #[arg(long, short)] pub output: Option<String>,
    #[arg(long)] pub no_build: bool,
}
#[derive(Debug, Args)]
pub struct LsArgs { pub spec: String, #[arg(long)] pub json: bool }
#[derive(Debug, Args)]
pub struct CheckArgs { pub spec: String }
#[derive(Debug, Args)]
pub struct AddArgs {
    pub spec: String,
    pub name: String,
    #[arg(long)] pub from_spec: Option<String>,
    #[arg(long)] pub provide: Option<String>,
    #[arg(long)] pub dry_run: bool,
}
#[derive(Debug, Args)]
pub struct RemoveArgs { pub spec: String, pub name: String }

fn print_issues(issues: &[validate::Issue]) {
    for i in issues {
        let p = if i.level == "error" { "error:" } else { "warn: " };
        eprintln!("{p} [{}] {}", i.scope, i.msg);
        if !i.detail.is_empty() { eprintln!("       {}", i.detail); }
    }
}

fn load(spec: &str) -> Result<(model::BundleFile, PathBuf, PathBuf)> {
    let spec_path = PathBuf::from(spec);
    if !spec_path.is_file() {
        anyhow::bail!("bundle spec не найден: {}", spec_path.display());
    }
    let file = loader::load_bundle(&spec_path)?;
    let cache = loader::bundle_cache_dir(&file.bundle.name);
    Ok((file, spec_path, cache))
}

fn spec_base_dir(spec_path: &std::path::Path) -> PathBuf {
    spec_path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

struct Resolved {
    base: model::ResolvedBase,
    comps: Vec<(model::Component, PathBuf)>,
    exts: Vec<PreparedExtension>,
    files: Vec<ResolvedFile>,
    auto_added: HashSet<String>,
}

fn resolve_all(
    file: &model::BundleFile, base_dir: &std::path::Path,
    cache: &std::path::Path, allow_fetch: bool,
) -> Result<Resolved> {
    let base = loader::resolve_base(&file.bundle.base, cache, base_dir, allow_fetch)?;

    let mut comps: Vec<(model::Component, PathBuf)> = Vec::new();
    let mut files: Vec<ResolvedFile> = Vec::new();
    for c in &file.components {
        let r = loader::resolve_source(&c.source, cache, base_dir, allow_fetch,
            &format!("component `{}`", c.name))?;
        comps.push((c.clone(), r.sif));
        for fr in &c.files {
            let rf = loader::resolve_file(fr, &c.name, base_dir,
                &format!("component `{}`", c.name))?;
            files.push(rf);
        }
    }

    let provide_map: std::collections::BTreeMap<String, String> =
        comps.iter().map(|(c, _)| (c.provide(), c.name.clone())).collect();

    let resolve_deps = file.bundle.resolve_dependencies;
    let expand_packs = file.bundle.expand_packs;

    let mut queue: VecDeque<model::Extension> = VecDeque::new();
    let mut auto_added: HashSet<String> = HashSet::new();
    for e in &file.extensions { queue.push_back(e.clone()); }

    let mut prepared: Vec<PreparedExtension> = Vec::new();
    let mut seen_ids: HashSet<String> = HashSet::new();

    while let Some(ext) = queue.pop_front() {
        let p = loader::prepare_extension(&ext, cache, base_dir, allow_fetch)?;
        if !seen_ids.insert(p.id.clone()) { continue; }
        for req in &p.requires {
            match provide_map.get(req) {
                Some(comp) => crate::log(format!(
                    "bundle: `{}` требует CLI `{}` — предоставляется компонентом `{}`",
                    p.id, req, comp)),
                None => crate::log(format!(
                    "bundle: `{}` требует CLI `{}` — компонент не найден", p.id, req)),
            }
        }
        if resolve_deps {
            for dep in &p.dependencies {
                if !seen_ids.contains(dep) {
                    auto_added.insert(dep.clone());
                    queue.push_back(model::Extension::online(dep));
                }
            }
        }
        if expand_packs {
            for pack in &p.extension_pack {
                if !seen_ids.contains(pack) {
                    auto_added.insert(pack.clone());
                    queue.push_back(model::Extension::online(pack));
                }
            }
        }
        prepared.push(p);
    }

    Ok(Resolved { base, comps, exts: prepared, files, auto_added })
}

fn entry_hint(file: &model::BundleFile) -> Option<&str> {
    let b = &file.bundle.base;
    if b.entry.is_some()       { return Some(""); }
    if b.setup.is_some()       { return Some(""); }
    if b.source.spec.is_some() { return Some(""); }
    None
}

pub fn build(a: BuildArgs) -> Result<i32> {
    let (file, spec_path, cache) = load(&a.spec)?;
    let base_dir = spec_base_dir(&spec_path);
    let allow_fetch = !a.no_build;

    let static_issues = validate::validate_static(
        &file.bundle.base.source, entry_hint(&file),
        &file.components, &file.extensions, file.bundle.strict);
    let preflight_issues = validate::preflight(&file, &base_dir, &cache);

    print_issues(&static_issues);
    print_issues(&preflight_issues);
    if validate::has_errors(&static_issues) || validate::has_errors(&preflight_issues) {
        return Ok(2);
    }

    let r = resolve_all(&file, &base_dir, &cache, allow_fetch)?;
    let resolved_issues = validate::validate_resolved(
        &r.base, &r.comps, &r.exts, file.bundle.strict);
    print_issues(&resolved_issues);
    if validate::has_errors(&resolved_issues) { return Ok(2); }

    loader::reset_extensions_index(&cache);
    let launcher = codegen::render_launcher(
        &file, &r.base, &r.comps, &r.exts, &r.files)?;

    let out_path = match a.output {
        Some(o) => PathBuf::from(o),
        None => spec_path.parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join(format!("run.{}.sh", file.bundle.name)),
    };
    std::fs::write(&out_path, launcher)
        .with_context(|| format!("не могу записать: {}", out_path.display()))?;
    set_exec(&out_path)?;

    println!("ok: {}", out_path.display());
    println!("    cache      = {}", cache.display());
    println!("    base       = {}", r.base.origin);
    println!("    utils      = {}",
        if r.comps.is_empty() { "-".to_string() }
        else { r.comps.iter().map(|(c, _)| c.name.as_str()).collect::<Vec<_>>().join(", ") });
    println!("    instances  = {}",
        if r.comps.iter().any(|(c, _)| c.has_instance()) {
            r.comps.iter().filter(|(c, _)| c.has_instance())
                .map(|(c, _)| c.name.as_str()).collect::<Vec<_>>().join(", ")
        } else { "-".to_string() });
    println!("    extensions = {}", r.exts.len());
    Ok(0)
}

pub fn check(a: CheckArgs) -> Result<i32> {
    let (file, spec_path, cache) = load(&a.spec)?;
    let base_dir = spec_base_dir(&spec_path);

    let static_issues = validate::validate_static(
        &file.bundle.base.source, entry_hint(&file),
        &file.components, &file.extensions, file.bundle.strict);
    let preflight_issues = validate::preflight(&file, &base_dir, &cache);

    print_issues(&static_issues);
    print_issues(&preflight_issues);
    if validate::has_errors(&static_issues) || validate::has_errors(&preflight_issues) {
        eprintln!("check: FAILED");
        return Ok(2);
    }
    match resolve_all(&file, &base_dir, &cache, false) {
        Ok(r) => {
            let ri = validate::validate_resolved(&r.base, &r.comps, &r.exts, file.bundle.strict);
            print_issues(&ri);
            if validate::has_errors(&ri) { eprintln!("check: FAILED"); return Ok(2); }
        }
        Err(e) => eprintln!("warn: {e:#}"),
    }
    println!("check: ok");
    Ok(0)
}

pub fn ls(a: LsArgs) -> Result<i32> {
    let (file, spec_path, cache) = load(&a.spec)?;
    let base_dir = spec_base_dir(&spec_path);

    let static_issues = validate::validate_static(
        &file.bundle.base.source, entry_hint(&file),
        &file.components, &file.extensions, file.bundle.strict);
    let preflight_issues = validate::preflight(&file, &base_dir, &cache);
    print_issues(&static_issues);
    print_issues(&preflight_issues);

    println!("Bundle:  {}", file.bundle.name);
    println!("Spec:    {}", spec_path.display());
    println!("Cache:   {}", cache.display());
    println!();

    println!("Components ({}):", file.components.len());
    for c in &file.components {
        let mode = match c.mode() {
            ComponentMode::Chroot   => "chroot",
            ComponentMode::Instance => "instance",
            ComponentMode::Both     => "both",
        };
        println!("  [{}] {:<8} {}", mode, c.name, c.source.origin());
        println!("       provide: {}", c.provide());
        if let Some(sd) = c.socket_dir() { println!("       socket:  {}", sd); }
        if let Some(sd) = c.state_dir()  { println!("       state:   {}", sd); }
        if let Some(d)  = c.daemon()     { println!("       daemon:  {}", d); }
        for fr in &c.files {
            let scope = fr.scope.as_deref().unwrap_or("base");
            println!("       file:    {} [{}]", fr.to, scope);
        }
    }
    println!();

    let _ = (a.json, cache);
    Ok(0)
}

pub fn add(a: AddArgs) -> Result<i32> {
    let spec_path = PathBuf::from(&a.spec);
    if !spec_path.is_file() {
        anyhow::bail!("bundle spec не найден: {}", spec_path.display());
    }
    let base_dir = spec_base_dir(&spec_path);
    let file = loader::load_bundle(&spec_path)?;
    if file.components.iter().any(|c| c.name == a.name) {
        anyhow::bail!("component `{}` уже присутствует", a.name);
    }

    let spec_rel = a.from_spec.clone().unwrap_or_else(|| format!("./{}.appspec", a.name));
    let spec_abs = loader::resolve_relative(&spec_rel, &base_dir);
    let spec_exists = spec_abs.is_file();

    let provide = a.provide.clone().unwrap_or_else(|| a.name.clone());

    let block = format!(
        "[[components]]\n\
         name    = \"{name}\"\n\
         spec    = \"{spec_rel}\"\n\
         provide = \"{provide}\"\n",
        name = a.name, spec_rel = spec_rel, provide = provide);

    if a.dry_run { print!("{block}"); return Ok(0); }

    let mut text = std::fs::read_to_string(&spec_path)?;
    if !text.ends_with('\n') { text.push('\n'); }
    if !text.ends_with("\n\n") { text.push('\n'); }
    text.push_str(&block);
    std::fs::write(&spec_path, text)?;

    println!("ok: component `{}` добавлен", a.name);
    if !spec_exists {
        eprintln!("ВНИМАНИЕ: {} не существует", spec_abs.display());
    }
    Ok(0)
}

pub fn remove(a: RemoveArgs) -> Result<i32> {
    let spec_path = PathBuf::from(&a.spec);
    if !spec_path.is_file() {
        anyhow::bail!("bundle spec не найден: {}", spec_path.display());
    }
    let file = loader::load_bundle(&spec_path)?;
    if !file.components.iter().any(|c| c.name == a.name) {
        anyhow::bail!("component `{}` не найден", a.name);
    }

    let text = std::fs::read_to_string(&spec_path)?;
    let lines: Vec<&str> = text.lines().collect();
    let (start, end) = locate_component_block(&lines, &a.name)
        .ok_or_else(|| anyhow!("не смог найти блок `{}`", a.name))?;

    let mut new_lines: Vec<&str> = Vec::with_capacity(lines.len());
    new_lines.extend_from_slice(&lines[..start]);
    new_lines.extend_from_slice(&lines[end..]);

    let mut out_lines: Vec<&str> = Vec::new();
    let mut prev_blank = false;
    for ln in new_lines {
        let blank = ln.trim().is_empty();
        if blank && prev_blank { continue; }
        out_lines.push(ln);
        prev_blank = blank;
    }
    let mut out = out_lines.join("\n");
    if !out.ends_with('\n') { out.push('\n'); }
    std::fs::write(&spec_path, out)?;

    println!("ok: component `{}` удалён", a.name);
    Ok(0)
}

fn locate_component_block(lines: &[&str], name: &str) -> Option<(usize, usize)> {
    let mut i = 0;
    while i < lines.len() {
        if lines[i].starts_with("[[components]]") {
            let mut found = false;
            let mut j = i + 1;
            while j < lines.len() && !lines[j].starts_with('[') {
                if is_name_assignment(lines[j], name) { found = true; break; }
                j += 1;
            }
            if found {
                let mut k = i + 1;
                while k < lines.len() {
                    let lk = lines[k];
                    if lk.starts_with('[') {
                        if lk.starts_with("[components.") { k += 1; continue; }
                        break;
                    }
                    k += 1;
                }
                return Some((i, k));
            }
        }
        i += 1;
    }
    None
}

fn is_name_assignment(line: &str, name: &str) -> bool {
    let t = line.trim();
    let Some((k, v)) = t.split_once('=') else { return false; };
    if k.trim() != "name" { return false; }
    v.trim().trim_start_matches('"').trim_end_matches('"') == name
}

fn set_exec(p: &std::path::Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut m = std::fs::metadata(p)?.permissions();
        m.set_mode(0o755);
        std::fs::set_permissions(p, m)?;
    }
    #[cfg(not(unix))] { let _ = p; }
    Ok(())
}