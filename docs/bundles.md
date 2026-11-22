# Bundles — составные окружения

Bundle — это base SIF плюс автономные компоненты плюс расширения.

~~~
[bundle]
name        = "vscodium-dev"
description = "VSCodium + git + extensions"
strict      = false
base        = { spec = "./vscodium.appspec" }

[[components]]
name    = "git"
spec    = "./git.appspec"
provide = "git"

[[extensions]]
name = "ms-azuretools.vscode-docker"
~~~

## Секция bundle

| Ключ | Тип | Default | Описание |
|------|-----|---------|----------|
| name | string | — | Имя bundle. Используется в имени launcher и в кэше |
| description | string | пусто | Описание для комментария в launcher |
| strict | bool | false | Если true — ошибки вместо warn-ов |
| resolve_dependencies | bool | true | Автодобавлять extensionDependencies |
| expand_packs | bool | false | Разворачивать extensionPack |
| base.source | — | — | Источник base SIF |
| base.setup | string | — | Setup для base, если не задан в самом спеке |
| base.entry | string | — | Entry для base, если не извлекается из setup |

## Источник

source (у base и у компонентов) — ровно один из:

- `spec = "..."` — универсальный источник (0.6.0), см. ниже.
- `sif = "./foo.sif"` — уже собранный.
- `cid = "bafy..."` — IPFS CID.
- `manifest = "./installed-dir"` — по manifest.json от appspec install.
- `repo = "foo:1.0"` — deprecated alias для `spec`, удаляется в 0.7.0.

### `spec` — universal source (0.6.0)

`spec = "..."` в `[bundle.base]` и `[[components]]` принимает три формы:

| Значение | Классификация | Разрешение |
|---|---|---|
| `./foo.appspec`, `/abs/foo.appspec`, `sub/foo.appspec` | explicit path | только локальный файл; ошибка, если его нет |
| `foo.appspec` (без `/`) | ambiguous | сначала `./foo.appspec`; если нет — `foo:latest` из репозитория |
| `foo`, `foo:latest`, `foo:1.0`, `foo:1.0-1` | repo ref | `latest` / max release для `foo:1.0` / точный ключ |

Правило классификации совпадает с `appspec run <package-or-file>`.

~~~toml
[bundle.base]
spec  = "./vscodium.appspec"
# или
# spec  = "vscodium:1.135.06055"
# spec  = "vscodium"
entry = "exec codium"

[[components]]
name = "git"
spec = "git.appspec"     # локально, иначе git:latest
provide = "git"
~~~

`spec = "foo.appspec"` и `repo = "foo"` означают «пакет foo, latest», но
`spec` сначала проверяет локальный файл. `spec = "./foo.appspec"` —
строго локальный вариант.

`bundle.lock.toml` хранит канонический ref (`foo:1.2.3-1`) независимо от
того, что было написано в `spec`.

## Секция components

| Ключ | Тип | Default | Описание |
|------|-----|---------|----------|
| name | string | — | Имя компонента |
| source | — | — | Как у base.source |
| mode | string | chroot | chroot / instance / both |
| provide | string | name | Имя CLI, которое предоставляет компонент |
| mount | path | /opt/utils/NAME | Куда монтировать sandbox |
| binary | path | /usr/bin/NAME | Бинарь внутри sandbox |
| target | path | /usr/local/bin/NAME | Куда положить bridge-скрипт |
| mounts | [string] | PWD, HOME | Что пробрасывать внутрь chroot |
| bins | [string \| table] | [] | Дополнительные бинари компонента, пробрасываемые в base через bridge (`/usr/bin/NAME` → `/usr/local/bin/NAME`; таблица `{ name, path, target }`) |
| forward | [string] | TERM, LANG, LC_ALL | Какие env пробрасывать |
| env | map | пусто | Env для sandbox или instance |
| contain | bool | true | --contain для instance |
| selftest | obj | — | cmd и expect_contains |
| files | [obj] | [] | Дополнительные файлы |

Только для chroot (и both):

- bridge — стратегия запуска:
  - `chroot` (по умолчанию) — chroot в sandbox компонента;
  - `apptainer` — зарезервировано;
  - `exec` — запуск бинаря из rootfs компонента **без chroot**, в
    namespace base. Нужен инструментам, чей дочерний процесс должен
    видеть base (`tofu` и его `local-exec`: `ansible-playbook`,
    `qemu-img`, `ssh`) и текущий проект. Компонент при этом остаётся
    отдельным атомарным sandbox'ом — bridge лишь запускает его бинарь.

Только для instance (и both):

- socket_dir — каталог на хосте под сокет.
- state_dir — каталог на хосте под state.
- daemon — команда, запускающая демон в instance.
- daemon_socket — путь к сокету внутри instance.

Пример `bins`:

~~~toml
[[components]]
name    = "libvirtd"
spec    = "./virt-daemon.appspec"
mode    = "both"          # bins требуют sandbox
bins    = ["qemu-img", "qemu-system-x86_64", "cloud-localds"]
~~~

Для нестандартных путей:

~~~toml
bins = [{ name = "mytool", path = "/usr/sbin/mytool", target = "/usr/local/bin/mytool" }]
~~~

Если `bins = ["bash"]` даёт `/usr/bin/bash`, а в sandbox (например,
Alpine) он лежит в `/bin/bash`, bridge автоматически пробует парный
каталог `/bin` ↔ `/usr/bin`.

## Как работает mode = chroot

1. SIF компонента разворачивается в sandbox: BUNDLE_DIR/sandboxes/NAME.
2. В base-инстанс кладётся bridge-скрипт в target.
3. Bridge при вызове:

   - биндит PWD и HOME внутрь sandbox;
   - пропускает SSH_AUTH_SOCK, если задан;
   - пропускает файлы из mounts, начинающиеся со слеша;
   - делает exec chroot SANDBOX BINARY со всеми аргументами.

Все bridge-скрипты (primary `provide` и дополнительные `bins`)
записываются внутрь base-instance в `/opt/appspec/bin` (writable tmpfs,
снимок на каждый запуск). Каждый bridge перед chroot монтирует внутрь
sandbox `/opt/appspec/bin` и mount-пути остальных sandbox-компонентов,
а также добавляет `/opt/appspec/bin` в PATH. Поэтому инструменты
компонентов доступны не только в base, но и внутри chroot других
компонентов.

`bins` требует `mode = "chroot"` или `"both"`: у `instance`-компонента
нет sandbox. Для таких компонентов используйте `mode = "both"`.

## Как работает mode = instance

1. Для компонента создаётся отдельный apptainer instance с командой daemon.
2. В instance монтируются socket_dir и state_dir.
3. Base-инстанс получает env CONTAINER_HOST и DOCKER_HOST, указывающие
   на сокет.

### Podman-специфика

Codegen распознаёт podman-подобные компоненты по:

- первый токен daemon — podman, podman-remote, buildah или docker;
- env содержит CONTAINER_HOST или DOCKER_HOST;
- среди instance-файлов есть /etc/containers/storage.conf;
- name совпадает с podman, buildah или docker.

Для них автоматически:

- дописывается --time=0 в podman system service, если не задан;
- генерируется wrapper /opt/appspec/podman-daemon-wrapper.sh с mount-fix,
  fallback на PATH, HOME и XDG_RUNTIME_DIR;
- кладётся user-level containers.conf с cgroups = disabled и
  netns = host;
- прокидываются /dev/fuse, /dev/net/tun, /sys/fs/cgroup в режиме ro.

## Секция extensions

| Ключ | Тип | Описание |
|------|-----|----------|
| name | string | ID расширения. Для online — publisher.name |
| file | path | Локальный .vsix |
| cid | string | IPFS CID .vsix |
| url | string | URL .vsix |
| requires | [string] | Имена CLI, которые должны быть в base |

Ровно один из file, cid, url — либо ничего (значит online через Open VSX).

Подготовленные расширения хранятся в BUNDLE_CACHE/extensions/ID-VERSION.
В base-инстанс монтируется BUNDLE_DIR/extensions как каталог расширений
редактора.

## Секция components.files

| Ключ | Тип | Описание |
|------|-----|----------|
| to | path | Абсолютный путь в контейнере |
| from | path | Источник относительно spec_dir |
| content | string | Inline содержимое |
| mode | u32 | Права. Default 0o644 |
| scope | string | base / sandbox / instance через запятую |

Файлы передаются через base64 внутри launcher-скрипта.

## Команды

~~~
appspec bundle check  bundle.appspec
appspec bundle build  bundle.appspec
appspec bundle ls     bundle.appspec
appspec bundle add    bundle.appspec git --from-spec ./git.appspec
appspec bundle remove bundle.appspec git
~~~

- check — валидация без сборки. Exit 2 при ошибках.
- build — собрать run.NAME.sh.
- ls — показать состав.
- add, remove — редактирование секции components в bundle.appspec.

appspec run bundle.appspec — build плюс run.
appspec clean bundle.appspec — удалить launcher и кэш.

## Кэш и state

По умолчанию:

- cache: `XDG_CACHE_HOME/appspec/bundles/NAME` (или `~/.cache/appspec/bundles/NAME`);
- state: `XDG_DATA_HOME/appspec/bundles/NAME` (или `~/.local/share/appspec/bundles/NAME`).

Переопределяется через `APPSPEC_BUNDLE_CACHE` (cache) и
`APPSPEC_BUNDLE_HOME` (state).

**Важно:** `[runtime]` базового спека наследуется bundle-ом. Если base
задаёт bind на `/config/home` (например `vscodium.appspec`:
`CONFIG_DIR="$HOME/vscodium_config"` и `$CONFIG_DIR/home:/config/home`),
bundle использует именно его. Home не попадает в cache и переживает
`appspec clean`. Fallback для base'ов без такого bind'а —
`STATE/NAME/home`.

Структура cache:

~~~
NAME/
  sandboxes/         распакованные SIF компонентов с mode=chroot|both
    COMPONENT-HASH/   HASH — первые 12 символов sha256 SIF
      .ready          маркер завершённой распаковки
    COMPONENT         symlink → COMPONENT-HASH (монтируется как /opt/utils/COMPONENT)
  extensions/        подготовленные .vsix
  run/               сокеты и state podman-компонентов
  state/             прочий state
~~~

`appspec clean BUNDLE` удаляет cache и launcher. Persistent state
сохраняется; `appspec clean --purge BUNDLE` удаляет и его.

Fingerprint sandbox'а — `sha256` SIF в имени каталога
(`COMPONENT-<hash12>`). Новая версия SIF получает новый sandbox, а
запущенные instance'ы продолжают видеть свой старый — без `rm -rf`
поверх активного bind'а.

### Trust и выбор репозитория (0.4.5)

С 0.4.5 `bundle build|check|ls|lock` принимают:

| Флаг | Default | Что |
|---|---|---|
| `--repo PATH` | `$APPSPEC_REPO` → XDG | Путь к локальному репозиторию |
| `--trusted-key PATH` | — | ed25519 pubkey для проверки `index.json`. Repeatable. Мержится с `~/.config/appspec/trusted-keys.toml` |
| `--allow-unsigned` | `false` | Осознанный opt-out: принять неподписанный индекс (warning) |
| `--locked` | `false` | (build/check) требовать `bundle.lock.toml`; дрейф — ошибка |

Поведение при repo-ref source (`spec = "name[:ver]"`):

- `trusted_keys` пуст и `--allow-unsigned` не задан → warning
  "no trusted keys configured; index signature not verified".
- `trusted_keys` непуст → **fail-closed**: подпись `index.json`
  обязательна и валидна. Невалидная подпись → `bundle build` / `bundle check`
  / `bundle lock` падает с exit != 0.
- `bundle lock` проверяет подпись **до** записи lock'а. Lock нельзя
  снять с подменённого индекса.

Пример:

```sh
# Обычный build — репо из $APPSPEC_REPO
appspec bundle build vscodium-bundle.appspec

# Явный репо + проверка подписи (fail-closed)
appspec bundle build vscodium-bundle.appspec \
    --repo /srv/appspec-repo \
    --trusted-key ~/.config/appspec/trusted-keys.toml

# Локальный репо без подписи — осознанный opt-out
appspec bundle build vscodium-bundle.appspec \
    --repo /srv/appspec-repo \
    --allow-unsigned
```

`bundle.lock.toml` пишется через `bundle lock --repo PATH`, что гарантирует
согласованность lock'а и подписанного индекса на момент создания.

## Публикация bundle в репозиторий (0.6.2)

```sh
appspec repo add vscodium-bundle.appspec --version 1.0
# или явно:
appspec repo add-bundle vscodium-bundle.appspec --version 1.0 --release 2
```

`repo add` распознаёт `[bundle]` автоматически. Для публикации источники
base/components должны быть repo-ref (`foo`, `foo:1.0`, `foo:1.0-1`) или
`cid`; локальные `spec`/`sif`/`manifest` и локальные файлы расширений
отклоняются. В пакет пишутся `spec.appspec`, `manifest.json` (резолвнутые
refs + хеши) и `meta.json` с `kind = "bundle"`.

С 0.6.3 работают `repo pull` bundle (path/HTTP/IPFS/IPNS, с рекурсивным
pull зависимостей) и `appspec run <bundle-name>`: манифест подставляет
канонические `name:version-release`, launcher собирается локально.
С 0.6.4 `repo pin` для bundle пинит оба блоба (bundle spec + manifest) и
обновляет `ipfs_cid`/`manifest_ipfs_cid`; OCI-экспорт bundle пока не
реализован. `repo ls|show|check|gc` понимают `kind = "bundle"`.
