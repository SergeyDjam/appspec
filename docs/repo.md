# appspec repo — локальный и удалённый репозиторий SIF

`appspec repo` — централизованный репозиторий SIF-образов, построенный
на метаданных, вкомпилированных в SIF при `appspec build`. Один SIF —
источник истины; репозиторий — индекс поверх него.

## Быстрый старт

```sh
# 1. Собрать SIF (как обычно)
appspec build examples/strawberry.appspec

# 2. Инициализировать репозиторий (по умолчанию — ~/.local/share/appspec/repo)
appspec repo init

# 3. Добавить SIF
appspec repo add examples/strawberry.sif

# 4. Посмотреть, что есть
appspec repo ls
appspec repo show strawberry:1.2.31-1
appspec repo search player
```

## Конфигурация

Phase 1: server и client config **разделены** и живут вне repository root.

* `repo-server.toml` — один репозиторий: `[repo].root`, `[policy]`,
  `[signing]`, `[server]`, `[[server.tokens]]`, `[server.tls]`.
* `repos.toml` — именованные профили источников:
  `source = { path | url | ipfs }`, `token`, `trusted_keys`.

Полный справочник — [`docs/repo-config.md`](repo-config.md).

Repository root — **data-only** (`index.json`, `blobs/`, `packages/`,
`refs/`, `.lock`). `config.toml` внутри root **не создаётся**; legacy —
через `repo migrate-config`.

Флаги:

| Флаг | Где | Что |
|---|---|---|
| `--config PATH`, `-c PATH` | `serve`, `sign`, `pull`, `publish-index`, `export-oci`, `token *`, `server init`, `migrate-config`, `init` | Путь к `repo-server.toml` (alias: `--server-config`) |
| `--profile NAME` | `pull`, `push` | Профиль из `repos.toml` |
| `--client-config PATH` | `client init`, `client ls` | Путь к `repos.toml` |
| `--root PATH` | локальные команды | Переопределяет `[repo].root` |

Новые команды:

| Команда | Что делает |
|---|---|
| `repo token gen NAME [--config PATH] [--write-file PATH] [--mode OCTAL]` | Сгенерировать, записать sha256 в server config |
| `repo token ls [--config PATH]` | Список (sha256-префиксы) |
| `repo token check NAME (--value \| --file \| stdin) [--config PATH]` | Exit 0/1 |
| `repo token rm NAME [--config PATH]` | Удалить |
| `repo client init --profile N (--source-path P \| --source-url U) [--token-env VAR] [--token-file P] [--default]` | Создать профиль |
| `repo client ls [--json]` | Список профилей |
| `repo server init --root PATH [--config PATH]` | Создать server config |
| `repo migrate-config <root> [--config PATH]` | Legacy → `repo-server.toml` |


## Раскладка

```
<root>/
├── config.toml
├── index.json              # кэш каталога (BTreeMap → детерминированный JSON)
├── index/.keep             # forward-compat для шардирования (v2)
├── blobs/sha256/<ab>/<…>   # content-addressed SIF'ы (immutable)
├── packages/<name>/<ver>/
│   ├── meta.json           # все поля org.appspec.* + computed
│   ├── spec.appspec        # полный .appspec (для воспроизводимости)
│   └── sha256              # sha256 SIF для быстрой проверки
├── refs/<name>.toml        # latest / stable / произвольные теги
└── .lock                   # flock на write-операции
```

Корень по умолчанию: `$APPSPEC_REPO` → `$XDG_DATA_HOME/appspec/repo` → `$HOME/.local/share/appspec/repo`.

## Команды

### Инициализация

```sh
appspec repo init [PATH] [--name NAME] [--description TEXT] [--token NAME]
```

`--token NAME` генерирует 32-байтный токен, печатает его **один раз**
в stdout (`token NAME: <hex>`) и сохраняет `sha256:` от него в `config.toml`.
Повторный `init` с тем же именем — no-op, с другим — ошибка.

### Локальные операции

| Команда | Что делает |
|---|---|
| `repo add <SIF>… [--arch A] [--force] [--release-suffix N] [--pin-ipfs]` | Добавить SIF |
| `repo ls [PKG] [--json]` | Список пакетов / версий |
| `repo show PKG:VER [--json] [--spec] [--sif PATH]` | Метаданные / спек / копия SIF |
| `repo reindex` | Пересобрать index.json из packages/ (идемпотентно) |
| `repo check [--deep]` | Проверить целостность (sha256 blob'ов, spec) |
| `repo gc [--dry-run] [--keep N]` | Удалить unreferenced blob'ы |
| `repo search [Q] [--license L] [--arch A] [--vendor V] [--requires P]` | Поиск |

`repo add` идемпотентен по ключу `(name, version, release, epoch, arch)`.
`--release-suffix N` даёт `release = "<original>.N"` (например `1.2`).

### Подписи

```sh
# Создать ключ и подписать (если ключа нет — сгенерируется)
appspec repo sign [--key PATH]
# → signed: <root>/index.json
# → pubkey: ed25519:<hex>

# Проверить
appspec repo verify --pubkey <(appspec repo sign --key X --root R | grep pubkey)
# или через config.toml: [signing].pubkey = "ed25519:<hex>"
```

Подпись — **ed25519** над **JCS-канонизацией** `index.json` без поля `signature`
(RFC 8785, минимум — только String/u64/BTreeMap/Vec/null/bool).

Ключи:
- приватный: `$APPSPEC_SIGNING_KEY` → `$XDG_CONFIG_HOME/appspec/signing.key` → `$HOME/.config/appspec/signing.key`, режим 0600;
- allowlist: `$XDG_CONFIG_HOME/appspec/trusted-keys.toml` → `$HOME/.config/appspec/trusted-keys.toml`:

```toml
[[keys]]
name   = "release-2026"
pubkey = "ed25519:<hex>"
```

### HTTP-сервер и публикация

```sh
# Сервер (read + push)
appspec repo serve --root <root> --listen 127.0.0.1:8080

# Клиент: push
APPSPEC_REPO_TOKEN=... appspec repo push --to http://host:8080 pkg.sif
# или
appspec repo push --to http://host:8080 --token-file ~/.token pkg.sif

# Клиент: pull
appspec repo pull pkg:1.0.0-1 --from http://host:8080 --trusted-key <pubkey.txt>
appspec repo pull pkg:1.0.0-1 --from /path/to/other/repo
appspec repo pull pkg:1.0.0-1 --via ipfs
appspec repo pull pkg:1.0.0-1 --from ipfs://<index-cid>
```

**Fail-closed (§10.2 ТЗ):** если в allowlist есть хоть один ключ, `pull --from URL`
**требует** валидную подпись `index.json`. `--allow-unsigned` — единственный
способ обойти (с warning'ом).

Только `http://`. Для `https://` — reverse proxy (nginx/caddy). TLS-конфиг
`[server.tls]` в `config.toml` парсится, но не используется.

### IPFS

```sh
appspec repo add --pin-ipfs pkg.sif            # пинит blob, пишет ipfs_cid
appspec repo publish-index --pin              # → CID в stdout и <root>/ipfs-index.cid
appspec repo pull pkg:1.0.0-1 --via ipfs      # из локального индекса
appspec repo pull pkg:1.0.0-1 --from ipfs://<index-cid>   # федерация
```

### OCI export

```sh
appspec repo export-oci --registry ghcr.io/user --dry-run appspec-index
appspec repo export-oci --registry ghcr.io/user appspec-index
```

Требует `oras` в `PATH` (`--dry-run` — не требует). Схема:

- `<registry>/<target>:latest` — artifact manifest, `artifactType = application/vnd.appspec.index.v1+json`, blob — index.json
- `<registry>/<name>:<ver>` — artifact manifest, `artifactType = application/vnd.appspec.sif.v1+json`, blob — SIF

## Спека: `[dependencies]` и `[build].base_digest`

### `[dependencies]` (E7)

```toml
[dependencies]
requires  = ["gstreamer >= 1.22", "qt6-base"]
provides  = ["music-player"]
conflicts = ["strawberry-nightly"]
```

Правила:
- `requires` — `name [op ver]`, операторы `=`, `>=`, `<=`, `>`, `<`, без запятых;
- `provides`/`conflicts`/`replaces` — голые имена пакетов, без операторов.

Лейблы `org.appspec.dependencies.*` пишутся **только если непусто**
(обратная совместимость). `repo add` читает их как JSON-массивы.

### `[build].base_digest` (E7)

```toml
[build]
base        = "debian:trixie"
base_digest = "sha256:<64 lowercase hex>"
```

`appspec build` подменяет `base` на `<repo>@<digest>` (тег отбрасывается).
В Dockerfile — `FROM debian@sha256:…`. В `meta.json` — поле `base_digest`.

## Bundle: `source = repo` и `bundle.lock` (E3)

```toml
[bundle]
name = "vscodium-dev"
base = { repo = "vscodium:1.135.06055" }

[[components]]
name    = "git"
repo    = "git:2.47.2"
provide = "git"
```

`repo = "name:ver"` — пятый источник наравне с `spec|sif|cid|manifest`.
Резолвится через локальный `index.json` (или `$APPSPEC_REPO`).

`appspec bundle lock` создаёт `bundle.lock.toml`:

```toml
[bundle]
name      = "vscodium-dev"
locked_at = "2026-11-01T00:00:00Z"

[[components]]
name        = "git"
repo        = "git:2.47.2"
sif_sha256  = "74df01f…"
spec_sha256 = "a1b2c3…"
```

`bundle build` при наличии lock'а сверяет `sif_sha256`; при дрейфе — warning,
но идёт по свежему индексу.

## HTTP API

| Метод | Путь | Ответ |
|---|---|---|
| GET | `/healthz` | `{"status":"ok"}` |
| GET | `/index.json` | index.json |
| GET | `/blobs/sha256/<ab>/<…>` | SIF (octet-stream) |
| GET | `/packages/<name>/<v>/meta.json` | meta.json |
| GET | `/packages/<name>/<v>/spec.appspec` | spec |
| GET | `/refs/<name>.toml` | refs |
| POST | `/api/v1/push` | push (Bearer-token) |

Все ошибки — `{"error": "<kind>", "reason": "..."}`.

## Переменные окружения

| Переменная | Значение |
|---|---|
| `APPSPEC_REPO` | Path \| URL \| profile name. Порядок: `--profile` → `--root`/`--to`/`--from` → `$APPSPEC_REPO` → `default` |
| `APPSPEC_REPO_TOKEN` | **Только токен** для `push`. Не destination. |
| `APPSPEC_REPO_SERVER_CONFIG` | Путь к `repo-server.toml` (после `--config`) |
| `APPSPEC_REPO_CLIENT_CONFIG` | Путь к `repos.toml` |
| `APPSPEC_SIGNING_KEY` | Приватный ключ (после `[signing].key`) |
| `XDG_CONFIG_HOME` | discovery `repo-server.toml` / `repos.toml` |
| `XDG_DATA_HOME` | fallback `default_root()` |
| `APPSPEC_APPTAINER`, `APPSPEC_IPFS`, `APPSPEC_ORAS` | Моки для тестов |
## Ограничения v1

- HTTP только, без TLS. Используйте reverse proxy.
- Chunked/keep-alive/HTTP2 не поддерживаются.
- Multi-arch: `name:ver-release` без `--arch` при коллизии — ошибка
  `ambiguous` (в v2 — `versions[ver][arch]`).
- Resumable upload / `pull --spec-only` через IPFS — не в v1.
- Федерация индексов (мерж нескольких) — не в v1.

## Тесты

`cargo test --release` — 76 тестов. Покрытие:

- `tests/repo_e1.rs` — init/add/ls/show/reindex/check/gc/search;
- `tests/e3.rs` — bundle + source=repo + lock;
- `tests/e4.rs` — HTTP server/client/push;
- `tests/e5.rs` — ed25519 + JCS, fail-closed pull;
- `tests/e6.rs` — IPFS (мок `ipfs`);
- `tests/e7.rs` — `[dependencies]` + `base_digest`;
- `tests/e8.rs` — OCI export (мок `oras`).

End-to-end: `scripts/smoke-e2e.sh`.
## IPNS

IPNS — mutable-имя репозитория. В отличие от `ipfs://<cid>` (immutable),
`ipns://<name>` резолвится в текущий index CID, что позволяет публиковать
обновления `index.json` без смены URL.

### Server config

```toml
[ipfs]
pin       = true
ipns_key  = "appspec-corp"          # имя в `ipfs key list`
ipns_name = "k51qzi5uqu5d..."       # кеш, обновляется при publish
```

- `ipns_key` — имя ключа в локальном `ipfs` keystore. Создать ключ:
  ```sh
  ipfs key gen appspec-corp
  ```
- `ipns_name` — производное от pubkey ключа. Кешируется в config для
  отображения; источник истины — DHT.

IPNS-ключ и signing-ключ по умолчанию **независимы**:

- IPNS = «я контролирую это имя сейчас» (mutable);
- Signing = «я выпустил этот index» (immutable).

Опционально можно связать их: `ipfs key import appspec-corp signing.key`.

### Публикация

```sh
appspec repo publish --ipns                   # publish + ipfs name publish
appspec repo publish --no-ipns                # publish без IPNS
appspec repo publish                          # IPNS, если ipns_key задан
appspec repo publish-index --ipns             # алиас, работает так же
```

`--ipns` без `[ipfs].ipns_key` — ошибка с подсказкой про `ipfs key gen`.

Вывод:

```
bafkrei...                          (index CID, stdout)
published: k51... (index bafkrei...) (IPNS name, stderr)
```

### Резолв

```sh
appspec repo resolve k51qzi5uqu5d...      # → index CID (только чтение)
appspec repo resolve k51... --timeout-secs 60
```

### Использование в pull

Через `--from`:

```sh
appspec repo pull pkg:1.0.0-1 --from ipns://k51... --trusted-key pub.txt
```

Через профиль (fallback chain `ipns → url`):

```toml
[[repos]]
name   = "corp"
source = { ipns = "k51qzi5uqu5d...", url = "http://repo.corp:8080" }
trusted_keys = ["file:~/.config/appspec/trusted-keys.toml"]
```

```sh
appspec repo pull pkg:1.0.0-1 --profile corp
```

При недоступности IPNS (timeout, daemon down) — падение в HTTP.
При недоступности обоих — агрегированная ошибка с двумя причинами.

### Latency

- `ipfs name publish` на локальном узле — секунды;
- `ipfs name resolve` с чужой машины — 1–3 минуты на первом резолве
  (DHT walk), потом кеш;
- `resolve --timeout-secs 30` по умолчанию; `pull` идёт с hard-timeout
  30 s и падает в fallback.

### Fail-closed

Подпись index проверяется независимо от транспорта. `trusted_keys`
в профиле работает и для `ipns://`, и для `http://`.

### Миграция: ipfs_cid в meta.json

С 0.4.4 `ipfs_cid` пишется в `meta.json` **всегда** (не только с
`--pin-ipfs`). Для legacy-пакетов:

```sh
appspec repo reindex --backfill-cid
```

Заполняет `ipfs_cid` из `sif_sha256` для старых записей.

### Ограничение CIDv1/raw

`cid_v1_from_sha256` даёт CIDv1/raw/sha2-256. Это совпадает с `ipfs add`
для файлов ≤ 256 KB (один блок). Для больших SIF `ipfs add` возвращает
dag-pb root, чей CID не выводится из sha256. `--pin-ipfs` на таком
файле упадёт с «CID mismatch» — защита от молчаливой поломки
`pull --via ipfs`.

### IPFS-публикация: end-to-end

`publish --ipns` публикует **index.json** в IPFS и привязывает к IPNS-имени.
Блобы SIF — отдельные IPFS-объекты, адресуемые по `ipfs_cid` из индекса.
Чтобы они были **доступны в сети**, их нужно **запинить**.

~~~sh
# Publisher
appspec repo init /srv/repo --config ~/.config/appspec/repo-server.toml
appspec repo add --pin-ipfs foo.sif
appspec repo add --pin-ipfs bar.sif
appspec repo sign
appspec repo publish --ipns
~~~

### IPFS-публикация: конфигурация и consumer

~~~toml
[ipfs]
ipns_key = "appspec-corp"
~~~

`pull --from ipns://<name>` идёт **только через IPFS**:
1. `ipfs name resolve` → index CID;
2. `ipfs cat` индекса → проверка подписи;
3. `ipfs cat <ipfs_cid>` для SIF → проверка `sif_sha256`;
4. запись в локальный репозиторий (`--root`, приёмник).

**Read-команды** (`ls`, `search`, `verify`) поддерживают `--root ipns://NAME`
и `--root ipfs://CID`. `show` — нет: per-package `meta.json` не адресуем,
индекс — один файл.

### IPFS-публикация: `repo pin`

Если пакеты добавлены без `--pin-ipfs`, блобы не запинены и `pull --from
ipns://` упадёт на timeout. `repo pin`:

- для `ipfs_cid = None` + существующего блоба: `ipfs add --pin` (raw / dag-pb),
  пишет CID в `meta.json`;
- для `ipfs_cid = Some(cid)`: `ipfs pin add <cid>` (skip если уже pinned);
- `reindex` → `ipfs_cid` появляется в `index.json`.

~~~sh
appspec repo pin --dry-run       # что будет сделано
appspec repo pin                 # добавить + запинить + reindex
appspec repo sign                # подписать новый index
appspec repo publish --ipns      # опубликовать
~~~

`publish --ipns` печатает warnings, если в индексе есть версии без `ipfs_cid`
или с `ipfs_cid`, но не запиненные.

## `appspec run <package>` — Index as source of truth

`index.json` описывает всё, что нужно клиенту: `sif_sha256`, `sif_size`,
`ipfs_cid` для каждой версии. Клиент читает индекс **один раз**, дальше
работает по CID'ам без обращения к серверу.

### Синтаксис

```sh
appspec run foo                       # latest, max release
appspec run foo:latest                # то же
appspec run foo:2.2.0                 # версия 2.2.0, max release среди 2.2.0-N
appspec run foo:2.2.0-1               # точно 2.2.0 release=1
```

Файл (не пакет): `appspec run ./foo.appspec`, `/abs/foo.appspec`,
`foo.appspec`.

### Флаги

| Флаг | Что |
|---|---|
| `--from URL\|PATH\|ipns://…` | источник (index + blob) |
| `--profile NAME` | профиль из `repos.toml` |
| `--root PATH` | локальный репозиторий (sink + cache) |
| `--trusted-key PATH` | ed25519 pubkey для подписи индекса |
| `--allow-unsigned` | принять неподписанный индекс |
| `--dry-run` | pull + launcher, без запуска |

### Резолюция версии

- `foo` / `foo:latest` — max `(version, release, epoch)` по `sort_key`;
- `foo:2.2.0` — filter `version == "2.2.0"`, max release;
- `foo:2.2.0-1` — точный ключ `2.2.0-1`.

### Источник

Порядок: **local cache** → `--from` / `--profile` / `$APPSPEC_REPO` /
`default` профиль. Первый успешный — используется для pull.

Blob кешируется в `blobs/sha256/<ab>/<…>`, spec — в
`packages/<name>/<ver_key>/spec.appspec`. Launcher — в
`~/.cache/appspec/run/run.<name>.<ver>.sh`.

### Publish + auto-pin

`appspec repo publish --ipns` (0.5.0) **автоматически пинит** все блобы
из `packages/`:

1. `ipfs_cid = None` + блоб есть → `ipfs add --pin` → CID в `meta.json`.
2. `ipfs_cid = Some` → `ipfs pin ls` → если не pinned, `ipfs pin add`.
3. `reindex` → `index.json` получает все CID.
4. Warning: «if previously signed, signature now invalid».

Флаг `--no-pin` — только публикация индекса, без пиннинга блобов
(поведение 0.4.x).

После `publish --ipns` любой клиент:

```sh
appspec run foo:2.2.0 \
    --from ipns://k51… \
    --trusted-key pub.txt
```
