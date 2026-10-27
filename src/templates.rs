// src/templates.rs
//
// Порядок поиска шаблонов:
//   0.  $APPSPEC_TEMPLATE_PATH (colon-separated)
//   1.  template_dir из спека
//   2.  <workspace>/templates/         — только под cargo test / APPSPEC_DEV
//   3.  <exe_dir>/templates/
//   4.  <spec_dir>/templates/, <spec_dir>/
//   5.  $XDG_CONFIG_HOME/appspec/templates/
//   6.  $XDG_DATA_DIRS/appspec/templates/
//   7.  вкомпилированный include_str!  — fallback
#![allow(dead_code)]

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use minijinja::{context, Environment};

use crate::spec::{Runtime, Spec};

// ===========================================================================
// Встроенные шаблоны
// ===========================================================================

const TPL_DF_DEB: &str = include_str!("../templates/Dockerfile.deb.j2");
const TPL_DF_TAR: &str = include_str!("../templates/Dockerfile.tar.j2");
const TPL_DF_APT: &str = include_str!("../templates/Dockerfile.apt.j2");
const TPL_DF_GENERIC: &str = include_str!("../templates/Dockerfile.generic.j2");
const TPL_DF_RPM: &str = include_str!("../templates/Dockerfile.rpm.j2");
const TPL_DF_ALPINE: &str = include_str!("../templates/Dockerfile.alpine.j2");
const TPL_DF_ALPINE_SLIM: &str = include_str!("../templates/Dockerfile.alpine-slim.j2");
const TPL_DF_FULL: &str = include_str!("../templates/Dockerfile.full.j2");
const TPL_DF_STATIC: &str = include_str!("../templates/Dockerfile.static.j2");
const TPL_DF_SCRATCH: &str = include_str!("../templates/Dockerfile.scratch.j2");

const TPL_RUN_DEFAULT: &str = include_str!("../templates/run.sh.j2");
const TPL_RUN_CLI: &str = include_str!("../templates/run-cli.sh.j2");
const TPL_INSTALLED_LAUNCHER: &str = include_str!("../templates/launcher.sh.j2");
const TPL_DESKTOP: &str = include_str!("../templates/app.desktop.j2");
const TPL_DF_DEB_LDD: &str = include_str!("../templates/Dockerfile.deb-ldd.j2");
const TPL_DF_QT_STATIC: &str = include_str!("../templates/Dockerfile.qt-static.j2");

fn builtin_dockerfile(name: &str) -> Option<&'static str> {
    Some(match name {
        "deb" => TPL_DF_DEB,
        "tar" | "tgz" | "targz" => TPL_DF_TAR,
        "apt" => TPL_DF_APT,
        "generic" => TPL_DF_GENERIC,
        "rpm" => TPL_DF_RPM,
        "alpine" => TPL_DF_ALPINE,
        "alpine-slim" => TPL_DF_ALPINE_SLIM,
        "full" => TPL_DF_FULL,
        "static" => TPL_DF_STATIC,
        "scratch" => TPL_DF_SCRATCH,
        "deb-ldd" | "ldd" => TPL_DF_DEB_LDD,
        "qt-static" | "qt" => TPL_DF_QT_STATIC,
        _ => return None,
    })
}

fn builtin_launcher(name: &str) -> Option<&'static str> {
    Some(match name {
        "default" | "run.sh" => TPL_RUN_DEFAULT,
        "cli" => TPL_RUN_CLI,
        _ => return None,
    })
}

// ===========================================================================
// Dev / build-dir detection
// ===========================================================================

pub fn is_dev_mode() -> bool {
    if std::env::var_os("APPSPEC_DEV").is_some() {
        return true;
    }
    std::env::current_exe()
        .ok()
        .map(|p| {
            let s = p.to_string_lossy();
            s.contains("/target/debug/deps/") || s.contains("/target/release/deps/")
        })
        .unwrap_or(false)
}

fn build_templates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("templates")
}

fn build_config_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("config")
}

// ===========================================================================
// XDG helpers
// ===========================================================================

pub fn xdg_config_home() -> Option<PathBuf> {
    if let Some(v) = std::env::var_os("XDG_CONFIG_HOME") {
        let p = PathBuf::from(v);
        if p.is_absolute() {
            return Some(p);
        }
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))
}

pub fn xdg_data_home() -> Option<PathBuf> {
    if let Some(v) = std::env::var_os("XDG_DATA_HOME") {
        let p = PathBuf::from(v);
        if p.is_absolute() {
            return Some(p);
        }
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share"))
}

pub fn xdg_data_dirs() -> Vec<PathBuf> {
    if let Some(v) = std::env::var_os("XDG_DATA_DIRS") {
        let list: Vec<PathBuf> = std::env::split_paths(&v)
            .filter(|p| p.is_absolute())
            .collect();
        if !list.is_empty() {
            return list;
        }
    }
    vec![
        PathBuf::from("/usr/local/share"),
        PathBuf::from("/usr/share"),
    ]
}

fn dedup(dirs: &mut Vec<PathBuf>) {
    let mut seen = HashSet::new();
    dirs.retain(|d| seen.insert(d.clone()));
}

// ===========================================================================
// Search paths
// ===========================================================================

pub fn search_dirs(spec_dir: &Path, extra_top: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    if let Some(v) = std::env::var_os("APPSPEC_TEMPLATE_PATH") {
        for p in std::env::split_paths(&v) {
            if p.is_absolute() {
                dirs.push(p);
            }
        }
    }

    if let Some(top) = extra_top {
        dirs.push(top.to_path_buf());
    }

    if is_dev_mode() {
        let b = build_templates_dir();
        if b.is_dir() {
            dirs.push(b);
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            dirs.push(d.join("templates"));
        }
    }

    dirs.push(spec_dir.join("templates"));
    dirs.push(spec_dir.to_path_buf());

    if let Some(cfg) = xdg_config_home() {
        dirs.push(cfg.join("appspec/templates"));
    }

    for data in xdg_data_dirs() {
        dirs.push(data.join("appspec/templates"));
    }

    dedup(&mut dirs);
    dirs
}

pub fn config_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    if let Some(v) = std::env::var_os("APPSPEC_CONFIG_PATH") {
        for p in std::env::split_paths(&v) {
            if p.is_absolute() {
                dirs.push(p);
            }
        }
    }

    if is_dev_mode() {
        let b = build_config_dir();
        if b.is_dir() {
            dirs.push(b);
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            dirs.push(d.join("config"));
        }
    }

    if let Some(cfg) = xdg_config_home() {
        dirs.push(cfg.join("appspec"));
    }

    dirs.push(PathBuf::from("/etc/appspec"));

    for data in xdg_data_dirs() {
        dirs.push(data.join("appspec"));
    }

    dedup(&mut dirs);
    dirs
}

pub fn find_config(name: &str) -> Option<PathBuf> {
    for d in config_dirs() {
        let p = d.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

pub fn find_config_any(names: &[&str]) -> Option<PathBuf> {
    let dirs = config_dirs();
    for d in &dirs {
        for n in names {
            let p = d.join(n);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

// ===========================================================================
// minijinja Environment + фильтры
// ===========================================================================

fn env() -> &'static Environment<'static> {
    static ENV: OnceLock<Environment<'static>> = OnceLock::new();
    ENV.get_or_init(|| {
        let mut e = Environment::new();
        e.add_filter("sh_quote", |s: String| sh_quote(&s));
        e.add_filter("json_array", |v: Vec<String>| json_array(&v));
        e.add_filter("docker_env", |s: String| docker_env(&s));
        e
    })
}

pub fn sh_quote(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "/_-.:=+@,".contains(c));
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

pub fn json_array(items: &[String]) -> String {
    let inner: Vec<String> = items
        .iter()
        .map(|s| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect();
    format!("[{}]", inner.join(", "))
}

/// Экранирование значения для `ENV K=V` в Dockerfile.
pub fn docker_env(value: &str) -> String {
    let safe = !value.is_empty()
        && value.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '/' | '_' | '-' | '.' | ':' | '+' | '=' | '@' | ',')
        });

    if safe {
        return value.to_string();
    }

    let mut out = String::with_capacity(value.len() + 8);
    for c in value.chars() {
        match c {
            ' ' => out.push_str("\\ "),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\'' => out.push_str("\\'"),
            '#' => out.push_str("\\#"),
            _ => out.push(c),
        }
    }
    out
}

// ===========================================================================
// SIF metadata (LABEL)
// ===========================================================================

/// Экранирование значения для `LABEL key = "value"`.
///
/// Docker выполняет подстановку `${VAR}`/`$VAR` в значениях LABEL,
/// поэтому `$` тоже экранируется. Переводы строк и табы заменяются
/// пробелами — метаданные должны быть однострочными. Полный исходный
/// текст `.appspec` доступен в `org.appspec.spec.content.base64`
/// со всей разметкой.
fn escape_label(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '$' => out.push_str("\\$"),
            '\n' | '\r' | '\t' => out.push(' '),
            _ => out.push(c),
        }
    }
    out
}

/// Сформировать набор LABEL-метаданных для финального Dockerfile.
///
/// Три пространства имён:
///   * `org.appspec.*` — собственные метаданные + содержимое спека;
///   * `org.label-schema.*` — стандарт label-schema.org (backward compat);
///   * `org.opencontainers.image.*` — современный OCI image spec.
///
/// Извлечение:
///   apptainer inspect --json --labels image.sif | jq .
///   apptainer inspect --json --labels image.sif \
///     | jq -r '."org.appspec.spec.content.base64"' | base64 -d
pub fn build_labels(spec: &Spec, spec_path: &Path) -> BTreeMap<String, String> {
    let pkg = &spec.package;
    let mut m = BTreeMap::new();

    // --- генератор / формат спека ---
    m.insert(
        "org.appspec.generator".into(),
        format!("appspec {}", env!("CARGO_PKG_VERSION")),
    );
    m.insert("org.appspec.spec.format".into(), "1".into());

    // --- RPM-style поля ---
    m.insert("org.appspec.package.name".into(), pkg.name.clone());
    m.insert("org.appspec.package.version".into(), pkg.version.clone());
    m.insert("org.appspec.package.arch".into(), spec.build.arch.clone());
    if let Some(v) = &pkg.release {
        m.insert("org.appspec.package.release".into(), v.clone());
    }
    if let Some(v) = pkg.epoch {
        m.insert("org.appspec.package.epoch".into(), v.to_string());
    }
    if let Some(v) = &pkg.summary {
        m.insert("org.appspec.package.summary".into(), v.clone());
    }
    if let Some(v) = &pkg.description {
        // Однострочная версия; оригинал — в spec.content.base64.
        m.insert(
            "org.appspec.package.description".into(),
            v.replace('\n', " ").replace('\r', " "),
        );
    }
    if let Some(v) = &pkg.license {
        m.insert("org.appspec.package.license".into(), v.clone());
    }
    if let Some(v) = &pkg.url {
        m.insert("org.appspec.package.url".into(), v.clone());
    }
    if let Some(v) = &pkg.vendor {
        m.insert("org.appspec.package.vendor".into(), v.clone());
    }
    if let Some(v) = &pkg.packager {
        m.insert("org.appspec.package.packager".into(), v.clone());
    }

    // --- label-schema.org (backward compat) ---
    m.insert("org.label-schema.schema-version".into(), "1.0".into());
    m.insert("org.label-schema.name".into(), pkg.name.clone());
    m.insert("org.label-schema.version".into(), pkg.version.clone());
    if let Some(v) = pkg.summary.as_ref().or(pkg.description.as_ref()) {
        m.insert(
            "org.label-schema.description".into(),
            v.replace('\n', " ").replace('\r', " "),
        );
    }
    if let Some(v) = &pkg.license {
        m.insert("org.label-schema.license".into(), v.clone());
    }
    if let Some(v) = &pkg.url {
        m.insert("org.label-schema.url".into(), v.clone());
    }
    if let Some(v) = &pkg.vendor {
        m.insert("org.label-schema.vendor".into(), v.clone());
    }

    // --- OCI image spec (современный стандарт) ---
    m.insert("org.opencontainers.image.title".into(), pkg.name.clone());
    m.insert("org.opencontainers.image.version".into(), pkg.version.clone());
    if let Some(v) = pkg.summary.as_ref().or(pkg.description.as_ref()) {
        m.insert(
            "org.opencontainers.image.description".into(),
            v.replace('\n', " ").replace('\r', " "),
        );
    }
    if let Some(v) = &pkg.license {
        m.insert("org.opencontainers.image.licenses".into(), v.clone());
    }
    if let Some(v) = &pkg.url {
        m.insert("org.opencontainers.image.url".into(), v.clone());
    }
    if let Some(v) = &pkg.vendor {
        m.insert("org.opencontainers.image.vendor".into(), v.clone());
    }

    // --- содержимое спека для восстановления ---
    match std::fs::read(spec_path) {
        Ok(raw) => {
            let filename = spec_path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            m.insert("org.appspec.spec.filename".into(), filename);
            m.insert("org.appspec.spec.size".into(), raw.len().to_string());

            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(&raw);
            m.insert(
                "org.appspec.spec.sha256".into(),
                format!("{:x}", hasher.finalize()),
            );

            m.insert(
                "org.appspec.spec.content.base64".into(),
                base64::engine::general_purpose::STANDARD.encode(&raw),
            );
        }
        Err(e) => {
            crate::log(format!(
                "warning: не могу прочитать {} для метаданных: {}",
                spec_path.display(),
                e
            ));
        }
    }

    m
}

fn render_label_block(labels: &BTreeMap<String, String>) -> String {
    let mut s = String::with_capacity(1024);
    s.push_str("# --- appspec metadata (LABEL) ---\n");
    for (k, v) in labels {
        // Современный синтаксис `LABEL key="value"`.
        // Пробелы вокруг `=` НЕДОПУСТИМЫ: Docker/buildah парсит такую строку
        // как legacy `LABEL key value`, и `= "value"` целиком становится
        // значением лейбла (отсюда префикс "= " в `apptainer inspect`).
        s.push_str(&format!("LABEL {}=\"{}\"\n", k, escape_label(v)));
    }
    s
}

/// Найти позицию последней инструкции `FROM `, начинающейся с начала
/// строки. Возвращает байтовое смещение.
fn find_last_from(df: &str) -> Option<usize> {
    let mut pos = 0usize;
    let mut last: Option<usize> = None;
    for line in df.split_inclusive('\n') {
        if line.starts_with("FROM ") {
            last = Some(pos);
        }
        pos += line.len();
    }
    last
}

/// Вставить блок LABEL сразу после последней `FROM ` (то есть в
/// начало финальной стадии). Если `FROM` не найдено — предупреждение,
/// Dockerfile остаётся без изменений.
fn inject_labels(mut df: String, labels: &BTreeMap<String, String>) -> String {
    let block = render_label_block(labels);

    let Some(pos) = find_last_from(&df) else {
        crate::log("warning: в Dockerfile нет инструкции `FROM` — метаданные не добавлены");
        return df;
    };

    // Смещение сразу после символа '\n', завершающего строку FROM.
    let after = pos
        + df[pos..]
            .find('\n')
            .map(|i| i + 1)
            .unwrap_or_else(|| df.len() - pos);

    df.insert_str(after, &block);
    df
}

// ===========================================================================
// Резолвинг и рендер
// ===========================================================================

fn load_template(
    explicit_file: Option<&Path>,
    extra_dir: Option<&Path>,
    spec_dir: &Path,
    name: &str,
    builtin: Option<&'static str>,
    variants: &[String],
) -> Result<(String, String)> {
    let dirs = search_dirs(spec_dir, extra_dir);

    if let Some(tf) = explicit_file {
        if tf.is_absolute() {
            if tf.is_file() {
                let src = std::fs::read_to_string(tf)
                    .with_context(|| format!("reading {}", tf.display()))?;
                return Ok((src, tf.display().to_string()));
            }
            bail!("template_file не найден: {}", tf.display());
        }
        for d in &dirs {
            let cand = d.join(tf);
            if cand.is_file() {
                let src = std::fs::read_to_string(&cand)
                    .with_context(|| format!("reading {}", cand.display()))?;
                return Ok((src, cand.display().to_string()));
            }
        }
        bail!(
            "template_file `{}` не найден.\n  искал в: {:#?}",
            tf.display(),
            dirs
        );
    }

    for d in &dirs {
        for v in variants {
            let cand = d.join(v);
            if cand.is_file() {
                let src = std::fs::read_to_string(&cand)
                    .with_context(|| format!("reading {}", cand.display()))?;
                return Ok((src, cand.display().to_string()));
            }
        }
    }

    if let Some(src) = builtin {
        return Ok((src.to_string(), format!("<builtin:{name}>")));
    }

    bail!(
        "шаблон `{}` не найден.\n  искал варианты: {:#?}\n  в каталогах: {:#?}",
        name,
        variants,
        dirs
    )
}

fn render(source: &str, origin: &str, ctx: minijinja::Value) -> Result<String> {
    env()
        .template_from_str(source)
        .map_err(|e| anyhow!("template parse error in {origin}: {e}"))?
        .render(ctx)
        .map_err(|e| anyhow!("template render error in {origin}: {e}"))
}

fn substitute(t: &str, spec: &Spec) -> String {
    t.replace("{name}", &spec.package.name)
        .replace("{version}", &spec.package.version)
        .replace("{arch}", &spec.build.arch)
}

fn heredoc_tag() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("APPSPEC_SETUP_{n:x}")
}

// ===========================================================================
// Построение shell-строк для bind'ов
// ===========================================================================

fn build_bind_lines(rt: &Runtime) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();

    for b in &rt.binds {
        if let Some(forced) = b.strip_prefix('!') {
            lines.push(format!(r#"BINDS="$BINDS --bind {forced}""#));
        } else {
            let src = b.split(':').next().unwrap_or(b.as_str());
            lines.push(format!(
                r#"[ -e "{src}" ] && BINDS="$BINDS --bind {b}" || true"#
            ));
        }
    }

    for b in &rt.optional_binds {
        let b_clean = b.strip_prefix('!').unwrap_or(b);
        let src = b_clean.split(':').next().unwrap_or(b_clean);
        lines.push(format!(
            r#"[ -e "{src}" ] && BINDS="$BINDS --bind {b_clean}" || true"#
        ));
    }

    lines
}

// ===========================================================================
// Dockerfile
// ===========================================================================

pub fn render_dockerfile(spec: &Spec, spec_dir: &Path, spec_path: &Path) -> Result<String> {
    let b = &spec.build;

    let name = b
        .template
        .clone()
        .unwrap_or_else(|| b.source.kind.clone().unwrap_or_else(|| "generic".into()));
    let builtin = builtin_dockerfile(&name);
    let variants = vec![format!("{name}.j2"), format!("Dockerfile.{name}.j2")];

    let (source, origin) = load_template(
        b.template_file.as_deref(),
        b.template_dir.as_deref(),
        spec_dir,
        &name,
        builtin,
        &variants,
    )?;

    let binary = b
        .collect
        .binary
        .clone()
        .unwrap_or_else(|| format!("/usr/bin/{}", spec.package.name));
    let cmd = b
        .scratch
        .cmd
        .clone()
        .unwrap_or_else(|| vec![binary.clone()]);
    let workdir = b.scratch.workdir.clone().unwrap_or_else(|| "/".into());

    let magicpak_version = b
        .collect
        .magicpak_version
        .clone()
        .unwrap_or_else(|| "v1.4.0".into());
    let busybox_url = b.collect.busybox_url.clone().unwrap_or_else(|| {
        "https://busybox.net/downloads/binaries/1.35.0-x86_64-linux-musl/busybox".into()
    });

    let collect_timeout = b
        .collect
        .timeout_secs
        .map(|n| n.to_string())
        .unwrap_or_else(|| "30".into());

    let rendered = render(
        &source,
        &origin,
        context! {
            pkg              => &spec.package,
            build            => &spec.build,
            runtime          => &spec.runtime,
            source_url       => substitute(&b.source.url, spec),
            binary           => binary,
            cmd              => cmd,
            workdir          => workdir,
            magicpak_version => magicpak_version,
            busybox_url      => busybox_url,
            collect_timeout  => collect_timeout,
        },
    )?;

    let labels = build_labels(spec, spec_path);
    Ok(inject_labels(rendered, &labels))
}

// ===========================================================================
// Launcher (`appspec run`)
// ===========================================================================

pub fn render_launcher(spec: &Spec, spec_dir: &Path) -> Result<String> {
    let rt = &spec.runtime;

    let default_name = match rt.mode.as_deref() {
        Some("cli") => "cli",
        _ => "default",
    };
    let name = rt.template.clone().unwrap_or_else(|| default_name.into());
    let builtin = builtin_launcher(&name);

    let variants = if name == "cli" {
        vec![
            "run-cli.sh.j2".to_string(),
            "cli.j2".to_string(),
            format!("run.{}.cli.j2", spec.package.name),
        ]
    } else {
        vec![
            format!("{name}.j2"),
            format!("run.{name}.sh.j2"),
            format!("run.{}.sh.j2", spec.package.name),
        ]
    };

    let (source, origin) = load_template(
        rt.template_file.as_deref(),
        rt.template_dir.as_deref(),
        spec_dir,
        &name,
        builtin,
        &variants,
    )?;

    let env_passthrough: Vec<String> = rt
        .env_passthrough
        .clone()
        .unwrap_or_else(|| vec!["DISPLAY".into()]);

    let env_checks: Vec<String> = env_passthrough
        .iter()
        .map(|v| format!(r#"[ -n "${{{v}:-}}" ] && X_ENV="$X_ENV --env {v}=${{{v}}}" || true"#))
        .collect();

    let bind_lines = build_bind_lines(rt);
    let create_dirs: Vec<String> = rt.create_dirs.clone();

    let setup = rt.setup.as_deref().map(str::trim_end).map(str::to_string);
    let entry = rt.entry.as_deref().map(str::trim_end).map(str::to_string);

    render(
        &source,
        &origin,
        context! {
            pkg             => &spec.package,
            runtime         => &spec.runtime,
            sif_file        => spec.sif_file(),
            image_name      => spec.image_name(),
            bind_lines      => bind_lines,
            env_checks      => env_checks,
            create_dirs     => create_dirs,
            xauthority_auto => rt.xauthority_auto.unwrap_or(true),
            pulse_auto      => rt.pulse_auto.unwrap_or(true),
            heredoc_tag     => heredoc_tag(),
            runtime_setup   => setup,
            runtime_entry   => entry,
            workdir         => rt.workdir.clone().unwrap_or_else(|| "/config/home".into()),
            persist => rt.persist.unwrap_or(false),
            instance_base => spec.package.name.clone(),
        },
    )
}

// ===========================================================================
// Launcher для `appspec install`
// ===========================================================================

pub fn render_installed_launcher(
    spec: &Spec,
    sif_path: &Path,
    setup_path: &Path,
    state_dir: &Path,
    scope: crate::install::Scope,
) -> Result<String> {
    let rt = &spec.runtime;

    let variants = vec!["launcher.j2".to_string(), "launcher.sh.j2".to_string()];
    let (source, origin) = load_template(
        None,
        None,
        Path::new("."),
        "launcher",
        Some(TPL_INSTALLED_LAUNCHER),
        &variants,
    )?;

    let bind_lines = build_bind_lines(rt);
    let create_dirs: Vec<String> = rt.create_dirs.clone();

    render(
        &source,
        &origin,
        context! {
            pkg         => &spec.package,
            runtime     => &spec.runtime,
            install     => &spec.install,
            scope       => scope.as_str(),
            sif_path    => sif_path.display().to_string(),
            setup_path  => setup_path.display().to_string(),
            state_dir   => state_dir.display().to_string(),
            bind_lines  => bind_lines,
            create_dirs => create_dirs,
        },
    )
}

// ===========================================================================
// .desktop
// ===========================================================================

pub fn render_desktop(
    spec: &Spec,
    launcher_path: &Path,
    scope: crate::install::Scope,
) -> Result<String> {
    render(
        TPL_DESKTOP,
        "<builtin:app.desktop>",
        context! {
            pkg           => &spec.package,
            install       => &spec.install,
            scope         => scope.as_str(),
            launcher_path => launcher_path.display().to_string(),
        },
    )
}