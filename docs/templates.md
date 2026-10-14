# Шаблоны

## Порядок поиска

1. APPSPEC_TEMPLATE_PATH (colon-separated)
2. build.template_dir из спека
3. workspace/templates/ — только под cargo test и APPSPEC_DEV=1
4. exe_dir/templates/ — рядом с установленным бинарём
5. spec_dir/templates/ и spec_dir/
6. XDG_CONFIG_HOME/appspec/templates/
7. XDG_DATA_DIRS/appspec/templates/
8. Вкомпилированный include_str! — fallback

Для конфигов — тот же порядок, плюс /etc/appspec/ после XDG_CONFIG_HOME.

## Варианты имён

Для Dockerfile с template = deb шаблон ищется как:

- deb.j2
- Dockerfile.deb.j2

Для launcher-шаблона с runtime.template = default:

- default.j2
- run.default.sh.j2
- run.NAME.sh.j2

где NAME — имя пакета из package.name.

## Встроенные шаблоны Dockerfile

| Имя | Стек | Назначение |
|-----|------|------------|
| deb | Debian + magicpak + apt-satisfy | Скачивание .deb |
| tar | Debian + magicpak | Скачивание tar / tgz |
| apt | Debian + magicpak | Только apt-пакеты |
| generic | Debian + magicpak | Пустой каркас |
| rpm | RPM + magicpak | ROSA/ALT/Fedora |
| alpine | Alpine + apk --root | Языковые тулчейны (Go/Rust/Python) |
| alpine-slim | Alpine + magicpak | CLI-утилиты, минимум |
| full | Debian, без magicpak | Electron/GUI с sandbox |
| static | Alpine, без magicpak | Статический бинарник и busybox и startscript |
| scratch | Alpine, без magicpak | Статический бинарник без всего |

## Как выбрать

Языковой тулчейн (Go, Rust, Python) — шаблон alpine. apk --root поставит
всё, magicpak бесполезен для больших stdlib.

Статический бинарник (uv musl, minikube) — шаблон scratch для чистого CLI,
static если нужен apptainer shell для отладки или instance start.

Динамический ELF (nasm, git, angie, jq) — шаблон alpine-slim. magicpak
подберёт нужные .so.

GUI с dlopen-плагинами (strawberry) — шаблон deb. magicpak --dynamic под
Xvfb.

Electron/GUI, виснущий на ptrace (vscodium) — шаблон full. Единственный
путь, который гарантированно работает.

## Требование к static и scratch

static и scratch требуют статический бинарник. Проверить:

~~~
file $(which uv)
~~~

Ожидается: ELF 64-bit LSB pie executable, static-pie linked.

Если бинарник динамический (musl-linked, но с .so-зависимостями) —
static и scratch тоже сработают, но только если все .so уже в базовом
слое. Обычно это не так, поэтому используй alpine-slim.

## Пользовательские шаблоны

Положи .j2 рядом со спеком:

~~~
project/
  myapp.appspec
  templates/
    Dockerfile.myapp.j2
~~~

В спеке:

~~~
[build]
template     = "myapp"
template_dir = "templates"
~~~

Глобально:

~~~
mkdir -p ~/.config/appspec/templates
cp my-custom.j2 ~/.config/appspec/templates/Dockerfile.foo.j2
~~~

Тогда template = foo подхватится в любом проекте.

## Контекст шаблона

Все переменные передаются через context! minijinja.

| Переменная | Описание |
|------------|----------|
| pkg | Секция package целиком |
| build | Секция build целиком |
| runtime | Секция runtime целиком |
| source_url | Раскрытый URL из build.source.url |
| binary | build.collect.binary или default |
| cmd | build.scratch.cmd или список с одним бинарём |
| workdir | build.scratch.workdir или слеш |
| magicpak_version | Версия magicpak |
| busybox_url | URL busybox |
| collect_timeout | Таймаут magicpak в секундах |

## Фильтры

| Фильтр | Описание |
|--------|----------|
| sh_quote | Shell-экранирование одинарными кавычками |
| json_array | JSON-массив строк для CMD |
| docker_env | Экранирование ENV K=V: пробелы через обратный слеш |

Примеры:

~~~
RUN echo {{ "hello world" | sh_quote }}
CMD {{ cmd | json_array }}
ENV GIT_SSH_COMMAND={{ "ssh -o X=Y" | docker_env }}
~~~

Что получится:

~~~
RUN echo 'hello world'
CMD ["/usr/bin/foo", "--bar"]
ENV GIT_SSH_COMMAND=ssh\ -o\ X=Y
~~~

## Отладка

Посмотреть, какой шаблон выбран:

~~~
APPSPEC_DEV=1 appspec gen foo.appspec
~~~

В stderr будет виден origin шаблона: либо путь к файлу, либо метка
builtin.

Проверить, что пользовательский шаблон подхватился, а не встроенный:

~~~
APPSPEC_DEV=1 appspec gen foo.appspec 2>&1 | grep -i template
~~~
