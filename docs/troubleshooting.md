# Типичные проблемы

Сборник симптомов, причин и решений. Сгруппирован по стадиям: Apptainer,
buildah, magicpak, сборка GUI, поведение лаунчера.

## Apptainer: starter-suid doesn't have setuid bit set

Симптом. apptainer exec или apptainer instance start возвращает
Installation issue: starter-suid doesn't have setuid bit set.

Причина. В /etc/apptainer/apptainer.conf стоит allow setuid = yes,
но у /usr/libexec/apptainer/bin/starter-suid нет setuid-бита.

Решение. Одно из двух:

~~~
# Вариант 1 — поставить бит
sudo chmod u+s /usr/libexec/apptainer/bin/starter-suid

# Вариант 2 — переключиться в rootless
sudo sed -i 's/^allow setuid = yes/allow setuid = no/' /etc/apptainer/apptainer.conf
~~~

Для rootless также нужно:

~~~
sysctl kernel.unprivileged_userns_clone
~~~

Ожидается значение 1.

## Apptainer: fuse: failed to exec fusermount

Симптом. apptainer exec foo.sif cmd возвращает
image driver squashfuse_ll instance exited with error:
fuse: failed to exec fusermount.

Причина. Нет бинарника fusermount (fuse2). В системе может быть
только fusermount3 — это другой бинарь, Apptainer его не находит.

Решение. Поставить fuse2:

~~~
sudo apt-get install fuse
which fusermount
~~~

Ожидается путь /usr/bin/fusermount, обычно symlink на fusermount2.

Если пакета нет — symlink на fusermount3 работает не всегда:

~~~
sudo ln -s /usr/bin/fusermount3 /usr/local/bin/fusermount
~~~

Крайний случай, для отладки — обойти FUSE целиком:

~~~
apptainer exec --unsquash foo.sif cmd
~~~

## buildah: Error during unshare(CLONE_NEWUSER)

Причина. Нет /etc/subuid и /etc/subgid для текущего пользователя.

Решение.

~~~
sudo usermod --add-subuids 100000-165535 --add-subgids 100000-165535 "$(whoami)"
~~~

После команды нужно перелогиниться. Проверка:

~~~
grep "^$(whoami):" /etc/subuid /etc/subgid
~~~

Ожидается строка user:100000:65536 в обоих файлах.

Также проверьте, что newuidmap и newgidmap имеют setuid-бит:

~~~
ls -la /usr/bin/newuidmap /usr/bin/newgidmap
~~~

Бит s в правах владельца обязателен.

## buildah: open /sys/fs/cgroup/devices/... No such file or directory

Причина. Хост работает на cgroup v1. Rootless buildah не может
создать device-cgroup.

Решение.

~~~
BUILDAH_ISOLATION=chroot APPSPEC_ENGINE=buildah appspec build foo.appspec
~~~

Или через переменную окружения сессии:

~~~
export BUILDAH_ISOLATION=chroot
~~~

Проверка версии cgroup:

~~~
stat -fc %T /sys/fs/cgroup
~~~

tmpfs означает cgroup v1, cgroup2fs означает cgroup v2.

## buildah: short-name did not resolve to an alias

Причина. Не настроен registry-конфиг. buildah не знает, где искать
короткие имена вроде debian:trixie.

Решение. Создать ~/.config/containers/registries.conf:

~~~
unqualified-search-registries = ["docker.io"]

[[registry]]
prefix = "docker.io"
location = "docker.io"
~~~

Не смешивайте v1 и v2. Секции [registries.search] и
unqualified-search-registries не должны присутствовать одновременно.

## magicpak: Unable to locate executable 'cc'

Причина. В builder-стадии нет C-компилятора. magicpak --dynamic
компилирует LD_PRELOAD-резолвер через cc.

Решение. Добавить gcc или build-essential в build.tools или
build.apt.extra. В шаблонах deb, tar, apt уже стоят gcc и libc6-dev.

## magicpak: IO error: No such file or directory на Alpine

Причина. Статический режим magicpak требует /etc/ld.so.cache, на
musl-системах его нет.

Решение. Использовать шаблон alpine (apk --root) или alpine-slim
(режим --experimental-noload-resolver).

## magicpak: source path is neither a regular file nor a symlink

Причина. В build.collect.includes попал глоб, раскрывающийся в
директорию. magicpak не умеет копировать каталоги через --include.

Решение. Один из двух:

- Добавить /* в конце пути, чтобы глоб раскрывался в файлы.
- Оставить каталог без /* — шаблон обойдёт его рекурсивно через
  find -type f.

Все актуальные шаблоны (deb, tar, apt, generic, alpine-slim, full)
фильтруют директории автоматически.

## magicpak: SIGILL и таймаут 124

Симптом. Лог содержит WARN trace_syscalls: stopped by signal SIGILL,
затем exit=124 и сообщение magicpak produced no usable bundle.

Причина. magicpak не работает с Electron/Chromium. Их sandbox
использует низкоуровневые системные вызовы, несовместимые с ptrace.

Решение. Использовать шаблон full, который не применяет magicpak.
SIF получится больше, но соберётся и заработает.

## GUI: Could not find the Qt platform plugin xcb

Причина. libqxcb.so не попал в бандл. magicpak не увидел dlopen,
потому что приложение не запускалось под Xvfb.

Решение. Убедиться, что magicpak --dynamic действительно запускает
приложение под Xvfb. В шаблонах deb, tar, apt это делается
автоматически через xvfb-run.

Если Qt всё равно не находит плагин, добавить в
build.collect.includes:

~~~
/usr/lib/x86_64-linux-gnu/qt6/plugins/platforms/*
/usr/lib/x86_64-linux-gnu/qt6/plugins/xcbglintegrations/*
~~~

## GUI: пустые квадраты вместо букв

Причина. В бандле нет шрифтов. fontconfig видит конфиг, но самих
файлов шрифтов нет, glif-ы не отрисовываются.

Решение. Один из двух путей.

Пробросить шрифты с хоста через runtime.binds:

~~~
"/usr/share/fonts:/usr/share/fonts:ro"
"$HOME/.local/share/fonts:/config/home/.local/share/fonts:ro"
~~~

Или добавить в build.apt.extra пакет fonts-dejavu-core. Этого хватит
для базового рендеринга кириллицы и латиницы.

## GUI: FATAL no such file /bin/sh

Причина. Финальный scratch не содержит shell. Apptainer instance
start и apptainer shell требуют /bin/sh.

Решение. В шаблонах для этого используется busybox как
мульти-applet. Если вы пишете свой шаблон — добавьте установку
busybox и создайте относительные симлинки в /bundle/bin.

## Electron: INTERNAL DATA STREAM ERROR при HTTPS

Причина. Нет TLS-бэкенда GIO (glib-networking). GStreamer или libsoup
не могут установить TLS-соединение.

Решение. Добавить в build.apt.extra:

~~~
glib-networking
ca-certificates
~~~

И в build.collect.includes:

~~~
/usr/lib/x86_64-linux-gnu/gio/modules/*
/etc/ssl/certs/*
~~~

После этого GIO сможет использовать libgiognutls.so из модулей.

## Terminal висит после appspec run

Причина. В runtime.setup присутствует exec angie или exec nginx
в конце скрипта. Пока процесс жив, apptainer exec не возвращает
управление.

Решение. Убрать exec и daemon off. Angie и nginx по умолчанию
демонизируются сами: форкаются и отдают управление родителю.

Если нужно, чтобы instance жил после завершения setup, установить
persist = true. Тогда лаунчер не ставит trap для остановки.

## appspec stop возвращает instance not running

Причина. Имя instance не совпадает. appspec stop ищет package.name
в выводе apptainer instance list.

Проверка:

~~~
apptainer instance list
~~~

Имя instance = package.name из спека. Если запускался не через
appspec run, а вручную, имя могло быть другим.

## appspec run не пересобирает SIF

Поведение по умолчанию. run не вызывает build. Если Dockerfile новее
SIF, выводится WARNING, но сборка не запускается.

Решение.

~~~
appspec build -f foo.appspec
appspec run   foo.appspec
~~~

Или одной командой:

~~~
appspec all -f foo.appspec
~~~

Флаг -f или --force пересобирает SIF даже если он уже существует.

## appspec build падает в dpkg с конфликтами пакетов

Симптом. В логе виден большой список Conflicts: с перечислением
пакетов репозитория.

Причина. Скорее всего, в DEPS .deb-пакета есть альтернативы через
вертикальную черту: A | B | C. apt-get install не понимает такую
грамматику — он видит вертикальную черту как отдельный аргумент.

Решение. Использовать apt-get satisfy вместо apt-get install. Это
уже сделано в шаблонах deb и full. Если пишете свой шаблон —
применяйте apt-get satisfy.

## Ошибка: TOML parse error at line N, column M

Общие причины:

1. Inline-таблицы { } записаны в несколько строк. В TOML inline-таблица
   должна быть на одной строке. Перенести поля в [[section]] массив
   таблиц.

2. Подтаблица в конце секции перекрывает плоские поля. В TOML
   [runtime.vars] съедает все последующие ключи. Плоские поля —
   до подтаблиц, подтаблицы — в самом конце.

3. Отсутствие обязательных полей. Например, забыт [package] в начале
   файла. Проверьте наличие всех требуемых секций.

## SIF слишком большой

Проверьте:

~~~
ls -la foo.sif
~~~

Если больше ожидаемого — возможные причины:

- Использован шаблон full вместо alpine или scratch.
- В build.apt.extra случайно попал большой набор пакетов.
- В build.collect.includes попали каталоги целиком, и magicpak
  включил их полностью.

Решение. Перейти на более минималистичный шаблон: scratch для чистого
CLI, static для CLI с shell, alpine для языковых тулчейнов.

## `repo push`: `no destination: pass --to URL, --profile NAME, or set $APPSPEC_REPO to a URL/profile`

**Симптом.** `APPSPEC_REPO_TOKEN=… appspec repo push pkg.sif` падает.

**Причина.** `$APPSPEC_REPO_TOKEN` — это **токен**, не destination. `push` нужны три вещи: URL, токен, файл.

**Решение.**

```sh
# 1. Явно
APPSPEC_REPO_TOKEN=… appspec repo push pkg.sif --to http://127.0.0.1:8844

# 2. Через env
APPSPEC_REPO=http://127.0.0.1:8844 APPSPEC_REPO_TOKEN=… appspec repo push pkg.sif

# 3. Через профиль (многоразово)
appspec repo client init --profile local \
    --source-url http://127.0.0.1:8844 \
    --token-env APPSPEC_REPO_TOKEN --default
APPSPEC_REPO_TOKEN=… appspec repo push pkg.sif
```

## `repo push --profile test`: `client config not found`

**Симптом.** `appspec repo push pkg.sif --profile test` → `client config not found; looked in: ~/.config/appspec/repos.toml, /etc/appspec/repos.toml`.

**Причина.** `test` — это имя **токена** (`repo token gen test`), а `--profile` ищет профиль в client config. Это разные сущности.

**Решение.**

```sh
appspec repo client init --profile local \
    --source-url http://127.0.0.1:8844 \
    --token-env APPSPEC_REPO_TOKEN --default

APPSPEC_REPO_TOKEN=… appspec repo push pkg.sif
```

Полный справочник — [`docs/repo-config.md`](repo-config.md#--profile-vs-имя-токена).

## `repo ls` без флагов → `repository not initialized at ~/.local/share/appspec/repo`

**Симптом.** `appspec repo ls` падает, хотя `~/.config/appspec/repo-server.toml` существует и `[repo].root` валиден.

**Причина (одна из двух).**

1. **Файл `repos.toml` битый** (не TOML). `search`/`ls`/`show`/`verify` в этом случае молча уходят в `default_root()` → `~/.local/share/appspec/repo`. Если там пусто — «not initialized».

2. **`repos.toml` валиден, но `default` не задан, `$APPSPEC_REPO` не задан**, а `repo ls` смотрит в client config, не в server. Discovery `repo-server.toml` для `ls`/`search`/`show`/`verify` **не делается**.

**Решение.**

```sh
# Проверить, что repos.toml — валидный TOML
cat ~/.config/appspec/repos.toml
# должно быть:
#   schema = 1
#   default = "local"
#   [[repos]]
#   name = "local"
#   source = { url = "http://127.0.0.1:8844" }
#   token = "env:APPSPEC_REPO_TOKEN"

# Пересоздать, если битый
rm ~/.config/appspec/repos.toml
appspec repo client init --profile local \
    --source-url http://127.0.0.1:8844 \
    --token-env APPSPEC_REPO_TOKEN --default

# Или явно без client config
APPSPEC_REPO=http://127.0.0.1:8844 appspec repo ls
```

## `repos.toml` создан, но `repo ls` пишет `client config exists but could not be loaded`

**Симптом.** После ручного редактирования `~/.config/appspec/repos.toml` любая команда падает.

**Причина.** Файл содержит не TOML. Классика — скопировали вывод `repo client ls` (там `default: local`, `local url:…`), а это не синтаксис TOML.

**Решение.**

```sh
rm ~/.config/appspec/repos.toml
appspec repo client init --profile local \
    --source-url http://127.0.0.1:8844 \
    --token-env APPSPEC_REPO_TOKEN --default
```

Или вручную:

```toml
schema = 1
default = "local"

[[repos]]
name = "local"
source = { url = "http://127.0.0.1:8844" }
token = "env:APPSPEC_REPO_TOKEN"
```

## `repo token gen ci --write-file /path --mode 400`: permission 0400 или 400?

**Симптом.** `stat -c '%a'` показывает `400`, а тест ждёт `0o400` — или наоборот.

**Причина.** `--mode` — **восьмеричный**, как `chmod`. `--mode 400` = `0o400 = 256` (r--------). `--mode 999` → ошибка `must be octal`.

**Решение.** Писать `400`, `0400` или `0o400` — все три эквивалентны.

Ошибки:

* `--mode must be octal (e.g. 400, 0400, 0o400), got …` — ввели десятичное.
* `--mode out of range: 1777 (allowed 0..=0777)` — ввели `1777`.

## `repo serve --root ./repo` → `warning: server config … points to /abs/repo, not ./repo`

**Симптом.** Warning, хотя `./repo` и `[repo].root` — один каталог.

**Причина (v1).** `norm_path` сравнивает лексически после приведения к absolute от текущего CWD. Если CWD при `serve` и при создании конфига разные — пути не совпадут.

**Решение.** Использовать `[repo].root` из конфига напрямую:

```sh
appspec repo serve --config ~/.config/appspec/repo-server.toml --listen 127.0.0.1:8844
```

Или создать конфиг с `--root`, совпадающим с реальным:

```sh
appspec repo server init --root "$(realpath ./repo)"
```

## `repo init` дважды: второй раз — `already initialized as 'test/a'`

**Поведение.** `repo init <root> --name X` проверяет `index.json::repo`. Если репозиторий уже инициализирован с другим именем — ошибка. С тем же — no-op.

**Что делать.** Если нужно сменить имя — пересоздать репозиторий:

```sh
mv /srv/repo /srv/repo.old
appspec repo init /srv/repo --name test/b --config ~/.config/appspec/repo-server.toml
```

## Legacy `<root>/config.toml`: `warning: … is deprecated; run 'appspec repo migrate-config …'`

**Причина.** Phase 1 не создаёт `<root>/config.toml`, но читает его для обратной совместимости. Warning — напоминание.

**Решение.**

```sh
appspec repo migrate-config /srv/repo
# → writes ~/.config/appspec/repo-server.toml
# → renames /srv/repo/config.toml to .config.toml.migrated
```

После этого warning исчезнет.

## `repo add` с `$APPSPEC_REPO=http://…`: `локальная операция, требует путь`

**Причина.** `add`/`check`/`gc`/`reindex`/`sign`/`serve`/`publish-index`/`export-oci` работают с диском. `$APPSPEC_REPO` может быть URL только для `search`/`ls`/`show`/`verify`/`pull`/`push`.

**Решение.**

* Для локальной операции: `--root PATH` или `--config PATH` побеждает env.
* Для публикации: `repo push --to <url>`.

## `repo publish --ipns`: `--ipns requires [ipfs].ipns_key`

**Симптом.** `appspec repo publish --ipns` падает с сообщением:

    `--ipns` requires `[ipfs].ipns_key` in server config;
    run `ipfs key gen <name>` and add `[ipfs].ipns_key = "<name>"`

**Причина.** В `repo-server.toml` нет секции `[ipfs].ipns_key`.

**Решение.**

    # 1. Создать ключ в локальном ipfs keystore
    ipfs key gen appspec-corp

    # 2. Добавить в server config (или `appspec repo publish --ipns --config ...` с уже прописанным ключом)
    cat >> ~/.config/appspec/repo-server.toml <<'TOML'
    [ipfs]
    ipns_key = "appspec-corp"
    TOML

    # 3. Опубликовать
    appspec repo publish --ipns --config ~/.config/appspec/repo-server.toml

## `repo publish --ipns`: `ipfs name publish failed for key X`

**Симптом.** Публикация падает с `ipfs name publish failed for key 'appspec-corp': Error: ...`.

**Причина.** `[ipfs].ipns_key` задан, но ключа с таким именем нет в
локальном `ipfs` keystore (опечатка, или ключ был на другой машине).

**Решение.**

    # Проверить
    ipfs key list | grep appspec-corp

    # Если нет — создать
    ipfs key gen appspec-corp

    # Если ключ есть на другой машине — экспорт + импорт
    ipfs key export appspec-corp -o appspec-corp.key    # на старой
    ipfs key import appspec-corp appspec-corp.key       # на новой

`ipfs key import` принимает только файлы, созданные `ipfs key export`.

## `ipfs name resolve` зависает / timeout

**Симптом.** `repo pull --from ipns://...` висит минуты.

**Причина.** Первый DHT-резолв требует walk по сети, 1–3 минуты в
зависимости от bootstrap-узлов и NAT.

**Решение.**

1. `pull` использует hard-timeout 30 s, после чего падает в fallback
   на `source.url`, если URL задан в профиле.
2. Если fallback не задан — увеличить timeout:
       appspec repo resolve k51... --timeout-secs 120
3. Проверить, что `ipfs` daemon жив:
       ipfs id
4. Если daemon в порядке, но resolve медленный — `ipfs config Routing.Type dht`
   и перезапуск daemon.


## `repo ls --root ipns://...` → `ipfs not found in PATH`

**Симптом.** `appspec repo ls --repo ipns://k51...` падает с `ipns:// source
requires ipfs binary in PATH`.

**Причина.** `ipns://` и `ipfs://` read-команды требуют `ipfs` binary
(резолв + cat идут через локальный IPFS daemon). Без него индекс недоступен.

**Решение.**

~~~sh
# Установить IPFS или указать мок
APPSPEC_IPFS=/path/to/ipfs appspec repo ls --repo ipns://k51...
~~~

## `pull --from ipns://` → `no ipfs_cid for X:Y in IPNS index`

**Симптом.** `pull --from ipns://...` падает на `no ipfs_cid for name:ver`.

**Причина.** Пакеты добавлены в репо до 0.4.4 (без `ipfs_cid`) — обычно это
старые `appspec 0.3.x` репозитории.

**Решение.**

~~~sh
appspec repo pin            # добавит ipfs_cid, запинит блобы, reindex
appspec repo sign           # переподписать индекс
appspec repo publish --ipns # опубликовать
~~~

`repo reindex --backfill-cid` в одиночку не помогает: он пишет `ipfs_cid` в
`meta.json`, но **не пинит** блоб, поэтому `ipfs cat` всё равно timeout.

## `repo show --root ipns://` → `repo show not supported in v1`

**Причина.** Индекс в IPFS — это один файл, а не директория. `meta.json` и
`spec.appspec` per-package не адресуемы через `ipns://` / `ipfs://`.

**Решение.**

~~~sh
appspec repo pull <name>:<ver> --from ipns://k51... --root ~/.local/share/appspec/repo
appspec repo show <name>:<ver> --root ~/.local/share/appspec/repo
~~~

Или `ls` / `search` для просмотра индекса без скачивания:
~~~sh
appspec repo ls     --repo ipns://k51...
appspec repo search foo --repo ipns://k51...
~~~

## `repo publish --ipns` → warnings про `ipfs_cid` или `not pinned`

**Симптом.** `publish --ipns` печатает `warning: N version(s) without
ipfs_cid` или `warning: M version(s) with ipfs_cid but not pinned`.

**Причина.** В индексе есть версии без `ipfs_cid` (старые) или с `ipfs_cid`,
но блоб не запинен на локальном узле. `pull --from ipns://` для таких пакетов
упадёт на timeout.

**Решение.**

~~~sh
appspec repo pin            # добавит CID и запинит все блобы
appspec repo sign
appspec repo publish --ipns # без warnings
~~~

## `appspec run foo` → `package foo not found`

**Причина.** Имя не найдено ни в локальном репо, ни в remote sources.

**Решение.**

```sh
# Проверить, что индекс читается
appspec repo ls --repo ipns://k51...

# Задать источник явно
appspec run foo --from ipns://k51...
appspec run foo --from http://127.0.0.1:8844
```

## `appspec run foo:2.2.0` → `2.2.0 not in index`

**Причина.** Версия отсутствует в индексе. Список версий:

```sh
appspec repo ls foo --repo <source>
```

## `appspec run foo` → `ipfs_cat <cid> timeout`

**Причина.** В индексе есть `ipfs_cid`, но блоб не запинен. Publisher
не выполнил `repo pin` или не запускал `publish --ipns` (0.5.0 auto-pin).

**Решение (на стороне publisher).**

```sh
appspec repo publish --ipns    # 0.5.0 auto-pins all blobs
appspec repo sign              # если index был подписан
appspec repo publish --ipns    # опубликовать подписанный
```

## `appspec run foo` → `signature required: index.json is not signed`

**Причина.** `--trusted-key` задан, но индекс не подписан. Fail-closed.

**Решение.**

```sh
# на стороне publisher:
appspec repo sign
appspec repo publish --ipns

# или на клиенте (осознанно):
appspec run foo --from ipns://k51... --allow-unsigned
```
