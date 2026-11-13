mod bundle;
mod install;
mod macros;
mod repo;
mod spec;
mod templates;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};

use crate::spec::Spec;

// ===========================================================================
// CLI
// ===========================================================================

#[derive(Parser)]
#[command(
    name = "appspec",
    version,
    about = "Apptainer SIF builder from TOML spec"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Сгенерировать Dockerfile + launcher (без сборки)
    Gen { spec: PathBuf },

    /// Сгенерировать и собрать SIF
    Build {
        spec: PathBuf,
        /// Пересобрать, даже если SIF уже существует
        #[arg(short, long)]
        force: bool,
    },

    /// Запустить: для обычного spec — SIF-лаунчер, для bundle — собранный bundle-лаунчер
    /// Запустить пакет из репозитория или локальный spec/bundle.
    /// Пакет: `foo`, `foo:1.0`, `foo:1.0-1`. Файл: `./foo.appspec`.
    Run {
        /// Имя пакета или путь к `.appspec`
        spec: String,
        /// Источник: PATH | http:// | ipns:// | ipfs:// | oci://
        #[arg(long)]
        from: Option<String>,
        /// Профиль из repos.toml
        #[arg(long)]
        profile: Option<String>,
        /// Локальный репозиторий (sink)
        #[arg(long)]
        root: Option<PathBuf>,
        /// ed25519 pubkey для проверки index.json (repeatable)
        #[arg(long = "trusted-key")]
        trusted_keys: Vec<PathBuf>,
        /// Принять неподписанный index.json
        #[arg(long)]
        allow_unsigned: bool,
        /// Только подготовить (pull + launcher), не запускать
        #[arg(long)]
        dry_run: bool,
    },

    /// Остановить instance, оставленный `appspec run` в режиме persist
    Stop { spec: PathBuf },

    /// build + run
    All {
        spec: PathBuf,
        /// Пересобрать, даже если SIF уже существует
        #[arg(short, long)]
        force: bool,
    },

    /// Удалить сгенерированные артефакты
    Clean { spec: PathBuf },

    /// Установить SIF + launcher + desktop-интеграцию
    Install {
        spec: PathBuf,
        #[arg(long, conflicts_with_all = ["global", "custom"])]
        user: bool,
        #[arg(long, conflicts_with_all = ["user", "custom"])]
        global: bool,
        #[arg(long, conflicts_with_all = ["user", "global"])]
        custom: bool,
        #[arg(long)]
        no_desktop: bool,
        #[arg(long)]
        ipfs: bool,
    },

    /// Удалить всё, что было установлено
    Uninstall {
        spec: PathBuf,
        #[arg(long, conflicts_with = "global")]
        user: bool,
        #[arg(long, conflicts_with = "user")]
        global: bool,
    },

    /// Опубликовать SIF в IPFS и напечатать CID
    IpfsPublish {
        spec: PathBuf,
        #[arg(long)]
        no_pin: bool,
    },

    /// Композитные окружения: base + компоненты (spec|sif|cid|manifest|repo) + extensions
    Bundle {
        #[command(subcommand)]
        cmd: bundle::BundleCommand,
    },

    /// Управление локальным репозиторием SIF
    Repo {
        #[command(subcommand)]
        cmd: repo::RepoCommand,
    },

    /// Сгенерировать shell-completion (bash, zsh, fish, elvish, powershell)
    ///
    /// Пример установки bash-completion:
    ///   appspec completions bash > /etc/bash_completion.d/appspec
    ///   # или (только для текущего пользователя):
    ///   mkdir -p ~/.local/share/bash-completion/completions
    ///   appspec completions bash > ~/.local/share/bash-completion/completions/appspec
    Completions {
        /// Целевой shell
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

// ===========================================================================
// main
// ===========================================================================

fn main() {
    let cli = Cli::parse();

    let result: Result<i32> = match cli.cmd {
        Cmd::Gen { spec } => cmd_gen(&spec).map(|_| 0),
        Cmd::Build { spec, force } => cmd_build(&spec, force).map(|_| 0),
        Cmd::Run {
            spec,
            from,
            profile,
            root,
            trusted_keys,
            allow_unsigned,
            dry_run,
        } => cmd_run(
            &spec,
            from.as_deref(),
            profile.as_deref(),
            root.as_deref(),
            &trusted_keys,
            allow_unsigned,
            dry_run,
        )
        .map(|_| 0),
        Cmd::Stop { spec } => cmd_stop(&spec).map(|_| 0),
        Cmd::All { spec, force } => {
            let s = spec.display().to_string();
            cmd_build(&spec, force)
                .and_then(|_| cmd_run(&s, None, None, None, &[], false, false))
                .map(|_| 0)
        }
        Cmd::Clean { spec } => cmd_clean(&spec).map(|_| 0),

        Cmd::Install {
            spec,
            user,
            global,
            custom,
            no_desktop,
            ipfs,
        } => {
            let _ = user;
            let scope = if global {
                install::Scope::Global
            } else if custom {
                install::Scope::Custom
            } else {
                install::Scope::User
            };
            install::install(
                &spec,
                install::InstallOpts {
                    scope,
                    with_desktop: !no_desktop,
                    with_ipfs: ipfs,
                },
            )
            .map(|m| {
                log(format!("installed: {} (scope={})", m.name, m.scope));
                0
            })
        }

        Cmd::Uninstall { spec, user, global } => {
            let _ = user;
            let scope = if global {
                install::Scope::Global
            } else {
                install::Scope::User
            };
            install::uninstall(&spec, scope).map(|_| 0)
        }

        Cmd::IpfsPublish { spec, no_pin } => cmd_ipfs_publish(&spec, no_pin).map(|_| 0),

        Cmd::Completions { shell } => {
            use clap::CommandFactory;
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "appspec", &mut std::io::stdout());
            Ok(0)
        }

        Cmd::Bundle { cmd } => bundle::run(cmd),

        Cmd::Repo { cmd } => repo::run(cmd),
    };

    match result {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("\x1b[1;31m[appspec]\x1b[0m {e:#}");
            std::process::exit(1);
        }
    }
}

// ===========================================================================
// helpers
// ===========================================================================

pub(crate) fn log(msg: impl AsRef<str>) {
    eprintln!("\x1b[1;36m[appspec]\x1b[0m {}", msg.as_ref());
}

/// Загрузка spec с раскрытием макросов `%{name}`, `%{?name}`,
/// `%{name:default}` и `%%`.
///
/// Порядок: читаем TOML как `toml::Value`, прогоняем через
/// `macros::expand_all`, конвертируем в `Spec`, при необходимости
/// квалифицируем base-образ.
pub(crate) fn load_spec(path: &Path) -> Result<Spec> {
    let text =
        fs::read_to_string(path).with_context(|| format!("reading spec {}", path.display()))?;
    load_spec_from_text(&text).with_context(|| format!("in {}", path.display()))
}

pub(crate) fn load_spec_from_text(text: &str) -> Result<Spec> {
    let mut value: toml::Value = toml::from_str(text).context("parsing spec")?;
    macros::expand_all(&mut value).context("expanding macros")?;
    let mut spec: Spec = value.try_into().context("converting spec")?;
    spec.validate().context("validating spec")?;
    spec.build.qualify_base_image();
    Ok(spec)
}

pub(crate) fn spec_dir(spec_path: &Path) -> PathBuf {
    spec_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Тип spec-файла: обычный (`[package]`) или bundle (`[bundle]`).
fn spec_kind(path: &Path) -> Result<SpecKind> {
    let text =
        fs::read_to_string(path).with_context(|| format!("reading spec {}", path.display()))?;
    let v: toml::Value =
        toml::from_str(&text).with_context(|| format!("parsing spec {}", path.display()))?;
    let has_bundle = v.get("bundle").is_some();
    let has_package = v.get("package").is_some();
    Ok(match (has_bundle, has_package) {
        (true, false) => SpecKind::Bundle,
        (false, true) => SpecKind::App,
        (true, true) => bail!(
            "{}: содержит и [bundle], и [package] — не могу определить тип",
            path.display()
        ),
        (false, false) => bail!(
            "{}: нет ни [package], ни [bundle] — не похоже на spec",
            path.display()
        ),
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SpecKind {
    App,
    Bundle,
}

fn bundle_name(path: &Path) -> Result<String> {
    let text = fs::read_to_string(path)?;
    let v: toml::Value = toml::from_str(&text)?;
    v.get("bundle")
        .and_then(|b| b.get("name"))
        .and_then(|n| n.as_str())
        .map(str::to_owned)
        .ok_or_else(|| {
            anyhow!(
                "{}: [bundle].name отсутствует или не строка",
                path.display()
            )
        })
}

// ---------------------------------------------------------------------------
// Engine detection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum Engine {
    Docker,
    Podman,
    Buildah,
}

impl Engine {
    fn binary(&self) -> &'static str {
        match self {
            Engine::Docker => "docker",
            Engine::Podman => "podman",
            Engine::Buildah => "buildah",
        }
    }
}

fn detect_engine() -> Result<Engine> {
    if let Ok(want) = std::env::var("APPSPEC_ENGINE") {
        let e = match want.as_str() {
            "docker" => Engine::Docker,
            "podman" => Engine::Podman,
            "buildah" => Engine::Buildah,
            other => bail!("APPSPEC_ENGINE={other} не поддерживается"),
        };
        if !which(e.binary()) {
            bail!("APPSPEC_ENGINE={} но бинарь не найден в PATH", e.binary());
        }
        return Ok(e);
    }

    for (name, engine) in [
        ("buildah", Engine::Buildah),
        ("podman", Engine::Podman),
        ("docker", Engine::Docker),
    ] {
        if which(name) {
            return Ok(engine);
        }
    }
    bail!("ни docker, ни podman, ни buildah не найдены в PATH")
}

fn which(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|p| p.join(name).is_file()))
}

fn run(args: &[&str]) -> Result<()> {
    log(format!("$ {}", args.join(" ")));
    let status = Command::new(args[0])
        .args(&args[1..])
        .status()
        .with_context(|| format!("failed to spawn {}", args[0]))?;
    if !status.success() {
        bail!("command failed: {}", args.join(" "));
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) fn set_exec(p: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perm = fs::metadata(p)?.permissions();
    perm.set_mode(0o755);
    fs::set_permissions(p, perm)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_exec(_: &Path) -> Result<()> {
    Ok(())
}

// ---------------------------------------------------------------------------
// build / export image (engine-specific)
// ---------------------------------------------------------------------------

fn build_image(engine: Engine, df: &Path, dir: &Path, image: &str) -> Result<()> {
    let df_str = df.to_str().context("non-UTF-8 Dockerfile path")?;
    let dir_str = dir.to_str().context("non-UTF-8 build context")?;

    match engine {
        Engine::Docker | Engine::Podman => run(&[
            engine.binary(),
            "build",
            "--network",
            "host",
            "--no-cache",
            "-t",
            image,
            "-f",
            df_str,
            dir_str,
        ]),
        Engine::Buildah => run(&[
            "buildah",
            "bud",
            "--no-cache",
            "--format",
            "oci",
            "-t",
            image,
            "-f",
            df_str,
            dir_str,
        ]),
    }
}

fn export_image(engine: Engine, image: &str, tar: &Path) -> Result<String> {
    let tar_str = tar.to_str().context("non-UTF-8 tar path")?;

    match engine {
        Engine::Docker | Engine::Podman => {
            run(&[engine.binary(), "save", image, "-o", tar_str])?;
            Ok(format!("docker-archive://{tar_str}"))
        }
        Engine::Buildah => {
            let tag = image.rsplit(':').next().unwrap_or("latest");
            let uri = format!("oci-archive:{tar_str}:{tag}");
            run(&["buildah", "push", image, &uri])?;
            Ok(format!("oci-archive://{tar_str}"))
        }
    }
}

// ===========================================================================
// commands
// ===========================================================================

fn cmd_gen(spec_path: &Path) -> Result<()> {
    match spec_kind(spec_path)? {
        SpecKind::App => cmd_gen_app(spec_path),
        SpecKind::Bundle => cmd_gen_bundle(spec_path),
    }
}

fn cmd_gen_app(spec_path: &Path) -> Result<()> {
    let spec = load_spec(spec_path)?;
    let dir = spec_dir(spec_path);
    let name = &spec.package.name;

    let df_path = dir.join(format!("Dockerfile.{name}"));
    let rl_path = dir.join(format!("run.{name}.sh"));

    // CHANGED: передаём spec_path для метаданных (SHA-256 + base64 спека)
    let df = templates::render_dockerfile(&spec, &dir, spec_path)?;
    let rl = templates::render_launcher(&spec, &dir)?;

    fs::write(&df_path, df)?;
    fs::write(&rl_path, rl)?;
    set_exec(&rl_path)?;

    log(format!("Wrote {}", df_path.display()));
    log(format!("Wrote {}", rl_path.display()));
    Ok(())
}

fn cmd_gen_bundle(bundle_path: &Path) -> Result<()> {
    use crate::bundle;
    let args = bundle::commands::BuildArgs {
        spec: bundle_path.display().to_string(),
        output: None,
        no_build: true,
        no_lock: false,
        repo: None,
        trusted_keys: Vec::new(),
        allow_unsigned: false,
    };
    let code = bundle::commands::build(args)?;
    if code != 0 {
        bail!("bundle gen завершился с кодом {code}");
    }
    Ok(())
}

fn cmd_build(spec_path: &Path, force: bool) -> Result<()> {
    match spec_kind(spec_path)? {
        SpecKind::App => cmd_build_app(spec_path, force),
        SpecKind::Bundle => cmd_build_bundle(spec_path),
    }
}

fn cmd_build_app(spec_path: &Path, force: bool) -> Result<()> {
    let spec = load_spec(spec_path)?;
    let dir = spec_dir(spec_path);
    let sif = dir.join(spec.sif_file());

    cmd_gen_app(spec_path)?;

    if sif.exists() && !force {
        log(format!(
            "SIF уже существует: {} (используй --force для пересборки)",
            sif.display()
        ));
        return Ok(());
    }

    let engine = detect_engine()?;
    log(format!("engine: {:?}", engine));

    let image = spec.image_name();
    let name = &spec.package.name;
    let df = dir.join(format!("Dockerfile.{name}"));
    let tar = dir.join(format!("{name}.tar"));

    log(format!("Building image {image}…"));
    build_image(engine, &df, &dir, &image)?;

    log(format!("Exporting image to {}…", tar.display()));
    let source_uri = export_image(engine, &image, &tar)?;

    // Пишем временный файл и передаём его через --startscript.
    // Apptainer ставит свой дефолтный startscript при сборке из OCI/tar
    // и игнорирует /.singularity.d/startscript из образа. В версиях,
    // где есть флаг `--startscript` — передаём его флагом. В остальных
    // (Apptainer ≥ 1.x, где startscript задаётся только через .def-секцию
    // `%startscript`) — применяем fallback через sandbox.
    let ss_body = "#!/bin/sh\n\
                   if [ \"$#\" -gt 0 ]; then exec \"$@\"; fi\n\
                   while true; do sleep 3600; done\n";

    let startscript_path = dir.join(format!(".{name}.startscript"));
    fs::write(&startscript_path, ss_body)
        .with_context(|| format!("writing {}", startscript_path.display()))?;
    set_exec(&startscript_path)?;

    let has_startscript_flag = apptainer_supports_startscript();

    if has_startscript_flag {
        log(format!("Building Apptainer SIF {}…", sif.display()));
        let result = run(&[
            "apptainer",
            "build",
            "--force",
            "--startscript",
            startscript_path
                .to_str()
                .context("non-UTF-8 startscript path")?,
            sif.to_str().context("non-UTF-8 SIF path")?,
            &source_uri,
        ]);
        let _ = fs::remove_file(&startscript_path);
        result?;
    } else {
        // Fallback: Apptainer не поддерживает --startscript как флаг,
        // startscript задаётся только через .def-секцию `%startscript`.
        // Последовательность:
        //   1. Собрать SIF из OCI/tar обычным способом.
        //   2. Распаковать SIF в sandbox.
        //   3. Переписать /.singularity.d/startscript и runscript.
        //   4. Упаковать sandbox обратно в SIF.
        let _ = fs::remove_file(&startscript_path);

        log(format!(
            "Building Apptainer SIF {} (pass 1: from OCI)…",
            sif.display()
        ));
        run(&[
            "apptainer",
            "build",
            "--force",
            sif.to_str().context("non-UTF-8 SIF path")?,
            &source_uri,
        ])?;

        patch_startscript_via_sandbox(&sif, ss_body, name, &dir)?;
    }

    let _ = fs::remove_file(&tar);
    log(format!("SIF готов: {}", sif.display()));
    Ok(())
}

/// Проверить, поддерживает ли `apptainer build` флаг `--startscript`.
///
/// На Apptainer 1.x такого флага нет — startscript задаётся только
/// через секцию `%startscript` в `.def`-файле. Функция ищет точное
/// совпадение с `--startscript` (с двумя дефисами). `%startscript`
/// (с процентом) не подходит — значит fallback.
fn apptainer_supports_startscript() -> bool {
    Command::new("apptainer")
        .args(["build", "--help"])
        .output()
        .ok()
        .map(|o| {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let stderr = String::from_utf8_lossy(&o.stderr);
            stdout.contains("--startscript") || stderr.contains("--startscript")
        })
        .unwrap_or(false)
}

/// Fallback для Apptainer без `--startscript`: собрать SIF как sandbox,
/// переписать `/.singularity.d/startscript`, упаковать обратно в SIF.
///
/// Требует, чтобы `sif` уже существовал. Вызывающий обязан сначала
/// собрать SIF из OCI/tar обычным `apptainer build`.
fn patch_startscript_via_sandbox(sif: &Path, ss_body: &str, name: &str, dir: &Path) -> Result<()> {
    let sandbox = dir.join(format!(".{name}.sandbox.tmp"));
    let _ = fs::remove_dir_all(&sandbox);

    // 1) Разбираем SIF в sandbox.
    log(format!(
        "  [startscript] extracting {} → sandbox",
        sif.display()
    ));
    run(&[
        "apptainer",
        "build",
        "--force",
        "--sandbox",
        sandbox.to_str().context("non-UTF-8 sandbox path")?,
        sif.to_str().context("non-UTF-8 SIF path")?,
    ])?;

    // 2) Кладём наш startscript и runscript.
    let ss = sandbox.join(".singularity.d/startscript");
    let rs = sandbox.join(".singularity.d/runscript");
    fs::create_dir_all(ss.parent().unwrap())?;
    fs::write(&ss, ss_body).with_context(|| format!("writing {}", ss.display()))?;
    set_exec(&ss)?;
    fs::copy(&ss, &rs).with_context(|| format!("copy to {}", rs.display()))?;

    // 3) Пакуем sandbox назад в SIF (в тот же файл, --force перезапишет).
    let _ = fs::remove_file(sif);
    log(format!(
        "  [startscript] repacking sandbox → {}",
        sif.display()
    ));
    run(&[
        "apptainer",
        "build",
        "--force",
        sif.to_str().context("non-UTF-8 SIF path")?,
        sandbox.to_str().context("non-UTF-8 sandbox path")?,
    ])?;

    // 4) Чистим временный sandbox.
    let _ = fs::remove_dir_all(&sandbox);

    log(format!("  [startscript] patched: {}", sif.display()));
    Ok(())
}

fn cmd_build_bundle(bundle_path: &Path) -> Result<()> {
    use crate::bundle;
    let args = bundle::commands::BuildArgs {
        spec: bundle_path.display().to_string(),
        output: None,
        no_build: false,
        no_lock: false,
        repo: None,
        trusted_keys: Vec::new(),
        allow_unsigned: false,
    };
    let code = bundle::commands::build(args)?;
    if code != 0 {
        bail!("bundle build завершился с кодом {code}");
    }
    Ok(())
}

fn is_package_name(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    if s.contains('/') || s.starts_with('.') || s.starts_with('~') || s.ends_with(".appspec") {
        return false;
    }
    true
}

fn cmd_run(
    spec: &str,
    from: Option<&str>,
    profile: Option<&str>,
    root: Option<&Path>,
    trusted_keys: &[PathBuf],
    allow_unsigned: bool,
    dry_run: bool,
) -> Result<()> {
    if is_package_name(spec) {
        return cmd_run_package(
            spec,
            from,
            profile,
            root,
            trusted_keys,
            allow_unsigned,
            dry_run,
        );
    }
    if dry_run {
        bail!("--dry-run applies to package names, not file paths");
    }
    let spec_path = Path::new(spec);
    match spec_kind(spec_path)? {
        SpecKind::App => cmd_run_app(spec_path),
        SpecKind::Bundle => cmd_run_bundle(spec_path),
    }
}

fn run_cache_dir() -> PathBuf {
    if let Ok(v) = std::env::var("XDG_CACHE_HOME") {
        if !v.is_empty() {
            return PathBuf::from(v).join("appspec/run");
        }
    }
    if let Ok(h) = std::env::var("HOME") {
        if !h.is_empty() {
            return PathBuf::from(h).join(".cache/appspec/run");
        }
    }
    PathBuf::from("/tmp/appspec-run")
}

fn open_local_repo_for_run(root: Option<&Path>) -> Result<crate::repo::layout::RepoRoot> {
    let root_path = match root {
        Some(p) => p.to_path_buf(),
        None => match std::env::var_os("APPSPEC_REPO") {
            Some(v) => {
                let s = v.to_string_lossy().to_string();
                if s.is_empty() || s.contains("://") {
                    crate::repo::default_root()
                } else {
                    PathBuf::from(s)
                }
            }
            None => crate::repo::default_root(),
        },
    };
    if root_path.join("index.json").is_file() {
        return crate::repo::layout::RepoRoot::open(&root_path);
    }
    // bootstrap empty repo
    fs::create_dir_all(root_path.join("blobs/sha256"))?;
    fs::create_dir_all(root_path.join("packages"))?;
    fs::create_dir_all(root_path.join("refs"))?;
    fs::create_dir_all(root_path.join("index"))?;
    let _ = fs::write(root_path.join("index/.keep"), b"");
    let empty = crate::repo::meta::IndexV1 {
        schema: 1,
        repo: "local/auto".into(),
        updated: crate::repo::util::now_iso8601(),
        generator: format!("appspec {}", env!("CARGO_PKG_VERSION")),
        signature: String::new(),
        packages: Default::default(),
    };
    fs::write(
        root_path.join("index.json"),
        serde_json::to_vec_pretty(&empty)?,
    )?;
    crate::repo::layout::RepoRoot::open(&root_path)
}

fn resolve_run_sources(from: Option<&str>, profile: Option<&str>) -> Result<Vec<String>> {
    if let Some(f) = from {
        return Ok(vec![f.to_string()]);
    }
    if let Some(pname) = profile {
        let (_, cc) = crate::repo::config::find_client_config(None)
            .with_context(|| format!("loading client config for profile `{}`", pname))?;
        let p = cc
            .profile(pname)
            .ok_or_else(|| anyhow!("unknown profile `{}`", pname))?;
        return crate::repo::commands::profile_sources(p);
    }
    if let Ok(v) = std::env::var("APPSPEC_REPO") {
        if !v.is_empty() {
            if v.starts_with("http://")
                || v.starts_with("https://")
                || v.starts_with("ipfs://")
                || v.starts_with("ipns://")
            {
                return Ok(vec![v]);
            }
            if let Ok((_, cc)) = crate::repo::config::find_client_config(None) {
                if let Some(p) = cc.profile(&v) {
                    return crate::repo::commands::profile_sources(p);
                }
            }
        }
    }
    if let Ok((_, cc)) = crate::repo::config::find_client_config(None) {
        if let Some(def) = &cc.default {
            if let Some(p) = cc.profile(def) {
                return crate::repo::commands::profile_sources(p);
            }
        }
    }
    Ok(Vec::new())
}

fn cmd_run_package(
    target: &str,
    from: Option<&str>,
    profile: Option<&str>,
    root: Option<&Path>,
    trusted_keys: &[PathBuf],
    allow_unsigned: bool,
    dry_run: bool,
) -> Result<()> {
    let (name, ver_spec) = crate::repo::run_package::parse_package_ref(target)?;
    log(format!("run: package={} ver_spec={:?}", name, ver_spec));

    let local_root = open_local_repo_for_run(root)?;

    // 1. try local index
    let mut resolved: Option<(String, crate::repo::meta::IndexVersion, String)> = None;
    if local_root.index_path().is_file() {
        if let Ok(text) = fs::read_to_string(local_root.index_path()) {
            if let Ok(idx) = serde_json::from_str::<crate::repo::meta::IndexV1>(&text) {
                if let Ok((k, v)) =
                    crate::repo::run_package::pick_version(&idx, &name, ver_spec.as_deref())
                {
                    resolved = Some((k.clone(), v.clone(), local_root.path.display().to_string()));
                }
            }
        }
    }

    // 2. try remote sources
    if resolved.is_none() {
        let sources = resolve_run_sources(from, profile)?;
        for src in &sources {
            if let Ok(text) = crate::repo::commands::read_index_text_from(src) {
                if let Ok(idx) = serde_json::from_str::<crate::repo::meta::IndexV1>(&text) {
                    if let Ok((k, v)) =
                        crate::repo::run_package::pick_version(&idx, &name, ver_spec.as_deref())
                    {
                        resolved = Some((k.clone(), v.clone(), src.clone()));
                        break;
                    }
                }
            }
        }
    }

    let (ver_key, ver, origin) = resolved.ok_or_else(|| {
        anyhow!(
            "package `{}` not found (local={}, or in remote sources)",
            name,
            local_root.path.display()
        )
    })?;
    log(format!(
        "resolved: {}:{} (sha256={}, ipfs_cid={})",
        name,
        ver_key,
        ver.sif_sha256.get(..12).unwrap_or(&ver.sif_sha256),
        ver.ipfs_cid.as_deref().unwrap_or("<none>")
    ));

    // 3. pull if blob missing
    let blob = local_root
        .path
        .join("blobs/sha256")
        .join(ver.sif_sha256.get(..2).unwrap_or("00"))
        .join(ver.sif_sha256.get(2..).unwrap_or(""));
    let pkg_dir = local_root.packages_dir().join(&name).join(&ver_key);

    let cached = blob.is_file() && pkg_dir.is_dir();
    if !cached {
        log(format!("pull {}:{} from {}", name, ver_key, origin));
        let opts = crate::repo::pull::PullOpts {
            sources: vec![origin.clone()],
            via: None,
            spec_only: false,
            sif_out: None,
            trusted_key: trusted_keys.first().cloned(),
            allow_unsigned,
        };
        crate::repo::pull::pull(&local_root, &format!("{}:{}", name, ver_key), opts)?;
    } else {
        log(format!("cache hit: {}", blob.display()));
    }
    if !blob.is_file() {
        bail!("blob missing after pull: {}", blob.display());
    }

    // 4. resolve spec
    let spec_path = pkg_dir.join("spec.appspec");
    let spec = if spec_path.is_file() {
        crate::load_spec(&spec_path)?
    } else {
        log("spec.appspec not in cache; loading from SIF labels");
        let labels = crate::repo::inspect::inspect(&blob)?;
        let b64 = labels
            .get("org.appspec.spec.content.base64")
            .ok_or_else(|| anyhow!("SIF has no org.appspec.spec.content.base64 label"))?;
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .context("decoding SIF spec label")?;
        let text = std::str::from_utf8(&bytes).context("spec label is not UTF-8")?;
        crate::load_spec_from_text(text)?
    };

    // 5. render launcher
    let launcher_text = templates::render_launcher_abs(&spec, &blob)?;
    let run_dir = run_cache_dir();
    fs::create_dir_all(&run_dir)?;
    let safe_ver = ver_key.replace('/', "_");
    let launcher_path = run_dir.join(format!("run.{}.{}.sh", name, safe_ver));
    fs::write(&launcher_path, &launcher_text)?;
    set_exec(&launcher_path)?;
    log(format!("launcher: {}", launcher_path.display()));

    if dry_run {
        log("dry-run: not executing");
        return Ok(());
    }

    log(format!("Running {}", launcher_path.display()));
    let status = Command::new("/bin/sh")
        .arg(&launcher_path)
        .status()
        .context("failed to spawn launcher")?;
    if !status.success() {
        bail!("launcher вернул ошибку");
    }
    Ok(())
}

fn cmd_run_app(spec_path: &Path) -> Result<()> {
    let spec = load_spec(spec_path)?;
    let dir = spec_dir(spec_path);
    let name = &spec.package.name;

    let sif = dir.join(spec.sif_file());
    let dockerfile = dir.join(format!("Dockerfile.{name}"));
    let launcher = dir.join(format!("run.{name}.sh"));

    if !launcher.exists() {
        cmd_gen_app(spec_path)?;
    }

    if sif.is_file() && dockerfile.is_file() {
        let sif_mtime = fs::metadata(&sif)?.modified()?;
        let df_mtime = fs::metadata(&dockerfile)?.modified()?;
        if df_mtime > sif_mtime {
            log(format!(
                "WARNING: {} новее, чем {}.\n\
                 \x20        Пересобери: appspec build -f {}",
                dockerfile.display(),
                sif.display(),
                spec_path.display()
            ));
        }
    }

    log(format!("Running {}", launcher.display()));
    let status = Command::new("/bin/sh")
        .arg(&launcher)
        .status()
        .context("failed to spawn launcher")?;
    if !status.success() {
        bail!("launcher вернул ошибку");
    }
    Ok(())
}

fn cmd_run_bundle(bundle_path: &Path) -> Result<()> {
    use crate::bundle;

    let args = bundle::commands::BuildArgs {
        spec: bundle_path.display().to_string(),
        output: None,
        no_build: false,
        no_lock: false,
        repo: None,
        trusted_keys: Vec::new(),
        allow_unsigned: false,
    };
    let code = bundle::commands::build(args)?;
    if code != 0 {
        bail!("bundle build завершился с кодом {code}");
    }

    let name = bundle_name(bundle_path)?;
    let launcher = bundle_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("run.{name}.sh"));

    if !launcher.is_file() {
        bail!("launcher не найден после сборки: {}", launcher.display());
    }

    log(format!("Running {}", launcher.display()));
    let status = Command::new("/bin/sh")
        .arg(&launcher)
        .status()
        .context("failed to spawn bundle launcher")?;
    if !status.success() {
        bail!("bundle launcher вернул ошибку");
    }
    Ok(())
}

/// Остановить instance, оставленный `appspec run` в режиме persist.
///
/// Имя instance = `[package].name`. Ровно то же имя генерирует
/// `run.sh.j2` для одиночных spec'ов. Bundle-сценарий сюда не попадает:
/// у него свои instance'ы, останавливаемые `appspec bundle clean`.
fn cmd_stop(spec_path: &Path) -> Result<()> {
    match spec_kind(spec_path)? {
        SpecKind::App => {
            let spec = load_spec(spec_path)?;
            let name = &spec.package.name;

            // Проверим, что instance действительно существует — иначе
            // `apptainer instance stop` вернёт ошибку, и это выглядит
            // пугающе для пользователя, который просто «запустил stop
            // на всякий случай».
            let list = Command::new("apptainer")
                .args(["instance", "list"])
                .output()
                .context("failed to run `apptainer instance list`")?;
            let text = String::from_utf8_lossy(&list.stdout);
            let running = text
                .lines()
                .skip(1)
                .filter_map(|l| l.split_whitespace().next())
                .any(|n| n == name);

            if !running {
                log(format!("instance '{}' не запущен", name));
                return Ok(());
            }

            log(format!("stopping instance '{}'", name));
            let status = Command::new("apptainer")
                .args(["instance", "stop", name])
                .status()
                .context("failed to spawn apptainer")?;
            if !status.success() {
                bail!("`apptainer instance stop {}` вернул ошибку", name);
            }
            log(format!("instance '{}' остановлен", name));
            Ok(())
        }
        SpecKind::Bundle => {
            bail!(
                "appspec stop — только для одиночных spec'ов. \
                 Bundle-инстансы останавливаются через `appspec bundle clean` \
                 или автоматически по trap'у в run.<bundle>.sh"
            );
        }
    }
}

fn cmd_clean(spec_path: &Path) -> Result<()> {
    match spec_kind(spec_path)? {
        SpecKind::App => cmd_clean_app(spec_path),
        SpecKind::Bundle => cmd_clean_bundle(spec_path),
    }
}

fn cmd_clean_app(spec_path: &Path) -> Result<()> {
    let spec = load_spec(spec_path)?;
    let dir = spec_dir(spec_path);
    let name = &spec.package.name;

    // Если instance запущен с persist — сначала остановим,
    // иначе останется висеть на удалённом SIF'е.
    let _ = cmd_stop(spec_path);

    for f in [
        dir.join(format!("Dockerfile.{name}")),
        dir.join(format!("run.{name}.sh")),
        dir.join(spec.sif_file()),
        dir.join(format!("{name}.tar")),
    ] {
        if f.exists() {
            let _ = fs::remove_file(&f);
            log(format!("Removed {}", f.display()));
        }
    }
    Ok(())
}

fn cmd_clean_bundle(bundle_path: &Path) -> Result<()> {
    use crate::bundle;

    let name = bundle_name(bundle_path)?;
    let dir = bundle_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();

    let launcher = dir.join(format!("run.{name}.sh"));
    if launcher.exists() {
        fs::remove_file(&launcher)?;
        log(format!("Removed {}", launcher.display()));
    }

    let cache = bundle::loader::bundle_cache_dir(&name);
    if cache.exists() {
        fs::remove_dir_all(&cache)
            .with_context(|| format!("removing cache {}", cache.display()))?;
        log(format!("Removed {}", cache.display()));
    }
    Ok(())
}

fn cmd_ipfs_publish(spec_path: &Path, no_pin: bool) -> Result<()> {
    let s = load_spec(spec_path)?;
    let dir = spec_dir(spec_path);
    let sif = dir.join(s.sif_file());
    if !sif.is_file() {
        bail!("нет SIF: {}", sif.display());
    }
    let cid = install::ipfs_publish(&sif, !no_pin)?;
    println!("{cid}");
    Ok(())
}
