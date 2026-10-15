use anyhow::Result;
use std::fmt::Write as _;

use crate::templates::sh_quote;

use super::model::{
    BundleFile, Component, FileScope, PreparedExtension, ResolvedBase, ResolvedFile,
};

// ===========================================================================
// Bridge (chroot mode)
// ===========================================================================

fn emit_bridge(c: &Component, mount: &str) -> String {
    let provide = c.provide();
    let binary = c.binary();
    let target = c.target();
    let forward = c.forward();
    let env = c.env_str();
    let mounts = c.mounts();

    let tag = c
        .name
        .to_uppercase()
        .replace(|ch: char| !ch.is_ascii_alphanumeric(), "_");
    let mut s = String::new();
    let w = &mut s;

    let _ = writeln!(w, "# ---- bridge: {} -> {} ----", c.name, provide);
    let _ = writeln!(w, "cat > {} <<'__BRIDGE_{}__'", sh_quote(&target), tag);
    let _ = writeln!(w, "#!/bin/sh");
    let _ = writeln!(w, "# appspec bridge: {provide} -> {name}", name = c.name);
    let _ = writeln!(w, "set -eu");
    let _ = writeln!(w);
    let _ = writeln!(w, "SANDBOX={}", sh_quote(mount));
    let _ =
        writeln!(w,
        "[ -d \"$SANDBOX\" ] || {{ echo \"bridge[{name}]: $SANDBOX отсутствует\" >&2; exit 127; }}",
        name = c.name);
    let _ = writeln!(w);
    let _ = writeln!(w, "PWD_REAL=\"$(pwd -P)\"");
    let _ = writeln!(w, "HOME_REAL=\"${{HOME:-/config/home}}\"");
    let _ = writeln!(w);
    let _ = writeln!(
        w,
        "mkdir -p \"$SANDBOX$PWD_REAL\" \"$SANDBOX$HOME_REAL\" 2>/dev/null || true"
    );
    let _ = writeln!(w);

    for m in &mounts {
        match m.as_str() {
            "PWD" => {
                let _ = writeln!(
                    w,
                    "mount --bind \"$PWD_REAL\" \"$SANDBOX$PWD_REAL\" 2>/dev/null || true"
                );
            }
            "HOME" => {
                let _ = writeln!(
                    w,
                    "mount --bind \"$HOME_REAL\" \"$SANDBOX$HOME_REAL\" 2>/dev/null || true"
                );
            }
            "SSH_AUTH_SOCK" => {
                let _ = writeln!(
                    w,
                    "if [ -n \"${{SSH_AUTH_SOCK:-}}\" ] && [ -e \"$SSH_AUTH_SOCK\" ]; then"
                );
                let _ =
                    writeln!(w,
                    "    mkdir -p \"$SANDBOX$(dirname \"$SSH_AUTH_SOCK\")\" 2>/dev/null || true");
                let _ = writeln!(w, "    if [ ! -e \"$SANDBOX$SSH_AUTH_SOCK\" ]; then");
                let _ = writeln!(
                    w,
                    "        ( : > \"$SANDBOX$SSH_AUTH_SOCK\" ) 2>/dev/null || true"
                );
                let _ = writeln!(w, "    fi");
                let _ = writeln!(w,
                    "    mount --bind \"$SSH_AUTH_SOCK\" \"$SANDBOX$SSH_AUTH_SOCK\" 2>/dev/null || true");
                let _ = writeln!(w, "fi");
            }
            abs if abs.starts_with('/') => {
                let _ = writeln!(w, "if [ -d \"{abs}\" ]; then");
                let _ = writeln!(w, "    mkdir -p \"$SANDBOX{abs}\" 2>/dev/null || true");
                let _ = writeln!(
                    w,
                    "    mount --bind \"{abs}\" \"$SANDBOX{abs}\" 2>/dev/null || true"
                );
                let _ = writeln!(w, "elif [ -e \"{abs}\" ]; then");
                let _ = writeln!(w, "    if [ ! -e \"$SANDBOX{abs}\" ]; then");
                let _ = writeln!(
                    w,
                    "        mkdir -p \"$SANDBOX$(dirname \"{abs}\")\" 2>/dev/null || true"
                );
                let _ = writeln!(w, "        ( : > \"$SANDBOX{abs}\" ) 2>/dev/null || true");
                let _ = writeln!(w, "    fi");
                let _ = writeln!(
                    w,
                    "    mount --bind \"{abs}\" \"$SANDBOX{abs}\" 2>/dev/null || true"
                );
                let _ = writeln!(w, "fi");
            }
            _ => {}
        }
    }

    let _ = writeln!(w);
    let _ = writeln!(w, "export HOME=\"$HOME_REAL\"");
    for v in &forward {
        let _ = writeln!(w, "export {0}=\"${{{0}:-}}\"", v);
    }
    for (k, v) in &env {
        let _ = writeln!(w, "export {}={}", k, sh_quote(v));
    }
    let _ = writeln!(w);
    let _ = writeln!(w, "cd \"$PWD_REAL\"");
    let _ = writeln!(w, "exec chroot \"$SANDBOX\" {} \"$@\"", sh_quote(&binary));
    let _ = writeln!(w, "__BRIDGE_{}__", tag);
    let _ = writeln!(w, "chmod 0755 {}", sh_quote(&target));
    s
}

// ===========================================================================
// Файлы
// ===========================================================================

fn dq(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' | '\\' | '`' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn emit_files_for(
    files: &[ResolvedFile],
    scope: FileScope,
    prefix_var: Option<&str>,
    banner: &str,
) -> String {
    let filtered: Vec<&ResolvedFile> = files.iter().filter(|f| f.scopes.contains(&scope)).collect();
    if filtered.is_empty() {
        return format!(": # {banner}: пусто");
    }

    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;

    let mut s = String::new();
    let w = &mut s;
    let _ = writeln!(w, "# ---- {banner} ----");
    for f in filtered {
        let encoded = b64.encode(f.content.as_bytes());
        let t = match prefix_var {
            Some(v) => format!("\"{}{}\"", v, f.to),
            None => dq(&f.to),
        };
        let _ = writeln!(w, "mkdir -p \"$(dirname {})\" 2>/dev/null || true", t);
        let _ = writeln!(w, "printf %s {} | base64 -d > {}", sh_quote(&encoded), t);
        if f.mode != 0o644 {
            let _ = writeln!(w, "chmod {:o} {} || true", f.mode, t);
        }
    }
    s
}

// ===========================================================================
// Fingerprint для state_dir
// ===========================================================================

/// Простой djb2-хеш по содержимому instance-файлов.
/// Используется для сброса state_dir при смене storage.conf.
fn instance_storage_fp(files: &[ResolvedFile]) -> u64 {
    let mut h: u64 = 5381;
    for f in files {
        for b in f.to.bytes() {
            h = h.wrapping_mul(33).wrapping_add(b as u64);
        }
        for b in f.content.bytes() {
            h = h.wrapping_mul(33).wrapping_add(b as u64);
        }
    }
    h
}

// ===========================================================================
// Podman helpers
// ===========================================================================

/// Определяем «это podman-подобный компонент» несколькими независимыми
/// признаками, потому что многие (как автор vscodium-bundle) пишут daemon-
/// обёртку (`/opt/appspec/daemon.sh`), которая сама внутри вызывает
/// `podman system service`, а не подставляют команду podman напрямую.
///
/// Признаки (любой из них — триггер):
///   1. первый токен `daemon` — podman/podman-remote/buildah/docker;
///   2. в env есть CONTAINER_HOST или DOCKER_HOST;
///   3. среди instance-файлов есть /etc/containers/{storage,containers}.conf;
///   4. имя компонента равно podman/buildah/docker.
fn is_podman_like(c: &Component, files: &[ResolvedFile]) -> bool {
    if let Some(d) = c.daemon() {
        let first = d.split_whitespace().next().unwrap_or("");
        let bn = std::path::Path::new(first)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        if matches!(bn, "podman" | "podman-remote" | "buildah" | "docker") {
            return true;
        }
    }
    let env = c.env_str();
    if env.contains_key("CONTAINER_HOST") || env.contains_key("DOCKER_HOST") {
        return true;
    }
    if files.iter().any(|f| {
        f.to == "/etc/containers/storage.conf" || f.to == "/etc/containers/containers.conf"
    }) {
        return true;
    }
    matches!(c.name.as_str(), "podman" | "buildah" | "docker")
}

/// Проверяет, есть ли в argv уже флаг --time/-t (в любой форме: `--time=0`,
/// `--time 0`, `-t=0`, `-t 0`). Именно `--time`, а не `--timeout` или
/// `--time-format` — поэтому split по '=' и точное сравнение.
fn has_time_flag(daemon: &str) -> bool {
    daemon.split_whitespace().any(|tok| {
        let name = tok.split('=').next().unwrap_or("");
        name == "--time" || name == "-t"
    })
}

/// `podman system service` без `--time` завершается через 5 секунд простоя
/// (default в podman). В apptainer instance это убивает PID 1 → instance
/// умирает → сокет исчезает. Если пользователь задал daemon без --time,
/// дописываем `--time=0` (жить вечно). Если --time задан явно (даже 10) —
/// уважаем выбор пользователя.
///
/// Применяется только когда daemon — это непосредственно podman. Для
/// обёрток (`/opt/appspec/daemon.sh`) — ответственность на авторе скрипта.
fn ensure_podman_service_timeout(daemon: &str) -> String {
    let first = daemon.split_whitespace().next().unwrap_or("");
    let bn = std::path::Path::new(first)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if !matches!(bn, "podman" | "podman-remote") {
        return daemon.to_string();
    }
    if !daemon.contains("system service") {
        return daemon.to_string();
    }
    if has_time_flag(daemon) {
        return daemon.to_string();
    }
    format!("{daemon} --time=0")
}

/// Генерируем wrapper-скрипт для daemon podman-компонента. Внутри — фазы:
///
///   0. Ранний redirect в `/srv/podman/wrapper.log` + `set -x`. Так мы
///      гарантированно видим, что wrapper запущен как PID 1, с каким
///      окружением, и что он exec'нул реальный daemon. Файл — на хосте,
///      потому что `/srv/podman` это bind на `$BUNDLE_DIR/run/podman`.
///
///   1. `mount --make-rshared /` (или `rslave` fallback).
///      Apptainer монтирует rootfs как `private`; podman пытается
///      `mount --make-rprivate` на своём graphroot и падает с EINVAL.
///
///   2. Fallback на случай пустого окружения (`--cleanenv`).
///      Если codegen не передал PATH/HOME/TMPDIR — выставляем разумные
///      дефолты. Без этого podman не находит conmon/crun/netavark.
///
///   3. `XDG_RUNTIME_DIR`. Если он задан, но не существует — создаём.
///      Если создать нельзя — падаем в `/tmp/run-user-$(id -u)`.
///
///   4. `exec` реального демона. Проверяем, что файл исполняем; иначе
///      печатаем FATAL с точным путём и выходим с 127.
fn podman_daemon_wrapper(real_daemon: &str, name: &str) -> String {
    let q = sh_quote(real_daemon);
    format!(
        "#!/bin/sh\n\
         # Autogenerated by appspec — DO NOT EDIT.\n\
         # component: {name}\n\
         #\n\
         # 0. Ранний redirect: до всего остального создаём /srv/podman\n\
         #    (bind на $BUNDLE_DIR/run/podman — увидим лог на хосте)\n\
         #    и направляем туда stdout/stderr + set -x.\n\
         mkdir -p /srv/podman 2>/dev/null || true\n\
         if [ -d /srv/podman ]; then\n\
             exec > /srv/podman/wrapper.log 2>&1\n\
         fi\n\
         set -x\n\
         echo \"=== wrapper started: pid=$$ ===\"\n\
         echo \"PATH=$PATH\"\n\
         echo \"HOME=$HOME\"\n\
         echo \"XDG_RUNTIME_DIR=${{XDG_RUNTIME_DIR:-<unset>}}\"\n\
         echo \"user=$(id -u):$(id -g)\"\n\
         \n\
         # 1. mount-fix.\n\
         if mount --make-rshared / 2>/dev/null; then\n\
             echo 'mount --make-rshared ok'\n\
         elif mount --make-rslave / 2>/dev/null; then\n\
             echo 'mount --make-rslave ok'\n\
         else\n\
             echo 'mount-fix: both failed (продолжаем)'\n\
         fi\n\
         \n\
         # 2. Fallback PATH/HOME/TMPDIR.\n\
         export PATH=\"${{PATH:-/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin}}\"\n\
         export HOME=\"${{HOME:-/root}}\"\n\
         export TMPDIR=\"${{TMPDIR:-/tmp}}\"\n\
         \n\
         # 3. XDG_RUNTIME_DIR.\n\
         if [ -z \"${{XDG_RUNTIME_DIR:-}}\" ]; then\n\
             XDG_RUNTIME_DIR=\"/tmp/run-user-$(id -u)\"\n\
         fi\n\
         if ! mkdir -p \"$XDG_RUNTIME_DIR\" 2>/dev/null; then\n\
             XDG_RUNTIME_DIR=\"/tmp/run-user-$(id -u)\"\n\
             mkdir -p \"$XDG_RUNTIME_DIR\" 2>/dev/null || true\n\
         fi\n\
         chmod 0700 \"$XDG_RUNTIME_DIR\" 2>/dev/null || true\n\
         export XDG_RUNTIME_DIR\n\
         echo \"final XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR\"\n\
         \n\
         # 4. Exec реального демона.\n\
         if [ ! -x {daemon} ]; then\n\
             echo \"FATAL: {name}: {daemon} не исполняется (нет файла или не chmod +x)\" >&2\n\
             exit 127\n\
         fi\n\
         echo \"=== exec {daemon} ===\"\n\
         exec {daemon} \"$@\"\n",
        name = name,
        daemon = q
    )
}

// ===========================================================================
// Instance prepare
// ===========================================================================

fn emit_instance_prepare(c: &Component, files: &[ResolvedFile]) -> String {
    let up = c.name.to_uppercase().replace('-', "_");
    let mut s = String::new();
    let w = &mut s;

    let _ = writeln!(w, "# ---- instance prepare: {} ----", c.name);
    let _ = writeln!(
        w,
        "INST_{up}_DIR=\"/tmp/inst-{name}-$$\"",
        up = up,
        name = c.name
    );
    let _ = writeln!(w, "mkdir -p \"$INST_{up}_DIR/files\"", up = up);

    if let Some(sd) = c.socket_dir() {
        let _ = writeln!(w, "mkdir -p \"{sd}\"", sd = sd);
    }

    // state_dir: если instance-файлы (storage.conf) изменились —
    // сбрасываем БД podman, иначе он переопределит драйвер старой записью.
    if let Some(sd) = c.state_dir() {
        let fp_hex = format!("{:x}", instance_storage_fp(files));
        let _ = writeln!(w, "_STATE_DIR=\"{sd}\"", sd = sd);
        let _ = writeln!(w, "_STATE_FP=\"$_STATE_DIR/.appspec-fp\"");
        let _ = writeln!(w, "_STATE_WANT={fp}", fp = sh_quote(&fp_hex));
        let _ = writeln!(
            w,
            "if [ -f \"$_STATE_FP\" ] && [ \"$(cat \"$_STATE_FP\")\" != \"$_STATE_WANT\" ]; then"
        );
        let _ = writeln!(
            w,
            "    echo '  state[{}]: instance-files changed → resetting' >&2",
            c.name
        );
        let _ = writeln!(w, "    rm -rf \"$_STATE_DIR\"");
        let _ = writeln!(w, "fi");
        let _ = writeln!(w, "mkdir -p \"$_STATE_DIR\"");
        let _ = writeln!(w, "printf '%s' \"$_STATE_WANT\" > \"$_STATE_FP\"");
    }

    if !files.is_empty() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        for f in files {
            let encoded = b64.encode(f.content.as_bytes());
            let rel = f.to.trim_start_matches('/');
            let _ = writeln!(
                w,
                "mkdir -p \"$(dirname \"$INST_{up}_DIR/files/{rel}\")\" 2>/dev/null || true",
                up = up,
                rel = rel
            );
            let _ = writeln!(
                w,
                "printf %s {enc} | base64 -d > \"$INST_{up}_DIR/files/{rel}\"",
                enc = sh_quote(&encoded),
                up = up,
                rel = rel
            );
            if f.mode != 0o644 {
                let _ = writeln!(
                    w,
                    "chmod {:o} \"$INST_{up}_DIR/files/{rel}\" || true",
                    f.mode,
                    up = up,
                    rel = rel
                );
            }
        }
    }
    let _ = writeln!(w);
    s
}

// ===========================================================================
// Instance start
// ===========================================================================

fn emit_instance_start(
    c: &Component,
    files: &[ResolvedFile],
    sif_var: &str,
    effective_daemon: &str,
) -> String {
    let up = c.name.to_uppercase().replace('-', "_");
    let inst_var = format!("INSTANCE_{}", up);
    let mut s = String::new();
    let w = &mut s;

    let podman_like = is_podman_like(c, files);

    let _ = writeln!(w, "# ---- instance start: {} ----", c.name);
    let _ = writeln!(w, "{}={}-$(date +%s)", inst_var, c.name);

    // Всё в subshell — чтобы `set --` не перебивал $@ основного скрипта.
    // ВАЖНО: ни одного многострочного `set --` с хвостовым `\`. Если
    // последняя строка блока заканчивается `\`, следующая строка
    // (начинающаяся с `[ -e … ]`) приклеится к `set --` как аргументы,
    // и `[` попадёт в $@, а потом в Apptainer как «путь к SIF».
    let _ = writeln!(w, "(");
    let _ = writeln!(w, "    set -- --writable-tmpfs --fakeroot --cleanenv");

    if c.contain() {
        let _ = writeln!(w, "    set -- \"$@\" --contain");
    }

    for m in c.mounts() {
        if !m.starts_with('/') {
            continue;
        }
        let _ = writeln!(
            w,
            "    [ -e \"{m}\" ] && set -- \"$@\" --bind \"{m}:{m}\" || true",
            m = m
        );
    }

    // --- podman-специфика: устройства и cgroup ---
    // Даже если пользователь поставил `--cgroups=disabled` в containers.conf,
    // сами устройства и cgroup-контроллер могут пригодиться (например,
    // `podman run` без явного `--cgroups=disabled`). Бинды безопасны:
    // отсутствие устройства — просто пропуск строки.
    if podman_like {
        let _ = writeln!(
            w,
            "    [ -e /dev/fuse ]      && set -- \"$@\" --bind /dev/fuse:/dev/fuse         || true"
        );
        let _ = writeln!(
            w,
            "    [ -e /dev/net/tun ]   && set -- \"$@\" --bind /dev/net/tun:/dev/net/tun   || true"
        );
        let _ = writeln!(w,
            "    [ -d /sys/fs/cgroup ] && set -- \"$@\" --bind /sys/fs/cgroup:/sys/fs/cgroup:ro || true");
    }

    if let (Some(sd), Some(ds)) = (c.socket_dir(), c.daemon_socket()) {
        let parent = std::path::Path::new(ds)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "/run/podman".into());
        let _ = writeln!(w, "    mkdir -p \"{sd}\"", sd = sd);
        let _ = writeln!(
            w,
            "    [ -d \"{sd}\" ] && set -- \"$@\" --bind \"{sd}:{parent}\" || true",
            sd = sd,
            parent = parent
        );
    }

    if let Some(sd) = c.state_dir() {
        let _ = writeln!(w, "    mkdir -p \"{sd}\"", sd = sd);
        let _ = writeln!(w,
            "    [ -d \"{sd}\" ] && set -- \"$@\" --bind \"{sd}:/var/lib/containers/storage\" || true",
            sd = sd);
    }

    for f in files {
        let rel = f.to.trim_start_matches('/');
        let host_rel = format!("$INST_{}_DIR/files/{}", up, rel);
        let _ = writeln!(
            w,
            "    [ -e \"{h}\" ] && set -- \"$@\" --bind \"{h}:{t}\" || true",
            h = host_rel,
            t = f.to
        );
    }

    // --- podman env defaults ДО пользовательских env,
    //     чтобы [components.env] мог их переопределить ---
    if podman_like {
        // --cleanenv вычищает PATH/HOME/XDG_RUNTIME_DIR. Podman без них
        // не находит helper-бинарники (conmon, crun, netavark, aardvark-dns,
        // pasta) и не может прочитать $HOME/.config/containers/*.
        let _ = writeln!(w,
            "    set -- \"$@\" --env PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin");
        let _ = writeln!(w, "    set -- \"$@\" --env HOME=/root");
        let _ = writeln!(w, "    set -- \"$@\" --env XDG_RUNTIME_DIR=/tmp/run-user-0");
        let _ = writeln!(w, "    set -- \"$@\" --env TMPDIR=/tmp");
        // --fakeroot уже в userns. Без этого podman re-exec'ится
        // в ещё один userns и ловит EPERM на хостах, где
        // user.max_user_namespaces < 2.
        let _ = writeln!(w, "    set -- \"$@\" --env _CONTAINERS_USERNS_CONFIGURED=");
        // buildah без chroot+userns — стабильнее в nested-userns.
        let _ = writeln!(w, "    set -- \"$@\" --env BUILDAH_ISOLATION=chroot");
    }

    let env_map = c.env_str();
    for (k, v) in &env_map {
        let _ = writeln!(w, "    set -- \"$@\" --env {k}={v}", k = k, v = sh_quote(v));
    }

    let _ = writeln!(w);
    let _ = writeln!(w, "    echo '=== starting instance: {} ===' >&2", c.name);
    let _ = writeln!(w, "    apptainer instance start \"$@\" \\");
    let _ = writeln!(w, "        \"${sif}\" \\", sif = sif_var);
    let _ = writeln!(w, "        \"${inst}\" \\", inst = inst_var);
    let _ = writeln!(w, "        {cmd}", cmd = sh_quote(effective_daemon));
    let _ = writeln!(w, ")");

    if let (Some(sd), Some(ds)) = (c.socket_dir(), c.daemon_socket()) {
        let leaf = std::path::Path::new(ds)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "podman.sock".into());
        let sock_host = format!("{}/{}", sd.trim_end_matches('/'), leaf);
        let _ = writeln!(w, "# ---- wait for socket (или смерть instance) ----");
        let _ = writeln!(w, "for _i in $(seq 1 60); do");
        let _ = writeln!(w, "    [ -S \"{q}\" ] && break", q = sock_host);
        let _ = writeln!(w, "    if ! apptainer instance list 2>/dev/null \\");
        let _ = writeln!(w, "           | awk 'NR>1 && NF>=1 {{print $1}}' \\");
        let _ = writeln!(w, "           | grep -qx \"$INSTANCE_{up}\"; then", up = up);
        let _ = writeln!(
            w,
            "        echo 'ERROR: instance[{n}] умер до появления сокета' >&2",
            n = c.name
        );
        let _ = writeln!(
            w,
            "        echo '  смотрите $BUNDLE_DIR/run/{n}/wrapper.log и daemon.log' >&2",
            n = c.name
        );
        let _ = writeln!(w, "        break");
        let _ = writeln!(w, "    fi");
        let _ = writeln!(w, "    sleep 0.5");
        let _ = writeln!(w, "done");
        let _ = writeln!(w, "if [ ! -S \"{q}\" ]; then", q = sock_host);
        let _ = writeln!(
            w,
            "    echo 'WARN: podman-сокет {} не появился ({q})' >&2",
            c.name,
            q = sock_host
        );
        let _ = writeln!(w, "fi");
    }
    let _ = writeln!(w);
    s
}

// ===========================================================================
// Selftest
// ===========================================================================

fn emit_selftest(c: &Component, strict: bool) -> String {
    let Some(st) = &c.selftest else {
        return String::new();
    };
    let Some(cmd_parts) = &st.cmd else {
        return String::new();
    };
    let cmd = cmd_parts
        .iter()
        .map(|p| sh_quote(p))
        .collect::<Vec<_>>()
        .join(" ");
    let expect = st.expect_contains.clone().unwrap_or_default();
    if strict {
        format!(
            "if ! {cmd} | grep -q {e}; then\n    \
            echo \"FATAL: selftest {n} failed: {cmd}\" >&2\n    exit 1\nfi",
            cmd = cmd,
            e = sh_quote(&expect),
            n = c.name
        )
    } else {
        format!(
            "if ! {cmd} | grep -q {e}; then\n    \
            echo \"WARN: selftest {n} failed: {cmd}\" >&2\nfi",
            cmd = cmd,
            e = sh_quote(&expect),
            n = c.name
        )
    }
}

// ===========================================================================
// Launcher
// ===========================================================================

pub fn render_launcher(
    file: &BundleFile,
    base: &ResolvedBase,
    resolved: &[(Component, std::path::PathBuf)],
    prepared_exts: &[PreparedExtension],
    files: &[ResolvedFile],
) -> Result<String> {
    let b = &file.bundle;

    let sandbox_comps: Vec<&(Component, std::path::PathBuf)> =
        resolved.iter().filter(|(c, _)| c.has_sandbox()).collect();
    let instance_comps: Vec<&(Component, std::path::PathBuf)> =
        resolved.iter().filter(|(c, _)| c.has_instance()).collect();

    let mut sif_var_lines: Vec<String> = Vec::new();
    for (c, sif_path) in resolved {
        let var_name = format!("SIF_{}", c.name.to_uppercase().replace('-', "_"));
        sif_var_lines.push(format!(
            "{var_name}={}",
            sh_quote(&sif_path.display().to_string())
        ));
    }

    let mut materialize_lines: Vec<String> = Vec::new();
    let mut sandbox_bind_lines: Vec<String> = Vec::new();

    for (c, _) in &sandbox_comps {
        let var_name = format!("SIF_{}", c.name.to_uppercase().replace('-', "_"));
        let sandbox_dir = format!("$BUNDLE_DIR/sandboxes/{}", c.name);

        materialize_lines.push(format!(
            "_materialize \"${var_name}\" \"{sandbox_dir}\" \"{name}\"",
            var_name = var_name,
            sandbox_dir = sandbox_dir,
            name = c.name
        ));
        sandbox_bind_lines.push(format!(
            "[ -d \"{sb}\" ] && BINDS=\"$BINDS --bind {sb}:{mount}\" || true",
            sb = sandbox_dir,
            mount = c.mount()
        ));
        for m in c.mounts() {
            if !m.starts_with('/') {
                continue;
            }
            let q = sh_quote(&m);
            sandbox_bind_lines.push(format!(
                "[ -e {q} ] && [ ! -d {q} ] && \
                 BINDS=\"$BINDS --bind {q}:{q}\" || true",
                q = q
            ));
        }
    }

    let bridges = if sandbox_comps.is_empty() {
        ":".into()
    } else {
        sandbox_comps
            .iter()
            .map(|(c, _)| emit_bridge(c, &c.mount()))
            .collect::<Vec<_>>()
            .join("\n\n")
    };

    let mut instance_prepare_block = String::new();
    let mut instance_start_block = String::new();
    let mut instance_cleanup_lines: Vec<String> = Vec::new();
    let mut instance_socket_binds_for_base: Vec<String> = Vec::new();
    let mut instance_env_for_base: Vec<(String, String)> = Vec::new();

    for (c, _) in &instance_comps {
        let up = c.name.to_uppercase().replace('-', "_");
        let var_name = format!("SIF_{}", up);
        let mut ifiles: Vec<ResolvedFile> = files
            .iter()
            .filter(|f| f.component == c.name && f.scopes.contains(&FileScope::Instance))
            .cloned()
            .collect();

        // Эффективный daemon: либо оригинал, либо путь к автогенерируемой
        // обёртке. Считаем до генерации файлов, чтобы знать, надо ли
        // добавлять wrapper в ifiles.
        let mut effective_daemon: String = c
            .daemon()
            .map(ensure_podman_service_timeout)
            .unwrap_or_else(|| "podman system service --time=0".into());

        if is_podman_like(c, &ifiles) {
            // (а) user-level containers.conf: cgroups=disabled + netns=host.
            //     Пишем в $HOME/.config/containers/containers.conf (HOME=/root),
            //     а НЕ в /etc/containers/containers.conf — иначе затрём
            //     системный файл из Alpine-бандла.
            //
            //     cgroups="disabled" — внутри Apptainer cgroup v1 без
            //       делегированных контроллеров; любой запуск с cgroups
            //       падает на mkdir /sys/fs/cgroup/{cpu,devices}/libpod_parent.
            //     netns="host" — Apptainer не поддерживает CLONE_NEWNET;
            //       без этого podman/netavark пытается создать netns и
            //       падает на lstat /run/user/$UID или на отсутствии iptables.
            let has_user_conf = ifiles
                .iter()
                .any(|f| f.to == "/root/.config/containers/containers.conf");
            if !has_user_conf {
                ifiles.push(ResolvedFile {
                    to: "/root/.config/containers/containers.conf".into(),
                    content: concat!(
                        "# Autogenerated by appspec — DO NOT EDIT\n",
                        "[containers]\n",
                        "cgroups = \"disabled\"\n",
                        "netns   = \"host\"\n",
                    )
                    .into(),
                    mode: 0o644,
                    source: "generated:podman-defaults".into(),
                    scopes: vec![FileScope::Instance],
                    component: c.name.clone(),
                });
            }

            // (б) wrapper-daemon. Путь — /opt/appspec/podman-daemon-wrapper.sh,
            //     рядом с пользовательским daemon.sh. Это гарантирует, что
            //     родительский каталог /opt/appspec/ существует в instance
            //     (создан bind'ом daemon.sh). Раньше был /usr/local/bin/,
            //     которого в Alpine-бандле нет — Apptainer молча пропускал
            //     bind, обёртка не появлялась, instance стартовал с дефолтным
            //     startscript'ом вместо нашего демона.
            let wrapper_content = podman_daemon_wrapper(&effective_daemon, &c.name);
            let wrapper_path = "/opt/appspec/podman-daemon-wrapper.sh";
            ifiles.push(ResolvedFile {
                to: wrapper_path.into(),
                content: wrapper_content,
                mode: 0o755,
                source: "generated:podman-daemon-wrapper".into(),
                scopes: vec![FileScope::Instance],
                component: c.name.clone(),
            });
            effective_daemon = wrapper_path.to_string();
        }

        instance_prepare_block.push_str(&emit_instance_prepare(c, &ifiles));
        instance_start_block.push_str(&emit_instance_start(
            c,
            &ifiles,
            &var_name,
            &effective_daemon,
        ));
        instance_cleanup_lines.push(format!(
            "    apptainer instance stop \"$INSTANCE_{up}\" 2>/dev/null || true",
            up = up
        ));

        if let (Some(sd), Some(ds)) = (c.socket_dir(), c.daemon_socket()) {
            let parent = std::path::Path::new(ds)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "/run/podman".into());
            instance_socket_binds_for_base.push(format!(
                "[ -d \"{sd}\" ] && BINDS=\"$BINDS --bind {sd}:{parent}\" || true",
                sd = sd,
                parent = parent
            ));

            let uri = format!("unix://{}", ds);
            instance_env_for_base.push(("DOCKER_HOST".into(), uri.clone()));
            instance_env_for_base.push(("CONTAINER_HOST".into(), uri.clone()));
        }
    }

    let instance_cleanup = if instance_cleanup_lines.is_empty() {
        ":".into()
    } else {
        instance_cleanup_lines.join("\n")
    };

    let selftests = sandbox_comps
        .iter()
        .map(|(c, _)| emit_selftest(c, b.strict))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let selftests = if selftests.is_empty() {
        ":".into()
    } else {
        selftests
    };

    let base_files_block = emit_files_for(files, FileScope::Base, None, "component files (base)");

    let mut sandbox_files_block = String::new();
    for (c, _) in &sandbox_comps {
        let cfs: Vec<ResolvedFile> = files
            .iter()
            .filter(|f| f.component == c.name && f.scopes.contains(&FileScope::Sandbox))
            .cloned()
            .collect();
        if cfs.is_empty() {
            continue;
        }
        let _ = writeln!(
            sandbox_files_block,
            "SANDBOX_DIR=\"$BUNDLE_DIR/sandboxes/{}\"",
            c.name
        );
        sandbox_files_block.push_str(&emit_files_for(
            &cfs,
            FileScope::Sandbox,
            Some("$SANDBOX_DIR"),
            &format!("sandbox files: {}", c.name),
        ));
        let _ = writeln!(sandbox_files_block);
    }
    if sandbox_files_block.is_empty() {
        sandbox_files_block = ": # sandbox files: пусто".into();
    }

    let utils_list = if resolved.is_empty() {
        "-".to_string()
    } else {
        resolved
            .iter()
            .map(|(c, _)| c.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let exts_list = if prepared_exts.is_empty() {
        "-".to_string()
    } else {
        prepared_exts
            .iter()
            .map(|e| e.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };

    let base_sif_q = sh_quote(&base.sif.display().to_string());

    let mut s = String::with_capacity(64 * 1024);
    let w = &mut s;

    writeln!(w, "#!/bin/sh")?;
    writeln!(w, "# Auto-generated by appspec bundle build — DO NOT EDIT")?;
    writeln!(
        w,
        "# Bundle:     {}  ({})",
        b.name,
        if b.description.is_empty() {
            "-"
        } else {
            b.description.as_str()
        }
    )?;
    writeln!(w, "# Base:       {}", base.origin)?;
    writeln!(w, "# Utils:      {}", utils_list)?;
    writeln!(w, "# Extensions: {}", exts_list)?;
    writeln!(w, "set -eu")?;
    writeln!(w)?;

    writeln!(w, "# ---- SIF paths ----")?;
    writeln!(w, "BASE_SIF={}", base_sif_q)?;
    for l in &sif_var_lines {
        writeln!(w, "{}", l)?;
    }
    writeln!(w)?;

    writeln!(w, "# ---- bundle cache ----")?;
    writeln!(w, "BUNDLE_NAME={}", sh_quote(&b.name))?;
    writeln!(
        w,
        "CONFIG_DIR=\"${{APPSPEC_BUNDLE_HOME:-$HOME/.cache/appspec/bundles/$BUNDLE_NAME}}\""
    )?;
    writeln!(w, "BUNDLE_DIR=\"$CONFIG_DIR\"")?;
    writeln!(w, "mkdir -p \"$BUNDLE_DIR/sandboxes\" \"$BUNDLE_DIR/home\" \"$BUNDLE_DIR/extensions\" \"$BUNDLE_DIR/run\" \"$BUNDLE_DIR/state\"")?;
    writeln!(w)?;

    writeln!(w, "# ---- companion sandboxes ----")?;
    writeln!(w, "_materialize() {{")?;
    writeln!(w, "    _sif=\"$1\"; _dst=\"$2\"; _lbl=\"$3\"")?;
    writeln!(
        w,
        "    [ -f \"$_sif\" ] || {{ echo \"FATAL: нет SIF: $_sif\" >&2; exit 1; }}"
    )?;
    writeln!(
        w,
        "    _fp=\"$(stat -c '%s-%Y' \"$_sif\" 2>/dev/null || echo 0)\""
    )?;
    writeln!(w, "    if [ -f \"$_dst/.fingerprint\" ] && [ \"$(cat \"$_dst/.fingerprint\")\" = \"$_fp\" ]; then")?;
    writeln!(w, "        return 0")?;
    writeln!(w, "    fi")?;
    writeln!(w, "    echo \"=== sandbox[$_lbl]: $_sif -> $_dst ===\" >&2")?;
    writeln!(w, "    rm -rf \"$_dst\"")?;
    writeln!(
        w,
        "    apptainer build --sandbox \"$_dst\" \"$_sif\" >/dev/null"
    )?;
    writeln!(w, "    printf '%s\\n' \"$_fp\" > \"$_dst/.fingerprint\"")?;
    writeln!(w, "}}")?;
    writeln!(w)?;
    if materialize_lines.is_empty() {
        writeln!(w, ":")?;
    } else {
        for l in &materialize_lines {
            writeln!(w, "{}", l)?;
        }
    }
    writeln!(w)?;

    writeln!(w, "# ---- sandbox files ----")?;
    writeln!(w, "{}", sandbox_files_block)?;
    writeln!(w)?;

    if !instance_prepare_block.is_empty() {
        writeln!(w, "# ---- instance prepare ----")?;
        write!(w, "{}", instance_prepare_block)?;
        writeln!(w)?;
    }
    if !instance_start_block.is_empty() {
        writeln!(w, "# ---- instance start ----")?;
        write!(w, "{}", instance_start_block)?;
        writeln!(w)?;
    }

    writeln!(w, "# ---- binds (base) ----")?;
    writeln!(w, "BINDS=\"\"")?;
    writeln!(w, "[ -e \"/tmp/.X11-unix\" ] && BINDS=\"$BINDS --bind /tmp/.X11-unix:/tmp/.X11-unix\" || true")?;
    writeln!(w, "[ -e \"/dev/dri\" ]         && BINDS=\"$BINDS --bind /dev/dri:/dev/dri\"            || true")?;
    writeln!(w, "[ -e \"/dev/snd\" ]         && BINDS=\"$BINDS --bind /dev/snd:/dev/snd\"            || true")?;
    writeln!(w, "[ -n \"${{XAUTHORITY:-}}\" ]  && BINDS=\"$BINDS --bind $XAUTHORITY:$XAUTHORITY\"      || true")?;
    writeln!(
        w,
        "if [ -n \"${{SSH_AUTH_SOCK:-}}\" ] && [ -S \"$SSH_AUTH_SOCK\" ]; then"
    )?;
    writeln!(
        w,
        "    BINDS=\"$BINDS --bind $SSH_AUTH_SOCK:$SSH_AUTH_SOCK\""
    )?;
    writeln!(w, "fi")?;
    writeln!(
        w,
        "if [ -n \"${{XDG_RUNTIME_DIR:-}}\" ] && [ -e \"$XDG_RUNTIME_DIR/pulse/native\" ]; then"
    )?;
    writeln!(
        w,
        "    BINDS=\"$BINDS --bind $XDG_RUNTIME_DIR/pulse/native:/tmp/pulse-native\""
    )?;
    writeln!(w, "fi")?;
    writeln!(w, "[ -d \"$BUNDLE_DIR/home\" ] && BINDS=\"$BINDS --bind $BUNDLE_DIR/home:/config/home\" || true")?;
    writeln!(w, "[ -d \"$BUNDLE_DIR/extensions\" ] && \\")?;
    writeln!(w, "    BINDS=\"$BINDS --bind $BUNDLE_DIR/extensions:/config/home/.vscodium/extensions\" || true")?;
    for l in &instance_socket_binds_for_base {
        writeln!(w, "{}", l)?;
    }
    for l in &sandbox_bind_lines {
        writeln!(w, "{}", l)?;
    }
    writeln!(w)?;

    writeln!(w, "X_ENV=\"\"")?;
    writeln!(w, "[ -n \"${{DISPLAY:-}}\" ]       && X_ENV=\"$X_ENV --env DISPLAY=$DISPLAY\"              || true")?;
    writeln!(w, "[ -n \"${{XAUTHORITY:-}}\" ]    && X_ENV=\"$X_ENV --env XAUTHORITY=$XAUTHORITY\"        || true")?;
    writeln!(w, "[ -n \"${{SSH_AUTH_SOCK:-}}\" ] && X_ENV=\"$X_ENV --env SSH_AUTH_SOCK=$SSH_AUTH_SOCK\"  || true")?;
    writeln!(
        w,
        "[ -e \"${{XDG_RUNTIME_DIR:-/nonexistent}}/pulse/native\" ] && \\"
    )?;
    writeln!(
        w,
        "    X_ENV=\"$X_ENV --env PULSE_SERVER=unix:/tmp/pulse-native\" || true"
    )?;
    for (k, v) in &instance_env_for_base {
        writeln!(w, "X_ENV=\"$X_ENV --env {}={}\"", k, sh_quote(v))?;
    }
    writeln!(w)?;

    writeln!(w, "# ---- instance (base) ----")?;
    writeln!(w, "INSTANCE_NAME=\"$BUNDLE_NAME-$(date +%s)\"")?;
    writeln!(w)?;
    writeln!(w, "apptainer instance start \\")?;
    writeln!(w, "    --writable-tmpfs \\")?;
    writeln!(w, "    --fakeroot \\")?;
    writeln!(w, "    --contain \\")?;
    writeln!(w, "    $BINDS \\")?;
    writeln!(w, "    \"$BASE_SIF\" \\")?;
    writeln!(w, "    \"$INSTANCE_NAME\"")?;
    writeln!(w)?;
    writeln!(w, "cleanup() {{")?;
    writeln!(
        w,
        "    apptainer instance stop \"$INSTANCE_NAME\" 2>/dev/null || true"
    )?;
    write!(w, "{}", instance_cleanup)?;
    writeln!(w)?;
    writeln!(w, "}}")?;
    writeln!(w, "trap cleanup EXIT INT TERM")?;
    writeln!(w)?;

    writeln!(w, "# ---- setup + launch ----")?;
    writeln!(w, "apptainer exec $X_ENV \\")?;
    writeln!(w, "    --pwd /config/home \\")?;
    writeln!(
        w,
        "    \"instance://$INSTANCE_NAME\" /bin/sh -s <<'__APPSPEC_BUNDLE_SETUP__'"
    )?;
    writeln!(w, "set -eu")?;
    writeln!(w, "export HOME=/config/home")?;
    writeln!(w)?;

    writeln!(w, "# ===== base setup (source: {}) =====", base.origin)?;
    if base.pre.is_empty() {
        writeln!(w, ":")?;
    } else {
        writeln!(w, "{}", base.pre)?;
    }
    writeln!(w)?;

    writeln!(w, "# ===== bridges =====")?;
    writeln!(w, "{}", bridges)?;
    writeln!(w)?;

    writeln!(w, "# ===== component files (base) =====")?;
    writeln!(w, "{}", base_files_block)?;
    writeln!(w)?;

    writeln!(w, "# ===== extensions =====")?;
    if prepared_exts.is_empty() {
        writeln!(w, ":")?;
    } else {
        for e in prepared_exts {
            writeln!(w, "echo '  ready: {} v{}' >&2", e.id, e.version)?;
        }
    }
    writeln!(w)?;

    writeln!(w, "# ===== selftests =====")?;
    writeln!(w, "{}", selftests)?;
    writeln!(w)?;

    writeln!(w, "# ===== launch =====")?;
    writeln!(w, "{}", base.entry)?;
    writeln!(w, "__APPSPEC_BUNDLE_SETUP__")?;
    writeln!(w)?;

    Ok(s)
}
