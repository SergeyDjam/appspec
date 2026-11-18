use anyhow::Result;
use std::collections::HashSet;
use std::fmt::Write as _;

use crate::templates::{build_bind_lines, sh_quote};

use super::model::{
    BundleFile, Component, ExtensionKind, FileScope, PreparedExtension, ResolvedBase, ResolvedFile,
};

// ===========================================================================
// Bridge (chroot mode)
// ===========================================================================

fn bridge_file_name(target: &str) -> String {
    let base = std::path::Path::new(target)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("bridge");
    let mut out = String::with_capacity(base.len());
    for ch in base.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push_str("bridge");
    }
    out
}

fn unique_bridge_file(target: &str, used: &mut HashSet<String>) -> String {
    let base = bridge_file_name(target);
    if used.insert(base.clone()) {
        return base;
    }
    let mut i = 2;
    loop {
        let cand = format!("{base}-{i}");
        if used.insert(cand.clone()) {
            return cand;
        }
        i += 1;
    }
}

fn bind_source(b: &str) -> String {
    let b = b.strip_prefix('!').unwrap_or(b);
    match b.split_once(':') {
        Some((s, _)) => s.to_string(),
        None => b.to_string(),
    }
}

fn covered_by_mount(mounts: &[String], abs: &str) -> bool {
    mounts.iter().any(|p| {
        p.starts_with('/')
            && p.as_str() != abs
            && (abs == p.as_str() || abs.starts_with(&format!("{}/", p)))
    })
}

fn bind_target(b: &str) -> String {
    let b = b.strip_prefix('!').unwrap_or(b);
    match b.split_once(':') {
        Some((_, rest)) => rest.split(':').next().unwrap_or(rest).to_string(),
        None => b.to_string(),
    }
}

fn emit_bridge(
    c: &Component,
    mount: &str,
    binary: &str,
    target: &str,
    shared_mounts: &[String],
    force_pwd_home: bool,
) -> String {
    let provide = c.provide();
    let forward = c.forward();
    let env = c.env_str();

    // bridge = "exec": не chroot, а прямой запуск бинаря из своего
    // satellite-rootfs. Нужен для tofu, чей local-exec должен видеть
    // base tools (ansible/qemu/...) и текущий проект.
    if c.bridge_kind() == "exec" {
        let short = binary.rsplit('/').next().unwrap_or(binary);
        let mut s = String::new();
        let w = &mut s;
        let _ = writeln!(w, "#!/bin/sh");
        let _ = writeln!(
            w,
            "# appspec bridge (exec): {provide} -> {name}",
            name = c.name
        );
        let _ = writeln!(w, "set -eu");
        let _ = writeln!(w, "SANDBOX={}", sh_quote(mount));
        // Находим rootfs satellite'а. В base обычно достаточно symlink
        // /opt/utils/NAME -> NAME-HASH, но оставляем остальные запасные
        // пути, чтобы bridge пережил нестандартный bind.
        let _ = writeln!(
            w,
            "if [ ! -e \"$SANDBOX{binary}\" ] && [ -f /opt/appspec/bin/sandbox-{name} ]; then",
            name = c.name
        );
        let _ = writeln!(
            w,
            "    _sbh=\"$(cat /opt/appspec/bin/sandbox-{name})\"",
            name = c.name
        );
        let _ = writeln!(
            w,
            "    if [ -e \"/opt/appspec/sandboxes/$_sbh{binary}\" ]; then SANDBOX=\"/opt/appspec/sandboxes/$_sbh\"; fi"
        );
        let _ = writeln!(w, "fi");
        let _ = writeln!(
            w,
            "if [ ! -e \"$SANDBOX{binary}\" ] && [ -f /opt/utils/.current/{name} ]; then",
            name = c.name
        );
        let _ = writeln!(
            w,
            "    _sbh=\"$(cat /opt/utils/.current/{name})\"",
            name = c.name
        );
        let _ = writeln!(
            w,
            "    if [ -e \"/opt/utils/$_sbh{binary}\" ]; then SANDBOX=\"/opt/utils/$_sbh\"; fi"
        );
        let _ = writeln!(w, "fi");
        let _ = writeln!(w, "if [ ! -e \"$SANDBOX{binary}\" ]; then");
        let _ = writeln!(w, "    for _sd in /opt/utils/{name}-*; do", name = c.name);
        let _ = writeln!(
            w,
            "        if [ -e \"$_sd{binary}\" ]; then SANDBOX=\"$_sd\"; break; fi"
        );
        let _ = writeln!(w, "    done");
        let _ = writeln!(w, "fi");
        let _ = writeln!(w, "BIN=\"$SANDBOX{binary}\"");
        let _ = writeln!(
            w,
            "if [ ! -x \"$BIN\" ]; then echo \"bridge[{name}]: $BIN отсутствует (satellite не смонтирован)\" >&2; exit 127; fi",
            name = c.name
        );
        let _ = writeln!(
            w,
            "export PATH=\"${{PATH:-/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin}}:/opt/appspec/bin\""
        );
        for v in &forward {
            let _ = writeln!(w, "export {0}=\"${{{0}:-}}\"", v);
        }
        for (k, v) in &env {
            let _ = writeln!(w, "export {}={}", k, sh_quote(v));
        }
        let _ = writeln!(w, "# {}: прямой запуск из rootfs satellite", short);
        let _ = writeln!(w, "exec \"$BIN\" \"$@\"");
        return s;
    }

    let mut mounts = c.mounts();
    if force_pwd_home {
        if !mounts.iter().any(|m| m == "PWD") {
            mounts.push("PWD".into());
        }
        if !mounts.iter().any(|m| m == "HOME") {
            mounts.push("HOME".into());
        }
    }
    // HOME биндим раньше PWD: иначе bind HOME поверх родителя может
    // скрыть уже смонтированный рабочий каталог.
    let mut ordered: Vec<String> = Vec::with_capacity(mounts.len());
    if mounts.iter().any(|m| m == "HOME") {
        ordered.push("HOME".into());
    }
    for m in &mounts {
        if m != "HOME" && m != "PWD" {
            ordered.push(m.clone());
        }
    }
    if mounts.iter().any(|m| m == "PWD") {
        ordered.push("PWD".into());
    }
    let mounts = ordered;

    let mut s = String::new();
    let w = &mut s;

    let _ = writeln!(w, "#!/bin/sh");
    let _ = writeln!(
        w,
        "# appspec bridge: {provide} -> {name} ({target})",
        name = c.name
    );
    let _ = writeln!(w, "set -eu");
    let _ = writeln!(w);
    let _ = writeln!(w, "SANDBOX={}", sh_quote(mount));
    // Прямой bind под /opt/appspec — самый надёжный путь.
    let _ = writeln!(
        w,
        "if [ -f \"/opt/appspec/sandbox-{name}/.ready\" ]; then SANDBOX=\"/opt/appspec/sandbox-{name}\"; fi",
        name = c.name
    );
    // Fallback 1: hashed sandbox через /opt/appspec/sandboxes (этот bind
    // виден там же, где /opt/appspec/bin, т.е. в VSCodium-терминале).
    let _ = writeln!(
        w,
        "if [ -f /opt/appspec/bin/sandbox-{name} ]; then",
        name = c.name
    );
    let _ = writeln!(
        w,
        "    _sbh=\"$(cat /opt/appspec/bin/sandbox-{name})\"",
        name = c.name
    );
    let _ = writeln!(
        w,
        "    [ -f \"/opt/appspec/sandboxes/$_sbh/.ready\" ] && SANDBOX=\"/opt/appspec/sandboxes/$_sbh\""
    );
    let _ = writeln!(w, "fi");
    // Fallback 2: тот же hashed-каталог через /opt/utils.
    let _ = writeln!(
        w,
        "if [ ! -f \"$SANDBOX/.ready\" ] && [ -f /opt/appspec/bin/sandbox-{name} ]; then",
        name = c.name
    );
    let _ = writeln!(
        w,
        "    _sbh=\"$(cat /opt/appspec/bin/sandbox-{name})\"",
        name = c.name
    );
    let _ = writeln!(
        w,
        "    [ -f \"/opt/utils/$_sbh/.ready\" ] && SANDBOX=\"/opt/utils/$_sbh\""
    );
    let _ = writeln!(w, "fi");
    let _ = writeln!(
        w,
        "if [ ! -f \"$SANDBOX/.ready\" ] && [ -f /opt/utils/.current/{name} ]; then",
        name = c.name
    );
    let _ = writeln!(
        w,
        "    _sbh=\"$(cat /opt/utils/.current/{name})\"",
        name = c.name
    );
    let _ = writeln!(
        w,
        "    [ -f \"/opt/utils/$_sbh/.ready\" ] && SANDBOX=\"/opt/utils/$_sbh\""
    );
    let _ = writeln!(w, "fi");
    let _ = writeln!(w, "if [ ! -f \"$SANDBOX/.ready\" ]; then");
    let _ = writeln!(w, "    for _sd in /opt/utils/{name}-*; do", name = c.name);
    let _ = writeln!(
        w,
        "        [ -f \"$_sd/.ready\" ] && {{ SANDBOX=\"$_sd\"; break; }}"
    );
    let _ = writeln!(w, "    done");
    let _ = writeln!(w, "fi");
    let _ = writeln!(w, "if [ ! -f \"$SANDBOX/.ready\" ]; then");
    let _ = writeln!(
        w,
        "    for _bb in /usr/bin/{bin} /bin/{bin}; do",
        bin = binary.rsplit('/').next().unwrap_or(binary)
    );
    let _ = writeln!(w, "        [ -x \"$_bb\" ] && exec \"$_bb\" \"$@\"");
    let _ = writeln!(w, "    done");
    let _ = writeln!(w, "    _pwd=\"$(pwd -P)\"");
    let _ = writeln!(
        w,
        "    if [ -x \"/opt/appspec/base/usr/bin/{bin}\" ]; then",
        bin = binary.rsplit('/').next().unwrap_or(binary)
    );
    let _ = writeln!(w, "        if [ -x /usr/sbin/chroot ]; then");
    let _ = writeln!(w, "            exec /usr/sbin/chroot /opt/appspec/base /bin/sh -c 'cd \"$1\" || cd /; shift; exec \"$@\"' sh \"$_pwd\" /usr/bin/{bin} \"$@\"", bin = binary.rsplit('/').next().unwrap_or(binary));
    let _ = writeln!(w, "        elif [ -x /bin/busybox ]; then");
    let _ = writeln!(w, "            exec /bin/busybox chroot /opt/appspec/base /bin/sh -c 'cd \"$1\" || cd /; shift; exec \"$@\"' sh \"$_pwd\" /usr/bin/{bin} \"$@\"", bin = binary.rsplit('/').next().unwrap_or(binary));
    let _ = writeln!(w, "        fi");
    let _ = writeln!(w, "    fi");
    let _ = writeln!(
        w,
        "    if [ -x \"/proc/1/root/usr/bin/{bin}\" ]; then",
        bin = binary.rsplit('/').next().unwrap_or(binary)
    );
    let _ = writeln!(w, "        if [ -x /usr/sbin/chroot ]; then");
    let _ = writeln!(w, "            exec /usr/sbin/chroot /proc/1/root /bin/sh -c 'cd \"$1\" || cd /; shift; exec \"$@\"' sh \"$_pwd\" /usr/bin/{bin} \"$@\"", bin = binary.rsplit('/').next().unwrap_or(binary));
    let _ = writeln!(w, "        elif [ -x /bin/busybox ]; then");
    let _ = writeln!(w, "            exec /bin/busybox chroot /proc/1/root /bin/sh -c 'cd \"$1\" || cd /; shift; exec \"$@\"' sh \"$_pwd\" /usr/bin/{bin} \"$@\"", bin = binary.rsplit('/').next().unwrap_or(binary));
    let _ = writeln!(w, "        fi");
    let _ = writeln!(w, "    fi");
    let _ =
        writeln!(w,
        "    echo \"bridge[{name}] (instance $(cat /opt/appspec/bin/.instance-id 2>/dev/null || echo unknown)): $SANDBOX отсутствует (sandbox bind не виден; нет .ready)\" >&2; exit 127",
        name = c.name);
    let _ = writeln!(w, "fi");
    let _ = writeln!(
        w,
        "[ -e \"$SANDBOX{}\" ] || {{ echo \"bridge[{name}] (instance $(cat /opt/appspec/bin/.instance-id 2>/dev/null || echo unknown)): {} отсутствует в $SANDBOX (SIF устарел? пересоберите spec)\" >&2; exit 127; }}",
        binary,
        binary,
        name = c.name
    );
    let _ = writeln!(w);
    let _ = writeln!(w, "PWD_REAL=\"$(pwd -P)\"");
    let _ = writeln!(w, "HOME_REAL=\"${{HOME:-/config/home}}\"");
    let _ = writeln!(w, "MOUNT_BIN=\"$(command -v mount 2>/dev/null || true)\"");
    let _ = writeln!(w, "MOUNT_APPLET=\"\"");
    let _ = writeln!(
        w,
        "if [ -z \"$MOUNT_BIN\" ] && [ -x /bin/busybox ]; then MOUNT_BIN=/bin/busybox; MOUNT_APPLET=mount; fi"
    );
    let _ = writeln!(
        w,
        "[ -n \"$MOUNT_BIN\" ] || {{ echo \"bridge[{name}]: mount не найден\" >&2; exit 127; }}",
        name = c.name
    );
    let _ = writeln!(w, "mount_bind() {{");
    let _ = writeln!(
        w,
        "    if [ -n \"$MOUNT_APPLET\" ]; then \"$MOUNT_BIN\" \"$MOUNT_APPLET\" --bind \"$@\"; else \"$MOUNT_BIN\" --bind \"$@\"; fi"
    );
    let _ = writeln!(w, "}}");
    let _ = writeln!(w, "CHROOT_BIN=\"$(command -v chroot 2>/dev/null || true)\"");
    let _ = writeln!(w, "CHROOT_APPLET=\"\"");
    let _ = writeln!(
        w,
        "if [ -z \"$CHROOT_BIN\" ] && [ -x /bin/busybox ]; then CHROOT_BIN=/bin/busybox; CHROOT_APPLET=chroot; fi"
    );
    let _ = writeln!(
        w,
        "[ -n \"$CHROOT_BIN\" ] || {{ echo \"bridge[{name}]: chroot не найден\" >&2; exit 127; }}",
        name = c.name
    );
    let _ = writeln!(w);
    let _ = writeln!(
        w,
        "mkdir -p \"$SANDBOX$PWD_REAL\" \"$SANDBOX$HOME_REAL\" 2>/dev/null || true"
    );
    let _ = writeln!(w);

    // Общие каталоги: /opt/appspec/bin и sandbox'ы остальных компонентов.
    // Это позволяет bridge'у, вызванному из чужого chroot, видеть соседей.
    let shared: Vec<&String> = shared_mounts
        .iter()
        .filter(|m| m.as_str() != mount)
        .collect();
    if !shared.is_empty() {
        let list = shared
            .iter()
            .map(|m| sh_quote(m.as_str()))
            .collect::<Vec<_>>()
            .join(" ");
        let _ = writeln!(w, "for _d in {list}; do");
        let _ = writeln!(w, "    _src=\"$_d\"");
        let _ = writeln!(w, "    _alt=\"/opt/appspec/sandbox-$(basename \"$_d\")\"");
        let _ = writeln!(w, "    [ -f \"$_alt/.ready\" ] && _src=\"$_alt\"");
        let _ = writeln!(
            w,
            "    _map=\"/opt/appspec/bin/sandbox-$(basename \"$_d\")\""
        );
        let _ = writeln!(w, "    if [ ! -d \"$_src\" ] && [ -f \"$_map\" ]; then");
        let _ = writeln!(w, "        _sbh=\"$(cat \"$_map\")\"");
        let _ = writeln!(
            w,
            "        [ -f \"/opt/appspec/sandboxes/$_sbh/.ready\" ] && _src=\"/opt/appspec/sandboxes/$_sbh\""
        );
        let _ = writeln!(w, "    fi");
        let _ = writeln!(w, "    if [ ! -d \"$_src\" ] && [ -f \"$_map\" ]; then");
        let _ = writeln!(w, "        _sbh=\"$(cat \"$_map\")\"");
        let _ = writeln!(
            w,
            "        [ -f \"/opt/utils/$_sbh/.ready\" ] && _src=\"/opt/utils/$_sbh\""
        );
        let _ = writeln!(w, "    fi");
        let _ = writeln!(
            w,
            "    if [ ! -d \"$_src\" ] && [ -f \"/opt/utils/.current/$(basename \"$_d\")\" ]; then"
        );
        let _ = writeln!(
            w,
            "        _sbh=\"$(cat \"/opt/utils/.current/$(basename \"$_d\")\")\""
        );
        let _ = writeln!(
            w,
            "        [ -f \"/opt/utils/$_sbh/.ready\" ] && _src=\"/opt/utils/$_sbh\""
        );
        let _ = writeln!(w, "    fi");
        let _ = writeln!(w, "    if [ ! -d \"$_src\" ]; then");
        let _ = writeln!(w, "        for _sd in \"$_d\"-*; do");
        let _ = writeln!(
            w,
            "            [ -f \"$_sd/.ready\" ] && {{ _src=\"$_sd\"; break; }}"
        );
        let _ = writeln!(w, "        done");
        let _ = writeln!(w, "    fi");
        let _ = writeln!(w, "    [ -d \"$_src\" ] || continue");
        let _ = writeln!(w, "    mkdir -p \"$SANDBOX$_d\" 2>/dev/null || true");
        let _ = writeln!(
            w,
            "    mount_bind \"$_src\" \"$SANDBOX$_d\" 2>/dev/null || true"
        );
        let _ = writeln!(w, "done");
        let _ = writeln!(w);
    }

    for m in &mounts {
        match m.as_str() {
            "PWD" => {
                // Если PWD не примонтировался, chroot бесполезен: команда
                // не найдёт относительные файлы проекта. Лучше явно
                // сказать об этом и попробовать base-бинарь.
                let _ = writeln!(
                    w,
                    "if ! mount_bind \"$PWD_REAL\" \"$SANDBOX$PWD_REAL\" 2>/dev/null; then"
                );
                let _ = writeln!(
                    w,
                    "    echo \"bridge[{name}]: не удалось смонтировать PWD $PWD_REAL в $SANDBOX\" >&2",
                    name = c.name
                );
                let _ = writeln!(
                    w,
                    "    for _bb in /usr/bin/{bin} /bin/{bin}; do",
                    bin = binary.rsplit('/').next().unwrap_or(binary)
                );
                let _ = writeln!(
                    w,
                    "        if [ -x \"$_bb\" ]; then exec \"$_bb\" \"$@\"; fi"
                );
                let _ = writeln!(w, "    done");
                let _ = writeln!(w, "    exit 127");
                let _ = writeln!(w, "fi");
            }
            "HOME" => {
                let _ = writeln!(
                    w,
                    "mount_bind \"$HOME_REAL\" \"$SANDBOX$HOME_REAL\" 2>/dev/null || \
                     echo \"bridge[{name}]: WARN: HOME $HOME_REAL не смонтирован\" >&2",
                    name = c.name
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
                    "    mount_bind \"$SSH_AUTH_SOCK\" \"$SANDBOX$SSH_AUTH_SOCK\" 2>/dev/null || true");
                let _ = writeln!(w, "fi");
            }
            "/dev/kvm" => {
                // Если APPSPEC_DISABLE_KVM=1, не пробрасываем KVM даже
                // в satellite-chroot: иначе qemu внутри него всё равно
                // получит нерабочий /dev/kvm и упадёт на ioctl.
                let _ = writeln!(
                    w,
                    "if [ -z \"${{APPSPEC_DISABLE_KVM:-}}\" ] && {{ [ -b \"/dev/kvm\" ] || [ -c \"/dev/kvm\" ]; }}; then"
                );
                let _ = writeln!(w, "    mkdir -p \"$SANDBOX/dev\" 2>/dev/null || true");
                let _ = writeln!(
                    w,
                    "    mount_bind /dev \"$SANDBOX/dev\" 2>/dev/null || true"
                );
                let _ = writeln!(w, "fi");
            }
            abs if abs.starts_with('/') => {
                if covered_by_mount(&mounts, abs) {
                    continue;
                }
                let _ = writeln!(w, "if [ -b \"{abs}\" ] || [ -c \"{abs}\" ]; then");
                let _ = writeln!(
                    w,
                    "    # device: bind parent dir, чтобы не создавать пустой файл-заглушку"
                );
                let _ = writeln!(
                    w,
                    "    if [ -e \"$SANDBOX{abs}\" ] && [ ! -b \"$SANDBOX{abs}\" ] && [ ! -c \"$SANDBOX{abs}\" ]; then rm -f \"$SANDBOX{abs}\"; fi"
                );
                let _ = writeln!(w, "    _parent=\"$(dirname \"{abs}\")\"");
                let _ = writeln!(w, "    mkdir -p \"$SANDBOX$_parent\" 2>/dev/null || true");
                let _ = writeln!(
                    w,
                    "    mount_bind \"$_parent\" \"$SANDBOX$_parent\" 2>/dev/null || true"
                );
                let _ = writeln!(w, "elif [ -d \"{abs}\" ]; then");
                let _ = writeln!(w, "    mkdir -p \"$SANDBOX{abs}\" 2>/dev/null || true");
                let _ = writeln!(
                    w,
                    "    mount_bind \"{abs}\" \"$SANDBOX{abs}\" 2>/dev/null || true"
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
                    "    mount_bind \"{abs}\" \"$SANDBOX{abs}\" 2>/dev/null || true"
                );
                let _ = writeln!(w, "fi");
            }
            _ => {}
        }
    }

    // В nested chroot bind всего /dev может быть запрещён (locked mount).
    // Добираем базовые device-файлы по одному; если и это нельзя —
    // хотя бы не оставляем regular-file заглушки.
    let _ = writeln!(w, "for _dn in null zero full random urandom tty; do");
    let _ = writeln!(
        w,
        "    if [ -e \"/dev/$_dn\" ] && [ ! -e \"$SANDBOX/dev/$_dn\" ]; then"
    );
    let _ = writeln!(w, "        mkdir -p \"$SANDBOX/dev\" 2>/dev/null || true");
    let _ = writeln!(
        w,
        "        ( : > \"$SANDBOX/dev/$_dn\" ) 2>/dev/null || true"
    );
    let _ = writeln!(
        w,
        "        mount_bind \"/dev/$_dn\" \"$SANDBOX/dev/$_dn\" 2>/dev/null || rm -f \"$SANDBOX/dev/$_dn\" || true"
    );
    let _ = writeln!(w, "    fi");
    let _ = writeln!(w, "done");
    let _ = writeln!(w);

    // Даём sandbox'у доступ к base-root через /opt/appspec/base. Нужно для
    // инструментов, которые есть в base, но не в минимальном satellite.
    let _ = writeln!(w, "if [ ! -d /opt/appspec/base ]; then");
    let _ = writeln!(
        w,
        "    mkdir -p \"$SANDBOX/opt/appspec/base\" 2>/dev/null || true"
    );
    let _ = writeln!(
        w,
        "    mount_bind / \"$SANDBOX/opt/appspec/base\" 2>/dev/null || true"
    );
    let _ = writeln!(w, "fi");
    let _ = writeln!(w);

    // POSIX shared memory: нужен Python multiprocessing (ansible) и
    // некоторым утилитам. В минимальных sandbox'ах /dev/shm отсутствует.
    let _ = writeln!(w, "mkdir -p \"$SANDBOX/dev/shm\" 2>/dev/null || true");
    let _ = writeln!(w, "chmod 1777 \"$SANDBOX/dev/shm\" 2>/dev/null || true");
    let _ = writeln!(w);
    let _ = writeln!(w, "export HOME=\"$HOME_REAL\"");
    let _ = writeln!(
        w,
        "export PATH=\"${{PATH:-/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin}}:/opt/appspec/bin\""
    );
    for v in &forward {
        let _ = writeln!(w, "export {0}=\"${{{0}:-}}\"", v);
    }
    for (k, v) in &env {
        let _ = writeln!(w, "export {}={}", k, sh_quote(v));
    }
    let _ = writeln!(w);

    let _ = writeln!(w, "run_chroot() {{");
    let _ = writeln!(w, "    if [ -n \"$CHROOT_APPLET\" ]; then");
    let _ = writeln!(w, "        exec \"$CHROOT_BIN\" chroot \"$@\"");
    let _ = writeln!(w, "    else");
    let _ = writeln!(w, "        exec \"$CHROOT_BIN\" \"$@\"");
    let _ = writeln!(w, "    fi");
    let _ = writeln!(w, "}}");
    let _ = writeln!(w);
    let _ = writeln!(w, "if [ -x \"$SANDBOX/bin/sh\" ]; then");
    let _ = writeln!(w, "    run_chroot \"$SANDBOX\" /bin/sh -c '");
    let _ = writeln!(w, "        cd \"$1\" || exit 1");
    let _ = writeln!(w, "        shift");
    let _ = writeln!(w, "        exec \"$@\"");
    let _ = writeln!(w, "    ' sh \"$PWD_REAL\" {} \"$@\"", sh_quote(binary));
    let _ = writeln!(w, "else");
    let _ = writeln!(
        w,
        "    # fallback: в sandbox нет /bin/sh — запускаем без cd"
    );
    let _ = writeln!(w, "    run_chroot \"$SANDBOX\" {} \"$@\"", sh_quote(binary));
    let _ = writeln!(w, "fi");
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
        // CHANGED: content: Vec<u8> — base64 над байтами, не над строкой.
        let encoded = b64.encode(&f.content);
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

fn instance_storage_fp(files: &[ResolvedFile]) -> u64 {
    let mut h: u64 = 5381;
    for f in files {
        for b in f.to.bytes() {
            h = h.wrapping_mul(33).wrapping_add(b as u64);
        }
        // CHANGED: f.content теперь Vec<u8> — итерируем по &Vec<u8>.
        for b in &f.content {
            h = h.wrapping_mul(33).wrapping_add(*b as u64);
        }
    }
    h
}

// ===========================================================================
// Podman helpers
// ===========================================================================

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

fn has_time_flag(daemon: &str) -> bool {
    daemon.split_whitespace().any(|tok| {
        let name = tok.split('=').next().unwrap_or("");
        name == "--time" || name == "-t"
    })
}

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

fn podman_daemon_wrapper(real_daemon: &str, name: &str) -> String {
    let q = sh_quote(real_daemon);
    format!(
        "#!/bin/sh\n\
         # Autogenerated by appspec — DO NOT EDIT.\n\
         # component: {name}\n\
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
         if mount --make-rshared / 2>/dev/null; then\n\
             echo 'mount --make-rshared ok'\n\
         elif mount --make-rslave / 2>/dev/null; then\n\
             echo 'mount --make-rslave ok'\n\
         else\n\
             echo 'mount-fix: both failed (продолжаем)'\n\
         fi\n\
         export PATH=\"${{PATH:-/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin}}\"\n\
         export HOME=\"${{HOME:-/root}}\"\n\
         export TMPDIR=\"${{TMPDIR:-/tmp}}\"\n\
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
            // CHANGED: content: Vec<u8> — base64 над байтами.
            let encoded = b64.encode(&f.content);
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

    let _ = writeln!(w, "(");
    let _ = writeln!(w, "    set -- --writable-tmpfs --fakeroot --cleanenv");

    if c.contain() {
        let _ = writeln!(w, "    set -- \"$@\" --contain");
    }

    let mounts = c.mounts();
    for m in &mounts {
        if !m.starts_with('/') {
            continue;
        }
        // Apptainer уже пробрасывает эти файлы.
        if m == "/etc/resolv.conf" || m == "/etc/hosts" {
            continue;
        }
        if covered_by_mount(&mounts, m) {
            continue;
        }
        let _ = writeln!(
            w,
            "    [ -e \"{m}\" ] && set -- \"$@\" --bind \"{m}:{m}\" || true",
            m = m
        );
    }

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

    if podman_like {
        let _ = writeln!(w,
            "    set -- \"$@\" --env PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin");
        let _ = writeln!(w, "    set -- \"$@\" --env HOME=/root");
        let _ = writeln!(w, "    set -- \"$@\" --env XDG_RUNTIME_DIR=/tmp/run-user-0");
        let _ = writeln!(w, "    set -- \"$@\" --env TMPDIR=/tmp");
        let _ = writeln!(w, "    set -- \"$@\" --env _CONTAINERS_USERNS_CONFIGURED=");
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
// Install-команда для расширения
// ===========================================================================

/// Собрать финальную install-команду для расширения.
///
/// Плейсхолдеры в `tpl` (все подставляются в один проход):
///   * `{file}`   — путь внутри `<mount_dir>` (legacy alias: `{target}`).
///   * `{target}` — то же, что `{file}` (совместимость с примерами).
///   * `{name}`   — ext.id (для type=file это ext.name).
///   * `{id}`     — ext.id.
///   * `{version}` — ext.version.
///
/// Вынесено в отдельную функцию, чтобы был unit-тест на regress
/// «плейсхолдер не подставился → codium --install-extension {target}».
pub(crate) fn build_install_command(tpl: &str, mount_dir: &str, ext: &PreparedExtension) -> String {
    let file_path = format!("{}/{}", mount_dir.trim_end_matches('/'), ext.rel_path());
    tpl.replace("{file}", &file_path)
        .replace("{target}", &file_path)
        .replace("{name}", &ext.id)
        .replace("{id}", &ext.id)
        .replace("{version}", &ext.version)
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

    let ext_mount_dir: String = b
        .extensions
        .as_ref()
        .map(|s| s.mount_dir.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/config/home/.vscodium/extensions".into());

    let ext_install_tpl: Option<&str> = b
        .extensions
        .as_ref()
        .map(|s| s.install.as_str())
        .filter(|s| !s.is_empty());

    let sandbox_comps: Vec<&(Component, std::path::PathBuf)> =
        resolved.iter().filter(|(c, _)| c.has_sandbox()).collect();
    let instance_comps: Vec<&(Component, std::path::PathBuf)> =
        resolved.iter().filter(|(c, _)| c.has_instance()).collect();

    // Пути instance-сокетов уже пробрасываются в base отдельным bind'ом
    // (instance_socket_binds_for_base). Повторно биндить их из хоста нельзя:
    // на хосте их может не быть, а порядок bind'ов ломает запуск base.
    let mut instance_paths: Vec<String> = Vec::new();
    for (c, _) in &instance_comps {
        if let Some(ds) = c.daemon_socket() {
            instance_paths.push(ds.to_string());
            if let Some(parent) = std::path::Path::new(ds).parent() {
                instance_paths.push(parent.to_string_lossy().into_owned());
            }
        }
    }
    let is_instance_path = |m: &str| {
        instance_paths
            .iter()
            .any(|p| m == p || m.starts_with(&format!("{p}/")))
    };

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
        let up = c.name.to_uppercase().replace('-', "_");
        let var_name = format!("SIF_{up}");
        let sb_var = format!("SB_{up}");

        // Путь sandbox'а включает hash SIF. Новая версия SIF получает
        // новый каталог, а старые instance'ы продолжают видеть свой
        // старый sandbox (никакого rm -rf поверх активного bind'а).
        materialize_lines.push(format!(
            "{sb_var}=\"$BUNDLE_DIR/sandboxes/{name}-$(_sif_hash \"${var_name}\")\"",
            sb_var = sb_var,
            name = c.name,
            var_name = var_name
        ));
        materialize_lines.push(format!(
            "_materialize \"${var_name}\" \"${sb_var}\" \"{name}\"",
            var_name = var_name,
            sb_var = sb_var,
            name = c.name
        ));
        // /opt/utils — один bind всего каталога sandboxes. Внутри
        // компонент доступен как symlink NAME -> NAME-HASH.
        materialize_lines.push(format!(
            "rm -rf \"$BUNDLE_DIR/sandboxes/{name}\" 2>/dev/null || true",
            name = c.name
        ));
        materialize_lines.push(format!(
            "ln -sfn \"$(basename \"${sb_var}\")\" \"$BUNDLE_DIR/sandboxes/{name}\"",
            sb_var = sb_var,
            name = c.name
        ));
        materialize_lines.push("mkdir -p \"$BUNDLE_DIR/sandboxes/.current\"".into());
        materialize_lines.push(format!(
            "printf '%s\\n' \"$(basename \"${sb_var}\")\" > \"$BUNDLE_DIR/sandboxes/.current/{name}\"",
            sb_var = sb_var,
            name = c.name
        ));
        materialize_lines.push(format!(
            "printf '%s\\n' \"$(basename \"${sb_var}\")\" > \"$BRIDGE_DIR/sandbox-{name}\"",
            sb_var = sb_var,
            name = c.name
        ));
        // Прямой bind под /opt/appspec (родитель /opt/appspec уже виден
        // через /opt/appspec/bin). Это не зависит от /opt/utils.
        sandbox_bind_lines.push(format!(
            "[ -d \"${sb_var}\" ] && BINDS=\"$BINDS --bind ${sb_var}:/opt/appspec/sandbox-{name}\" || true",
            sb_var = sb_var,
            name = c.name
        ));
        for m in c.mounts() {
            if !m.starts_with('/') {
                continue;
            }
            // Apptainer уже пробрасывает эти файлы в base/instance.
            if m == "/etc/resolv.conf" || m == "/etc/hosts" {
                continue;
            }
            if is_instance_path(&m) {
                continue;
            }
            let q = sh_quote(&m);
            if m == "/dev/kvm" {
                sandbox_bind_lines.push(format!(
                    "{{ [ -z \"${{APPSPEC_DISABLE_KVM:-}}\" ] && \
                     {{ [ -c {q} ] || [ -b {q} ]; }}; }} && \
                     BINDS=\"$BINDS --bind {q}:{q}\" || true",
                    q = q
                ));
            } else if m.starts_with("/dev/") {
                sandbox_bind_lines.push(format!(
                    "{{ [ -c {q} ] || [ -b {q} ]; }} && \
                     BINDS=\"$BINDS --bind {q}:{q}\" || true",
                    q = q
                ));
            } else {
                sandbox_bind_lines.push(format!(
                    "[ -e {q} ] && [ ! -d {q} ] && \
                     BINDS=\"$BINDS --bind {q}:{q}\" || true",
                    q = q
                ));
            }
        }
    }
    if !sandbox_comps.is_empty() {
        // Один bind на весь каталог sandboxes: внутри — hashed dirs и
        // symlink'и NAME -> NAME-HASH. Это надёжнее, чем bind каждого
        // /opt/utils/NAME (был случай, когда ansible-bind терялся).
        sandbox_bind_lines.insert(
            0,
            "[ -d \"$BUNDLE_DIR/sandboxes\" ] && \
             BINDS=\"$BINDS --bind $BUNDLE_DIR/sandboxes:/opt/utils\" || true"
                .into(),
        );
    }

    // Каждый bridge видит /opt/appspec/bin и sandbox'ы соседних компонентов,
    // поэтому цепочки вида tofu -> qemu-img -> libvirtd работают.
    let all_sandbox_mounts: Vec<String> = sandbox_comps.iter().map(|(c, _)| c.mount()).collect();
    let mut bridge_defs: Vec<(String, String, String)> = Vec::new();
    let mut used_bridge_files: HashSet<String> = HashSet::new();
    for (c, _) in &sandbox_comps {
        let mount = c.mount();
        let mut shared: Vec<String> = all_sandbox_mounts
            .iter()
            .filter(|m| m.as_str() != mount)
            .cloned()
            .collect();
        shared.push("/opt/appspec/bin".into());

        let primary_target = c.target();
        let file = unique_bridge_file(&primary_target, &mut used_bridge_files);
        let script = emit_bridge(c, &mount, &c.binary(), &primary_target, &shared, false);
        bridge_defs.push((file, primary_target, script));

        for bin in &c.bins {
            let target = bin.target();
            let file = unique_bridge_file(&target, &mut used_bridge_files);
            let script = emit_bridge(c, &mount, &bin.binary(), &target, &shared, true);
            bridge_defs.push((file, target, script));
        }
    }

    let mut bridges_block = String::new();
    if bridge_defs.is_empty() {
        bridges_block.push_str(": # bridges: пусто");
    } else {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        // Скрипты пишем внутрь base-instance (writable tmpfs), а не в
        // общий cache. Иначе старый запущенный instance видит новые
        // bridge'и из cache, но старые /opt/utils — и ломается.
        let _ = writeln!(bridges_block, "mkdir -p /opt/appspec/bin");
        for (file, _target, script) in &bridge_defs {
            let encoded = b64.encode(script.as_bytes());
            let _ = writeln!(
                bridges_block,
                "printf %s {} | base64 -d > \"/opt/appspec/bin/{}\"",
                sh_quote(&encoded),
                file
            );
            let _ = writeln!(bridges_block, "chmod 0755 \"/opt/appspec/bin/{}\"", file);
        }
        // Псевдонимы по basename target: PATH внутри sandbox ищет именно
        // `qemu-img`, а не уникальное имя файла bridge'а.
        for (file, target, _script) in &bridge_defs {
            let base = bridge_file_name(target);
            if &base != file {
                let _ = writeln!(
                    bridges_block,
                    "[ -e \"/opt/appspec/bin/{base}\" ] || ln -s \"{file}\" \"/opt/appspec/bin/{base}\""
                );
            }
        }
    }

    let mut bridge_links_block = String::new();
    if bridge_defs.is_empty() {
        bridge_links_block.push_str(": # bridge links: пусто");
    } else {
        for (file, target, _script) in &bridge_defs {
            let _ = writeln!(
                bridge_links_block,
                "mkdir -p \"$(dirname {})\" 2>/dev/null || true",
                sh_quote(target)
            );
            let _ = writeln!(
                bridge_links_block,
                "ln -sf /opt/appspec/bin/{} {}",
                file,
                sh_quote(target)
            );
        }
    }

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

        let mut effective_daemon: String = c
            .daemon()
            .map(ensure_podman_service_timeout)
            .unwrap_or_else(|| "podman system service --time=0".into());

        if is_podman_like(c, &ifiles) {
            let has_user_conf = ifiles
                .iter()
                .any(|f| f.to == "/root/.config/containers/containers.conf");
            if !has_user_conf {
                // CHANGED: content: Vec<u8>. `From<&str> for Vec<u8>` даёт
                // побайтовый UTF-8 — семантика та же, что была при String.
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

            let wrapper_content = podman_daemon_wrapper(&effective_daemon, &c.name);
            let wrapper_path = "/opt/appspec/podman-daemon-wrapper.sh";
            ifiles.push(ResolvedFile {
                to: wrapper_path.into(),
                // CHANGED: content: Vec<u8> — String требует явной конверсии.
                content: wrapper_content.into_bytes(),
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
            "SANDBOX_DIR=\"$SB_{}\"",
            c.name.to_uppercase().replace('-', "_")
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

    // --- настройки [runtime] базового спека -------------------------------
    let base_rt = base.runtime.as_ref();
    let base_bind_lines: Vec<String> = base_rt.map(build_bind_lines).unwrap_or_default();
    let mut base_bind_targets: HashSet<String> = HashSet::new();
    if let Some(rt) = base_rt {
        for b in rt.binds.iter().chain(rt.optional_binds.iter()) {
            base_bind_targets.insert(bind_target(b));
        }
    }
    let base_has_home_bind = base_bind_targets.contains("/config/home");
    let base_home_src: Option<String> = base_rt.and_then(|rt| {
        rt.binds
            .iter()
            .chain(rt.optional_binds.iter())
            .find(|b| bind_target(b.as_str()) == "/config/home")
            .map(|b| bind_source(b.as_str()))
    });
    let xauthority_auto = base_rt.and_then(|r| r.xauthority_auto).unwrap_or(true);
    let pulse_auto = base_rt.and_then(|r| r.pulse_auto).unwrap_or(true);
    let base_workdir = base_rt
        .and_then(|r| r.workdir.clone())
        .unwrap_or_else(|| "/config/home".into());

    let mut base_env_keys: Vec<String> = vec!["DISPLAY".into(), "SSH_AUTH_SOCK".into()];
    // Пробрасываем в base, чтобы скрипты (qemu в lesson'ах) могли
    // отключить KVM, даже если /dev/kvm всё-таки виден в namespace.
    base_env_keys.push("APPSPEC_DISABLE_KVM".into());
    if xauthority_auto {
        base_env_keys.push("XAUTHORITY".into());
    }
    if let Some(rt) = base_rt {
        if let Some(list) = &rt.env_passthrough {
            for k in list {
                base_env_keys.push(k.clone());
            }
        }
    }
    let mut seen_env: HashSet<String> = HashSet::new();
    base_env_keys.retain(|k| seen_env.insert(k.clone()));

    // --- base setup script (запускается как start-команда instance) -------
    let mut setup = String::new();
    {
        let sw = &mut setup;
        writeln!(sw, "set -eu")?;
        writeln!(sw, "export HOME=/config/home")?;
        writeln!(sw, "cd {} 2>/dev/null || cd /", sh_quote(&base_workdir))?;
        writeln!(sw, "mkdir -p /dev/shm 2>/dev/null || true")?;
        writeln!(sw, "chmod 1777 /dev/shm 2>/dev/null || true")?;
        writeln!(
            sw,
            "printf '%s\\n' \"${{APPSPEC_INSTANCE:-unknown}}\" > /opt/appspec/bin/.instance-id 2>/dev/null || true"
        )?;
        writeln!(sw)?;
        writeln!(sw, "# ===== base setup (source: {}) =====", base.origin)?;
        if base.pre.is_empty() {
            writeln!(sw, ":")?;
        } else {
            writeln!(sw, "{}", base.pre)?;
        }
        writeln!(sw)?;
        writeln!(sw, "# ===== bridges =====")?;
        writeln!(sw, "{}", bridges_block)?;
        writeln!(sw)?;
        writeln!(sw, "{}", bridge_links_block)?;
        writeln!(sw)?;
        writeln!(
            sw,
            "echo '  bridges: /opt/appspec/bin' >&2; ls -1 /opt/appspec/bin 2>/dev/null | sed 's/^/    /' >&2 || true"
        )?;
        writeln!(sw)?;
        writeln!(sw, "# ===== component files (base) =====")?;
        writeln!(sw, "{}", base_files_block)?;
        writeln!(sw)?;
        writeln!(sw, "# ===== extensions =====")?;
        if prepared_exts.is_empty() {
            writeln!(sw, ":")?;
        } else {
            // VS Code/VSCodium с profile-манифестом `extensions.json`
            // доверяет ему. Если файл остался пустым (`[]`) от прошлого
            // запуска, каталоги расширений игнорируются, и GUI пишет
            // «no installed extensions». Удаляем манифест перед стартом,
            // чтобы редактор пересканировал каталог и пересобрал его.
            writeln!(
                sw,
                "rm -f {} 2>/dev/null || true",
                sh_quote(&format!(
                    "{}/extensions.json",
                    ext_mount_dir.trim_end_matches('/')
                ))
            )?;
            for e in prepared_exts {
                writeln!(
                    sw,
                    "echo '  ready: {} v{} ({})' >&2",
                    e.id,
                    e.version,
                    e.kind.as_str()
                )?;
            }
            writeln!(sw, "echo '  extensions dir: {}' >&2", ext_mount_dir)?;
            writeln!(
                sw,
                "ls -1 {} 2>/dev/null | sed 's/^/    /' >&2 || true",
                sh_quote(&ext_mount_dir)
            )?;
            if let Some(tpl) = ext_install_tpl {
                for e in prepared_exts {
                    if e.kind != ExtensionKind::File {
                        continue;
                    }
                    let cmd = build_install_command(tpl, &ext_mount_dir, e);
                    writeln!(sw, "echo '  install: {}' >&2", sh_quote(&cmd))?;
                    writeln!(sw, "yes yes 2>/dev/null | {}", cmd)?;
                }
            }
        }
        writeln!(sw)?;
        writeln!(sw, "# ===== selftests =====")?;
        writeln!(sw, "{}", selftests)?;
        writeln!(sw)?;
        writeln!(sw, "# ===== launch =====")?;
        writeln!(sw, "{}", base.entry)?;
    }

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

    writeln!(w, "# ---- bundle cache / state ----")?;
    writeln!(w, "BUNDLE_NAME={}", sh_quote(&b.name))?;
    writeln!(
        w,
        "BUNDLE_CACHE_ROOT=\"${{APPSPEC_BUNDLE_CACHE:-${{XDG_CACHE_HOME:-$HOME/.cache}}/appspec/bundles}}\""
    )?;
    writeln!(w, "BUNDLE_DIR=\"$BUNDLE_CACHE_ROOT/$BUNDLE_NAME\"")?;
    writeln!(
        w,
        "BUNDLE_STATE_ROOT=\"${{APPSPEC_BUNDLE_HOME:-${{XDG_DATA_HOME:-$HOME/.local/share}}/appspec/bundles}}\""
    )?;
    writeln!(w, "BUNDLE_STATE=\"$BUNDLE_STATE_ROOT/$BUNDLE_NAME\"")?;
    writeln!(w, "BUNDLE_HOME=\"$BUNDLE_STATE/home\"")?;
    writeln!(w, "mkdir -p \"$BUNDLE_DIR/sandboxes\" \"$BUNDLE_DIR/extensions\" \"$BUNDLE_DIR/run\" \"$BUNDLE_DIR/state\"")?;
    if !base_has_home_bind {
        writeln!(w, "mkdir -p \"$BUNDLE_HOME\"")?;
    }
    writeln!(w, "INSTANCE_NAME=\"$BUNDLE_NAME-$(date +%s)\"")?;
    writeln!(w, "BRIDGE_DIR=\"$BUNDLE_DIR/run/$INSTANCE_NAME/bin\"")?;
    writeln!(w, "mkdir -p \"$BRIDGE_DIR\"")?;
    writeln!(
        w,
        "printf '%s\\n' \"$INSTANCE_NAME\" > \"$BRIDGE_DIR/.instance-id\""
    )?;
    writeln!(w)?;
    writeln!(
        w,
        "# ---- stop stale instances of this bundle (before sandbox refresh) ----"
    )?;
    writeln!(w, "for _stale in $(apptainer instance list 2>/dev/null \\")?;
    writeln!(w, "        | awk 'NR>1 && NF>=1 {{print $1}}' \\")?;
    writeln!(w, "        | grep \"^$BUNDLE_NAME-\" || true); do")?;
    writeln!(w, "    echo \"  stopping stale instance: $_stale\" >&2")?;
    writeln!(
        w,
        "    apptainer instance stop \"$_stale\" 2>/dev/null || true"
    )?;
    writeln!(w, "done")?;
    writeln!(w)?;

    // --- [runtime] base: vars / state_dirs / create_dirs -------------------
    if let Some(rt) = base_rt {
        if !rt.vars.is_empty() {
            writeln!(w, "# ---- base runtime vars ----")?;
            for (k, v) in &rt.vars {
                writeln!(w, "{k}={}", dq(v))?;
            }
            writeln!(w)?;
        }
        if !rt.state_dirs.is_empty() {
            writeln!(w, "# ---- base runtime state dirs ----")?;
            for d in &rt.state_dirs {
                writeln!(w, "mkdir -p \"$CONFIG_DIR/{d}\"")?;
            }
            writeln!(w)?;
        }
        if !rt.create_dirs.is_empty() {
            writeln!(w, "# ---- base runtime create_dirs ----")?;
            for d in &rt.create_dirs {
                writeln!(w, "mkdir -p {} 2>/dev/null || true", sh_quote(d))?;
            }
            writeln!(w)?;
        }
    }

    // Каталог расширений может лежать внутри bind'а /config/home.
    // Создаём его на хосте до старта instance, иначе bind target может
    // оказаться скрыт home-bind'ом и VSCodium не увидит расширения.
    if let Some(rel) = ext_mount_dir.strip_prefix("/config/home") {
        if rel.is_empty() || rel.starts_with('/') {
            if let Some(src) = &base_home_src {
                let p = format!("{}{}", src.trim_end_matches('/'), rel);
                writeln!(w, "mkdir -p {} 2>/dev/null || true", dq(&p))?;
                writeln!(w)?;
            } else if !base_has_home_bind {
                writeln!(w, "mkdir -p \"$BUNDLE_HOME{}\" 2>/dev/null || true", rel)?;
                writeln!(w)?;
            }
        }
    }

    writeln!(w, "# ---- companion sandboxes ----")?;
    writeln!(w, "_sif_hash() {{")?;
    writeln!(
        w,
        "    _h=\"$(sha256sum \"$1\" 2>/dev/null | cut -c1-12 || true)\""
    )?;
    writeln!(
        w,
        "    [ -n \"$_h\" ] || _h=\"$(stat -c '%s-%Y' \"$1\" 2>/dev/null | tr -d ' -' || true)\""
    )?;
    writeln!(w, "    printf '%s' \"${{_h:-0}}\"")?;
    writeln!(w, "}}")?;
    writeln!(w, "_materialize() {{")?;
    writeln!(w, "    _sif=\"$1\"; _dst=\"$2\"; _lbl=\"$3\"")?;
    writeln!(
        w,
        "    [ -f \"$_sif\" ] || {{ echo \"FATAL: нет SIF: $_sif\" >&2; exit 1; }}"
    )?;
    writeln!(w, "    if [ -f \"$_dst/.ready\" ]; then")?;
    writeln!(w, "        return 0")?;
    writeln!(w, "    fi")?;
    writeln!(w, "    echo \"=== sandbox[$_lbl]: $_sif -> $_dst ===\" >&2")?;
    writeln!(w, "    rm -rf \"$_dst\"")?;
    writeln!(
        w,
        "    apptainer build --sandbox \"$_dst\" \"$_sif\" >/dev/null"
    )?;
    writeln!(w, "    : > \"$_dst/.ready\"")?;
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
    // bind'ы из [runtime] базового спека (vscodium: $CONFIG_DIR/home:/config/home и т.п.)
    for l in &base_bind_lines {
        writeln!(w, "{}", l)?;
    }
    if !base_bind_targets.contains("/tmp/.X11-unix") {
        writeln!(w, "[ -e \"/tmp/.X11-unix\" ] && BINDS=\"$BINDS --bind /tmp/.X11-unix:/tmp/.X11-unix\" || true")?;
    }
    if !base_bind_targets.contains("/dev/dri") {
        writeln!(w, "[ -e \"/dev/dri\" ]         && BINDS=\"$BINDS --bind /dev/dri:/dev/dri\"            || true")?;
    }
    if !base_bind_targets.contains("/dev/snd") {
        writeln!(w, "[ -e \"/dev/snd\" ]         && BINDS=\"$BINDS --bind /dev/snd:/dev/snd\"            || true")?;
    }
    if xauthority_auto && !base_bind_targets.contains("$XAUTHORITY") {
        writeln!(w, "[ -n \"${{XAUTHORITY:-}}\" ]  && BINDS=\"$BINDS --bind $XAUTHORITY:$XAUTHORITY\"      || true")?;
    }
    if !base_bind_targets.contains("$SSH_AUTH_SOCK") {
        writeln!(
            w,
            "if [ -n \"${{SSH_AUTH_SOCK:-}}\" ] && [ -S \"$SSH_AUTH_SOCK\" ]; then"
        )?;
        writeln!(
            w,
            "    BINDS=\"$BINDS --bind $SSH_AUTH_SOCK:$SSH_AUTH_SOCK\""
        )?;
        writeln!(w, "fi")?;
    }
    if pulse_auto && !base_bind_targets.contains("/tmp/pulse-native") {
        writeln!(
            w,
            "if [ -n \"${{XDG_RUNTIME_DIR:-}}\" ] && [ -e \"$XDG_RUNTIME_DIR/pulse/native\" ]; then"
        )?;
        writeln!(
            w,
            "    BINDS=\"$BINDS --bind $XDG_RUNTIME_DIR/pulse/native:/tmp/pulse-native\""
        )?;
        writeln!(w, "fi")?;
    }
    if !base_has_home_bind {
        writeln!(
            w,
            "[ -d \"$BUNDLE_HOME\" ] && BINDS=\"$BINDS --bind $BUNDLE_HOME:/config/home\" || true"
        )?;
    }
    writeln!(w, "[ -d \"$BUNDLE_DIR/extensions\" ] && \\")?;
    writeln!(
        w,
        "    BINDS=\"$BINDS --bind $BUNDLE_DIR/extensions:{}\" || true",
        ext_mount_dir
    )?;
    writeln!(
        w,
        "[ -d \"$BRIDGE_DIR\" ] && BINDS=\"$BINDS --bind $BRIDGE_DIR:/opt/appspec/bin\" || true"
    )?;
    writeln!(w, "[ -d \"$BUNDLE_DIR/sandboxes\" ] && BINDS=\"$BINDS --bind $BUNDLE_DIR/sandboxes:/opt/appspec/sandboxes\" || true")?;
    for l in &instance_socket_binds_for_base {
        writeln!(w, "{}", l)?;
    }
    for l in &sandbox_bind_lines {
        writeln!(w, "{}", l)?;
    }
    writeln!(w)?;

    writeln!(w, "X_ENV=\"\"")?;
    for k in &base_env_keys {
        writeln!(
            w,
            "[ -n \"${{{k}:-}}\" ] && X_ENV=\"$X_ENV --env {k}=${k}\" || true"
        )?;
    }
    if pulse_auto && !base_bind_targets.contains("/tmp/pulse-native") {
        writeln!(
            w,
            "[ -e \"${{XDG_RUNTIME_DIR:-/nonexistent}}/pulse/native\" ] && \\"
        )?;
        writeln!(
            w,
            "    X_ENV=\"$X_ENV --env PULSE_SERVER=unix:/tmp/pulse-native\" || true"
        )?;
    }
    for (k, v) in &instance_env_for_base {
        writeln!(w, "X_ENV=\"$X_ENV --env {}={}\"", k, sh_quote(v))?;
    }
    writeln!(w)?;

    writeln!(w, "# ---- instance (base) ----")?;
    {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(setup.as_bytes());
        writeln!(
            w,
            "printf %s {} | base64 -d > \"$BRIDGE_DIR/setup.sh\"",
            sh_quote(&b64)
        )?;
        writeln!(w, "chmod 0755 \"$BRIDGE_DIR/setup.sh\"")?;
        writeln!(w)?;
    }
    writeln!(w, "apptainer instance start \\")?;
    writeln!(w, "    --writable-tmpfs \\")?;
    writeln!(w, "    --fakeroot \\")?;
    writeln!(w, "    --contain \\")?;
    writeln!(w, "    $BINDS \\")?;
    writeln!(w, "    $X_ENV \\")?;
    writeln!(w, "    --env APPSPEC_INSTANCE=\"$INSTANCE_NAME\" \\")?;
    writeln!(w, "    \"$BASE_SIF\" \\")?;
    writeln!(w, "    \"$INSTANCE_NAME\" \\")?;
    writeln!(w, "    /bin/sh /opt/appspec/bin/setup.sh")?;
    writeln!(w)?;
    writeln!(w, "cleanup() {{")?;
    writeln!(
        w,
        "    apptainer instance stop \"$INSTANCE_NAME\" 2>/dev/null || true"
    )?;
    writeln!(w, "    rm -rf \"$BRIDGE_DIR\" 2>/dev/null || true")?;
    write!(w, "{}", instance_cleanup)?;
    writeln!(w)?;
    writeln!(w, "}}")?;
    writeln!(w, "trap cleanup EXIT INT TERM")?;
    writeln!(w)?;

    writeln!(w, "# ---- wait for base instance (codium) ----")?;
    writeln!(w, "while apptainer instance list 2>/dev/null \\")?;
    writeln!(w, "    | awk 'NR>1 && NF>=1 {{print $1}}' \\")?;
    writeln!(w, "    | grep -qx \"$INSTANCE_NAME\"; do")?;
    writeln!(w, "    sleep 1")?;
    writeln!(w, "done")?;
    writeln!(w)?;

    Ok(s)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod install_command_tests {
    use super::*;
    use crate::bundle::model::ExtensionKind;
    use std::path::PathBuf;

    fn fake_ext(kind: ExtensionKind, id: &str, ver: &str, file_name: &str) -> PreparedExtension {
        PreparedExtension {
            id: id.into(),
            version: ver.into(),
            path: PathBuf::from(format!("/cache/extensions/{file_name}")),
            kind,
            source: "test".into(),
            dependencies: Vec::new(),
            extension_pack: Vec::new(),
            engines_vscode: None,
            requires: Vec::new(),
        }
    }

    #[test]
    fn install_command_substitutes_target_legacy_alias() {
        let ext = fake_ext(
            ExtensionKind::Vsix,
            "opentofu.vscode-opentofu",
            "0.6.3",
            "opentofu.vscode-opentofu-0.6.3.vsix",
        );
        let cmd = build_install_command(
            "codium --install-extension {target} --force",
            "/opt/extensions",
            &ext,
        );
        assert_eq!(
            cmd,
            "codium --install-extension /opt/extensions/opentofu.vscode-opentofu-0.6.3.vsix --force"
        );
        assert!(!cmd.contains("{target}"), "literal {{target}} left: {cmd}");
    }

    #[test]
    fn install_command_substitutes_file_and_id_and_version() {
        let ext = fake_ext(
            ExtensionKind::File,
            "grammalecte",
            "1",
            "Grammalecte-fr-v2.1.3.oxt",
        );
        let cmd = build_install_command(
            "unopkg add --shared {file}  # {id} v{version}",
            "/opt/extensions/",
            &ext,
        );
        assert_eq!(
            cmd,
            "unopkg add --shared /opt/extensions/Grammalecte-fr-v2.1.3.oxt  # grammalecte v1"
        );
        assert!(!cmd.contains('{'));
    }

    #[test]
    fn install_command_trims_trailing_slash_in_mount_dir() {
        let ext = fake_ext(ExtensionKind::File, "x", "1", "x.oxt");
        let a = build_install_command("cmd {file}", "/opt/extensions/", &ext);
        let b = build_install_command("cmd {file}", "/opt/extensions", &ext);
        assert_eq!(a, b);
        assert_eq!(a, "cmd /opt/extensions/x.oxt");
    }
}

#[cfg(test)]
mod content_tests {
    use super::*;
    use crate::bundle::model::{FileScope, ResolvedFile};

    /// `/dev/kvm` не должен пробрасываться в chroot satellite, когда
    /// KVM отключён через APPSPEC_DISABLE_KVM.
    #[test]
    fn dev_kvm_bind_respects_disable_env() {
        let c: Component = toml::from_str(
            r#"
name = "virt"
spec = "./virt.appspec"
binary = "/usr/bin/qemu-system-x86_64"
mounts = ["/dev/kvm"]
"#,
        )
        .expect("component");
        let sh = emit_bridge(
            &c,
            "/opt/utils/virt",
            "/usr/bin/qemu-system-x86_64",
            "/usr/local/bin/qemu-system-x86_64",
            &[] as &[String],
            false,
        );
        assert!(
            sh.contains("APPSPEC_DISABLE_KVM"),
            "kvm bind must honor APPSPEC_DISABLE_KVM:\n{sh}"
        );
        assert!(
            sh.contains("mount_bind /dev \"$SANDBOX/dev\""),
            "kvm bind must mount /dev, not a placeholder:\n{sh}"
        );
    }

    /// `bridge = "exec"` не должен ни монтировать, ни делать chroot:
    /// он запускает бинарь satellite'а прямо в namespace base.
    #[test]
    fn exec_bridge_runs_binary_without_chroot() {
        let c: Component = toml::from_str(
            r#"
name = "opentofu"
spec = "./opentofu.appspec"
binary = "/usr/local/bin/tofu"
target = "/usr/local/bin/tofu"
bridge = "exec"
"#,
        )
        .expect("component");
        let sh = emit_bridge(
            &c,
            "/opt/utils/opentofu",
            "/usr/local/bin/tofu",
            "/usr/local/bin/tofu",
            &[] as &[String],
            false,
        );
        assert!(
            sh.contains("BIN=\"$SANDBOX/usr/local/bin/tofu\""),
            "exec bridge must run satellite binary:\n{sh}"
        );
        assert!(
            sh.contains("exec \"$BIN\" \"$@\""),
            "exec bridge must exec binary:\n{sh}"
        );
        assert!(
            !sh.contains("mount_bind"),
            "exec bridge must not mount:\n{sh}"
        );
        assert!(!sh.contains("chroot"), "exec bridge must not chroot:\n{sh}");
        assert!(
            sh.contains(":/opt/appspec/bin"),
            "exec bridge must expose sibling bridges in PATH:\n{sh}"
        );
    }

    /// base64-кодирование `Vec<u8>` в `emit_files_for` должно давать
    /// то же, что `base64 -d` восстановит побайтово.
    /// Регресс на `String → Vec<u8>` (бинарные `.apk`, `.jar`, `.p12`).
    #[test]
    fn emit_files_for_roundtrips_binary_via_base64() {
        let bytes: Vec<u8> = (0u16..=255).map(|b| b as u8).collect();
        let files = vec![ResolvedFile {
            to: "/tmp/blob.bin".into(),
            content: bytes.clone(),
            mode: 0o644,
            source: "test".into(),
            scopes: vec![FileScope::Base],
            component: "t".into(),
        }];
        let sh = emit_files_for(&files, FileScope::Base, None, "test");

        // Извлекаем base64 из `printf %s <b64> | base64 -d > "/tmp/blob.bin"`
        let b64 = sh
            .split("printf %s ")
            .nth(1)
            .and_then(|s| s.split(' ').next())
            .expect("base64 token");
        use base64::Engine as _;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .expect("base64 decode");
        assert_eq!(decoded, bytes, "binary content must roundtrip");
    }

    /// Тот же тест для sandbox-scope и prefix_var — другая ветка кода
    /// в `emit_files_for` (t вычисляется как `"$PREFIX/to"`).
    #[test]
    fn emit_files_for_prefix_var_roundtrips_binary() {
        let bytes = b"\x00\x01\x02\xff\xfe\xfd".to_vec();
        let files = vec![ResolvedFile {
            to: "/etc/foo.bin".into(),
            content: bytes.clone(),
            mode: 0o600,
            source: "test".into(),
            scopes: vec![FileScope::Sandbox],
            component: "t".into(),
        }];
        let sh = emit_files_for(&files, FileScope::Sandbox, Some("$SANDBOX_DIR"), "test");
        let b64 = sh
            .split("printf %s ")
            .nth(1)
            .and_then(|s| s.split(' ').next())
            .expect("base64 token");
        use base64::Engine as _;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .expect("base64 decode");
        assert_eq!(decoded, bytes);
        assert!(sh.contains("\"$SANDBOX_DIR/etc/foo.bin\""));
        assert!(sh.contains("chmod 600"));
    }

    /// `instance_storage_fp` должен работать на `&[u8]` (раньше — `.bytes()`
    /// на `String`). Два прогона на одних данных → одинаковый fp;
    /// изменение байта → другой fp.
    #[test]
    fn instance_storage_fp_works_on_bytes() {
        let a = ResolvedFile {
            to: "/a".into(),
            content: vec![1, 2, 3, 4],
            mode: 0o644,
            source: "t".into(),
            scopes: vec![FileScope::Instance],
            component: "c".into(),
        };
        let b = ResolvedFile {
            content: vec![1, 2, 3, 5],
            ..a.clone()
        };
        let fp_a1 = instance_storage_fp(std::slice::from_ref(&a));
        let fp_a2 = instance_storage_fp(std::slice::from_ref(&a));
        let fp_b = instance_storage_fp(std::slice::from_ref(&b));
        assert_eq!(fp_a1, fp_a2);
        assert_ne!(fp_a1, fp_b);
    }
}
