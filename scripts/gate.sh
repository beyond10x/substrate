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
#
# The test step has a budget (AGENTS.md, section "The gate"): it ends within 120 s and no test runs
# longer than 60 s. cargo-nextest runs it, kills a test at 60 s and cancels the run at 120 s, and
# each of the two fails the step (`.config/nextest.toml`). The build and the run are two steps,
# each followed by a `gate-time:` line with its wall seconds, so the run's time is read without the
# compile; the run is followed by its ten slowest tests.
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

# timed <phase> <label> <command>...: a step that also prints the wall seconds it took, whether it
# passed or failed, as `gate-time: <label>: <n> s`.
timed() {
  local owner="$1" label="$2"
  shift 2
  if [[ "${phase}" == all || "${phase}" == "${owner}" ]]; then
    printf 'gate: %s\n' "$*"
    local started="${SECONDS}" status=0
    "$@" || status=$?
    printf 'gate-time: %s: %d s\n' "${label}" "$((SECONDS - started))"
    return "${status}"
  fi
}

target_dir="${CARGO_TARGET_DIR:-${root}/target}"
junit="${target_dir}/nextest/default/junit.xml"
# The test runs name their thread count and their retries themselves. cargo-nextest reads
# `NEXTEST_TEST_THREADS` and `NEXTEST_RETRIES` before `.config/nextest.toml`, so without the
# options a caller's environment decides both: the workstation's build pool sets a thread count
# for every job, and a retry count would turn a test that fails once into a passed one. The
# thread count has one home, the configuration file, and is read from it here.
test_threads=
if [[ "${phase}" == all || "${phase}" == tests ]]; then
  test_threads=$(sed -n 's/^test-threads = \([0-9][0-9]*\)$/\1/p' "${root}/.config/nextest.toml")
  if [[ ! "${test_threads}" =~ ^[0-9]+$ ]]; then
    echo "gate: .config/nextest.toml does not name one test-threads count" >&2
    exit 2
  fi
fi

step static cargo xtask check-ess
step static cargo xtask check-docs
# `cargo fetch` without `--target` downloads every target's dependencies: the package-rule cases
# run `cargo tree --target all` offline, and a cold cache holds only what the host build fetched.
step tests cargo fetch --locked
# The build step compiles every test binary and the binaries the tests start, the daemon among
# them, once; the run step then compiles nothing and no test builds anything.
timed tests 'test build' cargo nextest run --workspace --release --locked --no-run
# The report of an earlier run is removed, so the ranking is never read from one.
if [[ "${phase}" == all || "${phase}" == tests ]]; then
  rm -f "${junit}"
fi
tests_status=0
timed tests 'test run' cargo nextest run --workspace --release --locked \
  --test-threads "${test_threads}" --retries 0 || tests_status=$?
# The ten slowest tests, printed whether the run passed or failed. This is the release `xtask`
# the build step left beside the test binaries: `cargo xtask` here would compile a second,
# dev-profile one, and `cargo run --release -p xtask` a third, because a package-scoped build
# resolves other features than the workspace's test build.
slowest_status=0
step tests "${target_dir}/release/xtask" slowest-tests --junit "${junit}" || slowest_status=$?
if [[ "${tests_status}" -ne 0 ]]; then
  exit "${tests_status}"
fi
if [[ "${slowest_status}" -ne 0 ]]; then
  exit "${slowest_status}"
fi
# cargo-nextest runs no doctest.
timed tests 'doctests' cargo test --workspace --release --locked --doc
step static cargo fmt --all --check
step tests cargo clippy --workspace --all-targets --release --locked -- -D warnings
# A confinement-only consumer takes the host driver with `default-features = false`, without `git`
# and `kubernetes`: that build lints clean, and the confinement cases that need neither run in it,
# each target named so a renamed one fails instead of shrinking the lane. The variable makes
# `host_features` fail if that build has either feature after all.
step tests cargo clippy -p b10x-substrate-host --no-default-features --all-targets --release --locked \
  -- -D warnings
timed tests 'no-default lane, its build included' env SUBSTRATE_HOST_NO_DEFAULT_FEATURES=1 \
  cargo nextest run -p b10x-substrate-host --no-default-features --release --locked \
  --test host_features --test ess_confinement_local --test consumer_surface \
  --test-threads "${test_threads}" --retries 0
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
  0.19.0 0.20.0 0.21.0 0.22.0 0.23.0 0.24.0 0.25.0 0.26.0 0.27.0
step static cargo xtask check-json
step static cargo xtask check-toolchain

if [[ "${phase}" == all ]]; then
  printf 'gate: passed\n'
else
  printf 'gate: phase %s passed\n' "${phase}"
fi
