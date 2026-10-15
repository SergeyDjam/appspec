//! Поддержка макросов в стиле RPM spec: `%{name}`, `%{?name}`, `%{name:def}`.
//!
//! Макросы раскрываются на этапе `load_spec`, до десериализации в `Spec`.
//! Так работают и на полях вида `build.collect.binary`, и внутри
//! многострочных `setup`/`entry` — поскольку `%{…}` не пересекается с
//! shell-переменными (`${…}`), в скриптах они безопасны.
//!
//! Пользовательские макросы задаются в секции `[macros]` спека и могут
//! ссылаться друг на друга и на встроенные.

use std::collections::BTreeMap;

use anyhow::{bail, Result};

// ===========================================================================
// Точка входа
// ===========================================================================

pub fn expand_all(root: &mut toml::Value) -> Result<()> {
    let builtins = compute_builtins(root);

    // Собираем [macros] как map<String, String>.
    let mut user: BTreeMap<String, String> = BTreeMap::new();
    if let Some(t) = root.get("macros").and_then(|v| v.as_table()) {
        for (k, v) in t {
            if let Some(s) = v.as_str() {
                user.insert(k.clone(), s.to_string());
            }
        }
    }

    // Сливаем: сначала builtins, поверх — user (user выигрывает).
    let mut all: BTreeMap<String, String> = builtins;
    for (k, v) in user {
        all.insert(k, v);
    }

    // Стабилизируем значения самих макросов — они могут ссылаться
    // друг на друга и на встроенные. Ограничение от циклов.
    let mut depth = 0;
    loop {
        depth += 1;
        if depth > 16 {
            bail!("macros: слишком глубокая вложенность (похоже на цикл)");
        }
        let mut changed = false;
        let snapshot = all.clone();
        for (k, v) in snapshot {
            let expanded = expand_str(&v, &all);
            if expanded != v {
                all.insert(k, expanded);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Раскрываем по всему дереву, кроме [macros] (он уже канонический).
    if let Some(tbl) = root.as_table_mut() {
        for (k, v) in tbl.iter_mut() {
            if k == "macros" {
                continue;
            }
            expand_value(v, &all);
        }
    }

    Ok(())
}

// ===========================================================================
// Обход дерева
// ===========================================================================

fn expand_value(v: &mut toml::Value, m: &BTreeMap<String, String>) {
    match v {
        toml::Value::String(s) => *s = expand_str(s, m),
        toml::Value::Array(a) => {
            for x in a {
                expand_value(x, m);
            }
        }
        toml::Value::Table(t) => {
            for (_, x) in t.iter_mut() {
                expand_value(x, m);
            }
        }
        _ => {}
    }
}

// ===========================================================================
// Разбор и раскрытие одной строки
// ===========================================================================

fn expand_str(s: &str, m: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('%') => {
                chars.next();
                out.push('%');
            }
            Some('{') => {
                chars.next();
                let mut inner = String::new();
                let mut closed = false;
                for c2 in chars.by_ref() {
                    if c2 == '}' {
                        closed = true;
                        break;
                    }
                    inner.push(c2);
                }
                if !closed {
                    out.push_str("%{");
                    out.push_str(&inner);
                    continue;
                }

                let (name, default, optional) = parse_macro(&inner);
                if name.is_empty() {
                    out.push_str("%{");
                    out.push_str(&inner);
                    out.push('}');
                    continue;
                }

                match m.get(&name) {
                    Some(v) => out.push_str(v),
                    None => match (default, optional) {
                        (Some(d), _) => out.push_str(&d),
                        (None, true) => { /* %{?name}: пусто */ }
                        (None, false) => {
                            // Неизвестный макрос: оставляем как есть,
                            // чтобы опечатку было видно.
                            out.push_str("%{");
                            out.push_str(&inner);
                            out.push('}');
                        }
                    },
                }
            }
            _ => out.push('%'),
        }
    }

    out
}

/// `?name` → (?name, false), `name` → (name, false),
/// `name:default` → (name, Some(default)).
fn parse_macro(inner: &str) -> (String, Option<String>, bool) {
    let (name_part, default) = match inner.split_once(':') {
        Some((n, d)) => (n, Some(d.to_string())),
        None => (inner, None),
    };
    if let Some(n) = name_part.strip_prefix('?') {
        (n.to_string(), default, true)
    } else {
        (name_part.to_string(), default, false)
    }
}

// ===========================================================================
// Встроенные макросы
// ===========================================================================

fn compute_builtins(root: &toml::Value) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();

    // [package]
    let pkg = root.get("package").and_then(|v| v.as_table());
    let name = pkg
        .and_then(|p| p.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let version = pkg
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let bin = pkg
        .and_then(|p| p.get("bin"))
        .and_then(|v| v.as_str())
        .unwrap_or(name);

    m.insert("name".into(), name.into());
    m.insert("version".into(), version.into());
    m.insert("bin".into(), bin.into());

    // [build]
    let build = root.get("build").and_then(|v| v.as_table());
    let base = build
        .and_then(|b| b.get("base"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let arch = build
        .and_then(|b| b.get("arch"))
        .and_then(|v| v.as_str())
        .unwrap_or("amd64");

    m.insert("base".into(), base.into());
    m.insert("arch".into(), arch.into());

    // Дистрибутив-специфичные пути.
    // alpine    → /usr/lib
    // debian    → /usr/lib/x86_64-linux-gnu
    // rpm-*     → /usr/lib64
    let (_prefix, libdir) = if base.contains("alpine") {
        ("/usr", "/usr/lib")
    } else if base.contains("debian") || base.contains("ubuntu") {
        ("/usr", "/usr/lib/x86_64-linux-gnu")
    } else {
        ("/usr", "/usr/lib64")
    };

    m.insert("_prefix".into(), "/usr".into());
    m.insert("_bindir".into(), "/usr/bin".into());
    m.insert("_sbindir".into(), "/usr/sbin".into());
    m.insert("_libdir".into(), libdir.into());
    m.insert("_libexecdir".into(), "/usr/libexec".into());
    m.insert("_datadir".into(), "/usr/share".into());
    m.insert("_sysconfdir".into(), "/etc".into());
    m.insert("_localstatedir".into(), "/var".into());
    m.insert("_rundir".into(), "/run".into());
    m.insert("_tmppath".into(), "/tmp".into());

    m
}
