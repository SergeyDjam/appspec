# appspec

Сборка Apptainer SIF из декларативного спека (TOML). Dockerfile и launcher
генерируются через minijinja-шаблоны. Для составных окружений есть
подсистема bundles — base SIF + компоненты + расширения.

## Возможности

- Один спек — все артефакты. Dockerfile, run-скрипт, SIF, .desktop,
  launcher — генерируются из *.appspec.
- Десять шаблонов сборки: deb, tar, apt, generic, rpm, alpine,
  alpine-slim, full, static, scratch.
- Три движка сборки: buildah (rootless, OCI-native), podman, docker.
- Composable bundles: base SIF + компоненты (chroot/instance/both) +
  расширения из файла / URL / IPFS / Open VSX.
- RPM-style макросы: name, version, bin, optional, default.
- Пользовательские шаблоны: поиск в 6 местах с предсказуемым приоритетом.

## Установка

~~~
git clone <repo> appspec
cd appspec
cargo build --release
install -m755 target/release/appspec ~/.local/bin/
~~~

Зависимости времени выполнения:

- apptainer — обязательно.
- buildah / podman / docker — минимум один из трёх.
- ipfs — опционально, для ipfs-publish и bundle-источников cid.
- curl или wget — для bundle-расширений по URL.
- unzip — для подготовки .vsix.

## Быстрый старт

Одиночное приложение:

~~~
appspec gen    examples/strawberry.appspec
appspec build  examples/strawberry.appspec
appspec run    examples/strawberry.appspec
appspec stop   examples/strawberry.appspec
appspec all    examples/strawberry.appspec
appspec clean  examples/strawberry.appspec
~~~

Bundle:

~~~
appspec bundle check examples/vscodium-bundle.appspec
appspec bundle build examples/vscodium-bundle.appspec
appspec run          examples/vscodium-bundle.appspec
~~~

## Команды

| Команда | Описание |
|---------|----------|
| gen SPEC | Сгенерировать Dockerfile и run-скрипт |
| build SPEC | gen + image build + SIF. -f / --force — пересобрать |
| run SPEC | Запустить launcher (SIF или bundle) |
| stop SPEC | Остановить instance, оставшийся от persist = true |
| all SPEC | build + run. -f / --force — пересобрать |
| clean SPEC | Удалить Dockerfile, launcher, SIF, tar, кэш bundle |
| install SPEC | Установить SIF + launcher + .desktop |
| uninstall SPEC | Удалить всё из манифеста |
| ipfs-publish SPEC | Опубликовать SIF в IPFS |
| bundle {check,build,ls,add,remove} | Составные окружения |

## Спека *.appspec

TOML. Основные секции: package, build, runtime, install, macros.
Секция build.source.type определяет способ установки: deb, rpm, tar,
binary или пустая строка (только apt-пакеты).

Минимальный спек:

~~~
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
~~~

Полный справочник — в docs/spec.md.

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

Правило выбора — docs/templates.md.

## Макросы

Раскрываются в load_spec до парсинга TOML. Работают и в полях, и в
многострочных setup и entry.

- name, version, bin — из секции package.
- _prefix, _bindir, _sbindir, _libdir — дистрибутив-специфичные.
- {?optional} — пусто, если не задан.
- {name:default} — значение по умолчанию.
- %% — буквальный символ процента.
- Пользовательские — из секции macros.

См. docs/macros.md.

## Bundles

Bundle — составное окружение из base SIF + компонентов + расширений.

~~~
[bundle]
name = "vscodium-dev"
base = { spec = "./vscodium.appspec" }

[[components]]
name    = "git"
spec    = "./git.appspec"
provide = "git"

[[extensions]]
name = "ms-azuretools.vscode-docker"
~~~

Компоненты бывают трёх видов: chroot, instance, both.
Расширения — четырёх: file, cid, url, online (Open VSX).

Полное описание — docs/bundles.md.

## Переменные окружения

| Переменная | Значение |
|------------|----------|
| APPSPEC_ENGINE | docker / podman / buildah |
| BUILDAH_ISOLATION | oci (default) / chroot |
| APPSPEC_DEV | 1 — поиск шаблонов в workspace/templates/ |
| APPSPEC_TEMPLATE_PATH | Доп. каталоги шаблонов |
| APPSPEC_CONFIG_PATH | Доп. каталоги конфигов |
| APPSPEC_BUNDLE_CACHE | Каталог кэша bundles |
| APPSPEC_BUNDLE_HOME | Каталог state bundles |
| XDG_CONFIG_HOME, XDG_DATA_HOME, XDG_DATA_DIRS | Стандартные XDG |

## Документация

- docs/spec.md — справочник по *.appspec.
- docs/templates.md — шаблоны, выбор, свои.
- docs/bundles.md — составные окружения.
- docs/macros.md — макросы.
- docs/engines.md — buildah/podman/docker.
- docs/troubleshooting.md — типичные проблемы.

## Лицензия

MIT.
