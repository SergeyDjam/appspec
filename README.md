# appspec

Сборка Apptainer SIF из декларативного спека (TOML). Dockerfile и launcher
генерируются через minijinja-шаблоны. Для составных окружений есть
подсистема bundles — base SIF + компоненты + расширения. Для публикации,
поиска и распространения SIF — подсистема `appspec repo` (локальный
репозиторий, HTTP-сервер, IPFS, OCI export, ed25519-подписи).

## Возможности

- **Один спек — все артефакты.** Dockerfile, run-скрипт, SIF, .desktop,
  launcher — генерируются из *.appspec.
- **Десять шаблонов сборки:** deb, tar, apt, generic, rpm, alpine,
  alpine-slim, full, static, scratch.
- **Три движка сборки:** buildah (rootless, OCI-native), podman, docker.
- **Composable bundles:** base SIF + компоненты (chroot/instance/both) +
  расширения из файла / URL / IPFS / Open VSX / **локального репозитория**.
- **RPM-style макросы:** name, version, bin, optional, default.
- **Зависимости и воспроизводимость:** `[dependencies]` с requires/provides/
  conflicts/replaces, `[build].base_digest` для пина base-образа.
- **Локальный репозиторий SIF (`appspec repo`):** content-addressed blobs,
  RPM-style метаданные, поиск, sign/verify (ed25519), HTTP-сервер,
  push/pull, IPFS pin/pull, OCI export через `oras`.
- **Пользовательские шаблоны:** поиск в 6 местах с предсказуемым приоритетом.

## Установка

```
git clone <repo> appspec
cd appspec
cargo build --release
install -m755 target/release/appspec ~/.local/bin/
```

Зависимости времени выполнения:

| Компонент | Когда нужен |
|-----------|-------------|
| apptainer | всегда (сборка SIF, inspect labels) |
| buildah / podman / docker | хотя бы один (для build) |
| ipfs | опционально (`repo --pin-ipfs`, `pull --via ipfs`) |
| oras | опционально (`repo export-oci`, `pull --from oci://`) |
| curl или wget | bundle-расширения по URL |
| unzip | подготовка .vsix |

## Быстрый старт

Одиночное приложение:

```
appspec gen    examples/strawberry.appspec
appspec build  examples/strawberry.appspec
appspec run    examples/strawberry.appspec
appspec stop   examples/strawberry.appspec
appspec all    examples/strawberry.appspec
appspec clean  examples/strawberry.appspec
```

Bundle:

```
appspec bundle check examples/vscodium-bundle.appspec
appspec bundle lock  examples/vscodium-bundle.appspec
appspec bundle build examples/vscodium-bundle.appspec
appspec run          examples/vscodium-bundle.appspec
```

Репозиторий SIF:

```
appspec repo init
appspec repo add examples/strawberry.sif
appspec repo ls
appspec repo show strawberry:1.2.31-1
appspec repo search player

# подписать и проверить
appspec repo sign
appspec repo verify --pubkey ~/.config/appspec/signing.key.pub

# HTTP-сервер + publish
appspec repo serve --listen 127.0.0.1:8080 &
APPSPEC_REPO_TOKEN=... appspec repo push --to http://127.0.0.1:8080 pkg.sif
appspec repo pull pkg:1.0.0-1 --from http://127.0.0.1:8080 --trusted-key pub.txt

# IPFS
appspec repo add --pin-ipfs pkg.sif
appspec repo publish-index --pin
appspec repo pull pkg:1.0.0-1 --via ipfs

# OCI
appspec repo export-oci --registry ghcr.io/user appspec-index
appspec repo pull pkg:1.0.0-1 --from oci://ghcr.io/user/appspec-index:latest
```

Справочник по repo — [`docs/repo.md`](docs/repo.md).

## Команды

| Команда | Описание |
|---------|----------|
| `gen SPEC` | Сгенерировать Dockerfile и run-скрипт |
| `build SPEC [-f]` | gen + image build + SIF |
| `run SPEC` | Запустить launcher (SIF или bundle) |
| `stop SPEC` | Остановить instance от `persist = true` |
| `all SPEC [-f]` | build + run |
| `clean SPEC` | Удалить Dockerfile, launcher, SIF, tar, кэш |
| `install SPEC` | Установить SIF + launcher + .desktop |
| `uninstall SPEC` | Удалить всё из манифеста |
| `ipfs-publish SPEC` | Опубликовать SIF в IPFS |
| `bundle {check,build,ls,add,remove,lock}` | Составные окружения |
| `repo {init,add,ls,show,reindex,check,gc,search,sign,verify,serve,push,pull,publish-index,export-oci}` | Локальный / удалённый репозиторий SIF |

## Спека *.appspec

TOML. Основные секции: `package`, `build`, `runtime`, `install`, `macros`,
`dependencies`. Секция `build.source.type` определяет способ установки:
`deb`, `rpm`, `tar`, `binary` или пустая строка (только apt-пакеты).

Минимальный спек:

```toml
[package]
name    = "git"
version = "2.47.2"

[build]
base     = "alpine:3.21"
template = "alpine"

[build.apt]
extra = ["git", "openssh-client", "ca-certificates"]

[build.collect]
binary = "/usr/bin/git"

[build.scratch]
workdir = "/work"
cmd     = ["/usr/bin/git", "--version"]

[runtime]
mode    = "cli"
command = "/usr/bin/git"
```

### `[dependencies]` (E7)

```toml
[dependencies]
requires  = ["gstreamer >= 1.22", "qt6-base"]
provides  = ["music-player"]
conflicts = ["strawberry-nightly"]
```

Правила:
- `requires` — `name [op ver]`, операторы `=`, `>=`, `<=`, `>`, `<`, без запятых;
- `provides`/`conflicts`/`replaces` — голые имена пакетов.

Лейблы `org.appspec.dependencies.*` в SIF пишутся **только если непусто**
(обратная совместимость). `appspec repo add` читает их в `meta.json.requires` /
`.provides` / `.conflicts`, а `repo search --requires P` фильтрует по `provides`.

### `[build].base_digest` (E7)

```toml
[build]
base        = "debian:trixie"
base_digest = "sha256:<64 lowercase hex>"
```

`appspec build` подменяет `base` на `<repo>@<digest>` (тег отбрасывается).
Dockerfile получает `FROM debian@sha256:…`, что фиксирует base навсегда.

Полный справочник — в [`docs/spec.md`](docs/spec.md).

## Шаблоны

| Шаблон | Base | Размер SIF | Когда использовать |
|--------|------|------------|---------------------|
| deb | Debian | 250 МБ | .deb + magicpak |
| tar | Debian | 250 МБ | tar + magicpak |
| apt | Debian | 250 МБ | только apt-пакеты |
| generic | Debian | 250 МБ | пустой каркас |
| rpm | RPM-based | 250 МБ | .rpm + magicpak |
| alpine | Alpine | 30-150 МБ | apk --root (Go, Rust, Python) |
| alpine-slim | Alpine | 8-50 МБ | apk + magicpak (dynamic ELF) |
| full | Debian | 400-800 МБ | Electron, GUI с sandbox |
| static | Alpine | 30 МБ | статический бинарник + busybox |
| scratch | Alpine | 30 МБ | статический бинарник, CLI-only |

Правило выбора — [`docs/templates.md`](docs/templates.md).

## Макросы

Раскрываются в `load_spec` до парсинга TOML. Работают и в полях, и в
многострочных `setup` и `entry`.

- `%{name}`, `%{version}`, `%{bin}` — из секции `package`.
- `%{_prefix}`, `%{_bindir}`, `%{_sbindir}`, `%{_libdir}` — дистрибутив-специфичные.
- `%{?optional}` — пусто, если не задан.
- `%{name:default}` — значение по умолчанию.
- `%%` — буквальный символ процента.
- Пользовательские — из секции `[macros]`.

См. [`docs/macros.md`](docs/macros.md).

## Bundles

Bundle — составное окружение из base SIF + компонентов + расширений.

```toml
[bundle]
name = "vscodium-dev"
base = { spec = "./vscodium.appspec" }

[[components]]
name    = "git"
spec    = "./git.appspec"
provide = "git"

[[extensions]]
name = "ms-azuretools.vscode-docker"
```

Компоненты бывают трёх режимов: `chroot`, `instance`, `both`.
Расширения — четырёх источников: file, cid, url, online (Open VSX).

### `source = repo` и `bundle.lock` (E3)

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
Резолвится через локальный `index.json` (`$APPSPEC_REPO`).

`appspec bundle lock` создаёт `bundle.lock.toml` с `sif_sha256` и `spec_sha256`
каждого `repo`-компонента. `bundle build` сверяет `sif_sha256`: при дрейфе —
warning, но идёт по свежему индексу.

Полное описание — [`docs/bundles.md`](docs/bundles.md).

## Репозиторий SIF (`appspec repo`)

Локальный репозиторий строится на метаданных, вкомпилированных в SIF при
`appspec build` (лейблы `org.appspec.*`). SIF — источник истины; репозиторий —
индекс поверх него.

Раскладка:

```
<root>/
├── config.toml
├── index.json              # кэш каталога
├── blobs/sha256/<ab>/<…>   # content-addressed SIF'ы (immutable)
├── packages/<name>/<ver>/  # meta.json + spec.appspec + sha256
├── refs/<name>.toml        # latest / stable / произвольные теги
└── .lock                   # flock на write-операции
```

Корень: `$APPSPEC_REPO` → `$XDG_DATA_HOME/appspec/repo` → `$HOME/.local/share/appspec/repo`.

### Быстрый цикл

```
appspec repo init
appspec repo add pkg.sif
appspec repo check [--deep]
appspec repo gc [--dry-run] [--keep N]
appspec repo search [Q] [--license L] [--arch A] [--requires P]
```

### Публикация

```
# HTTP-сервер (read + push)
appspec repo serve --listen 127.0.0.1:8080

# push (второй терминал)
APPSPEC_REPO_TOKEN=... appspec repo push --to http://host:8080 pkg.sif

# pull с проверкой подписи
appspec repo pull pkg:1.0.0-1 --from http://host:8080 --trusted-key pub.txt
```

**Fail-closed (§10.2):** если в `~/.config/appspec/trusted-keys.toml` есть
хотя бы один ключ, `pull --from URL` **требует** валидную подпись `index.json`.

### Подписи (E5)

```
appspec repo sign                      # → ed25519, печатает pubkey
appspec repo verify --pubkey pub.txt
```

Схема: ed25519 над JCS-канонизацией (RFC 8785) `index.json` без поля `signature`.
Приватный ключ: `$XDG_CONFIG_HOME/appspec/signing.key` (0600).

### IPFS (E6)

```
appspec repo add --pin-ipfs pkg.sif
appspec repo publish-index --pin
appspec repo pull pkg:1.0.0-1 --via ipfs
appspec repo pull pkg:1.0.0-1 --from ipfs://<index-cid>
```

### OCI export (E8)

```
appspec repo export-oci --registry ghcr.io/user --dry-run appspec-index
appspec repo export-oci --registry ghcr.io/user appspec-index
```

Требует `oras` в `PATH`. Формат:

- `<registry>/<target>:latest` — artifact manifest, `artifactType = application/vnd.appspec.index.v1+json`
- `<registry>/<name>:<ver>` — artifact manifest, `artifactType = application/vnd.appspec.sif.v1+json`

Полный справочник — [`docs/repo.md`](docs/repo.md).

## Переменные окружения

| Переменная | Значение |
|------------|----------|
| `APPSPEC_ENGINE` | docker / podman / buildah |
| `BUILDAH_ISOLATION` | oci (default) / chroot |
| `APPSPEC_DEV` | 1 — поиск шаблонов в workspace/templates/ |
| `APPSPEC_TEMPLATE_PATH` | Доп. каталоги шаблонов |
| `APPSPEC_CONFIG_PATH` | Доп. каталоги конфигов |
| `APPSPEC_BUNDLE_CACHE` | Каталог кэша bundles |
| `APPSPEC_BUNDLE_HOME` | Каталог state bundles |
| `APPSPEC_REPO` | Корень локального репозитория SIF |
| `APPSPEC_REPO_TOKEN` | Токен для `repo push` |
| `APPSPEC_SIGNING_KEY` | Путь к приватному ed25519-ключу |
| `APPSPEC_APPTAINER` | Бинарь `apptainer` (для моков) |
| `APPSPEC_IPFS` | Бинарь `ipfs` (для моков) |
| `APPSPEC_ORAS` | Бинарь `oras` (для моков) |
| `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_DATA_DIRS` | Стандартные XDG |

## Тесты

```
cargo test --release                 # 76 тестов
./scripts/smoke-e2e.sh               # e2e (auto: full если возможно, иначе mock)
./scripts/smoke-e2e.sh --full        # требуются apptainer + build-engine
./scripts/smoke-e2e.sh --mock        # без внешних зависимостей
```

Покрытие:
- `tests/repo_e1.rs` — init/add/ls/show/reindex/check/gc/search;
- `tests/e3.rs` — bundle `source=repo` + `bundle lock`;
- `tests/e4.rs` — HTTP server/client/push;
- `tests/e5.rs` — ed25519 + JCS, fail-closed pull;
- `tests/e6.rs` — IPFS (мок `ipfs`);
- `tests/e7.rs` — `[dependencies]` + `base_digest`;
- `tests/e8.rs` — OCI export (мок `oras`).

## Документация

- [`docs/spec.md`](docs/spec.md) — справочник по *.appspec.
- [`docs/templates.md`](docs/templates.md) — шаблоны, выбор, свои.
- [`docs/bundles.md`](docs/bundles.md) — составные окружения.
- [`docs/macros.md`](docs/macros.md) — макросы.
- [`docs/engines.md`](docs/engines.md) — buildah/podman/docker.
- [`docs/troubleshooting.md`](docs/troubleshooting.md) — типичные проблемы.
- [`docs/repo.md`](docs/repo.md) — локальный и удалённый репозиторий SIF.

## Лицензия

MIT.
