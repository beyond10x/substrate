---
format: aep.planning-md/3
id: story:macos-seatbelt-confinement
kind: story
status: implemented
title: A Seatbelt profile confines a command on macOS, proven by an adversary suite on macos-15
scope:
- confidence: cited
  path: .github/workflows/macos.yml
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: adr/0037-macos-confinement-is-the-platform-sandbox.md
- confidence: cited
  path: crates/substrate-host/src/lib.rs
- confidence: cited
  path: crates/substrate-host/src/seatbelt.rs
- confidence: cited
  path: crates/substrate-host/tests/seatbelt_adversary.rs
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-10-21T17:13:15Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-21T17:13:16Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-21T18:59:38Z", actor: "human:timo", revision: 7, decided_on: {"recorded":{"test_result":2}}}
---
## Outcome
`b10x-substrate-host` gains `seatbelt`, the macOS confinement primitive ADR 0037 decides: a policy (one writable workspace, read-only roots, an exec allowlist, no network) renders to an SBPL profile that starts with `(deny default)`, and on macOS a command runs under that profile through `/usr/bin/sandbox-exec` with a cleared environment.

A path SBPL cannot carry as a plain literal (relative, a quote, a backslash, a control character) is refused before anything runs. A missing `sandbox-exec` refuses as `exec.sandbox-unavailable`. `HostDriver::open` keeps refusing on macOS; wiring the driver and its capability facts is a later story.

Design: [ADR 0037](../../../adr/0037-macos-confinement-is-the-platform-sandbox.md). No wire change.

## Acceptance
- Profile rendering, path refusal, the unlisted-program refusal and the missing-`sandbox-exec` refusal (`exec.sandbox-unavailable`) are unit-tested on every platform.
- `crates/substrate-host/tests/seatbelt_adversary.rs` runs on GitHub's `macos-15` runner in `.github/workflows/macos.yml`. Each attack below is refused, and each has a positive control that proves the same command succeeds where the profile allows it:
  - a write outside the workspace, and a write into a read-only root;
  - a read of an undeclared host file;
  - an outbound TCP connection to a listener the test owns;
  - an exec of a binary outside the allowlist;
  - a signal to a process outside the sandbox;
  - an inherited environment variable (`TMPDIR`) reaching the command.
- ADR 0037's tables mark each row the suite proves as tested, naming its case.
- The Linux workspace gate (`scripts/gate.sh`) passes unchanged.

## Scope
`crates/substrate-host/src/{lib.rs,seatbelt.rs}`, `crates/substrate-host/tests/seatbelt_adversary.rs`, `.github/workflows/macos.yml`, `adr/0037-macos-confinement-is-the-platform-sandbox.md`, `CHANGELOG.md`.
