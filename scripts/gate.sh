#!/usr/bin/env bash
# Full component gate for the standalone Substrate repository.
# Mirrors the monorepo gate for foundation/substrate (cargo test in --release,
# fmt + clippy in --all) plus this component's documented script checks.
set -euo pipefail

root=$(git rev-parse --show-toplevel)
cd "$root"

run() {
  printf 'gate: %s\n' "$*"
  "$@"
}

run cargo test --workspace --release --locked
run cargo fmt --all --check
run cargo clippy --workspace --all-targets --locked -- -D warnings
run cargo xtask check-links
run cargo xtask check-adrs
run cargo xtask check-secrets
run cargo xtask check-advisories
run cargo xtask check-licenses
run cargo xtask check-packages
run python3 scripts/check-contract-bundle.py
run python3 scripts/check-contract-bundle-0.2.0.py
run python3 scripts/check-contract-bundle-0.3.0.py
run python3 scripts/check-contract-bundle-0.4.0.py
# 0.5.0 and every successor are checked by the Rust verb: the four Python pairs stay only as the
# reproducibility proof of the bundles they froze (AGENTS.md, "The gate's own checks are cargo
# xtask verbs").
run cargo xtask check-bundle 0.5.0
run cargo xtask check-bundle 0.6.0
run cargo xtask check-bundle 0.7.0
run cargo xtask check-bundle 0.8.0
run cargo xtask check-bundle 0.9.0
run cargo xtask check-bundle 0.10.0
run cargo xtask check-bundle 0.11.0
run cargo xtask check-bundle 0.12.0
run cargo xtask check-json
run cargo xtask check-toolchain


printf 'gate: passed\n'
