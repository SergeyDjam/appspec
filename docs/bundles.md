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

- spec = "./foo.appspec" — соберём SIF, если нет.
- sif = "./foo.sif" — уже собранный.
- cid = "bafy..." — IPFS CID.
- manifest = "./installed-dir" — по manifest.json от appspec install.

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
| forward | [string] | TERM, LANG, LC_ALL | Какие env пробрасывать |
| env | map | пусто | Env для sandbox или instance |
| contain | bool | true | --contain для instance |
| selftest | obj | — | cmd и expect_contains |
| files | [obj] | [] | Дополнительные файлы |

Только для chroot (и both):

- bridge — chroot (по умолчанию) или apptainer.

Только для instance (и both):

- socket_dir — каталог на хосте под сокет.
- state_dir — каталог на хосте под state.
- daemon — команда, запускающая демон в instance.
- daemon_socket — путь к сокету внутри instance.

## Как работает mode = chroot

1. SIF компонента разворачивается в sandbox: BUNDLE_DIR/sandboxes/NAME.
2. В base-инстанс кладётся bridge-скрипт в target.
3. Bridge при вызове:

   - биндит PWD и HOME внутрь sandbox;
   - пропускает SSH_AUTH_SOCK, если задан;
   - пропускает файлы из mounts, начинающиеся со слеша;
   - делает exec chroot SANDBOX BINARY со всеми аргументами.

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

По умолчанию: HOME/.cache/appspec/bundles/NAME.

Переопределяется через APPSPEC_BUNDLE_CACHE или APPSPEC_BUNDLE_HOME.

Структура каталога:

~~~
NAME/
  sandboxes/         распакованные SIF компонентов с mode=chroot
    COMPONENT/
      .fingerprint   size-mtime SIF, для инвалидации
  home/              HOME base-инстанса
  extensions/        подготовленные .vsix
  run/               сокеты и state podman-компонентов
  state/             прочий state
~~~

Fingerprint — простая пара size-mtime. Если SIF пересобран с тем же
размером и mtime, sandbox не обновится. Для большинства сценариев
этого достаточно.

### Trust и выбор репозитория (0.4.5)

С 0.4.5 `bundle build|check|ls|lock` принимают:

| Флаг | Default | Что |
|---|---|---|
| `--repo PATH` | `$APPSPEC_REPO` → XDG | Путь к локальному репозиторию |
| `--trusted-key PATH` | — | ed25519 pubkey для проверки `index.json`. Repeatable. Мержится с `~/.config/appspec/trusted-keys.toml` |
| `--allow-unsigned` | `false` | Осознанный opt-out: принять неподписанный индекс (warning) |

Поведение при `source = repo`:

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
