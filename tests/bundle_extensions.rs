//! Bundle extensions end-to-end: подстановки в install-команде,
//! распаковка .vsix в каталог, bind на `mount_dir`.
//!
//! Покрытие:
//!   * `type = "file"` (например .oxt) — install исполняется
//!     через `yes yes | …` (auto-accept EULA), bind на custom mount_dir.
//!   * `type = "vsix"` — распаковка в каталог `<id>-<ver>/`, install
//!     НЕ исполняется (VSCodium подхватывает каталоги сам), bind на
//!     целевой `--extensions-dir` (`/config/home/.vscodium/extensions`).
//!   * смешанный сценарий — install только для file.
//!
//! Тесты, требующие `zip`/`unzip`, мягко пропускаются через `return`
//! (НЕ `process::exit(0)` — иначе пропадают все остальные тесты).

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use tempfile::TempDir;

fn appspec() -> Command {
    Command::cargo_bin("appspec").expect("appspec binary built")
}

fn which(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
}

fn has_zip_tools() -> bool {
    which("zip") && which("unzip")
}

fn write_bundle(
    dir: &Path,
    name: &str,
    extensions_block: &str,
    extensions_section: &str,
) -> PathBuf {
    let spec = dir.join("bundle.appspec");
    let body = format!(
        r#"
[bundle]
name = "{name}"

[bundle.base]
sif   = "./base.sif"
entry = "exec /bin/true"

{extensions_section}

{extensions_block}
"#
    );
    fs::write(&spec, body).unwrap();
    fs::write(dir.join("base.sif"), b"fake-sif-bytes").unwrap();
    spec
}

fn cached_exts(cache_dir: &Path, bundle_name: &str) -> PathBuf {
    cache_dir.join(bundle_name).join("extensions")
}

fn read_launcher(dir: &Path, bundle_name: &str) -> String {
    fs::read_to_string(dir.join(format!("run.{bundle_name}.sh"))).unwrap()
}

/// Собирает минимальный `.vsix` в `dst` с `extension/package.json`.
///
/// Требует `zip` в PATH — вызывающий обязан сделать
/// `if !has_zip_tools() { return; }` до вызова.
fn make_minimal_vsix(dir: &Path, publisher: &str, name: &str, version: &str) -> PathBuf {
    assert!(has_zip_tools(), "make_minimal_vsix requires zip/unzip");
    let build = dir.join("vsix-build");
    let ext = build.join("extension");
    fs::create_dir_all(&ext).unwrap();
    let pkg = format!(r#"{{"publisher":"{publisher}","name":"{name}","version":"{version}"}}"#);
    fs::write(ext.join("package.json"), pkg).unwrap();
    let out = dir.join("demo.vsix");
    let st = StdCommand::new("zip")
        .current_dir(&build)
        .args(["-qr"])
        .arg(&out)
        .arg("extension")
        .status()
        .expect("spawn zip");
    assert!(st.success(), "zip failed");
    out
}

// ===========================================================================
// type = "file" (.oxt): install исполняется, bind на custom mount_dir
// ===========================================================================

#[test]
fn file_extension_install_substitutes_target_and_has_yes_prefix() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    fs::write(tmp.path().join("demo.oxt"), b"fake-oxt").unwrap();

    let spec = write_bundle(
        tmp.path(),
        "testfile",
        r#"
[[extensions]]
name = "demo"
file = "./demo.oxt"
type = "file"
"#,
        r#"
[bundle.extensions]
mount_dir = "/opt/extensions"
install   = "tool-install --input {target} --id {id} --ver {version} --legacy {file}"
"#,
    );

    appspec()
        .env("APPSPEC_BUNDLE_CACHE", &cache)
        .args(["bundle", "build", spec.to_str().unwrap()])
        .assert()
        .success();

    let launcher = read_launcher(tmp.path(), "testfile");

    assert!(
        launcher.contains(
            "tool-install --input /opt/extensions/demo.oxt --id demo --ver 1 --legacy /opt/extensions/demo.oxt"
        ),
        "install command must substitute all placeholders, got:\n{launcher}"
    );
    assert!(
        !launcher.contains("--input {target}"),
        "literal {{target}} left:\n{launcher}"
    );
    assert!(
        !launcher.contains("--input {file}"),
        "literal {{file}} left:\n{launcher}"
    );
    assert!(
        launcher.contains("yes yes 2>/dev/null | tool-install --input /opt/extensions/demo.oxt"),
        "install must be piped through `yes yes 2>/dev/null |`:\n{launcher}"
    );
    assert!(
        launcher.contains("--bind $BUNDLE_DIR/extensions:/opt/extensions"),
        "custom mount_dir bind missing:\n{launcher}"
    );
}

// ===========================================================================
// default mount_dir = /config/home/.vscodium/extensions
// ===========================================================================

#[test]
fn default_mount_dir_is_vscodium_extensions() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    fs::write(tmp.path().join("x.oxt"), b"x").unwrap();

    let spec = write_bundle(
        tmp.path(),
        "testdefault",
        r#"
[[extensions]]
name = "x"
file = "./x.oxt"
type = "file"
"#,
        "",
    );

    appspec()
        .env("APPSPEC_BUNDLE_CACHE", &cache)
        .args(["bundle", "build", spec.to_str().unwrap()])
        .assert()
        .success();

    let launcher = read_launcher(tmp.path(), "testdefault");
    assert!(
        launcher.contains("--bind $BUNDLE_DIR/extensions:/config/home/.vscodium/extensions"),
        "default extensions bind missing:\n{launcher}"
    );
}

#[test]
fn custom_mount_dir_overrides_default_bind() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    fs::write(tmp.path().join("x.oxt"), b"x").unwrap();

    let spec = write_bundle(
        tmp.path(),
        "testcustom",
        r#"
[[extensions]]
name = "x"
file = "./x.oxt"
type = "file"
"#,
        r#"
[bundle.extensions]
mount_dir = "/opt/custom-ext"
"#,
    );

    appspec()
        .env("APPSPEC_BUNDLE_CACHE", &cache)
        .args(["bundle", "build", spec.to_str().unwrap()])
        .assert()
        .success();

    let launcher = read_launcher(tmp.path(), "testcustom");
    assert!(launcher.contains("--bind $BUNDLE_DIR/extensions:/opt/custom-ext"));
    assert!(!launcher.contains("/config/home/.vscodium/extensions"));
}

// ===========================================================================
// type = "vsix": распаковка в каталог, install НЕ исполняется
// ===========================================================================
//
// VSCodium подхватывает каталоги `<id>-<ver>/` из `--extensions-dir`
// сам. `codium --install-extension` не нужен и в headless-режиме зависает
// в GUI. Поэтому codegen НЕ генерирует `install:` для vsix.

#[test]
fn vsix_extension_extracts_to_dir_and_skips_install() {
    if !has_zip_tools() {
        eprintln!("SKIP: zip/unzip не в PATH");
        return;
    }
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    make_minimal_vsix(tmp.path(), "demo", "widget", "1.0.0");

    // Даже если install-шаблон задан, для vsix он не должен исполняться.
    let spec = write_bundle(
        tmp.path(),
        "testvsix",
        r#"
[[extensions]]
name = "widget"
file = "./demo.vsix"
type = "vsix"
"#,
        r#"
[bundle.extensions]
install = "codium --install-extension {target} --force"
"#,
    );

    appspec()
        .env("APPSPEC_BUNDLE_CACHE", &cache)
        .args(["bundle", "build", spec.to_str().unwrap()])
        .assert()
        .success();

    let exts = cached_exts(&cache, "testvsix");

    // 1. В кэше — КАТАЛОГ `<id>-<ver>/`, а не `.vsix`-файл под тем же именем.
    let extracted = exts.join("demo.widget-1.0.0");
    assert!(
        extracted.is_dir(),
        "expected extracted dir {extracted:?}, got: {:?}",
        fs::read_dir(&exts).map(|it| it
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>())
    );
    assert!(
        extracted.join("package.json").is_file(),
        "extracted dir must contain package.json (contents of extension/)"
    );

    // 2. Исходный .vsix (в спеке `file = "./demo.vsix"`, т.е. рядом
    //    со спеком, а НЕ в кэше) не тронут: prepare_as_archive его
    //    только читает. Попадание в `exts/` — это поведение для
    //    `url =` / online-источников; для локального `file =` файл
    //    остаётся там, где лежал.
    assert!(
        tmp.path().join("demo.vsix").is_file(),
        "source .vsix (relative to spec) must remain untouched"
    );
    assert!(
        !exts.join("demo.vsix").exists(),
        "local `file =` source must not be cached into exts/"
    );

    // 3. Launcher НЕ содержит `install:` — ни `codium --install-extension`,
    //    ни вообще `yes yes | …`.
    let launcher = read_launcher(tmp.path(), "testvsix");
    assert!(
        !launcher.contains("install:"),
        "vsix must NOT produce install command, got:\n{launcher}"
    );
    assert!(
        !launcher.contains("codium --install-extension"),
        "codium --install-extension must not appear in launcher:\n{launcher}"
    );

    // 4. Bind на default mount_dir (без [bundle.extensions].mount_dir).
    assert!(
        launcher.contains("--bind $BUNDLE_DIR/extensions:/config/home/.vscodium/extensions"),
        "default extensions bind missing:\n{launcher}"
    );

    // 5. Печатается ready с пометкой (vsix).
    assert!(
        launcher.contains("ready: demo.widget v1.0.0 (vsix)"),
        "missing `ready:` line, got:\n{launcher}"
    );
}

#[test]
fn vsix_with_custom_mount_dir_still_skips_install() {
    if !has_zip_tools() {
        eprintln!("SKIP: zip/unzip не в PATH");
        return;
    }
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    make_minimal_vsix(tmp.path(), "demo", "widget", "1.0.0");

    let spec = write_bundle(
        tmp.path(),
        "testvsixdir",
        r#"
[[extensions]]
name = "widget"
file = "./demo.vsix"
type = "vsix"
"#,
        r#"
[bundle.extensions]
mount_dir = "/opt/extensions"
install   = "codium --install-extension {target}"
"#,
    );

    appspec()
        .env("APPSPEC_BUNDLE_CACHE", &cache)
        .args(["bundle", "build", spec.to_str().unwrap()])
        .assert()
        .success();

    let launcher = read_launcher(tmp.path(), "testvsixdir");
    // install для vsix всё равно не генерируется.
    assert!(!launcher.contains("install:"));
    // mount_dir работает для bind.
    assert!(launcher.contains("--bind $BUNDLE_DIR/extensions:/opt/extensions"));
}

// ===========================================================================
// смешанный сценарий: file + vsix в одном бандле
// ===========================================================================

#[test]
fn mix_file_and_vsix_install_only_for_file() {
    if !has_zip_tools() {
        eprintln!("SKIP: zip/unzip не в PATH");
        return;
    }
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    fs::write(tmp.path().join("demo.oxt"), b"oxt").unwrap();
    make_minimal_vsix(tmp.path(), "demo", "widget", "1.0.0");

    let spec = write_bundle(
        tmp.path(),
        "testmix",
        r#"
[[extensions]]
name = "grammalecte"
file = "./demo.oxt"
type = "file"

[[extensions]]
name = "widget"
file = "./demo.vsix"
type = "vsix"
"#,
        r#"
[bundle.extensions]
mount_dir = "/opt/extensions"
install   = "tool {target}"
"#,
    );

    appspec()
        .env("APPSPEC_BUNDLE_CACHE", &cache)
        .args(["bundle", "build", spec.to_str().unwrap()])
        .assert()
        .success();

    let launcher = read_launcher(tmp.path(), "testmix");

    // Для file — install есть, через `yes yes |`.
    assert!(
        launcher.contains("yes yes 2>/dev/null | tool /opt/extensions/demo.oxt"),
        "file install missing:\n{launcher}"
    );
    // Для vsix — install нет.
    assert!(
        !launcher.contains("tool /opt/extensions/demo.widget-1.0.0"),
        "vsix must not produce install command:\n{launcher}"
    );
    // В кэше: file — файл, vsix — каталог.
    let exts = cached_exts(&cache, "testmix");
    assert!(exts.join("demo.oxt").is_file());
    assert!(exts.join("demo.widget-1.0.0").is_dir());

    // ready для обоих, с разными kind.
    assert!(launcher.contains("ready: grammalecte v1 (file)"));
    assert!(launcher.contains("ready: demo.widget v1.0.0 (vsix)"));
}

// ===========================================================================
// preflight не ругается на уже скачанное расширение
// ===========================================================================

#[test]
fn preflight_no_warn_after_extension_prepared() {
    let tmp = TempDir::new().unwrap();
    let cache = tmp.path().join("cache");
    fs::create_dir_all(&cache).unwrap();
    fs::write(tmp.path().join("demo.oxt"), b"x").unwrap();

    let spec = write_bundle(
        tmp.path(),
        "testpf",
        r#"
[[extensions]]
name = "demo"
file = "./demo.oxt"
type = "file"
"#,
        "",
    );

    appspec()
        .env("APPSPEC_BUNDLE_CACHE", &cache)
        .args(["bundle", "build", spec.to_str().unwrap()])
        .assert()
        .success();

    let out = appspec()
        .env("APPSPEC_BUNDLE_CACHE", &cache)
        .args(["bundle", "check", spec.to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();
    let s = String::from_utf8_lossy(&out);
    assert!(!s.contains("demo.oxt не скачан"));
}
