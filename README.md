# appspec

Сборка Apptainer SIF из TOML-спека. Dockerfile и launcher генерируются
через minijinja-шаблоны.

## Команды

```sh
cargo build --release

appspec gen    examples/strawberry.toml   # Dockerfile.strawberry + run.strawberry.sh
appspec build  examples/strawberry.toml   # + docker/buildah build + apptainer build
appspec run    examples/strawberry.toml   # запустить instance
appspec all    examples/strawberry.toml   # build + run
appspec clean  examples/strawberry.toml   # удалить артефакты
