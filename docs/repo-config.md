# Конфигурация `appspec repo`

Phase 1: **два независимых конфига**, оба вне repository root.

| Файл | Что описывает | Кто использует |
|---|---|---|
| `repo-server.toml` | Один репозиторий: `root`, `policy`, `signing`, `server.listen`, `tokens`, `tls` | `serve`, `add`, `check`, `gc`, `reindex`, `sign`, `publish-index`, `export-oci`, `token`, `server init`, `migrate-config` |
| `repos.toml` | Профили источников: `path` \| `url` \| `ipfs`, `token`, `trusted_keys`, `proxy` | `pull`, `push`, `search`, `ls`, `show`, `verify`, `client init`, `client ls` |

Repository root — **data-only**: `index.json`, `blobs/`, `packages/`, `refs/`, `.lock`. `config.toml` внутри root **не создаётся**; если он там есть — это legacy, см. [migrate-config](#migrate-config).

---

## Server config: `repo-server.toml`

### Discovery

1. `--config PATH` (alias `--server-config PATH`, короткий `-c PATH`)
2. `$APPSPEC_REPO_SERVER_CONFIG`
3. `$XDG_CONFIG_HOME/appspec/repo-server.toml` (обычно `~/.config/appspec/repo-server.toml`)
4. `/etc/appspec/repo-server.toml`
5. **legacy** `<root>/config.toml` — warning, но читается
6. ничего не найдено — defaults (data-only репо; работает для `add`/`check`/`gc`/`reindex`/`sign`, но `push` не пройдёт без токенов)

Если `<root>/config.toml` найден, печатается:

```
warning: /srv/repo/config.toml is deprecated; run `appspec repo migrate-config /srv/repo`
```

### Формат

```toml
schema = 1

[repo]
root        = "/var/lib/appspec/repo"      # ОБЯЗАТЕЛЬНО, абсолютный
name        = "corp/stable"
description = "Stable channel"
created_at  = "2026-11-09T00:00:00Z"
generator   = "appspec 0.4.0"

[policy]
immutable_versions = true
allow_force        = false
auto_tag_latest    = "semver"

[signing]
key    = "/etc/appspec/signing.key"        # опционально
pubkey = "ed25519:…"                        # обновляется `repo sign`

[server]
listen             = "127.0.0.1:8080"
max_push_bytes     = 2147483648
read_timeout_secs  = 600
write_timeout_secs = 600

[[server.tokens]]
name = "ci"
hash = "sha256:…"                          # только sha256

[server.tls]
cert = "/etc/appspec/tls/cert.pem"
key  = "/etc/appspec/tls/key.pem"
```

### Инварианты

* `schema = 1` — обязателен. Несовпадение → hard error.
* `[repo].root` — обязателен, абсолютный (после `~`-раскрытия). Относительный → `[repo].root must be absolute`.
* `[repo].name` — обязателен, непустой.
* Файл пишется с mode `0600`.

### Когда создаётся

| Команда | Что делает |
|---|---|
| `repo init <root> --config PATH` | Создаёт data-only root + server config |
| `repo init <root>` (без `--config`) | Только data-only root; подсказка `server init` |
| `repo server init --root <root> [--config PATH]` | Создаёт server config для существующего data-only root |
| `repo migrate-config <root> [--config PATH]` | Legacy `<root>/config.toml` → `repo-server.toml` |
| `repo sign [--config PATH]` | Обновляет `[signing].pubkey` |
| `repo token gen NAME [--config PATH]` | Добавляет `[[server.tokens]]` |

---

## Client config: `repos.toml`

### Discovery

1. `--client-config PATH` (только на `client init` / `client ls`)
2. `$APPSPEC_REPO_CLIENT_CONFIG`
3. `$XDG_CONFIG_HOME/appspec/repos.toml`
4. `/etc/appspec/repos.toml`

Если файла нет — команды `search`/`ls`/`show`/`verify`/`pull`/`push` идут в fallback: `$APPSPEC_REPO` → `default_root()`.

### Формат

```toml
schema = 1
default = "local"

[[repos]]
name   = "local"
source = { path = "/var/lib/appspec/repo" }

[[repos]]
name   = "corp"
source = { url = "http://repo.corp.example:8080" }
token  = "env:APPSPEC_REPO_TOKEN"       # индирекция, не plaintext
proxy  = "http://proxy.corp:3128"       # задел на v2, в v1 не используется
trusted_keys   = ["file:~/.config/appspec/trusted-keys.toml"]
allow_unsigned = false

[[repos]]
name   = "mirror"
source = { path = "/srv/appspec-mirror" }
```

### `source` — ровно одно из

| Поле | Формат | v1 |
|---|---|---|
| `path` | `/abs` или `~/…` | да |
| `url`  | `http://…` или `https://…` | да (https — reverse proxy) |
| `ipfs` | `bafy…` | задел; read-команды возвращают ошибку |

`kind_count() != 1` → `source must contain exactly one of path | url | ipfs`.

### `token` — индирекция

| Форма | Значение |
|---|---|
| `env:VAR` | `$VAR`. Ошибка, если переменная не задана. |
| `file:PATH` | Содержимое файла, `trim()`. |
| `literal:…` | Plaintext. Печатает warning. |
| `keyring:SERVICE` | **v2**; в v1 — `keyring backend not implemented`. |

### Создание / инспекция

```sh
appspec repo client init \
    --profile corp \
    --source-url http://repo.corp:8080 \
    --token-env APPSPEC_REPO_TOKEN \
    --default

appspec repo client init \
    --profile local \
    --source-path /var/lib/appspec/repo

appspec repo client ls
appspec repo client ls --json

# Явный путь к client config
appspec repo client init --client-config /etc/appspec/repos.toml --profile …
appspec repo client ls   --client-config /etc/appspec/repos.toml
```

### Ошибки

| Ситуация | Сообщение |
|---|---|
| Файл есть, но не парсится | `client config exists but could not be loaded; fix it …` + TOML-ошибка |
| Файла нет | `client config not found; looked in: …` |
| Дубликат `name` | `duplicate profile …` |
| `default` на несуществующий | `default 'x' is not a known profile; known: […]` |
| `source` с двумя полями | `exactly one of path \| url \| ipfs` |
| `source.url` не http(s) | `must start with http:// or https://` |

---

## Разрешение источника

Команды `pull`, `push`, `search`, `ls`, `show`, `verify` резолвят в порядке:

1. `--profile NAME` → профиль из client config;
2. `--root PATH` \| `--to URL` \| `--from URL|PATH` — явно;
3. `$APPSPEC_REPO` — path, URL или **имя профиля** (различаем по `://`, началу с `/`, `~`, `.`);
4. `default` в client config;
5. error с подсказками.

### `$APPSPEC_REPO` — три роли

| Значение | Что значит |
|---|---|
| `/var/lib/repo` или `~/repo` | Путь к data-директории |
| `http://host:8080` | URL сервера (read-команды, pull-источник, push-приёмник) |
| `corp` | Имя профиля в client config |

Если `corp` не найден как профиль и не похож на путь — трактуется как относительный путь.

### `$APPSPEC_REPO_TOKEN` — только токен

**Не** destination. Многие путают: `APPSPEC_REPO_TOKEN=… appspec repo push pkg.sif` **без** `$APPSPEC_REPO=http://…` даст `no destination`.

---

## Токены (server-side)

`repo token gen NAME` — **не** создаёт профиль. Создаёт запись `[[server.tokens]]` в server config и печатает plaintext **один раз**.

### Жизненный цикл

```sh
# 1. Сгенерировать. Plaintext — в stdout, только один раз.
appspec repo token gen ci
# → token ci: 034f…
# → (save it now; only sha256 is stored in ~/.config/appspec/repo-server.toml)

# 2. Сразу в файл — для CI.
appspec repo token gen ci --write-file /run/secrets/ci --mode 0400

# 3. Инспекция (без plaintext!)
appspec repo token ls

# 4. Проверка
appspec repo token check ci --value 034f…
appspec repo token check ci --file /run/secrets/ci

# 5. Удаление
appspec repo token rm ci
```

### `--mode` — восьмеричный, как `chmod`

| Ввод | Значение |
|---|---|
| `400` | `0o400` = 256 |
| `0400` | `0o400` = 256 |
| `0o400` | `0o400` = 256 |
| `999` | ошибка `--mode must be octal` |
| `1777` | ошибка `--mode out of range` |

По умолчанию — `0o400` (`r--------`).

### Серверная сторона

`repo token gen` пишет **только sha256**. Проверка — `verify_token` в `server.rs` через `sha256(provided)` и `constant_time_eq`.

---

## `--profile` vs имя токена

Это **разные сущности**, и это самая частая путаница:

| | `repo token gen X` | `repo client init --profile X` |
|---|---|---|
| Где живёт | `repo-server.toml`, `[[server.tokens]]` | `repos.toml`, `[[repos]]` |
| Что это | Credential для **сервера** | Named source для **клиента** |
| Нужен для | `repo serve` | `repo pull` / `repo push` / `repo ls --profile X` |
| Plaintext | Печатается один раз (или `--write-file`) | Хранится как индирекция (`env:`, `file:`) |

`repo push --profile test` при отсутствии профиля `test` в `repos.toml`:

```
unknown profile `test` in client config; known: ["local"].
Profiles live in ~/.config/appspec/repos.toml.
If you meant the server-side token name, that goes in
`appspec repo token gen <name>` (server config), not `--profile`.
```

---

## `migrate-config`

Переносит legacy `<root>/config.toml` → `repo-server.toml`.

```sh
appspec repo migrate-config /srv/repo
```

Что делает:

1. Читает `<root>/config.toml` (`layout = 1`);
2. Пишет `repo-server.toml` в discovery-путь (или `--config PATH`) с `[repo].root = /srv/repo`;
3. Переименовывает `<root>/config.toml` → `<root>/.config.toml.migrated`;
4. Печатает `migrated: <path>` + `[repo].root` + `renamed: …`.

Если `<root>/config.toml` нет — `no config.toml at <root>; nothing to migrate`.
Если `repo-server.toml` уже есть — `server config already exists …; pass --config to use a different path`.

---

## Локальные операции vs URL

`add`, `check`, `gc`, `reindex`, `sign`, `serve`, `publish-index`, `export-oci` работают с диском. `$APPSPEC_REPO=http://…` для них — ошибка:

```
Error: `repo add` — локальная операция, требует путь к каталогу
(получено `http://127.0.0.1:8844` из $APPSPEC_REPO);
передайте --root PATH или --config PATH
```

`--root PATH` и `--config PATH` отключают проверку.

`pull` — исключение: `$APPSPEC_REPO=http://…` здесь источник, sink — `--root` или XDG.

---

## Discovery для `repo serve` без флагов

Порядок в `open_root`:

1. `--config PATH` — `[repo].root` из него;
2. `--root PATH` — `RepoRoot::open` (discovery + match по `[repo].root`);
3. `$APPSPEC_REPO=<path|profile>` — уважаем явное намерение;
4. **discovery** `repo-server.toml` → `[repo].root`;
5. fallback `default_root()`.

Поэтому:

```sh
$ appspec repo serve --listen 127.0.0.1:8844
[appspec] repo serve: listening on http://127.0.0.1:8844
```

работает **без** `--root`, если есть `~/.config/appspec/repo-server.toml`.

---

## Сравнение путей

`--root ./repo` и `[repo].root = /abs/repo` — один и тот же каталог. `norm_path` приводит relative к absolute (относительно CWD, без разрешения симлинков) перед сравнением. Поэтому ложный warning `points to X, not ./repo` не появляется, если пути действительно эквивалентны.

---

## v1-ограничения

* HTTPS на клиенте — только через reverse proxy. `http_request` возвращает `only http:// URLs supported in v1`.
* `source.ipfs` — задел; read-команды возвращают `ipfs:// show not supported in v1`.
* `token = "keyring:…"` — v2.
* `proxy` в профиле — задел; v1 использует `HTTP_PROXY`/`HTTPS_PROXY` из env.
* `repo serve` и `repo add` на одном `<root>` используют **два разных** лока (server `write_lock` vs `<root>/.lock`). Параллельная запись возможна; фикс — Phase 2 (единый lock).

---

## IPNS (mutable-имя репозитория)

### Server config: `[ipfs]`

| Поле | Тип | Default | Описание |
|---|---|---|---|
| `pin` | bool | `false` | `repo add` → `ipfs pin add` |
| `ipns_key` | string | — | имя ключа в `ipfs key list` |
| `ipns_name` | string | — | кеш `ipfs name publish` |

`ipns_key` обязателен для `repo publish --ipns`. Создать ключ:

```sh
ipfs key gen appspec-corp
```

`ipns_name` обновляется автоматически при успешном `publish --ipns`.
Если server config не найден — warning, имя не сохраняется.

### Client config: `source.ipns`

| Поле | Формат | v1 |
|---|---|---|
| `ipns` | `k51...` (ed25519) или `12D3Koo...` (RSA) | да |

`source.ipns` можно комбинировать с `source.url` — это fallback chain
`ipns → url`, порядок фиксирован. `source.path` остаётся
взаимоисключающим с сетевыми полями.

`source.ipfs` (legacy v1, сырой index-CID) — deprecated; при чтении
выдаёт warning. Удаляется в v2.

Пример:

```toml
# ~/.config/appspec/repo-server.toml
schema = 1
[repo]
root = "/var/lib/appspec/repo"
name = "corp/stable"

[ipfs]
pin       = true
ipns_key  = "appspec-corp"
ipns_name = "k51qzi5uqu5d..."

# ~/.config/appspec/repos.toml
schema = 1
default = "corp"

[[repos]]
name   = "corp"
source = { ipns = "k51qzi5uqu5d...", url = "http://repo.corp:8080" }
token  = "env:APPSPEC_REPO_TOKEN"
trusted_keys   = ["file:~/.config/appspec/trusted-keys.toml"]
allow_unsigned = false
```

