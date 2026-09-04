---
format: aep.planning-md/1
id: story:the-delegated-lane-script-honours-cargo-target-dir
kind: story
status: draft
title: The delegated lane script honours CARGO_TARGET_DIR
scope:
- confidence: cited
  path: scripts/delegated-lane.sh
revision: 2
---
# Story: The delegated lane script honours CARGO_TARGET_DIR

## Context

`scripts/delegated-lane.sh` builds and then invokes `${PWD}/target/debug/substrate-daemon` by a
hardcoded path. Under any external `CARGO_TARGET_DIR` — which every worktree-per-unit workflow sets,
because two trees must never share one build directory — the binary is not there and the step fails
to find what it just built.

Found during the 2026-09-04 security wave: unit u4 could not run the full lane for this reason and
ran a hand-copied host half instead, in the wave's own scratch directory: a runner that
reproduces the script's systemd-scope, controller check and process-free-root setup verbatim and
then runs only the first `cargo test`. That is a correct workaround and a bad standing arrangement:
the copy drifts from the script it was copied from, and it lives outside the repository, so nothing
here can see it rot.

## Acceptance

`bash scripts/delegated-lane.sh` completes its daemon, host, SDK and MCP steps with
`CARGO_TARGET_DIR` set to a directory outside the repository.

## Notes

`cargo build --message-format=json` reports the executable path it produced, which removes the
guess entirely. Failing that, `${CARGO_TARGET_DIR:-$PWD/target}` is the one-line form.

The same script also builds three packages beyond the daemon, which is why a package-scoped unit
cannot run it whole. Worth deciding whether the lane should take a package filter.
