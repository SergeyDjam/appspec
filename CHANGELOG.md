# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
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
