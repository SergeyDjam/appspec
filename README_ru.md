[English](README.md) | [Русский](README_ru.md)

# appspec

Сборка Apptainer SIF из декларативного спека (TOML). Dockerfile и launcher
генерируются через minijinja-шаблоны. Для составных окружений есть
подсистема bundles — base SIF + компоненты + расширения. Для публикации,
поиска и распространения SIF — подсистема `appspec repo` (локальный
репозиторий, HTTP-сервер, IPFS, OCI export, ed25519-подписи).

## Возможности

- **Один спек — все артефакты.** Dockerfile, run-скрипт, SIF, .desktop,
  launcher — генерируются из `*.appspec`.
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
- **Shell-completions:** bash, zsh, fish, elvish, powershell.

## Установка

```sh
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
| unzip | распаковка `.vsix` / `.zip` / `.oxt` в bundle |
| tar | распаковка `.tar.gz` / `.tgz` в bundle |
| zip | только для `cargo test` (генерация тестовых `.vsix`) |

### Bash completion

```sh
# для текущего пользователя
mkdir -p ~/.local/share/bash-completion/completions
appspec completions bash > ~/.local/share/bash-completion/completions/appspec

# или системно
sudo appspec completions bash > /etc/bash_completion.d/appspec
```

Аналогично для `zsh`, `fish`, `elvish`, `powershell`:

```sh
appspec completions zsh  > ~/.zsh/completions/_appspec
appspec completions fish > ~/.config/fish/completions/appspec.fish
```

## Быстрый старт

Одиночное приложение:

```sh
appspec gen    examples/strawberry.appspec
appspec build  examples/strawberry.appspec
appspec run    examples/strawberry.appspec
appspec stop   examples/strawberry.appspec
appspec all    examples/strawberry.appspec
appspec clean  examples/strawberry.appspec
```

Bundle:

```sh
appspec bundle check examples/vscodium-bundle.appspec
appspec bundle lock  examples/vscodium-bundle.appspec
appspec bundle build examples/vscodium-bundle.appspec
appspec run          examples/vscodium-bundle.appspec
```

Bundle с расширениями (`.vsix` для VSCodium, `.oxt` для LibreOffice):

```sh
# VSCodium: расширения распаковываются в каталоги, install не нужен
appspec bundle build examples/vscodium-bundle.appspec

# LibreOffice: .oxt копируется в mount_dir, install через unopkg
# (локальный .oxt положить в examples/extensions/ — см. README там же)
appspec build -f examples/libreoffice.appspec
appspec bundle build examples/libreoffice-bundle.appspec
appspec run          examples/libreoffice-bundle.appspec
```

Репозиторий SIF:

```sh
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
| `completions <SHELL>` | Сгенерировать bash/zsh/fish/elvish/powershell-completion |

## Спека `*.appspec`

TOML. Основные секции: `package`, `build`, `runtime`, `install`, `macros`,
`dependencies`. Секция `build.source.type` определяет способ установки:
`deb`, `rpm`, `tar`, `binary` или пустая строка (только apt/apk-пакеты).

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
(обратная совместимость). `appspec repo add` читает их в `meta.json.requires`
/ `.provides` / `.conflicts`, а `repo search --requires P` фильтрует по
`provides`.

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

### Типы расширений

`[[extensions]].type` — `vsix` (default), `file`, `archive`, `auto`.
При `auto` тип определяется по суффиксу URL/файла (`.vsix` → vsix,
`.oxt`/`.deb`/`.rpm`/`.sh`/`.bin`/`.exe` → file, `.zip`/`.tar.gz`/`.tgz`/
`.tar.xz`/`.tar.bz2`/`.tar` → archive; для online без суффикса — vsix).

Поведение по типам:

| `type` | Что делает `bundle build` | Что монтируется | `[bundle.extensions].install` |
|--------|---------------------------|-----------------|-------------------------------|
| `vsix` | распаковывает `.vsix` в каталог `<publisher>.<name>-<version>/` в `$BUNDLE_DIR/extensions/` | `$BUNDLE_DIR/extensions/` → `mount_dir` | **не исполняется** (VSCodium сам подхватывает каталоги из `--extensions-dir`) |
| `file` | копирует файл как есть в `$BUNDLE_DIR/extensions/<basename>` | `$BUNDLE_DIR/extensions/` → `mount_dir` | **исполняется** через `yes yes \| …` (auto-accept EULA: `unopkg`, `.deb`-installers) |
| `archive` | распаковывает `.zip`/`.tar.gz` в каталог `<name>-archive/` | `$BUNDLE_DIR/extensions/` → `mount_dir` | **не исполняется** (пользователь сам решает, что делать с распакованным каталогом) |

`[bundle.extensions].mount_dir` — куда монтировать `$BUNDLE_DIR/extensions`
внутри base-инстанса. По умолчанию `/config/home/.vscodium/extensions` —
целевой `--extensions-dir` VSCodium. Для расширений LibreOffice
переопределяется на `/opt/extensions` (см. ниже).

`[bundle.extensions].install` — shell-команда, исполняется **только для
`type = "file"`**. Плейсхолдеры:

| Плейсхолдер | Значение |
|-------------|----------|
| `{file}` | путь внутри `mount_dir` (`<mount_dir>/<basename>`) |
| `{target}` | алиас `{file}` (совместимость со старыми спеками) |
| `{name}` | `ext.id` (для `type = "file"` — `ext.name`) |
| `{id}` | `ext.id` |
| `{version}` | версия (для `type = "file"` — `"1"`) |

#### Пример: VSCodium + `.vsix`

```toml
[bundle]
name = "vscodium-dev"
base = { spec = "./vscodium.appspec" }

[[extensions]]
name = "opentofu"
file = "./extensions/opentofu-0.6.3.vsix"
type = "vsix"

[[extensions]]
name = "ms-azuretools.vscode-docker"
```
- `mount_dir` не задаём — берётся default `/config/home/.vscodium/extensions`.
- `install` не нужен — `.vsix` распаковываются в каталоги, VSCodium их сам видит.

#### Пример: LibreOffice + `.oxt`

```toml
[bundle.extensions]
mount_dir = "/opt/extensions"
install   = "unopkg add --shared {file}"

[[extensions]]
name = "grammalecte"
file = "./extensions/Grammalecte-fr-v2.1.3.oxt"
type = "file"
```
- `mount_dir = "/opt/extensions"` — свой путь, чтобы не конфликтовать с VSCodium.
- `install` исполняется как `yes yes 2>/dev/null | unopkg add --shared /opt/extensions/Grammalecte-fr-v2.1.3.oxt` (EULA принимается автоматически).

#### Источники расширений

`[[extensions]]` содержит ровно один из `file` | `cid` | `url`, либо ни
одного (online через Open VSX, только для `type = "vsix"`).

- `file = "./path.vsix"` — локальный (относительно `bundle.appspec`).
- `url = "https://…"` — скачивается в `$BUNDLE_DIR/extensions/<basename>` при `bundle build`. Прямые URL у многих сайтов меняются при релизах — для воспроизводимости предпочтителен `file =`.
- `cid = "bafy…"` — IPFS, `ipfs get` в `$BUNDLE_DIR/extensions/<cid>.bin`.

Полный пример со всеми типами — `examples/libreoffice-bundle.appspec`.
Инструкция по подготовке локальных `.oxt`/`.vsix` — `examples/extensions/README.md`.

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

### Конфигурация

Phase 1: **два независимых конфига**, оба вне repository root.

* **Server** (`repo-server.toml`) — один репозиторий: `[repo].root`,
  `[policy]`, `[signing]`, `[server]`, `[[server.tokens]]`, `[server.tls]`.
* **Client** (`repos.toml`) — именованные профили источников:
  `source = { path | url | ipfs }`, `token`, `trusted_keys`.

Полный справочник — [`docs/repo-config.md`](docs/repo-config.md).

```sh
# Server: data-only root + внешний config
appspec repo init /srv/repo --config ~/.config/appspec/repo-server.toml
appspec repo server init --root /srv/repo      # если root уже есть
appspec repo migrate-config /srv/repo          # импорт legacy <root>/config.toml

# Client: профиль источника
appspec repo client init \
    --profile corp \
    --source-url http://repo.corp:8080 \
    --token-env APPSPEC_REPO_TOKEN \
    --default
appspec repo client ls
```

Repository root — **data-only** (`index.json`, `blobs/`, `packages/`, `refs/`,
`.lock`). `config.toml` внутри root не создаётся; legacy поддерживается с
warning'ом и `migrate-config`.

### Быстрый цикл

```sh
appspec repo init /srv/repo --config ~/.config/appspec/repo-server.toml
appspec repo add pkg.sif
appspec repo check [--deep]
appspec repo gc [--dry-run] [--keep N]
appspec repo search demo --config ~/.config/appspec/repo-server.toml
```

### Токены (server-side)

```sh
# Сгенерировать, plaintext — один раз в stdout
appspec repo token gen ci

# Сразу в файл (для CI), восьмеричный mode как chmod
appspec repo token gen ci --write-file /run/secrets/ci --mode 0400

appspec repo token ls
appspec repo token check ci --file /run/secrets/ci    # exit 0/1
appspec repo token rm ci
```

`repo token gen` пишет **только sha256** в server config. Plaintext на диске
не появляется.

### Публикация

```sh
# 1. Сервер
appspec repo serve --listen 127.0.0.1:8844
# (если ~/.config/appspec/repo-server.toml существует, --root/--config не нужны)

# 2. Push (три варианта destination)
APPSPEC_REPO_TOKEN=… appspec repo push pkg.sif --to http://127.0.0.1:8844
APPSPEC_REPO=http://127.0.0.1:8844 APPSPEC_REPO_TOKEN=… appspec repo push pkg.sif
APPSPEC_REPO_TOKEN=… appspec repo push pkg.sif    # если есть default-профиль

# 3. Pull с проверкой подписи
appspec repo pull pkg:1.0.0-1 --profile corp --trusted-key pub.txt
```

**`$APPSPEC_REPO_TOKEN` — только токен**, не destination. Если задан только
он — `push` вернёт `no destination`.

**`--profile NAME`** — это профиль из `repos.toml`, не имя токена. Не путать
с `repo token gen NAME`.

**Fail-closed:** если в `~/.config/appspec/trusted-keys.toml` есть хотя бы
один ключ, `pull --from URL` требует валидную подпись `index.json`.

### Подписи (E5)

```sh
appspec repo sign                           # pubkey пишется в server config
appspec repo sign --config /etc/appspec/repo-server.toml
appspec repo verify --pubkey pub.txt
```

Схема: ed25519 над JCS-канонизацией (RFC 8785) `index.json` без поля
`signature`. Приватный ключ: `[signing].key` (server config) → `--key` →
`$APPSPEC_SIGNING_KEY` → `~/.config/appspec/signing.key`.

### IPFS (E6) и OCI (E8)

Без изменений относительно v0.3.x:

```sh
appspec repo add --pin-ipfs pkg.sif
appspec repo publish-index --pin
appspec repo pull pkg:1.0.0-1 --via ipfs

appspec repo export-oci --registry ghcr.io/user appspec-index
appspec repo pull pkg:1.0.0-1 --from oci://ghcr.io/user/appspec-index:latest
```

### Discovery

* `repo-server.toml`: `--config`/`-c` → `$APPSPEC_REPO_SERVER_CONFIG` → XDG →
  `/etc` → legacy `<root>/config.toml` (warning).
* `repos.toml`: `--client-config` → `$APPSPEC_REPO_CLIENT_CONFIG` → XDG → `/etc`.

`repo serve` без флагов работает, если server config найден: `[repo].root`
из него и есть «текущий репозиторий».
## Переменные окружения

| Переменная | Значение |
|---|---|
| `APPSPEC_ENGINE` | docker / podman / buildah |
| `BUILDAH_ISOLATION` | oci (default) / chroot |
| `APPSPEC_DEV` | 1 — поиск шаблонов в workspace/templates/ |
| `APPSPEC_TEMPLATE_PATH` | Доп. каталоги шаблонов |
| `APPSPEC_CONFIG_PATH` | Доп. каталоги конфигов |
| `APPSPEC_BUNDLE_CACHE` | Каталог кэша bundles |
| `APPSPEC_BUNDLE_HOME` | Каталог state bundles |
| `APPSPEC_REPO` | Path \| URL \| profile name. Для локальных команд — путь; для read/pull/push — также URL или имя профиля |
| `APPSPEC_REPO_TOKEN` | **Только токен** для `repo push`. Не destination. |
| `APPSPEC_REPO_SERVER_CONFIG` | Путь к `repo-server.toml` (после `--config`) |
| `APPSPEC_REPO_CLIENT_CONFIG` | Путь к `repos.toml` |
| `APPSPEC_SIGNING_KEY` | Приватный ed25519-ключ (после `[signing].key`) |
| `APPSPEC_APPTAINER` | Бинарь `apptainer` (для моков) |
| `APPSPEC_IPFS` | Бинарь `ipfs` (для моков) |
| `APPSPEC_ORAS` | Бинарь `oras` (для моков) |
| `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_DATA_DIRS` | Стандартные XDG |
## Тесты

```sh
cargo test --release                 # unit + integration
cargo test --release --bin appspec   # только unit-тесты bin-крейта
./scripts/smoke-e2e.sh               # e2e (auto: full если возможно, иначе mock)
./scripts/smoke-e2e.sh --full        # требуются apptainer + build-engine
./scripts/smoke-e2e.sh --mock        # без внешних зависимостей
```

Покрытие:
- `tests/cli.rs` — CLI-обвязка (help, gen, ошибки спека);
- `tests/repo_e1.rs` — init/add/ls/show/reindex/check/gc/search;
- `tests/e3.rs` — bundle `source=repo` + `bundle lock`;
- `tests/e4.rs` — HTTP server/client/push;
- `tests/e5.rs` — ed25519 + JCS, fail-closed pull;
- `tests/e6.rs` — IPFS (мок `ipfs`);
- `tests/e7.rs` — `[dependencies]` + `base_digest`;
- `tests/e8.rs` — OCI export (мок `oras`);
- `tests/e9.rs` — `pull --from oci://` (мок `oras`);
- `tests/e10.rs` — TLS для `repo serve`;
- `tests/bundle_extensions.rs` — bundle extensions: подстановки `{file}`/`{target}`, распаковка `.vsix` в каталог (install не исполняется), `type = "file"` + `yes yes | …`, bind на `mount_dir`.

Unit-тесты внутри `src/bundle/loader.rs`, `src/bundle/model.rs`,
`src/bundle/codegen.rs` (`cargo test --release --bin appspec`):

- `prepare_vsix_extracts_to_dir` — `.vsix` распаковывается в каталог `<id>-<ver>/`, исходник не трогается;
- `prepare_vsix_idempotent_with_same_fingerprint` — повторный build по тому же fingerprint — no-op;
- `prepare_vsix_refreshes_dir_when_fingerprint_differs` — при смене fingerprint каталог пересоздаётся;
- `prepare_file_skips_copy_when_src_equals_target` — для `type = "file"` через URL/CID копирование «в себя» не происходит (регресс `copy → copy: ENOENT`);
- `rel_path_*` — basename для vsix/file/archive;
- `install_command_substitutes_*` — подстановка `{file}`/`{target}`/`{name}`/`{id}`/`{version}` в install-команду.

## Документация

- [`docs/spec.md`](docs/spec.md) — справочник по `*.appspec`.
- [`docs/templates.md`](docs/templates.md) — шаблоны, выбор, свои.
- [`docs/bundles.md`](docs/bundles.md) — составные окружения.
- [`docs/macros.md`](docs/macros.md) — макросы.
- [`docs/engines.md`](docs/engines.md) — buildah/podman/docker.
- [`docs/troubleshooting.md`](docs/troubleshooting.md) — типичные проблемы.
- [`docs/repo.md`](docs/repo.md) — локальный и удалённый репозиторий SIF.

## Лицензия

GPL-2.0-or-later. See [LICENSE](LICENSE) for the full text.

## IPNS (mutable-имя репозитория)

С 0.4.4 репозиторий можно адресовать по IPNS. Index по-прежнему
обычный IPFS-объект; IPNS даёт ему стабильное mutable-имя.

```sh
# Сервер: опубликовать index через IPNS (требует [ipfs].ipns_key)
appspec repo publish --ipns

# Клиент: резолв через IPNS, при медленном DHT — fallback на HTTP
appspec repo pull pkg:1.0.0-1 --from ipns://k51...
# Или через профиль (fallback chain: ipns -> url)
appspec repo pull pkg:1.0.0-1 --profile corp
```

Конфигурация (`[ipfs].ipns_key`, `source.ipns`), latency и fail-closed
семантика — см. [`docs/repo.md`](docs/repo.md) и
[`docs/repo-config.md`](docs/repo-config.md).
