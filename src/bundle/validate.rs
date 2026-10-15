use serde::Serialize;
use std::collections::HashSet;
use std::path::Path;

use super::loader;
use super::model::{
    BundleFile, Component, ComponentMode, Extension, PreparedExtension, ResolvedBase, SourceSpec,
};

#[derive(Debug, Clone, Serialize)]
pub struct Issue {
    pub level: &'static str,
    pub scope: String,
    pub msg: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub detail: String,
}

impl Issue {
    fn error(scope: impl Into<String>, msg: impl Into<String>) -> Self {
        Self {
            level: "error",
            scope: scope.into(),
            msg: msg.into(),
            detail: String::new(),
        }
    }
    fn warn(scope: impl Into<String>, msg: impl Into<String>) -> Self {
        Self {
            level: "warn",
            scope: scope.into(),
            msg: msg.into(),
            detail: String::new(),
        }
    }
    fn detail(mut self, d: impl Into<String>) -> Self {
        self.detail = d.into();
        self
    }
}

pub fn has_errors(issues: &[Issue]) -> bool {
    issues.iter().any(|i| i.level == "error")
}

pub fn preflight(file: &BundleFile, base_dir: &Path, cache: &Path) -> Vec<Issue> {
    let mut out = Vec::new();

    check_source("base", &file.bundle.base.source, base_dir, cache, &mut out);

    for c in &file.components {
        let scope = format!("component:{}", c.name);
        check_source(&scope, &c.source, base_dir, cache, &mut out);

        for fr in &c.files {
            if let Err(e) = fr.validate(&scope) {
                out.push(Issue::error(&scope, format!("{:#}", e)));
                continue;
            }
            if let Some(src) = &fr.from {
                let p = loader::resolve_relative(src, base_dir);
                if !p.is_file() {
                    out.push(Issue::error(
                        &scope,
                        format!("файл `{src}` не найден: {}", p.display()),
                    ));
                }
            }
        }

        if c.has_instance() {
            for dev in &["/dev/fuse", "/dev/net/tun"] {
                if c.mounts().iter().any(|m| m == dev) && !Path::new(dev).exists() {
                    out.push(
                        Issue::warn(
                            &scope,
                            format!("{dev} отсутствует на хосте — instance может не заработать"),
                        )
                        .detail("проверьте modprobe fuse/tun на хосте"),
                    );
                }
            }
        }

        for m in c.mounts() {
            if !m.starts_with('/') {
                continue;
            }
            let p = Path::new(&m);
            if p.is_dir() {
                continue;
            }
            if !p.exists() {
                out.push(Issue::warn(
                    &scope,
                    format!("mount `{m}` отсутствует на хосте — bridge пропустит"),
                ));
            }
        }
    }

    for e in &file.extensions {
        let scope = format!("extension:{}", e.name);
        if let Some(f) = &e.file {
            let p = loader::resolve_relative(f, base_dir);
            if !p.is_file() {
                out.push(Issue::error(
                    &scope,
                    format!(".vsix не найден: {}", p.display()),
                ));
            }
        } else if let Some(cid) = &e.cid {
            let cached = cache.join(format!("{}.vsix", loader::sanitize_filename(cid)));
            if !cached.is_file() {
                out.push(Issue::warn(
                    &scope,
                    format!("CID {} не выкачан (скачается при build)", cid),
                ));
            }
        } else if let Some(url) = &e.url {
            let cached = cache.join(format!("{}.vsix", loader::sanitize_filename(&e.name)));
            if !cached.is_file() {
                out.push(Issue::warn(
                    &scope,
                    format!("{} не скачан (скачается при build)", url),
                ));
            }
        }
    }

    out
}

fn check_source(
    scope: &str,
    src: &SourceSpec,
    base_dir: &Path,
    cache: &Path,
    out: &mut Vec<Issue>,
) {
    if src.kind_count() != 1 {
        out.push(
            Issue::error(scope, "source должен содержать ровно один источник").detail(src.origin()),
        );
        return;
    }
    if let Some(s) = &src.spec {
        let p = loader::resolve_relative(s, base_dir);
        if !p.is_file() {
            out.push(Issue::error(
                scope,
                format!("spec не найден: {}", p.display()),
            ));
            return;
        }
        match crate::load_spec(&p) {
            Ok(spec) => {
                let sif = p.parent().unwrap_or(Path::new(".")).join(spec.sif_file());
                if !sif.is_file() {
                    out.push(Issue::warn(
                        scope,
                        format!("SIF не собран (соберётся при build): {}", sif.display()),
                    ));
                }
            }
            Err(_) => out.push(Issue::error(
                scope,
                format!("spec не парсится: {}", p.display()),
            )),
        }
    } else if let Some(s) = &src.sif {
        let p = loader::resolve_relative(s, base_dir);
        if !p.is_file() {
            out.push(Issue::error(
                scope,
                format!("SIF не найден: {}", p.display()),
            ));
        }
    } else if let Some(cid) = &src.cid {
        let cached = cache.join(format!("{}.sif", loader::sanitize_filename(cid)));
        if !cached.is_file() {
            out.push(Issue::warn(scope, format!("CID {} не выкачан", cid)));
        }
    } else if let Some(m) = &src.manifest {
        let d = loader::resolve_relative(m, base_dir);
        if !d.join("manifest.json").is_file() {
            out.push(Issue::error(
                scope,
                format!("manifest.json не найден: {}", d.display()),
            ));
        }
    }
}

pub fn validate_static(
    base_src: &SourceSpec,
    entry_hint: Option<&str>,
    components: &[Component],
    extensions: &[Extension],
    strict: bool,
) -> Vec<Issue> {
    let mut out = Vec::new();

    if base_src.kind_count() != 1 {
        out.push(
            Issue::error("base", "source должен содержать ровно один источник")
                .detail(base_src.origin()),
        );
    }
    if entry_hint.is_none() {
        out.push(Issue::error(
            "base",
            "не указан entry (нет `exec …` в setup и нет [bundle.base].entry)",
        ));
    }

    for c in components {
        let scope = format!("component:{}", c.name);
        if !c.target().starts_with('/') {
            out.push(Issue::error(
                &scope,
                format!("target должен быть абсолютным: {}", c.target()),
            ));
        }
        if c.mode() == ComponentMode::Chroot {
            if !c.mount().starts_with('/') {
                out.push(Issue::error(
                    &scope,
                    format!("mount должен быть абсолютным: {}", c.mount()),
                ));
            }
            if !c.binary().starts_with('/') {
                out.push(Issue::error(
                    &scope,
                    format!("binary должен быть абсолютным: {}", c.binary()),
                ));
            }
        }
        if strict && c.selftest.is_none() {
            out.push(Issue::warn(&scope, "strict = true, но selftest не задан"));
        }
    }

    let mut seen_provides = std::collections::BTreeMap::<String, String>::new();
    for c in components {
        if let Some(prev) = seen_provides.insert(c.provide(), c.name.clone()) {
            out.push(Issue::error(
                format!("component:{}", c.name),
                format!("provide `{}` уже занят `{}`", c.provide(), prev),
            ));
        }
    }

    for e in extensions {
        let scope = format!("extension:{}", e.name);
        if e.kind_count() > 1 {
            out.push(Issue::error(
                &scope,
                "укажите не более одного из file | cid | url",
            ));
        }
    }

    out
}

pub fn validate_resolved(
    base: &ResolvedBase,
    resolved_components: &[(Component, std::path::PathBuf)],
    prepared_exts: &[PreparedExtension],
    strict: bool,
) -> Vec<Issue> {
    let mut out = Vec::new();

    if !base.sif.is_file() {
        out.push(Issue::error(
            "base",
            format!("SIF не найден: {}", base.sif.display()),
        ));
    }
    for (c, sif) in resolved_components {
        if !sif.is_file() {
            out.push(Issue::error(
                format!("component:{}", c.name),
                format!("SIF не найден: {}", sif.display()),
            ));
        }
    }

    let installed: HashSet<&str> = prepared_exts.iter().map(|e| e.id.as_str()).collect();
    for e in prepared_exts {
        for dep in &e.dependencies {
            if !installed.contains(dep.as_str()) {
                let msg = format!("extensionDependency `{dep}` не установлена");
                if strict {
                    out.push(Issue::error(format!("extension:{}", e.id), msg));
                } else {
                    out.push(Issue::warn(format!("extension:{}", e.id), msg));
                }
            }
        }
    }

    let provided: std::collections::BTreeMap<String, String> = resolved_components
        .iter()
        .map(|(c, _)| (c.provide(), c.name.clone()))
        .collect();
    for e in prepared_exts {
        for req in &e.requires {
            if !provided.contains_key(req) {
                let msg =
                    format!("расширение требует CLI `{req}`, но компонент его не предоставляет");
                if strict {
                    out.push(Issue::error(format!("extension:{}", e.id), msg));
                } else {
                    out.push(Issue::warn(format!("extension:{}", e.id), msg));
                }
            }
        }
    }

    out
}
