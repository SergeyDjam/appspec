mod bundle;
mod install;
mod macros;
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
    Run { spec: PathBuf },

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

    /// Композитные окружения: base + компоненты (spec|sif|cid|manifest) + extensions
    Bundle {
        #[command(subcommand)]
        cmd: bundle::BundleCommand,
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
        Cmd::Run { spec } => cmd_run(&spec).map(|_| 0),
        Cmd::Stop { spec } => cmd_stop(&spec).map(|_| 0),
        Cmd::All { spec, force } => cmd_build(&spec, force)
            .and_then(|_| cmd_run(&spec))
            .map(|_| 0),
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

        Cmd::Bundle { cmd } => bundle::run(cmd),
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

    let mut value: toml::Value =
        toml::from_str(&text).with_context(|| format!("parsing spec {}", path.display()))?;

    macros::expand_all(&mut value)
        .with_context(|| format!("expanding macros in {}", path.display()))?;

    let mut spec: Spec = value
        .try_into()
        .with_context(|| format!("converting spec {}", path.display()))?;

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
fn set_exec(p: &Path) -> Result<()> {
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

    let df = templates::render_dockerfile(&spec, &dir)?;
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
    // где есть `--startscript`, — передаём его флагом. В остальных —
    // собираем SIF, распаковываем в sandbox, патчим, пакуем обратно.
    let ss_body = "#!/bin/sh\n\
                   if [ \"$#\" -gt 0 ]; then exec \"$@\"; fi\n\
                   while true; do sleep 3600; done\n";

    log(format!("Building Apptainer SIF {}…", sif.display()));

    let startscript_path = dir.join(format!(".{name}.startscript"));
    fs::write(&startscript_path, ss_body)
        .with_context(|| format!("writing {}", startscript_path.display()))?;
    set_exec(&startscript_path)?;

    let has_startscript_flag = apptainer_supports_startscript();

    if has_startscript_flag {
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
        // Fallback: собрать SIF, распаковать, патчить, упаковать.
        let _ = fs::remove_file(&startscript_path);
        patch_startscript_via_sandbox(&sif, ss_body, name, &dir)?;
    }

    let _ = fs::remove_file(&tar);
    log(format!("SIF готов: {}", sif.display()));
    Ok(())
}

/// Проверить, поддерживает ли `apptainer build` флаг `--startscript`.
fn apptainer_supports_startscript() -> bool {
    Command::new("apptainer")
        .args(["build", "--help"])
        .output()
        .ok()
        .map(|o| {
            let s = String::from_utf8_lossy(&o.stdout);
            s.contains("--startscript")
        })
        .unwrap_or(false)
}

/// Fallback для Apptainer без `--startscript`: собрать SIF как sandbox,
/// переписать `/.singularity.d/startscript`, упаковать обратно в SIF.
fn patch_startscript_via_sandbox(sif: &Path, ss_body: &str, name: &str, dir: &Path) -> Result<()> {
    let sandbox = dir.join(format!(".{name}.sandbox.tmp"));
    let _ = fs::remove_dir_all(&sandbox);

    // 1) Разбираем SIF в sandbox. --force перезапишет, если есть.
    //    Сначала соберём SIF штатно, потом `apptainer build --sandbox` из него.
    //    Напрямую `build --sandbox` из oci-archive тоже работает,
    //    но оставляем двухшаговую схему для предсказуемости.
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
    };
    let code = bundle::commands::build(args)?;
    if code != 0 {
        bail!("bundle build завершился с кодом {code}");
    }
    Ok(())
}

fn cmd_run(spec_path: &Path) -> Result<()> {
    match spec_kind(spec_path)? {
        SpecKind::App => cmd_run_app(spec_path),
        SpecKind::Bundle => cmd_run_bundle(spec_path),
    }
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

            // Проверим, что instance действительно существует —
            // иначе `apptainer instance stop` вернёт ошибку, и это
            // выглядит пугающе для пользователя, который просто
            // «запустил stop на всякий случай».
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
