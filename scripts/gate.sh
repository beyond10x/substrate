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

run cargo test --workspace --locked
run cargo fmt --all --check
run cargo clippy --workspace --all-targets --locked -- -D warnings
run cargo xtask check-links
run cargo xtask check-adrs
run python3 scripts/check-contract-bundle.py
run python3 scripts/check-contract-bundle-0.2.0.py
run python3 scripts/check-contract-bundle-0.3.0.py
run python3 scripts/check-contract-bundle-0.4.0.py
# 0.5.0 and every successor are checked by the Rust verb: the four Python pairs stay only as the
# reproducibility proof of the bundles they froze (AGENTS.md, "The gate's own checks are cargo
# xtask verbs").
run cargo xtask check-bundle 0.5.0
run python3 scripts/test_contract_json_gate.py
run cargo xtask check-toolchain


printf 'gate: passed\n'
