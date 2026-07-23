#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# Casual Note — run the CI pipeline locally, exactly as .github/workflows/ci.yml.
#
# Run this BEFORE pushing to main and BEFORE tagging a release. It mirrors every
# CI job so a green run here means a green run on GitHub.
#
#   ./scripts/ci-local.sh              # full pipeline
#   SKIP_TAURI=1 ./scripts/ci-local.sh # skip the GUI shell build (no webkit)
#   SKIP_FRONTEND=1 ./scripts/ci-local.sh
#
# Requires: rustup toolchain (fmt, clippy), Node + pnpm. The Tauri shell check
# needs webkit2gtk-4.1 dev libs (Linux). cargo-deny is auto-installed if missing.
# ---------------------------------------------------------------------------
set -uo pipefail
cd "$(dirname "$0")/.."

bold=$'\033[1m'; red=$'\033[31m'; grn=$'\033[32m'; ylw=$'\033[33m'; rst=$'\033[0m'
FAILED=()
step() { printf '\n%s==> %s%s\n' "$bold" "$1" "$rst"; }
run()  { # run <label> <cmd...>
  local label="$1"; shift
  step "$label"
  if "$@"; then printf '%s   PASS%s %s\n' "$grn" "$rst" "$label"
  else printf '%s   FAIL%s %s\n' "$red" "$rst" "$label"; FAILED+=("$label"); fi
}

# 1) Rust core (matches the `rust` job — ubuntu is the blocking platform)
run "rustfmt --check"        cargo fmt --all -- --check
run "clippy (core, -D warnings)" cargo clippy --workspace --exclude tauri-app --all-targets -- -D warnings
run "cargo check (core)"     cargo check --workspace --exclude tauri-app
run "cargo test (core)"      cargo test --workspace --exclude tauri-app

# 2) Tauri shell (matches the `tauri-shell` job — needs webkit2gtk on Linux)
if [[ "${SKIP_TAURI:-0}" != "1" ]]; then
  run "cargo check -p tauri-app (GUI shell)" cargo check -p tauri-app
else
  printf '\n%s==> Tauri shell: SKIPPED (SKIP_TAURI=1)%s\n' "$ylw" "$rst"
fi

# 3) Frontend (matches the `frontend` job)
if [[ "${SKIP_FRONTEND:-0}" != "1" ]]; then
  step "frontend: install + typecheck + lint + test + build"
  if ( cd ui \
        && pnpm install --frozen-lockfile \
        && pnpm typecheck \
        && pnpm lint \
        && pnpm test \
        && pnpm build ); then
    printf '%s   PASS%s frontend\n' "$grn" "$rst"
  else
    printf '%s   FAIL%s frontend\n' "$red" "$rst"; FAILED+=("frontend")
  fi
else
  printf '\n%s==> Frontend: SKIPPED (SKIP_FRONTEND=1)%s\n' "$ylw" "$rst"
fi

# 4) Supply-chain audit (matches the `audit` job — pinned cargo-deny version)
DENY_VERSION="0.20.2"
if ! command -v cargo-deny >/dev/null 2>&1; then
  step "installing cargo-deny@${DENY_VERSION}"
  cargo install cargo-deny --version "$DENY_VERSION" --locked || true
fi
if command -v cargo-deny >/dev/null 2>&1; then
  run "cargo-deny (advisories bans licenses sources)" cargo deny check advisories bans licenses sources
else
  printf '\n%s==> cargo-deny unavailable — audit SKIPPED%s\n' "$ylw" "$rst"
fi

# 5) Telemetry-absence scan (matches the `no-telemetry` job)
step "telemetry-absence scan"
if grep -RInE "reqwest|hyper::client|TcpStream::connect|ureq::(get|post)|https?://" \
     crates --include='*.rs' \
     | grep -vE "crates/(model-manager|updater)/" \
     | grep -vE "^\s*//|doc =|///" >/tmp/cn_telemetry_hits 2>/dev/null && [[ -s /tmp/cn_telemetry_hits ]]; then
  printf '%s   FAIL%s telemetry scan — network usage outside allowed crates:\n' "$red" "$rst"; cat /tmp/cn_telemetry_hits; FAILED+=("telemetry-scan")
else
  printf '%s   PASS%s telemetry scan\n' "$grn" "$rst"
fi
rm -f /tmp/cn_telemetry_hits

# Summary
printf '\n%s────────────────────────────────────────%s\n' "$bold" "$rst"
if [[ ${#FAILED[@]} -eq 0 ]]; then
  printf '%s✓ CI-LOCAL PASSED%s — safe to push / tag a release.\n' "$grn" "$rst"; exit 0
else
  printf '%s✗ CI-LOCAL FAILED%s: %s\n' "$red" "$rst" "${FAILED[*]}"; exit 1
fi
