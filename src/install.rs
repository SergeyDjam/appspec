use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::spec::Spec;
use crate::templates;

// --- scope -----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Scope {
    User,
    Global,
    Custom,
}

impl Scope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Scope::User => "user",
            Scope::Global => "global",
            Scope::Custom => "custom",
        }
    }
}

/// Разрешённые абсолютные пути для конкретного scope.
pub struct Paths {
    pub sif_dir: PathBuf,
    pub bin_dir: PathBuf,
    pub desktop_dir: PathBuf,
    pub icon_dir: PathBuf,
    pub state_dir: PathBuf,
}

pub fn resolve_paths(spec: &Spec, scope: Scope) -> Result<Paths> {
    let home = dirs_home()?;
    let xdg_data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
    let xdg_bin = std::env::var_os("XDG_BIN_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/bin"));

    let name = &spec.package.name;
    let inst = &spec.install;
    let ov = match scope {
        Scope::User | Scope::Custom => &inst.user,
        Scope::Global => &inst.global,
    };

    let (sif_dir, bin_dir, desktop_dir, icon_dir, state_dir) = match scope {
        Scope::User => (
            xdg_data.join("apptainer"),
            xdg_bin.clone(),
            xdg_data.join("applications"),
            xdg_data.join("icons/hicolor"),
            xdg_data.join(format!("appspec/{name}")),
        ),
        Scope::Global => (
            PathBuf::from("/usr/local/share/apptainer"),
            PathBuf::from("/usr/local/bin"),
            PathBuf::from("/usr/share/applications"),
            PathBuf::from("/usr/share/icons/hicolor"),
            PathBuf::from(format!("/var/lib/appspec/{name}")),
        ),
        Scope::Custom => {
            let sif_dir = inst
                .sif_dir
                .clone()
                .ok_or_else(|| anyhow!("scope = custom требует [install].sif_dir"))?;
            let bin_dir = ov.bin_dir.clone().unwrap_or_else(|| sif_dir.join("bin"));
            let desktop_dir = ov
                .desktop_dir
                .clone()
                .unwrap_or_else(|| xdg_data.join("applications"));
            let icon_dir = ov
                .icon_dir
                .clone()
                .unwrap_or_else(|| xdg_data.join("icons/hicolor"));
            let state_dir = ov
                .state_dir
                .clone()
                .unwrap_or_else(|| sif_dir.join(format!("{name}-state")));
            (sif_dir, bin_dir, desktop_dir, icon_dir, state_dir)
        }
    };

    Ok(Paths {
        sif_dir: ov.sif_dir.clone().unwrap_or(sif_dir),
        bin_dir: ov.bin_dir.clone().unwrap_or(bin_dir),
        desktop_dir: ov.desktop_dir.clone().unwrap_or(desktop_dir),
        icon_dir: ov.icon_dir.clone().unwrap_or(icon_dir),
        state_dir: ov.state_dir.clone().unwrap_or(state_dir),
    })
}

fn dirs_home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("$HOME не установлен"))
}

// --- манифест --------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub scope: String,
    pub installed_at: String,
    pub launcher: PathBuf,
    pub sif: PathBuf,
    pub files: Vec<PathBuf>,
}

impl Manifest {
    fn path_in(state_dir: &Path) -> PathBuf {
        state_dir.join("manifest.json")
    }

    pub fn save(&self, state_dir: &Path) -> Result<()> {
        fs::create_dir_all(state_dir)?;
        let text = serde_json::to_string_pretty(self)?;
        fs::write(Self::path_in(state_dir), text)?;
        Ok(())
    }

    pub fn load(state_dir: &Path) -> Result<Self> {
        let text = fs::read_to_string(Self::path_in(state_dir))
            .with_context(|| format!("manifest не найден в {}", state_dir.display()))?;
        Ok(serde_json::from_str(&text)?)
    }
}

// --- установка -------------------------------------------------------------

pub struct InstallOpts {
    pub scope: Scope,
    pub with_desktop: bool,
    pub with_ipfs: bool,
}

pub fn install(spec_path: &Path, opts: InstallOpts) -> Result<Manifest> {
    let spec = crate::load_spec(spec_path)?;
    let spec_dir = crate::spec_dir(spec_path);
    let sif_src = spec_dir.join(spec.sif_file());
    if !sif_src.is_file() {
        bail!(
            "SIF не найден: {}\nСначала: appspec build {}",
            sif_src.display(),
            spec_path.display()
        );
    }

    let paths = resolve_paths(&spec, opts.scope)?;
    let name = &spec.package.name;

    // 1. каталоги
    for d in [&paths.sif_dir, &paths.bin_dir, &paths.state_dir] {
        fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
    }

    let mut installed: Vec<PathBuf> = Vec::new();

    // 2. SIF
    let sif_dst = paths.sif_dir.join(format!("{name}.sif"));
    copy(&sif_src, &sif_dst)?;
    installed.push(sif_dst.clone());

    // 3. setup.sh внутри state-dir
    let setup_path = paths.state_dir.join("setup.sh");
    let setup = spec.runtime.setup.clone().unwrap_or_default();
    let entry = spec
        .runtime
        .entry
        .clone()
        .unwrap_or_else(|| format!("exec {name} \"$@\""));
    let setup_script = format!(
        "#!/bin/sh\nset -eu\n{}\n{}\n",
        setup.trim_end(),
        entry.trim_end()
    );
    fs::write(&setup_path, setup_script)?;
    set_exec(&setup_path)?;
    installed.push(setup_path.clone());

    // 4. launcher
    let launcher_name = spec
        .install
        .launcher_name
        .clone()
        .unwrap_or_else(|| name.clone());
    let launcher = paths.bin_dir.join(&launcher_name);
    let launcher_src = templates::render_installed_launcher(
        &spec,
        &sif_dst,
        &setup_path,
        &paths.state_dir,
        opts.scope,
    )?;
    fs::write(&launcher, launcher_src)?;
    set_exec(&launcher)?;
    installed.push(launcher.clone());

    // 5. desktop + icons
    if opts.with_desktop && spec.install.desktop.unwrap_or(false) {
        fs::create_dir_all(&paths.desktop_dir)?;

        let desktop = paths.desktop_dir.join(format!("{name}.desktop"));
        let desktop_content = templates::render_desktop(&spec, &launcher, opts.scope)?;
        fs::write(&desktop, desktop_content)?;
        installed.push(desktop);

        // E-icon: primary — SIF-метки; fallback — spec_dir.
        // PNG/JPEG ресайзятся в 16/22/24/32/48/64/128/256, SVG — в scalable/apps.
        let icon_files = install_icons(&spec, &spec_dir, &sif_dst, &paths.icon_dir)?;
        installed.extend(icon_files);
    }

    // 6. extra_files
    for ef in &spec.install.extra_files {
        let src = spec_dir.join(&ef.from);
        if !src.is_file() {
            bail!("extra_files: источник не найден: {}", src.display());
        }
        let dst = PathBuf::from(
            ef.to
                .replace("{{name}}", name)
                .replace("{{sif}}", &paths.sif_dir.display().to_string())
                .replace("{{bin}}", &paths.bin_dir.display().to_string())
                .replace("{{desktop}}", &paths.desktop_dir.display().to_string())
                .replace("{{icons}}", &paths.icon_dir.display().to_string())
                .replace("{{state}}", &paths.state_dir.display().to_string()),
        );
        if let Some(p) = dst.parent() {
            fs::create_dir_all(p)?;
        }
        copy(&src, &dst)?;
        installed.push(dst);
    }

    // 7. IPFS
    if opts.with_ipfs || spec.install.ipfs.enabled.unwrap_or(false) {
        let cid = ipfs_publish(&sif_dst, spec.install.ipfs.pin.unwrap_or(true))?;
        println!("IPFS CID: {cid}");
        let cid_path = paths.state_dir.join("ipfs.cid");
        fs::write(&cid_path, &cid)?;
        installed.push(cid_path);
    }

    // 8. манифест
    let manifest = Manifest {
        name: name.clone(),
        version: spec.package.version.clone(),
        scope: opts.scope.as_str().into(),
        installed_at: now_iso8601(),
        launcher,
        sif: sif_dst,
        files: installed,
    };
    manifest.save(&paths.state_dir)?;

    // 9. обновление кэшей рабочего стола (best effort)
    if opts.with_desktop && spec.install.desktop.unwrap_or(false) {
        let _ = Command::new("update-desktop-database")
            .arg(&paths.desktop_dir)
            .status();
        let _ = Command::new("gtk-update-icon-cache")
            .arg(&paths.icon_dir)
            .status();
    }

    Ok(manifest)
}

// --- удаление --------------------------------------------------------------

pub fn uninstall(spec_path: &Path, scope: Scope) -> Result<()> {
    let spec = crate::load_spec(spec_path)?;
    let paths = resolve_paths(&spec, scope)?;
    let manifest = Manifest::load(&paths.state_dir)
        .with_context(|| format!("manifest не найден в {}", paths.state_dir.display()))?;

    for f in &manifest.files {
        if f.exists() {
            if f.is_dir() {
                let _ = fs::remove_dir_all(f);
            } else {
                let _ = fs::remove_file(f);
            }
            eprintln!("\x1b[1;36m[appspec]\x1b[0m removed {}", f.display());
        }
    }
    if paths.state_dir.exists() {
        let _ = fs::remove_dir_all(&paths.state_dir);
    }
    Ok(())
}

// --- вспомогательные -------------------------------------------------------

fn copy(from: &Path, to: &Path) -> Result<()> {
    if let Some(p) = to.parent() {
        fs::create_dir_all(p)?;
    }
    fs::copy(from, to).with_context(|| format!("copy {} → {}", from.display(), to.display()))?;
    Ok(())
}

#[cfg(unix)]
fn set_exec(p: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut m = fs::metadata(p)?.permissions();
    m.set_mode(0o755);
    fs::set_permissions(p, m)?;
    Ok(())
}
#[cfg(not(unix))]
fn set_exec(_: &Path) -> Result<()> {
    Ok(())
}

fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let s = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("@{s}")
}

pub fn ipfs_publish(path: &Path, pin: bool) -> Result<String> {
    let mut cmd = Command::new("ipfs");
    cmd.args(["add", "--quieter"]);
    cmd.arg(if pin { "--pin=true" } else { "--pin=false" });
    cmd.arg(path);
    let out = cmd.output().context("не удалось запустить `ipfs`")?;
    if !out.status.success() {
        bail!(
            "ipfs add завершился с ошибкой: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}

// --- E-icon: установка иконки (SIF → spec_dir, resize, раскладка) ---------

fn resolve_icon(
    spec: &Spec,
    spec_dir: &Path,
    sif_path: &Path,
) -> Result<(Vec<u8>, String, String)> {
    // 1. SIF labels (primary)
    if sif_path.is_file() {
        if let Ok(labels) = crate::repo::inspect::inspect(sif_path) {
            if let Some(b64) = labels.get("org.appspec.icon.content.base64") {
                use base64::Engine as _;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .with_context(|| "decoding org.appspec.icon.content.base64")?;
                let filename = labels
                    .get("org.appspec.icon.filename")
                    .map(str::to_string)
                    .unwrap_or_else(|| "icon.png".into());
                let mimetype = labels
                    .get("org.appspec.icon.mimetype")
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        if filename.to_ascii_lowercase().ends_with(".svg") {
                            "image/svg+xml".into()
                        } else {
                            "image/png".into()
                        }
                    });
                return Ok((bytes, filename, mimetype));
            }
        }
    }

    // 2. spec_dir (fallback)
    let icon_rel = spec
        .install
        .icon
        .as_ref()
        .ok_or_else(|| anyhow!("[install].icon не задан (и в SIF нет меток)"))?;
    let icon_src = spec_dir.join(icon_rel);
    if !icon_src.is_file() {
        bail!(
            "иконка не найдена: ни в SIF (labels), ни в {}",
            icon_src.display()
        );
    }
    let bytes =
        fs::read(&icon_src).with_context(|| format!("reading icon {}", icon_src.display()))?;
    let filename = icon_src
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "icon.png".to_string());
    let lower = filename.to_ascii_lowercase();
    let mimetype = if lower.ends_with(".svg") {
        "image/svg+xml".to_string()
    } else if lower.ends_with(".png") {
        "image/png".to_string()
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg".to_string()
    } else {
        "application/octet-stream".to_string()
    };
    Ok((bytes, filename, mimetype))
}

fn install_icons(
    spec: &Spec,
    spec_dir: &Path,
    sif_path: &Path,
    icon_dir: &Path,
) -> Result<Vec<PathBuf>> {
    // [install].icon не задан — иконки не нужны, не ошибка.
    if spec.install.icon.is_none() {
        return Ok(Vec::new());
    }

    let (bytes, filename, mimetype) = resolve_icon(spec, spec_dir, sif_path)?;
    let name = &spec.package.name;
    let mut installed: Vec<PathBuf> = Vec::new();

    // SVG — только scalable/apps, без растровых вариантов.
    if mimetype == "image/svg+xml" || filename.to_ascii_lowercase().ends_with(".svg") {
        let dst = icon_dir.join("scalable/apps").join(format!("{}.svg", name));
        if let Some(p) = dst.parent() {
            fs::create_dir_all(p)?;
        }
        fs::write(&dst, &bytes).with_context(|| format!("writing {}", dst.display()))?;
        installed.push(dst);
        return Ok(installed);
    }

    // PNG/JPEG — resize в стандартные размеры.
    if mimetype == "image/png" || mimetype == "image/jpeg" {
        let img = image::load_from_memory(&bytes)
            .with_context(|| format!("decoding {} icon for {}", mimetype, name))?;
        for &size in &[16u32, 22, 24, 32, 48, 64, 128, 256] {
            let resized = img.resize_exact(size, size, image::imageops::FilterType::Lanczos3);
            let dst = icon_dir
                .join(format!("{}x{}/apps", size, size))
                .join(format!("{}.png", name));
            if let Some(p) = dst.parent() {
                fs::create_dir_all(p)?;
            }
            resized
                .save_with_format(&dst, image::ImageFormat::Png)
                .with_context(|| format!("writing {}", dst.display()))?;
            installed.push(dst);
        }
        return Ok(installed);
    }

    // Неизвестный формат — копируем как есть в 256x256.
    let ext = filename.rsplit_once('.').map(|(_, e)| e).unwrap_or("png");
    let dst = icon_dir
        .join("256x256/apps")
        .join(format!("{}.{}", name, ext));
    if let Some(p) = dst.parent() {
        fs::create_dir_all(p)?;
    }
    fs::write(&dst, &bytes).with_context(|| format!("writing {}", dst.display()))?;
    installed.push(dst);
    Ok(installed)
}
