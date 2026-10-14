# Движки сборки

appspec умеет собирать image тремя движками: buildah, podman, docker.
Каждый проходит две стадии — build и save — после чего Apptainer
превращает результат в SIF.

## Автовыбор

Без переменной APPSPEC_ENGINE движки пробуются в порядке:

1. buildah
2. podman
3. docker

Первый найденный в PATH используется. Если ни одного нет — ошибка.

Явное указание:

~~~
APPSPEC_ENGINE=buildah appspec build foo.appspec
APPSPEC_ENGINE=podman  appspec build foo.appspec
APPSPEC_ENGINE=docker  appspec build foo.appspec
~~~

## buildah

Команды:

~~~
buildah bud --no-cache --format oci -t IMG -f DF DIR
buildah push IMG oci-archive:TAR:tag
apptainer build SIF oci-archive://TAR
~~~

Флаг --format oci обязателен: buildah по умолчанию может собрать в
docker-формате, который Apptainer тоже умеет читать, но OCI-формат
даёт короткий путь без промежуточных конвертаций.

### Rootless

buildah работает без демона. В rootless-режиме требует:

- Файлы /etc/subuid и /etc/subgid с диапазоном UID для текущего
  пользователя.
- kernel.unprivileged_userns_clone = 1 или эквивалент в ядре.
- newuidmap и newgidmap с setuid-битом.

Проверка:

~~~
grep "^$(whoami):" /etc/subuid /etc/subgid
sysctl kernel.unprivileged_userns_clone
ls -la /usr/bin/newuidmap /usr/bin/newgidmap
~~~

Ожидается строка вида user:100000:65536, значение 1, и бит s в
правах.

### cgroup v1

На хостах с cgroup v1 rootless buildah падает с сообщением:

~~~
open /sys/fs/cgroup/devices/buildah-buildah…: No such file or directory
~~~

Причина: --isolation=oci (значение по умолчанию) пытается создать
device-cgroup, а /sys/fs/cgroup/devices принадлежит root.

Решение — переключить изоляцию на chroot:

~~~
BUILDAH_ISOLATION=chroot APPSPEC_ENGINE=buildah appspec build foo.appspec
~~~

В режиме chroot buildah не создаёт cgroup для шагов RUN. Для сборки
образов это эквивалентно.

Можно поставить значение глобально:

~~~
echo 'export BUILDAH_ISOLATION=chroot' >> ~/.bashrc
~~~

### Registry-конфиг

Если buildah не резолвит короткое имя образа:

~~~
short-name "debian:trixie" did not resolve to an alias and no
unqualified-search registries are defined
~~~

Создайте ~/.config/containers/registries.conf:

~~~
unqualified-search-registries = ["docker.io", "quay.io", "ghcr.io"]

[[registry]]
prefix = "docker.io"
location = "docker.io"
~~~

Не смешивайте v1 и v2 синтаксис. Файл либо старый со секциями
[registries.search], либо новый с unqualified-search-registries.
Микс приводит к ошибке mixing sysregistry v1/v2 is not supported.

## podman

Команды:

~~~
podman build --network host --no-cache -t IMG -f DF DIR
podman save IMG -o TAR
apptainer build SIF docker-archive://TAR
~~~

Работает и в rootless через тот же userns, что и buildah. Registry
конфиг читается из того же места: ~/.config/containers/registries.conf
или /etc/containers/registries.conf.

## docker

Классический путь:

~~~
docker build --network host --no-cache -t IMG -f DF DIR
docker save IMG -o TAR
apptainer build SIF docker-archive://TAR
~~~

В rootless-режиме требует dockerd-rootless-setuptool и systemd-user.
Для большинства сценариев выгоднее buildah.

## Про --startscript

После сборки image appspec пытается использовать флаг apptainer build
--startscript FILE. Если Apptainer поддерживает — используется
напрямую. Если нет, применяется fallback:

1. Собрать SIF как sandbox через apptainer build --sandbox.
2. Переписать /.singularity.d/startscript в этом sandbox.
3. Собрать SIF из sandbox.

Определяется функцией apptainer_supports_startscript: запускается
apptainer build --help и ищется подстрока --startscript.

Флаг важен для instance start: без него Apptainer подставляет свой
дефолтный startscript, который не всегда умеет exec переданной
команды.

## Что происходит по шагам

Один вызов appspec build выполняет:

1. cmd_gen — рендеринг Dockerfile и run-скрипта через minijinja.
2. build_image — Dockerfile собирается движком в локальный image.
3. export_image — image сохраняется в tar.
4. apptainer build — SIF создаётся из tar.
5. Временный tar удаляется.

Если поставить --force, шаги 2-5 выполняются всегда. Без него, если
SIF уже существует, шаги пропускаются.
