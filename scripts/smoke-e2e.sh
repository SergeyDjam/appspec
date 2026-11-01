#!/bin/sh
# scripts/smoke-e2e.sh — end-to-end smoke test для appspec repo (E1–E8).
#
# Один preflight-pass → один отчёт → один прогон.
#
# Usage:
#   ./scripts/smoke-e2e.sh               # auto: full если возможно, иначе mock
#   ./scripts/smoke-e2e.sh --full        # требуются все зависимости, иначе fail
#   ./scripts/smoke-e2e.sh --mock        # всегда mock (не требует apptainer)
#   ./scripts/smoke-e2e.sh --no-build    # не запускать appspec build
#
# Env: APPSPEC, SPEC, SIF — пути (default: из examples/ и target/release).

set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
APPSPEC=${APPSPEC:-$ROOT/target/release/appspec}
SPEC=${SPEC:-$ROOT/examples/strawberry.appspec}
SIF=${SIF:-$ROOT/examples/strawberry.sif}

MODE=auto
DO_BUILD=1
for arg in "$@"; do
    case "$arg" in
        --full)     MODE=full ;;
        --mock)     MODE=mock ;;
        --no-build) DO_BUILD=0 ;;
        -h|--help)  sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *)          printf "unknown arg: %s\n" "$arg" >&2; exit 2 ;;
    esac
done

TMP=$(mktemp -d)
SERVE_PID=
cleanup() {
    [ -n "${SERVE_PID:-}" ] && kill -TERM "$SERVE_PID" 2>/dev/null || true
    rm -rf "$TMP"
}
trap cleanup EXIT INT TERM

ok()   { printf "  \033[1;32m✓\033[0m %-20s %s\n" "$1" "$2"; }
warn() { printf "  \033[1;33m○\033[0m %-20s %s\n" "$1" "$2"; }
miss() { printf "  \033[1;31m✗\033[0m %-20s %s\n" "$1" "$2"; }
fail() { printf "\033[1;31m✗\033[0m %s\n" "$*"; exit 1; }
step() { printf "\n\033[1;36m== %s\033[0m\n" "$*"; }
have() { command -v "$1" >/dev/null 2>&1; }

# ===========================================================================
# PREFLIGHT — один проход, полный отчёт
# ===========================================================================
step "preflight: dependency scan"

[ -x "$APPSPEC" ] || fail "appspec binary not found: $APPSPEC (run: cargo build --release)"

MISSING_CRITICAL=""

# --- baseline ---
if have sha256sum; then ok "sha256sum" "baseline integrity"
else miss "sha256sum" "baseline integrity (REQUIRED)"; MISSING_CRITICAL="$MISSING_CRITICAL sha256sum"; fi

# --- build engine ---
BUILD_ENGINE=""
for e in buildah podman docker; do
    if have "$e"; then BUILD_ENGINE=$e; break; fi
done
if [ -n "$BUILD_ENGINE" ]; then ok "$BUILD_ENGINE" "build engine (appspec build)"
else miss "buildah|podman|docker" "build engine (REQUIRED for appspec build)"; MISSING_CRITICAL="$MISSING_CRITICAL build-engine"; fi

# --- apptainer ---
HAVE_APPTAINER=0
if have apptainer; then HAVE_APPTAINER=1; ok "apptainer" "SIF build + inspect labels"
else miss "apptainer" "SIF build + inspect labels (mock fallback)"; MISSING_CRITICAL="$MISSING_CRITICAL apptainer"; fi
if [ "$BUILD_ENGINE" = "buildah" ]; then
    if buildah info --format '{{.Host.IDMappedMounts}}' >/dev/null 2>&1; then
        ok "buildah-userns" "user namespace OK"
    else
        warn "buildah-userns" "userns недоступен — build может упасть (см. BUILDAH_ISOLATION=chroot)"
    fi
fi

# --- optional (skipped cleanly, not fatal) ---
HAVE_CURL=0; have curl && HAVE_CURL=1
if [ "$HAVE_CURL" = "1" ]; then ok "curl" "http /healthz checks"
else warn "curl" "http /healthz checks (skipped)"; fi

HAVE_IPFS=0; have ipfs && HAVE_IPFS=1
if [ "$HAVE_IPFS" = "1" ]; then ok "ipfs" "IPFS pin/pull (covered by e6 tests)"
else warn "ipfs" "IPFS pin/pull (skipped; use e6 tests)"; fi

HAVE_ORAS=0; have oras && HAVE_ORAS=1
if [ "$HAVE_ORAS" = "1" ]; then ok "oras" "export-oci real push"
else warn "oras" "export-oci real push (skipped; --dry-run tested)"; fi

HAVE_PYTHON=0; have python3 && HAVE_PYTHON=1

# ===========================================================================
# MODE DECISION
# ===========================================================================
step "mode decision"

if [ "$MODE" = "full" ]; then
    if [ "$DO_BUILD" = "1" ] && { [ -z "$BUILD_ENGINE" ] || [ "$HAVE_APPTAINER" = "0" ]; }; then
        echo; printf "\033[1;31m--full requires (missing):\033[0m\n"
        [ "$HAVE_APPTAINER" = "0" ] && echo "  apptainer              https://apptainer.org/docs/admin/main/installation.html"
        [ -z "$BUILD_ENGINE" ]      && echo "  buildah|podman|docker  apt install buildah   (or podman / docker)"
        echo
        echo "Install all of the above, then: ./scripts/smoke-e2e.sh --full"
        echo "Or drop --full to auto-fallback to mock mode."
        exit 1
    fi
    if [ "$DO_BUILD" = "0" ]; then
        [ "$HAVE_APPTAINER" = "1" ] || fail "--full --no-build requires apptainer (for label inspection)"
        [ -f "$SIF" ] || fail "--full --no-build: SIF missing: $SIF"
    fi
    USE_MOCK=0
elif [ "$MODE" = "mock" ]; then
    USE_MOCK=1
else
    # auto
    if [ -z "$BUILD_ENGINE" ] || [ "$HAVE_APPTAINER" = "0" ]; then
        USE_MOCK=1
        printf "\n  \033[1;33mauto→mock\033[0m  missing: %s\n" "$MISSING_CRITICAL"
        printf "  install all of:  apptainer + buildah|podman|docker\n"
        printf "  to run the full pipeline.\n"
    else
        USE_MOCK=0
        if [ "$DO_BUILD" = "0" ] && [ ! -f "$SIF" ]; then
            fail "auto: apptainer present, but SIF missing and --no-build given"
        fi
    fi
fi

# ===========================================================================
# SETUP
# ===========================================================================
if [ "$USE_MOCK" = "1" ]; then
    step "setup: mock apptainer + synthetic SIF"
    MOCK_DIR="$TMP/mock"
    mkdir -p "$MOCK_DIR"
    head -c 4096 /dev/urandom > "$TMP/test.sif"
    SIF="$TMP/test.sif"

    SPEC_BODY="$TMP/spec-body.appspec"
    printf '[package]\nname = "smoke"\nversion = "0.0.1"\n' > "$SPEC_BODY"
    SPEC_SHA=$(sha256sum "$SPEC_BODY" | cut -d' ' -f1)
    SPEC_B64=$(base64 -w0 < "$SPEC_BODY" 2>/dev/null || base64 "$SPEC_BODY")

    cat > "$MOCK_DIR/labels.json" <<JSON
{
  "data": { "attributes": { "labels": {
    "org.appspec.package.name": "smoke",
    "org.appspec.package.version": "0.0.1",
    "org.appspec.package.release": "1",
    "org.appspec.package.epoch": "0",
    "org.appspec.package.arch": "amd64",
    "org.appspec.spec.content.base64": "$SPEC_B64",
    "org.appspec.spec.sha256": "$SPEC_SHA",
    "org.appspec.spec.filename": "smoke.appspec",
    "org.appspec.generator": "appspec smoke-mock"
  }}}
}
JSON
    if [ "$HAVE_PYTHON" = "1" ]; then
        python3 -c "import json;json.load(open('$MOCK_DIR/labels.json'))" \
            || fail "mock labels.json is not valid JSON"
    fi

    cat > "$MOCK_DIR/apptainer" <<'MOCK'
#!/bin/sh
if [ "$1" = inspect ] && [ "$2" = --json ] && [ "$3" = --labels ]; then
    cat "$MOCK_LABELS_JSON"; exit 0
fi
echo "mock apptainer: unexpected: $*" >&2; exit 1
MOCK
    chmod +x "$MOCK_DIR/apptainer"
    export APPSPEC_APPTAINER="$MOCK_DIR/apptainer"
    export MOCK_LABELS_JSON="$MOCK_DIR/labels.json"

    ok "mock apptainer" "$MOCK_DIR/apptainer"
    ok "synthetic SIF" "$SIF (4096 B random)"
    ok "labels" "$MOCK_DIR/labels.json"
else
    step "setup: full mode"
    ok "apptainer" "$(command -v apptainer)"
    ok "engine"    "$BUILD_ENGINE"
    if [ "$DO_BUILD" = "1" ]; then
        # ВАЖНО: pipe скрывает exit code. Ловим через $? до sed.
        set +e
        "$APPSPEC" build -f "$SPEC" > "$TMP/build.log" 2>&1
        rc=$?
        set -e
        sed 's/^/    /' "$TMP/build.log"
        if [ "$rc" != "0" ]; then
            echo
            printf "\033[1;31mappspec build failed (exit %s)\033[0m\n" "$rc"
            echo "  подсказки:"
            echo "    - userns: BUILDAH_ISOLATION=chroot $APPSPEC build -f $SPEC"
            echo "    - /etc/subuid:  sudo usermod --add-subuids 100000-165535 --add-subgids 100000-165535 \$USER"
            echo "    - rootless:     sudo sysctl kernel.unprivileged_userns_clone=1"
            exit 1
        fi
        ok "SIF built" "$SIF"
    else
        [ -f "$SIF" ] || fail "SIF missing: $SIF"
        ok "SIF exists" "$SIF"
    fi
fi

# ===========================================================================
# TESTS
# ===========================================================================

step "1/9  repo: init → add → check"
REPO="$TMP/repo"
"$APPSPEC" repo init "$REPO" --name "smoke/e2e" >/dev/null
"$APPSPEC" repo add --root "$REPO" "$SIF" 2>&1 | tee "$TMP/add.log" | sed 's/^/    /'
grep -q "added:" "$TMP/add.log" || fail "repo add"

PKG=$("$APPSPEC" repo ls --root "$REPO" | awk 'NR==1{print $1}')
[ -n "$PKG" ] || fail "repo ls returned empty"
KEY=$("$APPSPEC" repo ls "$PKG" --root "$REPO" | awk 'NR==1{print $1}')
[ -n "$KEY" ] || fail "repo ls $PKG empty"
"$APPSPEC" repo show "$PKG:$KEY" --root "$REPO" >/dev/null
"$APPSPEC" repo check --root "$REPO" >/dev/null || fail "repo check"
ok "package"  "$PKG:$KEY"
ok "check"    "ok"

step "2/9  spec roundtrip (sha256)"
"$APPSPEC" repo show "$PKG:$KEY" --spec --root "$REPO" > "$TMP/spec.out"
EXP_SHA=$("$APPSPEC" repo show "$PKG:$KEY" --root "$REPO" | awk '/Spec sha256:/{print $3; exit}')
ACT_SHA=$(sha256sum "$TMP/spec.out" | cut -d' ' -f1)
[ -n "$EXP_SHA" ] || fail "repo show: no spec_sha256 in output"
[ "$EXP_SHA" = "$ACT_SHA" ] || fail "spec roundtrip: $EXP_SHA != $ACT_SHA"
ok "spec_sha256" "${EXP_SHA%${EXP_SHA#????????}}…"

step "3/9  reindex идемпотентен"
cp "$REPO/index.json" "$TMP/idx-before.json"
"$APPSPEC" repo reindex --root "$REPO" >/dev/null
diff -q "$TMP/idx-before.json" "$REPO/index.json" >/dev/null \
    || fail "reindex changed index.json без изменений"
ok "idempotent" "byte-identical"

step "4/9  sign → verify + tamper detection"
KEYFILE="$TMP/signing.key"
PUBKEY=$("$APPSPEC" repo sign --key "$KEYFILE" --root "$REPO" | awk '/pubkey:/{print $2}')
[ -n "$PUBKEY" ] || fail "repo sign: no pubkey"
echo "$PUBKEY" > "$TMP/pub.txt"
"$APPSPEC" repo verify --pubkey "$TMP/pub.txt" --root "$REPO" >/dev/null
ok "sign+verify" "$PUBKEY"

cp "$REPO/index.json" "$TMP/idx-signed.json"
if [ "$HAVE_PYTHON" = "1" ]; then
    python3 - "$REPO/index.json" <<'PY'
import json, sys
p = sys.argv[1]; d = json.load(open(p)); d["repo"] = "tampered"; json.dump(d, open(p, "w"))
PY
else
    sed -i 's/"repo"[[:space:]]*:[[:space:]]*"[^"]*"/"repo": "tampered"/' "$REPO/index.json"
fi
if "$APPSPEC" repo verify --pubkey "$TMP/pub.txt" --root "$REPO" >/dev/null 2>&1; then
    fail "verify должен был упасть на tampered index"
fi
cp "$TMP/idx-signed.json" "$REPO/index.json"
ok "tamper" "rejected"

step "5/9  serve + push + pull --from (HTTP)"
REPO2="$TMP/repo2"
INIT_OUT=$("$APPSPEC" repo init "$REPO2" --name "smoke/e2e-target" --token ci)
TOKEN=$(echo "$INIT_OUT" | awk '/token ci:/{print $3}')
[ -n "$TOKEN" ] || fail "init: no token"

PORT=18099
"$APPSPEC" repo serve --root "$REPO2" --listen "127.0.0.1:$PORT" >"$TMP/serve.log" 2>&1 &
SERVE_PID=$!
sleep 0.5
if [ "$HAVE_CURL" = "1" ]; then
    curl -fsS "http://127.0.0.1:$PORT/healthz" >/dev/null || fail "server did not start"
fi

APPSPEC_REPO_TOKEN="$TOKEN" "$APPSPEC" repo push --to "http://127.0.0.1:$PORT" "$SIF" >/dev/null \
    || fail "repo push"
ok "push" "ok"

REPO3="$TMP/repo3"
"$APPSPEC" repo init "$REPO3" --name "smoke/pull" >/dev/null
"$APPSPEC" repo pull "$PKG:$KEY" --from "http://127.0.0.1:$PORT" --root "$REPO3" >/dev/null
"$APPSPEC" repo check --root "$REPO3" >/dev/null
ok "pull"     "ok"
ok "check"    "ok"

kill -TERM "$SERVE_PID" 2>/dev/null || true; SERVE_PID=; wait 2>/dev/null || true

step "6/9  pull --from URL с --trusted-key (fail-closed)"
"$APPSPEC" repo sign --key "$KEYFILE" --root "$REPO2" >/dev/null
"$APPSPEC" repo serve --root "$REPO2" --listen "127.0.0.1:$PORT" >"$TMP/serve2.log" 2>&1 &
SERVE_PID=$!
sleep 0.5

REPO4="$TMP/repo4"
"$APPSPEC" repo init "$REPO4" --name "smoke/pull-signed" >/dev/null
"$APPSPEC" repo pull "$PKG:$KEY" \
    --from "http://127.0.0.1:$PORT" --trusted-key "$TMP/pub.txt" --root "$REPO4" >/dev/null \
    || fail "pull --trusted-key"
ok "verify" "signature ok"

kill -TERM "$SERVE_PID" 2>/dev/null || true; SERVE_PID=; wait 2>/dev/null || true

step "7/9  bundle check с source = repo"
BUNDLE_DIR="$TMP/bundle"; mkdir -p "$BUNDLE_DIR"
cat > "$BUNDLE_DIR/base.appspec" <<'TOML'
[package]
name    = "smoke-base"
version = "0.0.1"
[build]
base     = "alpine:3.21"
template = "alpine"
[runtime]
TOML
cat > "$BUNDLE_DIR/bundle.appspec" <<TOML
[bundle]
name = "smoke-bundle"
[bundle.base]
spec = "./base.appspec"
[[components]]
name    = "$PKG"
repo    = "$PKG:$KEY"
provide = "$PKG"
TOML
APPSPEC_REPO="$REPO" "$APPSPEC" bundle check "$BUNDLE_DIR/bundle.appspec" >/dev/null \
    || fail "bundle check"
ok "bundle check" "ok"

step "8/9  bundle lock"
APPSPEC_REPO="$REPO" "$APPSPEC" bundle lock "$BUNDLE_DIR/bundle.appspec" >/dev/null
[ -f "$BUNDLE_DIR/bundle.lock.toml" ] || fail "bundle.lock.toml not written"
grep -q "sif_sha256" "$BUNDLE_DIR/bundle.lock.toml" || fail "lock without sif_sha256"
ok "lock" "$BUNDLE_DIR/bundle.lock.toml"

step "9/9  repo export-oci --dry-run"
"$APPSPEC" repo export-oci --root "$REPO" \
    --registry "ghcr.io/smoke" --dry-run appspec-index > "$TMP/oci.log"
grep -q "oras push" "$TMP/oci.log" || fail "no oras push in dry-run log"
grep -q "application/vnd.appspec.index.v1+json" "$TMP/oci.log" || fail "no index artifact-type"
ok "dry-run" "commands printed"

# ===========================================================================
printf "\n\033[1;32mALL OK\033[0m — smoke test passed (%s mode)\n" \
    "$([ "$USE_MOCK" = "1" ] && echo mock || echo full)"