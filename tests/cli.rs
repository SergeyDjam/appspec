use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

fn appspec() -> Command {
    Command::cargo_bin("appspec").expect("appspec binary built")
}

#[test]
fn help_lists_all_commands() {
    let mut cmd = appspec();
    cmd.arg("--help");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("gen"))
        .stdout(predicate::str::contains("build"))
        .stdout(predicate::str::contains("run"))
        .stdout(predicate::str::contains("stop"))
        .stdout(predicate::str::contains("all"))
        .stdout(predicate::str::contains("clean"))
        .stdout(predicate::str::contains("install"))
        .stdout(predicate::str::contains("bundle"));
}

#[test]
fn gen_errors_on_missing_spec() {
    let mut cmd = appspec();
    cmd.args(["gen", "/tmp/nonexistent-987654321.appspec"]);
    cmd.assert().failure();
}

#[test]
fn gen_rejects_spec_without_package_or_bundle() {
    let dir = TempDir::new().unwrap();
    let spec = dir.path().join("empty.appspec");
    fs::write(&spec, "[nothing]\nfoo = 1\n").unwrap();

    let mut cmd = appspec();
    cmd.args(["gen", spec.to_str().unwrap()]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("ни [package], ни [bundle]"));
}

#[test]
fn gen_rejects_spec_with_both() {
    let dir = TempDir::new().unwrap();
    let spec = dir.path().join("both.appspec");
    fs::write(
        &spec,
        "[package]\nname = \"x\"\nversion = \"1\"\n\n[bundle]\nname = \"x\"\n",
    )
    .unwrap();

    let mut cmd = appspec();
    cmd.args(["gen", spec.to_str().unwrap()]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("и [bundle], и [package]"));
}

#[test]
fn gen_produces_dockerfile_and_launcher() {
    let dir = TempDir::new().unwrap();
    let spec = dir.path().join("demo.appspec");
    fs::write(
        &spec,
        r#"
[package]
name    = "demo"
version = "0.0.1"

[build]
base     = "alpine:3.21"
template = "alpine"

[build.apt]
extra = ["ca-certificates"]

[build.collect]
binary = "/usr/bin/demo"

[build.scratch]
workdir = "/work"
cmd     = ["/usr/bin/demo", "--version"]

[runtime]
mode    = "cli"
command = "/usr/bin/demo"
"#,
    )
    .unwrap();

    let mut cmd = appspec();
    cmd.args(["gen", spec.to_str().unwrap()]);
    cmd.assert().success();

    let df = dir.path().join("Dockerfile.demo");
    let sh = dir.path().join("run.demo.sh");

    assert!(df.is_file(), "Dockerfile.demo missing");
    assert!(sh.is_file(), "run.demo.sh missing");

    let df_text = fs::read_to_string(&df).unwrap();
    // base либо квалифицирован (docker.io/library/alpine:3.21),
    // либо короткий (alpine:3.21). Оба допустимы: qualify_base_image()
    // опционален, Docker принимает короткие имена.
    let has_base = df_text.contains("FROM docker.io/library/alpine:3.21")
        || df_text.contains("FROM alpine:3.21");
    assert!(has_base, "expected alpine base, got:\n{df_text}");
    assert!(
        df_text.contains("apk --root /bundle"),
        "expected apk --root, got:\n{df_text}"
    );

    let sh_text = fs::read_to_string(&sh).unwrap();
    assert!(sh_text.contains("exec apptainer exec"));
    assert!(sh_text.contains("--containall"));
    assert!(sh_text.contains("/usr/bin/demo"));
}

#[test]
fn gen_applies_macros() {
    let dir = TempDir::new().unwrap();
    let spec = dir.path().join("macros.appspec");
    fs::write(
        &spec,
        r#"
[package]
name    = "web"
version = "1"
bin     = "nginx"

[build]
base     = "alpine:3.21"
template = "alpine"

[build.apt]
extra = ["%{bin}", "ca-certificates"]

[build.collect]
binary = "%{_sbindir}/%{bin}"

[build.scratch]
workdir = "/srv"
cmd     = ["%{_sbindir}/%{bin}", "-g", "daemon off;"]

[runtime]
sif_file = "web.sif"
"#,
    )
    .unwrap();

    let mut cmd = appspec();
    cmd.args(["gen", spec.to_str().unwrap()]);
    cmd.assert().success();

    let df_text = fs::read_to_string(dir.path().join("Dockerfile.web")).unwrap();
    assert!(df_text.contains("nginx"));
    assert!(df_text.contains("/usr/sbin/nginx"));
    assert!(!df_text.contains("%{bin}"));
    assert!(!df_text.contains("%{_sbindir}"));
}

#[test]
fn gen_handles_deb_template_with_source() {
    let dir = TempDir::new().unwrap();
    let spec = dir.path().join("deb.appspec");
    fs::write(
        &spec,
        r#"
[package]
name    = "foo"
version = "1.0"

[build]
base     = "debian:trixie"
template = "deb"

[build.source]
type = "deb"
url  = "https://example.invalid/foo_{version}_amd64.deb"

[build.collect]
binary = "/usr/bin/foo"

[build.scratch]
cmd = ["/usr/bin/foo"]

[runtime]
"#,
    )
    .unwrap();

    let mut cmd = appspec();
    cmd.args(["gen", spec.to_str().unwrap()]);
    cmd.assert().success();

    let df_text = fs::read_to_string(dir.path().join("Dockerfile.foo")).unwrap();
    assert!(
        df_text.contains("foo_1.0_amd64.deb"),
        "expected version-substituted URL, got:\n{df_text}"
    );
    assert!(df_text.contains("apt-get satisfy"));
    assert!(df_text.contains("magicpak"));
}

#[test]
fn bundle_check_reports_missing_source() {
    let dir = TempDir::new().unwrap();
    let spec = dir.path().join("bundle.appspec");
    fs::write(
        &spec,
        r#"
[bundle]
name = "demo"
base = { spec = "./missing.appspec" }
"#,
    )
    .unwrap();

    let mut cmd = appspec();
    cmd.args(["bundle", "check", spec.to_str().unwrap()]);
    cmd.assert().code(2);
}

#[test]
fn bundle_rejects_both_package_and_bundle() {
    let dir = TempDir::new().unwrap();
    let spec = dir.path().join("bad.appspec");
    fs::write(
        &spec,
        "[package]\nname=\"x\"\nversion=\"1\"\n\n[bundle]\nname=\"y\"\n",
    )
    .unwrap();

    let mut cmd = appspec();
    cmd.args(["bundle", "check", spec.to_str().unwrap()]);
    cmd.assert().failure();
}
