#!/usr/bin/env bash
# Full component gate for the standalone Substrate repository.
# Runs tests and Clippy in one release-profile artifact graph, then formatting and
# this component's documented script checks.
#
#     bash scripts/gate.sh           every step, in this order: the bar for `main`
#     bash scripts/gate.sh tests     the `tests` phase alone: workspace tests and Clippy
#     bash scripts/gate.sh static    the `static` phase alone: formatting, the `cargo xtask`
#                                    checks and the four frozen Python bundle checkers
#
# CI runs the two phases as parallel jobs and requires both (`.github/workflows/gate.yml`). Every
# step belongs to exactly one phase, so the two phases together are the whole gate.
set -euo pipefail

if [[ "$#" -gt 1 ]]; then
  echo "gate: takes at most one argument, a phase (tests or static); got: $*" >&2
  exit 2
fi
phase="${1:-all}"
case "${phase}" in
  all | tests | static) ;;
  *)
    echo "gate: unknown phase '${phase}'; the phases are tests and static" >&2
    exit 2
    ;;
esac

root=$(git rev-parse --show-toplevel)
cd "$root"

# step <phase> <command>...: runs the command when the gate runs every step or that phase.
step() {
  local owner="$1"
  shift
  if [[ "${phase}" == all || "${phase}" == "${owner}" ]]; then
    printf 'gate: %s\n' "$*"
    "$@"
  fi
}

step static cargo xtask check-ess
step static cargo xtask check-docs
step tests cargo test --workspace --release --locked
step static cargo fmt --all --check
step tests cargo clippy --workspace --all-targets --release --locked -- -D warnings
step static cargo xtask check-links
step static cargo xtask check-adrs
step static cargo xtask check-secrets
step static cargo xtask check-advisories
step static cargo xtask check-licenses
step static cargo xtask check-packages
step static cargo xtask check-mcp-boundary
step static python3 scripts/check-contract-bundle.py
step static python3 scripts/check-contract-bundle-0.2.0.py
step static python3 scripts/check-contract-bundle-0.3.0.py
step static python3 scripts/check-contract-bundle-0.4.0.py
# 0.5.0 and every successor are checked by the Rust verb: the four Python pairs stay only as the
# reproducibility proof of the bundles they froze (AGENTS.md, "The gate's own checks are cargo
# xtask verbs").
step static cargo xtask check-bundles \
  0.5.0 0.6.0 0.7.0 0.8.0 0.9.0 0.10.0 0.11.0 0.12.0 0.13.0 0.14.0 0.15.0 0.16.0 0.17.0 0.18.0 \
  0.19.0 0.20.0 0.21.0 0.22.0 0.23.0 0.24.0
step static cargo xtask check-json
step static cargo xtask check-toolchain

if [[ "${phase}" == all ]]; then
  printf 'gate: passed\n'
else
  printf 'gate: phase %s passed\n' "${phase}"
fi
