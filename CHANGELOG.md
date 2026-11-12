# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
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
