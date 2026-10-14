# Макросы

Раскрываются на этапе load_spec, до парсинга TOML в структуры Spec.
Работают и в простых полях вида binary или build.apt.extra, и внутри
многострочных setup и entry.

Синтаксис в стиле RPM spec. Символ процента, затем фигурные скобки.

## Синтаксис

| Форма | Поведение |
|-------|-----------|
| %{name} | Значение name |
| %{?name} | Пусто, если name не задан |
| %{name:default} | default, если name не задан |
| %% | Буквальный символ процента |

Неизвестный макрос без вопросительного знака и без default остаётся как
есть — чтобы опечатку было видно при первом запуске gen.

## Встроенные макросы

Из секции package:

- name — name.
- version — version.
- bin — bin, если задан; иначе name.

Из секции build:

- base — базовый образ.
- arch — архитектура.

Дистрибутив-специфичные. Зависят от значения base.

| Макрос | Alpine | Debian и Ubuntu | RPM |
|--------|--------|-----------------|-----|
| %{_prefix} | /usr | /usr | /usr |
| %{_bindir} | /usr/bin | /usr/bin | /usr/bin |
| %{_sbindir} | /usr/sbin | /usr/sbin | /usr/sbin |
| %{_libdir} | /usr/lib | /usr/lib/x86_64-linux-gnu | /usr/lib64 |
| %{_libexecdir} | /usr/libexec | /usr/libexec | /usr/libexec |
| %{_datadir} | /usr/share | /usr/share | /usr/share |
| %{_sysconfdir} | /etc | /etc | /etc |
| %{_localstatedir} | /var | /var | /var |
| %{_rundir} | /run | /run | /run |
| %{_tmppath} | /tmp | /tmp | /tmp |

## Пользовательские макросы

Задаются в секции macros. Могут ссылаться друг на друга и на встроенные.
Раскрываются итеративно, до 16 проходов. Защита от бесконечных циклов
встроена.

~~~
[macros]
install_prefix = "%{_prefix}/local"
bin_path       = "%{install_prefix}/bin/%{bin}"
~~~

Если значение после очередного прохода не меняется, итерации
останавливаются. Если за 16 проходов стабилизация не наступила — ошибка.

## Порядок раскрытия

1. Вычисляются встроенные макросы из package и build.
2. Из секции macros собираются пользовательские.
3. Пользовательские накладываются поверх встроенных — при совпадении
   имени пользовательский выигрывает.
4. Значения самих макросов стабилизируются итеративно.
5. Раскрываются все строки в дереве, кроме самой секции macros.

## Пример

Исходный спек:

~~~
[package]
name    = "web"
version = "1.0.0"
bin     = "nginx"

[build]
base     = "alpine:3.21"
template = "alpine"

[build.apt]
extra = ["%{bin}", "ca-certificates"]

[build.collect]
binary = "%{_sbindir}/%{bin}"

[build.scratch]
workdir = "/srv"
cmd     = ["%{_sbindir}/%{bin}", "-g", "daemon off;"]
~~~

После раскрытия:

~~~
[build.apt]
extra = ["nginx", "ca-certificates"]

[build.collect]
binary = "/usr/sbin/nginx"

[build.scratch]
cmd = ["/usr/sbin/nginx", "-g", "daemon off;"]
~~~

## Отладка

Посмотреть, что получилось после раскрытия, можно через промежуточный
вывод. Самый простой способ — сгенерировать артефакты и посмотреть,
что подставилось:

~~~
appspec gen foo.appspec
grep -n "nginx" Dockerfile.foo
grep -n "%{" Dockerfile.foo
~~~

Второй grep должен быть пуст — если в Dockerfile остались
нераскрытые макросы, значит какой-то из них не был определён и
остался как есть. Ищите в спеке опечатку.
