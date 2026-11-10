[English](README.md) | [Русский](README_ru.md)

# appspec

Apptainer SIF builder from a declarative TOML spec. Dockerfile and launcher
are rendered from minijinja templates. Bundles provide composable
environments — base SIF + components + extensions. `appspec repo` provides
a local/remote SIF repository with publishing, search, signing, IPFS and
OCI export.

## Features

- **One spec — every artifact.** Dockerfile, run script, SIF, .desktop,
  launcher — all generated from `*.appspec`.
- **Ten build templates:** deb, tar, apt, generic, rpm, alpine,
  alpine-slim, full, static, scratch.
- **Three build engines:** buildah (rootless, OCI-native), podman, docker.
- **Composable bundles:** base SIF + components (chroot/instance/both) +
  extensions from file / URL / IPFS / Open VSX / **local repository**.
- **RPM-style macros:** name, version, bin, optional, default.
- **Dependencies and reproducibility:** `[dependencies]` with
  requires/provides/conflicts/replaces, `[build].base_digest` to pin the
  base image.
- **Local SIF repository (`appspec repo`):** content-addressed blobs,
  RPM-style metadata, search, sign/verify (ed25519), HTTP server,
  push/pull, IPFS pin/pull, OCI export via `oras`.
- **Custom templates:** lookup in 6 places with a predictable priority.
- **Shell completions:** bash, zsh, fish, elvish, powershell.

## Installation

```sh
git clone <repo> appspec
cd appspec
cargo build --release
install -m755 target/release/appspec ~/.local/bin/
```

Runtime dependencies:

| Component | When needed |
|-----------|-------------|
| apptainer | always (SIF build, label inspection) |
| buildah / podman / docker | at least one (for build) |
| ipfs | optional (`repo --pin-ipfs`, `pull --via ipfs`) |
| oras | optional (`repo export-oci`, `pull --from oci://`) |
| curl or wget | bundle extensions by URL |
| unzip | unpack `.vsix` / `.zip` / `.oxt` in bundles |
| tar | unpack `.tar.gz` / `.tgz` in bundles |
| zip | only for `cargo test` (generating test `.vsix`) |

### Bash completion

```sh
# per-user
mkdir -p ~/.local/share/bash-completion/completions
appspec completions bash > ~/.local/share/bash-completion/completions/appspec

# or system-wide
sudo appspec completions bash > /etc/bash_completion.d/appspec
```

The same works for `zsh`, `fish`, `elvish`, `powershell`:

```sh
appspec completions zsh  > ~/.zsh/completions/_appspec
appspec completions fish > ~/.config/fish/completions/appspec.fish
```

## Quick start

Single application:

```sh
appspec gen    examples/strawberry.appspec
appspec build  examples/strawberry.appspec
appspec run    examples/strawberry.appspec
appspec stop   examples/strawberry.appspec
appspec all    examples/strawberry.appspec
appspec clean  examples/strawberry.appspec
```

Bundle:

```sh
appspec bundle check examples/vscodium-bundle.appspec
appspec bundle lock  examples/vscodium-bundle.appspec
appspec bundle build examples/vscodium-bundle.appspec
appspec run          examples/vscodium-bundle.appspec
```

Bundle with extensions (`.vsix` for VSCodium, `.oxt` for LibreOffice):

```sh
# VSCodium: extensions are unpacked into directories, no install step
appspec bundle build examples/vscodium-bundle.appspec

# LibreOffice: .oxt is copied into mount_dir and installed via unopkg
# (put a local .oxt into examples/extensions/ — see the README there)
appspec build -f examples/libreoffice.appspec
appspec bundle build examples/libreoffice-bundle.appspec
appspec run          examples/libreoffice-bundle.appspec
```

SIF repository:

```sh
appspec repo init
appspec repo add examples/strawberry.sif
appspec repo ls
appspec repo show strawberry:1.2.31-1
appspec repo search player

# sign and verify
appspec repo sign
appspec repo verify --pubkey ~/.config/appspec/signing.key.pub

# HTTP server + publish
appspec repo serve --listen 127.0.0.1:8080 &
APPSPEC_REPO_TOKEN=... appspec repo push --to http://127.0.0.1:8080 pkg.sif
appspec repo pull pkg:1.0.0-1 --from http://127.0.0.1:8080 --trusted-key pub.txt

# IPFS
appspec repo add --pin-ipfs pkg.sif
appspec repo publish-index --pin
appspec repo pull pkg:1.0.0-1 --via ipfs

# OCI
appspec repo export-oci --registry ghcr.io/user appspec-index
appspec repo pull pkg:1.0.0-1 --from oci://ghcr.io/user/appspec-index:latest
```

See [`docs/repo.md`](docs/repo.md) for details.

## Commands

| Command | Description |
|---------|-------------|
| `gen SPEC` | Generate Dockerfile and run script |
| `build SPEC [-f]` | gen + image build + SIF |
| `run SPEC` | Run launcher (SIF or bundle) |
| `stop SPEC` | Stop instance left by `persist = true` |
| `all SPEC [-f]` | build + run |
| `clean SPEC` | Remove Dockerfile, launcher, SIF, tar, cache |
| `install SPEC` | Install SIF + launcher + .desktop |
| `uninstall SPEC` | Remove everything from the manifest |
| `ipfs-publish SPEC` | Publish SIF to IPFS |
| `bundle {check,build,ls,add,remove,lock}` | Composable environments |
| `repo {init,add,ls,show,reindex,check,gc,search,sign,verify,serve,push,pull,publish-index,export-oci}` | Local / remote SIF repository |
| `completions <SHELL>` | Generate bash/zsh/fish/elvish/powershell completion |

## Spec `*.appspec`

TOML. Main sections: `package`, `build`, `runtime`, `install`, `macros`,
`dependencies`. `build.source.type` determines the install method:
`deb`, `rpm`, `tar`, `binary`, or empty string (apt/apk packages only).

Minimal spec:

```toml
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
```

### `[dependencies]` (E7)

```toml
[dependencies]
requires  = ["gstreamer >= 1.22", "qt6-base"]
provides  = ["music-player"]
conflicts = ["strawberry-nightly"]
```

Rules:
- `requires` — `name [op ver]`, operators `=`, `>=`, `<=`, `>`, `<`, no commas;
- `provides` / `conflicts` / `replaces` — bare package names.

Labels `org.appspec.dependencies.*` are written to the SIF **only if non-empty**
(backwards compatible). `appspec repo add` reads them into `meta.json.requires`
/ `.provides` / `.conflicts`, and `repo search --requires P` filters by
`provides`.

### `[build].base_digest` (E7)

```toml
[build]
base        = "debian:trixie"
base_digest = "sha256:<64 lowercase hex>"
```

`appspec build` replaces `base` with `<repo>@<digest>` (tag is stripped).
Dockerfile gets `FROM debian@sha256:…`, which pins the base forever.

Full reference — in [`docs/spec.md`](docs/spec.md) (Russian).

## Templates

| Template | Base | SIF size | When to use |
|----------|------|----------|-------------|
| deb | Debian | 250 MB | .deb + magicpak |
| tar | Debian | 250 MB | tar + magicpak |
| apt | Debian | 250 MB | apt packages only |
| generic | Debian | 250 MB | empty skeleton |
| rpm | RPM-based | 250 MB | .rpm + magicpak |
| alpine | Alpine | 30-150 MB | apk --root (Go, Rust, Python) |
| alpine-slim | Alpine | 8-50 MB | apk + magicpak (dynamic ELF) |
| full | Debian | 400-800 MB | Electron, GUI with sandbox |
| static | Alpine | 30 MB | static binary + busybox |
| scratch | Alpine | 30 MB | static binary, CLI-only |

How to choose — [`docs/templates.md`](docs/templates.md) (Russian).

## Macros

Expanded in `load_spec` before TOML parsing. Work in fields and in
multiline `setup` and `entry`.

- `%{name}`, `%{version}`, `%{bin}` — from `package`.
- `%{_prefix}`, `%{_bindir}`, `%{_sbindir}`, `%{_libdir}` — distro-specific.
- `%{?optional}` — empty if not set.
- `%{name:default}` — default value.
- `%%` — literal percent.
- User macros — from `[macros]`.

See [`docs/macros.md`](docs/macros.md) (Russian).

## Bundles

A bundle is a composite environment: base SIF + components + extensions.

```toml
[bundle]
name = "vscodium-dev"
base = { spec = "./vscodium.appspec" }

[[components]]
name    = "git"
spec    = "./git.appspec"
provide = "git"

[[extensions]]
name = "ms-azuretools.vscode-docker"
```

Components can be `chroot`, `instance`, or `both`. Extensions can come
from file, cid, url, or online (Open VSX).

### Extension types

`[[extensions]].type` — `vsix` (default), `file`, `archive`, `auto`.
With `auto`, the type is detected from the URL/file suffix
(`.vsix` → vsix, `.oxt`/`.deb`/`.rpm`/`.sh`/`.bin`/`.exe` → file,
`.zip`/`.tar.gz`/`.tgz`/`.tar.xz`/`.tar.bz2`/`.tar` → archive;
online without suffix → vsix).

Behaviour per type:

| `type` | What `bundle build` does | What is mounted | `[bundle.extensions].install` |
|--------|---------------------------|------------------|-------------------------------|
| `vsix` | unpacks the `.vsix` into a directory `<publisher>.<name>-<version>/` under `$BUNDLE_DIR/extensions/` | `$BUNDLE_DIR/extensions/` → `mount_dir` | **not executed** (VSCodium picks up directories from `--extensions-dir` on its own) |
| `file` | copies the file as-is to `$BUNDLE_DIR/extensions/<basename>` | `$BUNDLE_DIR/extensions/` → `mount_dir` | **executed** via `yes yes \| …` (auto-accept EULA: `unopkg`, `.deb`-installers) |
| `archive` | unpacks `.zip`/`.tar.gz` into `<name>-archive/` | `$BUNDLE_DIR/extensions/` → `mount_dir` | **not executed** (user decides what to do with the unpacked directory) |

`[bundle.extensions].mount_dir` — where to mount `$BUNDLE_DIR/extensions`
inside the base instance. Default is `/config/home/.vscodium/extensions` —
the `--extensions-dir` target of VSCodium. For LibreOffice extensions,
override to `/opt/extensions` (see below).

`[bundle.extensions].install` — shell command, executed **only for
`type = "file"`**. Placeholders:

| Placeholder | Value |
|-------------|-------|
| `{file}` | path inside `mount_dir` (`<mount_dir>/<basename>`) |
| `{target}` | alias for `{file}` (backwards compat with old specs) |
| `{name}` | `ext.id` (for `type = "file"` — `ext.name`) |
| `{id}` | `ext.id` |
| `{version}` | version (for `type = "file"` — `"1"`) |

#### Example: VSCodium + `.vsix`

```toml
[bundle]
name = "vscodium-dev"
base = { spec = "./vscodium.appspec" }

[[extensions]]
name = "opentofu"
file = "./extensions/opentofu-0.6.3.vsix"
type = "vsix"

[[extensions]]
name = "ms-azuretools.vscode-docker"
```
- `mount_dir` is not set — default `/config/home/.vscodium/extensions` is used.
- `install` is not needed — `.vsix` are unpacked into directories, VSCodium sees them directly.

#### Example: LibreOffice + `.oxt`

```toml
[bundle.extensions]
mount_dir = "/opt/extensions"
install   = "unopkg add --shared {file}"

[[extensions]]
name = "grammalecte"
file = "./extensions/Grammalecte-fr-v2.1.3.oxt"
type = "file"
```
- `mount_dir = "/opt/extensions"` — dedicated path so it does not clash with VSCodium.
- `install` runs as `yes yes 2>/dev/null | unopkg add --shared /opt/extensions/Grammalecte-fr-v2.1.3.oxt` (EULA accepted automatically).

#### Extension sources

`[[extensions]]` contains exactly one of `file` | `cid` | `url`, or none
(online via Open VSX, only for `type = "vsix"`).

- `file = "./path.vsix"` — local (relative to `bundle.appspec`).
- `url = "https://…"` — downloaded to `$BUNDLE_DIR/extensions/<basename>` on `bundle build`. Direct URLs on many sites change per release; for reproducibility prefer `file =`.
- `cid = "bafy…"` — IPFS, `ipfs get` into `$BUNDLE_DIR/extensions/<cid>.bin`.

Full example with all types — `examples/libreoffice-bundle.appspec`.
Instructions for local `.oxt`/`.vsix` — `examples/extensions/README.md`.

### `source = repo` and `bundle.lock` (E3)

```toml
[bundle]
name = "vscodium-dev"
base = { repo = "vscodium:1.135.06055" }

[[components]]
name    = "git"
repo    = "git:2.47.2"
provide = "git"
```

`repo = "name:ver"` — the fifth source alongside `spec|sif|cid|manifest`.
Resolved via local `index.json` (`$APPSPEC_REPO`).

`appspec bundle lock` creates `bundle.lock.toml` with `sif_sha256` and
`spec_sha256` for each `repo` component. `bundle build` verifies
`sif_sha256`: on drift — warning, but goes with the fresh index.

Full description — [`docs/bundles.md`](docs/bundles.md) (Russian).

## SIF repository (`appspec repo`)

The local repository is built on metadata compiled into the SIF at
`appspec build` time (labels `org.appspec.*`). The SIF is the source of
truth; the repository is an index on top of it.

### Configuration

Phase 1: **two independent configs**, both outside the repository root.

* **Server** (`repo-server.toml`) — one repository: `[repo].root`,
  `[policy]`, `[signing]`, `[server]`, `[[server.tokens]]`, `[server.tls]`.
* **Client** (`repos.toml`) — named source profiles:
  `source = { path | url | ipfs }`, `token`, `trusted_keys`.

Full reference — [`docs/repo-config.md`](docs/repo-config.md).

```sh
# Server: data-only root + external config
appspec repo init /srv/repo --config ~/.config/appspec/repo-server.toml
appspec repo server init --root /srv/repo      # if root already exists
appspec repo migrate-config /srv/repo          # import legacy <root>/config.toml

# Client: source profile
appspec repo client init \
    --profile corp \
    --source-url http://repo.corp:8080 \
    --token-env APPSPEC_REPO_TOKEN \
    --default
appspec repo client ls
```

Repository root is **data-only** (`index.json`, `blobs/`, `packages/`,
`refs/`, `.lock`). `config.toml` inside root is no longer created; legacy is
supported with a warning and `migrate-config`.

### Quick cycle

```sh
appspec repo init /srv/repo --config ~/.config/appspec/repo-server.toml
appspec repo add pkg.sif
appspec repo check [--deep]
appspec repo gc [--dry-run] [--keep N]
appspec repo search demo --config ~/.config/appspec/repo-server.toml
```

### Tokens (server-side)

```sh
# Generate, plaintext shown once in stdout
appspec repo token gen ci

# Straight to file (for CI), octal mode like chmod
appspec repo token gen ci --write-file /run/secrets/ci --mode 0400

appspec repo token ls
appspec repo token check ci --file /run/secrets/ci    # exit 0/1
appspec repo token rm ci
```

`repo token gen` writes **only sha256** into server config. Plaintext is
never persisted.

### Publishing

```sh
# 1. Server
appspec repo serve --listen 127.0.0.1:8844
# (if ~/.config/appspec/repo-server.toml exists, --root/--config not needed)

# 2. Push (three ways to set destination)
APPSPEC_REPO_TOKEN=… appspec repo push pkg.sif --to http://127.0.0.1:8844
APPSPEC_REPO=http://127.0.0.1:8844 APPSPEC_REPO_TOKEN=… appspec repo push pkg.sif
APPSPEC_REPO_TOKEN=… appspec repo push pkg.sif    # if a default profile exists

# 3. Pull with signature verification
appspec repo pull pkg:1.0.0-1 --profile corp --trusted-key pub.txt
```

**`$APPSPEC_REPO_TOKEN` is the token only**, not the destination. If only
that is set, `push` returns `no destination`.

**`--profile NAME`** is a client-config profile, not a token name. Do not
confuse with `repo token gen NAME`.

**Fail-closed:** if `~/.config/appspec/trusted-keys.toml` has at least one
key, `pull --from URL` requires a valid `index.json` signature.

### Signatures (E5)

```sh
appspec repo sign                           # pubkey is written back to server config
appspec repo sign --config /etc/appspec/repo-server.toml
appspec repo verify --pubkey pub.txt
```

Scheme: ed25519 over JCS canonicalisation (RFC 8785) of `index.json`
without the `signature` field. Private key: `[signing].key` (server config)
→ `--key` → `$APPSPEC_SIGNING_KEY` → `~/.config/appspec/signing.key`.

### IPFS (E6) and OCI (E8)

Unchanged from v0.3.x:

```sh
appspec repo add --pin-ipfs pkg.sif
appspec repo publish-index --pin
appspec repo pull pkg:1.0.0-1 --via ipfs

appspec repo export-oci --registry ghcr.io/user appspec-index
appspec repo pull pkg:1.0.0-1 --from oci://ghcr.io/user/appspec-index:latest
```

### Discovery

* `repo-server.toml`: `--config`/`-c` → `$APPSPEC_REPO_SERVER_CONFIG` → XDG
  → `/etc` → legacy `<root>/config.toml` (warning).
* `repos.toml`: `--client-config` → `$APPSPEC_REPO_CLIENT_CONFIG` → XDG → `/etc`.

`repo serve` with no flags works if a server config is found: `[repo].root`
becomes the "current repository".
## Environment variables

| Variable | Meaning |
|---|---|
| `APPSPEC_ENGINE` | docker / podman / buildah |
| `BUILDAH_ISOLATION` | oci (default) / chroot |
| `APPSPEC_DEV` | 1 — look for templates in workspace/templates/ |
| `APPSPEC_TEMPLATE_PATH` | Extra template directories |
| `APPSPEC_CONFIG_PATH` | Extra config directories |
| `APPSPEC_BUNDLE_CACHE` | Bundle cache directory |
| `APPSPEC_BUNDLE_HOME` | Bundle state directory |
| `APPSPEC_REPO` | Path \| URL \| profile name. Local commands need a path; read/pull/push also accept URL or profile name |
| `APPSPEC_REPO_TOKEN` | Token only, for `repo push`. Not the destination. |
| `APPSPEC_REPO_SERVER_CONFIG` | Path to `repo-server.toml` (after `--config`) |
| `APPSPEC_REPO_CLIENT_CONFIG` | Path to `repos.toml` |
| `APPSPEC_SIGNING_KEY` | Private ed25519 key (after `[signing].key`) |
| `APPSPEC_APPTAINER` | `apptainer` binary (for mocks) |
| `APPSPEC_IPFS` | `ipfs` binary (for mocks) |
| `APPSPEC_ORAS` | `oras` binary (for mocks) |
| `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_DATA_DIRS` | Standard XDG |
## Tests

```sh
cargo test --release                 # unit + integration
cargo test --release --bin appspec   # only unit tests of the bin crate
./scripts/smoke-e2e.sh               # e2e (auto: full if possible, else mock)
./scripts/smoke-e2e.sh --full        # requires apptainer + build-engine
./scripts/smoke-e2e.sh --mock        # no external dependencies
```

Coverage:
- `tests/cli.rs` — CLI wrapper (help, gen, spec errors);
- `tests/repo_e1.rs` — init/add/ls/show/reindex/check/gc/search;
- `tests/e3.rs` — bundle `source=repo` + `bundle lock`;
- `tests/e4.rs` — HTTP server/client/push;
- `tests/e5.rs` — ed25519 + JCS, fail-closed pull;
- `tests/e6.rs` — IPFS (mock `ipfs`);
- `tests/e7.rs` — `[dependencies]` + `base_digest`;
- `tests/e8.rs` — OCI export (mock `oras`);
- `tests/e9.rs` — `pull --from oci://` (mock `oras`);
- `tests/e10.rs` — TLS for `repo serve`;
- `tests/bundle_extensions.rs` — bundle extensions: `{file}`/`{target}` substitution, `.vsix` unpacking into a directory (install not executed), `type = "file"` + `yes yes | …`, bind to `mount_dir`.

Unit tests inside `src/bundle/loader.rs`, `src/bundle/model.rs`,
`src/bundle/codegen.rs` (run with `cargo test --release --bin appspec`):

- `prepare_vsix_extracts_to_dir` — `.vsix` is unpacked into `<id>-<ver>/`, source is untouched;
- `prepare_vsix_idempotent_with_same_fingerprint` — repeated build with same fingerprint is a no-op;
- `prepare_vsix_refreshes_dir_when_fingerprint_differs` — directory is recreated when fingerprint changes;
- `prepare_file_skips_copy_when_src_equals_target` — for `type = "file"` via URL/CID, no copy-in-place (regression `copy → copy: ENOENT`);
- `rel_path_*` — basename for vsix/file/archive;
- `install_command_substitutes_*` — placeholders `{file}`/`{target}`/`{name}`/`{id}`/`{version}` in the install command.

## Documentation

- [`docs/spec.md`](docs/spec.md) — `*.appspec` reference (Russian).
- [`docs/templates.md`](docs/templates.md) — templates, choice, custom (Russian).
- [`docs/bundles.md`](docs/bundles.md) — composite environments (Russian).
- [`docs/macros.md`](docs/macros.md) — macros (Russian).
- [`docs/engines.md`](docs/engines.md) — buildah/podman/docker (Russian).
- [`docs/troubleshooting.md`](docs/troubleshooting.md) — common problems (Russian).
- [`docs/repo.md`](docs/repo.md) — local and remote SIF repository (Russian).

## License

MIT.
