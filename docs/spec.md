# Спека .appspec

Полный справочник по формату. Спека — TOML. Тип определяется наличием
секции: package — одиночное приложение, bundle — составное окружение.
В одном файле не может быть обеих секций.

## package

| Ключ | Тип | Обязательный | Описание |
|------|-----|--------------|----------|
| name | string | да | Имя приложения. Используется как имя instance, имя SIF |
| version | string | да | Версия. Доступна в URL через {version} |
| description | string | нет | Попадает в .desktop как Comment= |
| bin | string | нет | Имя бинарника. По умолчанию = name |

## build

| Ключ | Тип | Default | Описание |
|------|-----|---------|----------|
| base | string | debian:trixie | Базовый образ. Короткие имена квалифицируются |
| arch | string | amd64 | Архитектура. {arch} в URL |
| tools | [string] | ca-certificates, wget, dpkg-dev, binutils, patchelf | Пакеты в builder |
| template | string | по source.type | Имя шаблона |
| template_file | path | — | Путь к .j2 явно |
| template_dir | path | — | Каталог с пользовательскими шаблонами |
| distro | string | — | Для rpm-шаблона: rosa, alt, fedora |
| version | string | — | Версия дистрибутива, для rpm-шаблона |
| pkg_manager | string | — | dnf / urpmi / apt-get / apk |

## build.source

| Ключ | Тип | Описание |
|------|-----|----------|
| type | string | deb, rpm, tar, tgz, binary, пустая строка |
| url | string | URL источника. Поддерживает {name}, {version}, {arch} |
| extract_to | path | Куда распаковывать tar. Default: /opt |
| strip_components | u32 | --strip-components |
| dest | path | Целевой путь для binary. Default: /usr/local/bin/tool |
| exclude_deps | [string] | Имена, исключаемые из Depends .deb |

Поведение по типам:

- deb — скачиваем, ставим Depends через apt-get satisfy, распаковываем.
- rpm — скачиваем, rpm2cpio и cpio.
- tar / tgz / targz — распаковываем в extract_to.
- binary — скачиваем в dest, chmod +x.
- пустая строка — источника нет, всё ставится через build.apt.extra.

## build.apt

| Ключ | Тип | Описание |
|------|-----|----------|
| extra | [string] | Дополнительные пакеты. Раскрываются макросами |

Для Debian — имена из apt. Для Alpine — из apk. Для RPM — из dnf или urpmi.

## build.collect

| Ключ | Тип | Default | Описание |
|------|-----|---------|----------|
| binary | path | /usr/bin/NAME | Целевой бинарник |
| includes | [string] | [] | Глобы для принудительного включения в бандл |
| target_args | [string] | [] | Аргументы бинарю при сборе (например -v) |
| timeout_secs | u64 | 30 | Таймаут на magicpak --dynamic |
| magicpak_version | string | v1.4.0 | Версия magicpak |
| busybox_url | string | 1.35.0-musl | URL busybox |

Правила includes:

- Каталоги со /* на конце раскрываются в список файлов.
- Каталоги без /* обходятся рекурсивно через find -type f.
- Директории отсеиваются, чтобы не уронить magicpak.

## build.scratch

| Ключ | Тип | Default | Описание |
|------|-----|---------|----------|
| workdir | path | / | WORKDIR в финальном образе |
| cmd | [string] | [binary] | CMD в финальном образе |
| env | map | {} | ENV в финальном образе. Значения эскейпятся |

## runtime

| Ключ | Тип | Default | Описание |
|------|-----|---------|----------|
| sif_file | path | NAME.sif | Имя SIF |
| image_name | string | NAME:latest | Имя image для движка |
| template | string | по mode | Имя launcher-шаблона |
| template_file, template_dir | — | — | Явный путь / каталог |
| mode | string | instance | instance или cli |
| command | path | — | Команда для CLI-режима |
| workdir | path | /config/home | --pwd для apptainer exec |
| persist | bool | false | Если true — instance остаётся после выхода |
| state_dirs | [string] | [] | Подкаталоги в CONFIG_DIR, создаются на хосте |
| binds | [string] | [] | Bind-mount. С префиксом ! — форсированный |
| optional_binds | [string] | [] | Всегда с проверкой существования |
| env_passthrough | [string] | [DISPLAY] | Имена переменных для проброса |
| xauthority_auto | bool | true | Пробрасывать XAUTHORITY |
| pulse_auto | bool | true | Пробрасывать PulseAudio и PipeWire |
| create_dirs | [string] | [] | Каталоги на хосте до старта |
| setup | string | — | Shell-скрипт внутри instance перед запуском |
| entry | string | — | Финальная команда |
| vars | map | {} | Переменные для binds, create_dirs, setup |

## runtime.vars

Произвольные переменные, используются в подстановках:

~~~
[runtime.vars]
CONFIG_DIR = "$HOME/strawberry_config"
MUSIC_DIR  = "${MUSIC_DIR:-$HOME/Music}"
~~~

В binds и create_dirs — shell-раскрытие. $VAR и ${VAR:-default} работают
как в shell.

## install

| Ключ | Тип | Описание |
|------|-----|----------|
| scope | string | user / global / custom |
| launcher_name | string | Имя команды-лаунчера |
| desktop | bool | Генерировать .desktop |
| icon | path | PNG или SVG |
| categories | [string] | Из freedesktop spec |
| mime_types | [string] | MIME-типы |
| comment | string | Comment= в .desktop |
| wm_class | string | StartupWMClass= |
| sif_dir | path | Каталог для SIF при scope = custom |
| extra_files | [obj] | Массив с полями from и to |
| user, global | объекты | Переопределения путей |
| ipfs | объект | Настройки IPFS |

## macros

Пользовательские макросы. Раскрываются в фигурных скобках со знаком
процента. Синтаксис и встроенные макросы — docs/macros.md.

~~~
[macros]
install_prefix = "/usr/local"
~~~

## Порядок применения

1. Парсинг TOML в промежуточное дерево.
2. Раскрытие макросов во всех строках, кроме секции macros.
3. Десериализация в структуры Spec.
4. Квалификация base: alpine:3.21 становится docker.io/library/alpine:3.21.
5. Генерация Dockerfile через выбранный шаблон.
6. Генерация launcher-скрипта.
