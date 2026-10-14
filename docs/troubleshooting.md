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
