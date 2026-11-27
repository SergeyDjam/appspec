# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.6.17] - 2026-11-27

### Added

- **`appspec uninstall <package>`** removes a package installed by name:
  - reads `$XDG_DATA_HOME/appspec/installed/<name>.json` (global:
    `/var/lib/appspec/installed`);
  - bundle: removes the installed launcher;
  - SIF: removes all files from the install manifest (SIF, setup, launcher,
    desktop/icons) and the state dir;
  - falls back to `<bin-dir>/<name>` and
    `$XDG_DATA_HOME/appspec/<name>/manifest.json` for installs made before
    the registry existed.
  Local `.appspec` paths still use the old path-based uninstall.

## [0.6.16] - 2026-11-27

### Changed

- **`appspec run <package>` prefers installed launchers.** New behaviour:
  1. installed package (`~/.local/bin/<name>` or the SIF install manifest)
     → run it directly;
  2. otherwise local index → newest local version;
  3. otherwise/`--refresh` → remote sources.
  `--refresh` bypasses installed launchers and re-resolves from repositories.
  Exact refs (`foo:1.0-1`) and explicit `--from`/`--profile`/`--root` also
  bypass the installed shortcut.
- `appspec install <package>` writes
  `$XDG_DATA_HOME/appspec/installed/<name>.json` with kind, version and
  launcher path (global: `/var/lib/appspec/installed`).

## [0.6.15] - 2026-11-27

### Added

- **`appspec install <package>` from the repository** (no local `.appspec`
  required):
  - resolves the newest version across local cache and configured remote
    sources (`--from`/`--profile`/`$APPSPEC_REPO`) and pulls it if needed;
  - **SIF packages** go through the normal install path: SIF, `setup.sh`,
    launcher, optional desktop/icon (spec is recovered from SIF labels when
    `spec.appspec` is absent);
  - **bundle packages** build their launcher and install it to
    `$XDG_BIN_HOME` (or `~/.local/bin`; `/usr/local/bin` for `--global`).
  - New flags: `--from`, `--profile`, `--root`, `--trusted-key`,
    `--allow-unsigned`. `--custom` requires a local spec.

`appspec uninstall` still accepts a spec path, not a package name.

## [0.6.14] - 2026-11-27

### Changed

- **Single repository lock.** `repo serve` push handlers now take the same
  `<root>/.lock` flock as `repo add`, `repo pin`, `repo sign` and
  `publish-index`. The in-process `write_lock` mutex remains only to
  serialize server threads. This closes the documented race between HTTP
  push and a local `repo add` on the same root.

## [0.6.13] - 2026-11-26

### Fixed

- **VSIX extensions fetched by CID/URL are unzipped even when the cache
  file has no extension** (`<cid>.bin`). `prepare_as_archive` now treats
  `parse_vsix` as "this is a zip" instead of relying on the filename, so
  `tar: Это не похоже на tar-архив` no longer breaks bundle builds.
- Manifest extensions now store `kind` (`vsix`/`file`/`archive`), and
  `run` materializes it as `type = ...`; older manifests fall back to
  inferring the kind from the original `file`/`url`.

## [0.6.12] - 2026-11-26

### Fixed

- **Base entry/runtime recovery when `spec.appspec` is missing.** Packages
  pulled from IPFS have only the SIF blob; `resolve_repo_source` now falls
  back to `apptainer inspect --labels` and decodes
  `org.appspec.spec.content.base64`. This restores `[runtime].setup` →
  `[bundle.base].entry` (and inherited runtime binds/home), fixing
  `не удалось определить entry для base` for IPNS-pulled bundles.

## [0.6.11] - 2026-11-26

### Fixed

- **`appspec run <package>` chooses the newest version across local and
  remote indexes.** Previously a cached local index won unconditionally:
  `run vscodium-dev` kept using `0.1.0-1` even when IPNS advertised
  `0.1.1-1`, so it never pulled the fixed bundle (inlined wrapper, new
  manifest). Now all configured remote sources are queried and the maximum
  `sort_key` wins. Exact refs still resolve to the same key.

## [0.6.10] - 2026-11-26

### Fixed

- **Macros in bundle specs are expanded** (`%{_bindir}`, `%{_sbindir}`,
  `%{_libdir}`, `%{name}`, `%{version}`, custom `[macros]`). Previously
  `bundle build`/`check` saw literal `%{_bindir}/git` and failed
  `binary должен быть абсолютным`.
  `load_bundle` now expands macros before deserializing; bundle publishing
  stores the expanded spec. Builtins `name`/`version` come from
  `[bundle].name`/`.version`; other path builtins use the same defaults as
  specs (`/usr/bin`, `/usr/sbin`, `/usr/lib64`, …).

## [0.6.9] - 2026-11-26

### Fixed

- **IPFS pull keeps packages in the index.** When `spec.appspec` is not
  available from IPFS, `repo pull` no longer deletes `meta.json` along with
  it. The SIF blob is enough for `bundle build`/`run`, and dropping meta
  removed the package from the freshly rebuilt `index.json` (causing
  `... not in repo index` errors for bundles pulled from IPNS).
- **`run <bundle>` rewrites extensions from the manifest.** Published
  bundles may reference a local `.vsix` that was converted to a CID; the
  materialized spec now replaces `file`/`cid`/`url` in `[[extensions]]`
  with the manifest's actual source instead of looking for a missing local
  file.
- **Bundle publish inlines UTF-8 `[[components.files]].from` files** into
  `content` in the stored spec. Self-contained specs no longer need
  adjacent local files such as `libvirtd-wrapper.sh`. Binary files still
  require `content`/URL/CID.

## [0.6.8] - 2026-11-23

### Fixed

- **Auto-published dependency SIFs without labels:** if an existing
  `<name>.sif` has no `org.appspec.*` labels (old/foreign/empty build),
  `repo add bundle.appspec` now rebuilds it with
  `appspec build --force <spec>` and retries the add. Inspect errors include
  the SIF path, so failures identify the exact file.

## [0.6.7] - 2026-11-23

### Added

- **Bundle dependencies are auto-published:** for local `spec = "./x.appspec"`
  references missing from the repo, `repo add bundle.appspec` first
  builds/locates the SIF and adds it, then reindexes and publishes the
  bundle. Existing packages with their blob present are left untouched.
  `--no-deps` disables auto-publishing; `--no-build` still forbids building
  missing SIFs. `--force`/`--pin-ipfs` apply to dependencies too.

## [0.6.6] - 2026-11-23

### Added

- **`appspec repo add package.appspec`** now accepts an app spec:
  1. reads the spec and resolves its SIF (`[runtime].sif_file` or
     `<name>.sif`);
  2. rebuilds it if missing or older than the spec;
  3. adds the resulting SIF exactly like `repo add package.sif`.
  `--no-build` disables rebuilding (missing SIF is an error, stale SIF is
  used with a warning). `--arch`, `--release-suffix`, `--force`,
  `--pin-ipfs` apply to the resulting SIF.
- Mixed arguments work: SIFs, `.appspec` app specs and `[bundle]` specs
  can be passed to one `repo add` (add specs first, then bundles).
- `repo add` classifies inputs by TOML: `[package]` → app spec,
  `[bundle]` → bundle, both → error.


## [0.6.5] - 2026-11-22

### Added

- **OCI export/import for bundle packages:**
  - `repo export-oci` pushes `kind = "bundle"` as an artifact with type
    `application/vnd.appspec.bundle.v1+json`; the blob is an
    `OciBundlePayload` containing the original `spec.appspec` and raw
    `manifest.json`, so their sha256 values stay verifiable;
    annotations include `org.appspec.kind=bundle`, `bundle_sha256`,
    `manifest_sha256`, `arch`, `epoch`.
  - `repo pull --from oci://...` restores bundle spec + manifest, writes
    blobs/meta and recursively pulls base/components through the same OCI
    index. `--spec-only` works for OCI bundles.
- `export-oci --dry-run` prints the bundle push command.
- **Bundle version from TOML:** `repo add` uses `[bundle] version`
  (and optional `release`/`epoch`) when `--version`/`--release`/`--epoch`
  are not passed.
- **Local bundle specs are publishable:** `spec = "./vscodium.appspec"`
  is resolved to the matching repo package by package name + version
  (`vscodium:0.1.0` → canonical `vscodium:0.1.0-N`). If the package is not
  in the repo, the error names it and suggests `appspec repo add`.
- **Local extension files are published as CIDs:** `[[extensions]].file`
  is pinned with `ipfs add --pin` and stored in the manifest as `cid`.
  Without `ipfs`, the error suggests using `cid`/`url`.

### Notes

- Aggregate OCI payload is a single JSON blob (not a multi-layer
  artifact), matching the existing single-blob pull implementation.

## [0.6.4] - 2026-11-22

### Added

- **`repo pin` supports bundle packages:** both the bundle spec blob
  (`bundle_sha256`) and the resolved `manifest.json` blob
  (`manifest_sha256`) are added/pinned to IPFS. The command updates
  `meta.ipfs_cid` and `meta.manifest_ipfs_cid` and reindexes as usual.
  `--dry-run`, `--dht`, batch `pin ls` and timeout handling apply.

### Notes

- OCI export/pull of bundles is still not implemented (export skips
  bundles with a warning, pull gives an explicit error).

## [0.6.3] - 2026-11-22

### Added

- **`repo pull` supports bundle packages** (`kind = "bundle"`) from:
  - local path — copies `spec.appspec`, `manifest.json`, bundle/manifest
    blobs and recursively pulls base/components by canonical repo refs;
  - HTTP — same, fetching `/packages/...` and `/blobs/sha256/...`;
  - IPFS/IPNS — fetches spec and manifest by `ipfs_cid` /
    `manifest_ipfs_cid` and recursively pulls dependencies.
- **`appspec run <bundle-name>`**:
  - resolves the bundle from the local/remote index and pulls it if needed;
  - rewrites base/component sources using the manifest's canonical
    `name:version-release` refs;
  - builds the launcher with the local repo (`bundle build --no-lock`) and
    runs it; `--dry-run` stops after building the launcher.
- Bundle pull also works with `--spec-only` (spec + manifest, no blobs/deps).

### Fixed

- **`repo add <bundle>` idempotency:** `manifest.json` is deterministic
  (no `built_at`/`generator`). Previously the timestamp changed the manifest
  hash, so a second add in a different second failed with a false
  "different sha256" conflict. Provenance remains in `meta.json`/`index.json`.

### Notes

- `repo pin` for bundles (bundle + manifest blobs) and OCI export of
  bundles are still not implemented; `pin` reports bundle entries as
  skipped and `oci export`/`pull` give explicit errors.

## [0.6.2] - 2026-11-21

### Added

- **`repo add <bundle.appspec>`** publishes a bundle as a repository
  package (`kind = "bundle"`). `repo add-bundle` is a visible alias.
  Bundle version is passed with `--version` (`--release` default `1`,
  `--epoch` default `0`).
- Bundle publishing writes:
  - `spec.appspec` — original bundle TOML;
  - `manifest.json` — `BundleManifestV1` with canonical
    `name:version-release` refs and SIF/spec hashes;
  - `meta.json` + index entry with `bundle_sha256`, `manifest_sha256`,
    `manifest_ipfs_cid` and `kind = "bundle"`;
  - blobs for the bundle spec and manifest (so `gc`/IPFS see them).
- Bundle sources must be repo refs (`foo`, `foo:1.0`, `foo:1.0-1`) or
  `cid`; local `spec`/`sif`/`manifest` paths and local extension files
  are rejected with a hint.
- `repo check` verifies bundle/manifest blobs and hashes;
  `repo ls`/`show`/`search` display `kind` and the correct artifact hash;
  `repo gc` keeps bundle + manifest blobs.

### Notes

- `repo pull` of a bundle, `repo pin` of bundles and OCI export of bundles
  currently fail with a clear "not supported yet" message; these are the
  next step (C).

## [0.6.1] - 2026-11-21

### Added

- **Repository schema groundwork for bundles** (A.2 of bundle-as-artifact):
  - `MetaV1.kind` / `IndexVersion.kind` with `sif` (default) and `bundle`;
    old `meta.json`/`index.json` entries without `kind` still parse as `sif`.
  - `bundle_sha256`/`bundle_size`, `manifest_sha256`/`manifest_size` and
    `manifest_ipfs_cid` fields for bundle packages.
  - `BundleManifestV1` (`src/bundle/manifest.rs`): resolved base/components/
    extensions with canonical `name:version-release` refs and hashes.
  - `PackageKind`, `MetaV1::is_bundle()`, `IndexVersion::is_bundle()`,
    `artifact_sha256()` helpers.

`repo add`/`pull`/`index` currently write `kind = "sif"`; publishing and
running bundles are the next steps (B/C).

## [0.6.0] - 2026-11-20

### Added

- **`spec = "..."` is universal in bundle sources** (base and components).
  Resolution is three-tier:
  - **explicit path** (`./foo.appspec`, `/abs/foo.appspec`,
    `sub/foo.appspec`, `~/foo.appspec`): local file only; error if missing.
  - **ambiguous** (`foo.appspec` — no path separator, ends with
    `.appspec`): try local `./foo.appspec` first; if missing, fall back to
    `foo:latest` from the repository.
  - **repo ref** (`foo`, `foo:latest`, `foo:1.0`, `foo:1.0-1`): resolve
    through `index.json` with the same rules as `appspec run <package>`
    (`latest`, max release for version-only, exact key).

### Deprecated

- **`repo = "name[:ver]"`** in `SourceSpec`: alias for
  `spec = "name[:ver]"`. Normalized at `load_bundle` with a warning.
  Removed in 0.7.0.

### Changed

- `resolve_repo_source` / `check_repo_source` accept `latest` and
  version-only refs (`foo:1.0` → max release) via
  `run_package::pick_version`.
- **`bundle.lock.toml` stores canonical repo refs** (`name:version-release`),
  so `foo`, `foo:latest` and `foo:1.0` are locked and drift-checked
  correctly.
- `lock::generate` detects repo sources from `spec` (not only the
  deprecated `repo` field).

### Migration

- Old: `repo = "git:2.47.2-1"` → New: `spec = "git:2.47.2-1"`.
- Local (unchanged): `spec = "./git.appspec"`.
- New: `spec = "git.appspec"` — local if present, otherwise `git:latest`.

## [0.5.5] - 2026-11-19

### Added

- **`[[components]].bins`** — дополнительные бинари компонента. Codegen
  генерирует для них bridge'и в `/opt/appspec/bin` внутри base-instance
  и создаёт symlink'и в `target`
  (`/usr/bin/NAME` → `/usr/local/bin/NAME`, либо `{ name, path, target }`).
  `bins` требует `mode = "chroot"` или `"both"`.

- **Вложенные bridge'и**: каждый bridge перед chroot монтирует
  `/opt/appspec/bin` и mount-пути остальных sandbox-компонентов, а также
  добавляет `/opt/appspec/bin` в PATH. Цепочки вида
  `tofu -> qemu-img -> libvirtd` теперь работают.

- **`bridge = "exec"`** — запуск бинаря компонента из его rootfs без
  chroot, в namespace base. Нужен `tofu`: его `local-exec` должен видеть
  base-tools (`ansible-playbook`, `qemu-img`, `ssh`) и текущий проект,
  а не пустой chroot opentofu-sandbox. Компонент при этом остаётся
  отдельным атомарным sandbox'ом.

### Fixed

- **`vscodium.appspec`: `wget` и `curl` добавлены в `[build.apt].extra`.**
  В `template = "full"` список `tools` не гарантирует наличие этих
  утилит в rootfs, из-за чего в base-instance не было `wget`/`curl`.

- **Bridge для `bins`/`binary` ищет `/usr/bin/NAME` и `/bin/NAME`.** Alpine
  кладёт `bash` в `/bin/bash`, а спецификация `bins = ["bash"]` по
  умолчанию даёт `/usr/bin/bash` — bridge падал с
  `bridge[ansible]: /usr/bin/bash отсутствует`. Теперь путь
  переразрешается в парный каталог, если в sandbox есть только он.
  Ошибка стала показывать реально проверенный путь.

- **VSCodium снова видит подготовленные расширения.** Если в
  `--extensions-dir` оставался пустой `extensions.json` (`[]`) от
  прошлого запуска, VS Code/VSCodium доверял манифесту и не сканировал
  каталоги — GUI показывал «no installed extensions». Launcher теперь
  удаляет `extensions.json` перед стартом редактора, чтобы он
  пересканировал каталог и пересобрал манифест.

- **`APPSPEC_DISABLE_KVM` пробрасывается в base-instance**, а bridge'и
  при `APPSPEC_DISABLE_KVM=1` не монтируют `/dev/kvm` в satellite-chroot.
  Раньше qemu внутри virt-daemon chroot всё равно получал нерабочий
  `/dev/kvm` и падал с `failed to initialize kvm: Inappropriate ioctl
  for device`. Проверка `[ -e /dev/kvm ]` в lesson'ах должна
  дополнительно учитывать `APPSPEC_DISABLE_KVM`.

- **`appspec run` останавливает старые instance'ы этого bundle**
  (`<name>-*`) перед запуском нового. Иначе уже запущенный codium с тем же
  `--user-data-dir` перехватывал новый запуск, и терминалы оставались в
  старом instance со старыми `/opt/utils`.

- **Bridge-скрипты пишутся внутрь base-instance в `/opt/appspec/bin`**
  (снимок на запуск), а не в общий cache. Это убирает ситуацию, когда
  старый instance видит новые bridge'и, но старые `/opt/utils`.

- **`/dev/kvm` больше не превращается в пустой файл.** Bridge для
  device-файлов биндит родительский каталог (`/dev`), а не создаёт
  regular-file заглушку. `/dev/kvm` попадает в base только если на хосте
  это char/block device. Добавлен `APPSPEC_DISABLE_KVM=1` для TCG.

- **Каталог расширений внутри `/config/home` создаётся до старта
  instance.** Иначе bind `$BUNDLE_DIR/extensions:/config/home/.vscodium/extensions`
  мог быть скрыт home-bind'ом, и VSCodium не видел расширения. Launcher
  также печатает список смонтированных расширений.

- **bundle пересобирает SIF, если spec новее SIF.** Раньше
  `resolve_source` собирал только отсутствующий SIF, поэтому изменения в
  `virt-daemon.appspec` (например добавление `cloud-image-utils`) не
  попадали в старый `virt-daemon.sif`, и `bins`-bridge падал с
  `exec: /usr/bin/cloud-localds: not found`. Теперь при
  `spec.mtime > sif.mtime` вызывается `appspec build --force`.

- **bundle home больше не лежит в cache.** `[runtime]` базового спека
  наследуется полностью: `vars`, `state_dirs`, `binds`, `optional_binds`,
  `create_dirs`, `env_passthrough`, `xauthority_auto`, `pulse_auto`.
  Для `vscodium-bundle` home теперь `~/vscodium_config/home`, как у
  одиночного `vscodium.appspec`. Fallback для base'ов без bind на
  `/config/home` — `APPSPEC_BUNDLE_HOME`/XDG data.

- **`appspec clean` больше не удаляет persistent state.** Удаляется только
  cache. Для удаления state — `appspec clean --purge BUNDLE`.

### Changed

- `APPSPEC_BUNDLE_HOME` теперь действительно означает state, а не cache.
  Cache переопределяется через `APPSPEC_BUNDLE_CACHE`.

## [0.5.4] - 2026-11-16

### Added

- **`bundle build --locked`**: strict lock mode. Requires `bundle.lock.toml`;
  any drift in `sif_sha256` (base or components) → hard error (exit != 0).
  Conflicts with `--no-lock`.

- **`[[base]]` in `bundle.lock.toml`**: if base source is `repo:`,
  its `sif_sha256`/`spec_sha256`/`base_digest` are now locked. Previously
  only components were locked; base could drift silently.

- **`pull --to PATH`**: alias for `pull --root PATH` (symmetry with
  `push --to URL`).

### Fixed

- **`bundle {build,check,lock}` with `source = repo`**: `resolve_repo_source`
  read `packages/<name>/<ver>/spec.appspec` hard-coded. Now reads
  `meta.spec_filename` (fallback `spec.appspec`) — supports repositories
  that store specs under different names.

- **`open_repo_root_for` fake stub**: previously constructed
  `RepoRoot::with_default_config("/nonexistent-appspec-repo")` when
  no `repo:` sources. Now returns `Option<RepoRoot>` — explicit
  absence, no fake path that could be accidentally read.

### Changed

- **`bundle build`** (without `--locked`): behavior unchanged — drift
  in lock emits warning and proceeds with fresh index (§9.2).
  `--locked` is opt-in strictness.


### Part B: repo + icons polish

- **`publish --ipns --sign`**; auto-sign when `--pin-all` rebuilt the
  index and `[signing].key` is configured.
- **`publish --ipns`** info summary after `--pin-all` (fresh index).
- **`repo check`** verifies `icon_sha256` (I2 for icons).
- **`appspec install`** tries SIF icon labels even when `[install].icon`
  is absent (repo-pulled packages).
- **`resolve_icon`** explicit hint when `apptainer` missing / SIF unreadable.
- **`pull --to`** alias truly implemented.
- test: `nonexistent_repo_root_yields_directory_not_found`.

## [0.5.3] - 2026-11-15

### Fixed

- **`repo pin --dht` fetches from network for blobs that are actually on
  disk.** `pin_one` for `ipfs_cid = Some` never checked the blob file.
  After `repo add` without `--pin-ipfs` (0.4.8+), `ipfs_cid` is computed
  via `add_file_only_hash` (`--only-hash`, no store). Blob file exists in
  `blobs/sha256/<sha>`; block is not in the IPFS blockstore. `pin_one`
  went `is_local_block → false → SKIP` (or DHT timeout with `--dht`).

  Now: for `ipfs_cid = Some`, blob file exists → `ipfs add --pin <file>`
  (fast local, verifies CID matches `meta.ipfs_cid`). Only if file is
  missing → `is_local_block` / DHT.

- **`repo pin` was slow on large repos** — N × `is_pinned` (5s each).
  Now: single batched `ipfs pin ls --type=recursive` → HashSet; per-package
  check is O(1).

- **`RepoRoot::open` misleading error**: "missing index.json" for a
  non-existent directory (wrong cwd or `--root` typo). Now: "repository
  directory not found" + hints (resolved cwd path, server config `[repo].root`).

### Added

- **`ipfs::list_pinned()`** — one batched `ipfs pin ls --type=recursive`,
  30s timeout, returns `HashSet<String>`.

### Changed

- **`repo pin` summary** — `added` counter now also counts `add --pin`
  for `ipfs_cid = Some` + blob file (previously only `ipfs_cid = None`).


## [0.5.2] - 2026-11-15

### Added

- **Parallel pinning** — `repo pin --jobs N` (default 8). Multiple
  worker threads pop packages from a shared queue and run `ipfs add --pin`
  / `ipfs pin add` concurrently. Wall time drops ~N× for local blobs.

- **Local-only default** — `repo pin` now **skips DHT** by default.
  If `ipfs_cid` is set but block is not local (`ipfs block stat` fails),
  the package is skipped with `SKIP not local (use --dht to fetch)`.
  This avoids hours of DHT walk for blobs the publisher doesn't have.

- **`--dht` opt-in** — `repo pin --dht` attempts DHT fetch for non-local
  blobs. Still bounded (parallel × `--timeout` per blob).

- **Progress with ETA** — `[i/N] name:ver — action (Xs) ETA~Ys` on stderr.

### Fixed

- **`publish --ipns` preflight warning was misleading** — `warning: N
  version(s) with ipfs_cid but not pinned` ran **before** reindex,
  counted blobs about to be pinned, and was **slow** (N × 5s
  `is_pinned`). Removed.

- **`repo pin` summary** — new field `skipped (not local)`.

### Changed

- `repo pin` default = parallel (8), local only, skip DHT.
  For previous behaviour (sequential, DHT for all `ipfs_cid=Some`):
  `repo pin --dht --jobs 1 --timeout 600`.


## [0.5.1] - 2026-11-14

### Fixed

- **`repo publish --ipns` hung on DHT**: auto-pin in 0.5.0 called
  `ipfs pin add` for every blob; for blobs not present locally that
  walks the DHT (hours). Auto-pin is now **opt-in** via `--pin-all`.
  Default `publish --ipns` only publishes the index (seconds).

- **`ipfs::is_pinned` / `pin` / `add_file` had no timeout** — could
  hang forever on `ipfs pin ls` or `ipfs pin add` against a stuck daemon
  or DHT walk. Now: `is_pinned` = 5s, `pin` = 600s, `add_file` = 600s.
  Timeout uses `timeout(1)` binary if available.

- **`repo pin` was silent** — no progress, looked frozen on large repos.
  Now prints `[N/M] name:ver — action` on stderr (elapsed, CID, local/DHT).
  `--quiet` suppresses. `--timeout N` (default 600s) per blob.

- **`publish_index` preflight warning** ("N versions without ipfs_cid"
  etc.) was misleading (ran before reindex, saw stale data) and slow
  (14 × 5s `is_pinned`). Removed. `appspec repo pin --dry-run` shows
  the same picture, faster and without side effects.

### Added

- **`appspec repo pin --dry-run`** now shows `would add+pin (local)`
  vs `would pin (DHT, timeout 600s)` per package, so it's clear which
  blobs are cheap to pin and which require network.

- **`appspec repo publish --pin-all`** — opt-in replacement for the
  0.5.0 auto-pin. Equivalent to `appspec repo pin && appspec repo publish --ipns`.

- **`ipfs::is_local_block`** — `ipfs block stat <cid>` with 5s timeout.
  Used by `repo pin` to pick "local (fast)" vs "DHT (slow)" hint.

### Changed

- **`PublishIndexArgs --no-pin`** is deprecated (no-op). Old scripts
  still work; new code should use `--pin-all` for opt-in pinning or
  omit it for default "publish only".

### Migration

- Old: `publish --ipns` = publish + auto-pin (slow). New: publish only.
  Use `publish --ipns --pin-all` for previous behavior (with progress
  and timeouts).


## [0.5.0] - 2026-11-14

### Added

- **`appspec run <package>[:<version>[-<release>]]`**: run a package
  directly from a repository, using `index.json` as the source of truth.
  Version resolution:
  - `foo` or `foo:latest` — max semver, max release within;
  - `foo:2.2.0` — exact version, **max release** among `2.2.0-N`;
  - `foo:2.2.0-1` — exact version+release.
  Source fallback: local cache → `--from` → `--profile` → `$APPSPEC_REPO`
  → default profile. Flags: `--from`, `--profile`, `--root`,
  `--trusted-key`, `--allow-unsigned`, `--dry-run`.
  Files still work: `appspec run ./foo.appspec`.

- **`repo publish --ipns` auto-pin**: before publishing the index, pins
  every blob in `packages/` (adding `ipfs_cid` if missing via
  `ipfs add --pin`, then `ipfs pin add` for existing CID). After auto-pin
  `index.json` is rebuilt; if it was signed, the signature becomes
  invalid and a warning is printed (rerun `appspec repo sign`).

### Changed

- **`publish --ipns`** default behaviour includes auto-pin. Use
  `--no-pin` for the previous behaviour (index only, no blob pinning).

- **`appspec run`** now accepts a package name (no path separator,
  no `.appspec` suffix) in addition to file paths. File paths must be
  given as `./foo.appspec`, `/abs/foo.appspec`, or `foo.appspec`.

### Internal

- `templates::render_launcher_abs(spec, sif_path)` — renders `run.sh`
  with an absolute SIF path (used by `run <package>`).
- `repo::run_package::{parse_package_ref, pick_version}` — pure helpers.
- `load_spec_from_text` — extracted from `load_spec` for reuse.

## [0.4.9] - 2026-11-14

### Added

- **`appspec repo pin`**: pin all SIF blobs in IPFS and fill missing
  `ipfs_cid` in `meta.json`. For each package:
  - if `ipfs_cid` is set: `ipfs pin add <cid>` (skips already-pinned);
  - if `ipfs_cid` is `None` and blob exists: `ipfs add --pin` (raw CID
    for ≤ 256 KB, dag-pb for larger), writes CID to `meta.json`;
  - then `reindex` to propagate `ipfs_cid` into `index.json`.

  This is the missing piece between `repo add` (local) and a
  fully-published IPFS repository: without pinning, `pull --from ipns://`
  fails because blobs are unreachable from the network.

  Flags: `--dry-run`, `--root` / `--repo`.

- **IPNS/IPFS read commands**: `repo ls`, `repo search`, `repo verify`
  now accept `--root ipns://<name>` and `--root ipfs://<cid>`.
  They resolve the IPNS name (or read the CID directly), `ipfs cat` the
  index, and operate on it in memory. No local repo required.

- **`--repo` alias for `--root`** in `repo {ls, search, show, verify, pin}`.

### Changed

- **`repo show` from `ipns://` / `ipfs://`** now fails with a clear
  message pointing to `pull` / `ls` / `search`. The index is a single
  file, not a directory tree, so per-package `meta.json` / `spec.appspec`
  are not addressable over IPFS.

- **`repo publish --ipns`** now preflights the index and prints warnings
  when:
  - N versions have no `ipfs_cid` → `pull --from ipns://` will fail for
    them (fix: `appspec repo pin`);
  - M versions have `ipfs_cid` but blob is not pinned → not reachable via
    IPFS (fix: `appspec repo pin`);
  - `ipfs` binary missing → cannot verify pinning state.

  This makes the publish → pin → pull workflow explicit.

- **`repo verify`** now uses the same index reader as `ls`/`search`,
  unifying the code path. HTTP, IPFS, IPNS are handled identically; the
  `--pubkey` requirement is the same for all remote origins.

### Internal

- `ipfs::is_pinned(cid)` — checks `ipfs pin ls --type=recursive <cid>`.
- `commands::read_index_text_from(origin)` — single entry point for
  reading `index.json` from any origin (HTTP / IPFS / IPNS / path).


## [0.4.8] - 2026-11-14

### Fixed

- **`appspec repo publish --ipns`** and any `ipfs add` call failed with
  `chunker parameters may not exceed the maximum chunk size of 2096896`.
  Root cause: `ipfs::add_file` passed `--chunker=size-1073741824` (1 GB),
  which IPFS rejects (hard limit ~2 MiB). Removed; default chunker
  (256 KB) is used.

- **`repo add --pin-ipfs` on SIF > 256 KB** was broken: `ipfs add
  --raw-leaves` for large files returns a dag-pb root, whose CID is not
  derivable from sha256. The 0.4.4 invariant "local raw CID == ipfs CID"
  only held for ≤ 256 KB. New strategy:
  - files ≤ 256 KB: local raw CID (matches `ipfs add`);
  - files > 256 KB: `ipfs add --offline --only-hash --cid-version=1
    --raw-leaves` (works without daemon, correct dag-pb CID);
  - no `ipfs` binary + file > 256 KB: `ipfs_cid = null`.

- **`repo reindex --backfill-cid`** no longer rewrites existing dag-pb
  CIDs to raw (raw CID points to a non-existent block). Only fills `null`
  entries, using `ipfs add --only-hash` if `ipfs` is available.

- **`repo check`** I2 (`ipfs_cid ↔ sif_sha256`): dag-pb CIDs now emit a
  warning instead of an error (valid for `ipfs cat`, just not derivable
  from sha256). Only raw CIDs (`bafkrei…`) are strictly checked.

### Added

- **`ipfs::add_file_only_hash`** — computes the CID IPFS would assign
  without storing any blocks (`ipfs add --offline --only-hash`). No
  daemon required.

### Changed

- **`ipfs::add_file`** no longer bails on non-raw CID; caller decides
  whether raw is required based on file size.


## [0.4.7] - 2026-11-13

### Added

- **Icons in SIF (E-icon)**: `[install].icon` is embedded into the SIF
  as base64 labels at build time:
  `org.appspec.icon.filename`, `.mimetype`, `.sha256`, `.size`,
  `.content.base64`. Allows `appspec install` from a repository
  (where the icon file itself is not present on disk).

- **Icon metadata in repo**: `repo add` extracts the icon from SIF
  labels, saves it as `packages/<name>/<ver>/icon.<ext>`, and records
  `icon_sha256`, `icon_mimetype`, `icon_size`, `icon_filename` in
  `meta.json` and `index.json`. `repo pull` (HTTP / path) fetches
  the icon alongside `spec.appspec`.

- **Multi-size icons on install**: `appspec install` resizes PNG/JPEG
  icons to 16/22/24/32/48/64/128/256 px under
  `<icon_dir>/<N>x<N>/apps/<name>.png`. SVG is placed into
  `<icon_dir>/scalable/apps/<name>.svg`. Icon source resolution:
  SIF labels first, then `spec_dir/[install].icon` (backward compat).

- **New dependency**: `image = { version = "0.25", default-features = false,
  features = ["png", "jpeg"] }`.

### Changed

- `[install].icon` in `.appspec` is now treated as a **build-time
  source only**. After `appspec install`, the icon lives in
  `~/.local/share/icons/hicolor/` (or `/usr/share/icons/hicolor/`
  for global scope) and is embedded in the SIF.


## [0.4.6] - 2026-11-12

### Fixed

- **`appspec install`**: `templates/launcher.sh.j2` did not emit
  `[runtime.vars]` before `bind_lines` / `create_dirs`. Installed
  launchers for specs that reference `$CONFIG_DIR`, `$MUSIC_DIR`, etc.
  in `binds` / `create_dirs` failed at runtime with
  `unbound variable` under `set -eu` (regression vs. `appspec run`).

  Fix: `launcher.sh.j2` now renders `runtime.vars` (same loop as
  `run.sh.j2`), then overrides `CONFIG_DIR="$STATE"` so the
  install-scope state directory wins over the spec's `$HOME/...`
  default. Regression tests added to `src/templates.rs`.


## [0.4.5] - 2026-11-13

### Added

- **`bundle {build,check,ls,lock} --repo PATH`**: explicit repository root.
  Default: `$APPSPEC_REPO` → `$XDG_DATA_HOME/appspec/repo`.
- **`bundle {build,check,lock} --trusted-key PATH`** (repeatable): ed25519
  public keys for `index.json` verification. Merged with
  `~/.config/appspec/trusted-keys.toml`.
- **`bundle {build,check,lock} --allow-unsigned`**: explicit opt-out for
  unsigned indices (warning emitted).

### Changed

- **`bundle build` / `bundle check` / `bundle lock`** now verify the
  `index.json` signature when repo sources are used. Fail-closed:
  configured `--trusted-key` + invalid signature → exit != 0.
  Empty trusted keys → warning (behaviour matches `repo pull`).
- **`bundle lock`** uses `--repo`; verifies the index signature before
  writing the lock, so a locked bundle cannot be created from a
  compromised index.
- **`bundle build` with `bundle.lock.toml`** verifies the signature in
  `check_drift` as well. Signature failure in lock check is a warning
  (build proceeds to `resolve_all`, which is fail-closed).

### Not verified in this release

- `scripts/smoke-e2e.sh --full`: requires `apptainer` + `buildah|podman|docker`.
  Not run in the 0.4.5 build environment.

### Migration

- Old CLI invocations without `--repo` keep working: default resolution is
  unchanged (`$APPSPEC_REPO` → XDG).
- Old `repos.toml` and `repo-server.toml` parse without changes.


## [0.4.4] - 2026-11-12

### Added

- **IPFS**: SIF sha256 -> CIDv1 computed locally, no `ipfs` daemon required.
  `repo add` writes `ipfs_cid` to `meta.json` unconditionally
  (was: only with `--pin-ipfs`).
- **IPNS**: mutable repository name. `[ipfs].ipns_key` and `[ipfs].ipns_name`
  in `repo-server.toml`; `repo publish --ipns` publishes index via
  `ipfs name publish`; `repo resolve <ipns_name>` resolves name -> CID.
- **IPNS pull**: `source.ipns` in `repos.toml`; `pull --from ipns://NAME`;
  `pull --profile` with `{ ipns, url }` falls back from IPNS to HTTP.
- **`client init --source-ipns k51...`**: create IPNS-only or IPNS+URL profile.
- **`repo reindex --backfill-cid`**: fill or rewrite `ipfs_cid` for packages
  added before 0.4.4. Rewrites non-raw (legacy dag-pb) CIDs to raw.
- **`sha256_from_cid`**: decode CIDv1/raw back to sha256-hex. Used in
  `repo check` for I2 (`ipfs_cid <-> sif_sha256`).

### Changed

- **`PullOpts.from: Option<String>` -> `PullOpts.sources: Vec<String>`**.
  Public API of `repo::pull`. Breaking for callers; internal to appspec.
- **`ipfs::add_file`** passes `--raw-leaves --cid-version=1
  --chunker=size-1073741824` so returned CID matches local
  `cid_v1_from_sha256`. Files >1 GB are rejected with a clear error
  (was: silently producing dag-pb CID).
- **`repo publish-index`** is now an alias of `repo publish`
  (behavior unchanged; `--ipns` is opt-in via config).
- **`repo check`** verifies `ipfs_cid` consistency (I2). Legacy non-raw CIDs
  emit a warning (skip); run `repo reindex --backfill-cid` to fix.

### Deprecated

- **`source.ipfs` in `repos.toml`**: use `source.ipns`. Reading emits a
  warning. Removed in v2.
- **`repo init --token NAME`**: use `repo token gen NAME --config PATH`.
  Kept for backward compat, warning emitted.

### Fixed

- `ipfs::add_file` for SIF >256 KB: was producing dag-pb CID unrelated to
  sha256, causing `--pin-ipfs` to fail with "CID mismatch" and `pull --via ipfs`
  to be broken. Now uses raw-leaves with 1 GB chunker, matches local CID.
- `scripts/smoke-e2e.sh`: `repo init --token` -> `repo server init` +
  `repo token gen`; IPNS steps 10/11 (publish + resolve); mock `ipfs` is
  always exported.

### Tests

- 367 unit + integration tests, 0 failed. New:
  - `tests/ipfs_cid.rs` — I1 against real `ipfs add --only-hash`
    (skipped if `ipfs` not in PATH).
  - `tests/ipns.rs` — `publish --ipns`, `resolve`, `pull_from_ipns`,
    fallback `ipns -> url`, mock `ipfs name {publish,resolve}`.
  - `tests/repo_config.rs` — `client init --source-ipns`
    (with/without URL, path+ipns conflict).
  - `tests/repo_e1.rs::add_and_show_roundtrip` — asserts `ipfs_cid`
    is set and consistent with `sif_sha256`.

### Not verified in this release

- `scripts/smoke-e2e.sh --full`: requires `apptainer` + `buildah|podman|docker`.
  Not run in the 0.4.4 build environment.

### Known issues

- SIF >1 GB cannot be pinned via `--pin-ipfs` (would need >1 GB chunker).
  Workaround: `--chunker=size-<larger>` in a fork, or split the SIF.
- Existing repositories built with 0.4.3 may contain legacy dag-pb
  `ipfs_cid` in `meta.json`. `repo check` will warn; `repo reindex
  --backfill-cid` rewrites them to raw.
