---
title: Release notes
description: What changed in the newest Substrate release, generated from CHANGELOG.md.
---

# What changed in the newest release

This page ends with the newest released section of `CHANGELOG.md`, and `cargo xtask check-docs`
fails when the two differ. Earlier releases are in the
[full changelog](https://github.com/beyond10x/substrate/blob/main/CHANGELOG.md). The
[confinement adapters](./concepts/adapters.md) page explains each adapter the notes name.

The newest release is 0.28.0, published on 25 November 2026. The
[status page](./status.md#newest-release) summarizes it.

A release that moves the contract bundle needs its SDK clients upgraded with the daemon. See the
[deployment rules](./guides/deployment.md#rules-that-do-not-change-by-posture).

<!-- Generated from CHANGELOG.md by `cargo xtask docs-releases`. Edit CHANGELOG.md, not the text below. -->

## 0.28.0 (2026-11-25)

### Added in 0.28.0

- An exec start and a session start carry an executable allow-list and deny-list (design 37):
  `sandbox.exec_subpaths` and `sandbox.exec_denied_paths`, at most `MAX_EXEC_SUBPATHS` and
  `MAX_EXEC_DENIED_PATHS` (64 each) unique entries of 1 to `MAX_EXEC_LIST_ENTRY_BYTES` (4,095)
  bytes. An absent allow-list is no list; a present one holds the run to its program and what
  the entries admit; a deny entry is never executed, also beneath an allow entry. The requested
  and the applied sandbox of the exec record echo both lists as sent. The three wire starts of
  the host driver apply them through the helper of design 35, as an adapter start does. A start
  without the members serializes, hashes and runs as before.
- `substrate-daemon --exec-gate <path>` names the statically linked `substrate-exec-gate`
  helper. It has no default and no environment variable and is refused beside
  `--driver kubernetes`. A helper the host driver's open refuses stops the daemon before it
  binds its socket. Without the flag neither list's fact is published.
- The capability snapshot publishes `confinement.exec-denied-paths` beside
  `confinement.exec-subpaths`, each where the host driver proved it, and `sandbox.requires.facts`
  admits `exec-denied-paths`. The published set of confinement properties is the `linux`
  adapter's proof with no exception.
- The capability snapshot's optional `withheld` says why a fact the deployment was configured
  to prove is absent: at most `MAX_WITHHELD_FACTS` (12) entries of a fact, a cause
  (`file-system-unlisted`, `mount-id-unreported`) and a configuration address
  (`config.exec_gate`, `config.workspace_root`, `config.writable_roots`), each once. An entry
  carries no host path and no file-system type. A daemon whose open withholds the facts starts
  and serves without them.
- A start that names a list is refused before the program starts by one of eight codes, the rows of `substrate_wire::EXEC_LIST_REFUSALS`:
  `exec.sandbox-exec-subpaths-empty` (422), `confinement.exec-denied-paths-without-subpaths`
  (422), `confinement.fact-unserved` (501) at `sandbox.exec_subpaths` or
  `sandbox.exec_denied_paths`, `session.pty-unserved` (501) at `mode`,
  `exec.sandbox-path-unrepresentable` (422), `exec.sandbox-exec-subpath-undeclared` (422),
  `exec.sandbox-program-unlisted` (422) and `exec.sandbox-place-unserved` (501). A list over its
  bound, an empty or over-long entry, an entry written twice and `null` are
  `request.schema-invalid` (422) at the member. The daemon checks the lexical rules and the
  first three codes in that order after `exec.capability-stale`, for every driver, and the
  operation is recorded as refused; the host driver answers the other five for an accepted
  operation, whose recorded state is `terminal` with the refusal as its error. A message names a path of the run's view and a cause by name,
  never a host path and never a file-system type. A start with an execution capsule and a list
  is `exec.sandbox-path-unrepresentable` at `argv`: its program lies beneath `/runtime`, which
  no list can name.
- Contract bundle `substrate-wire/0.29.0` (predecessor `0.28.0`, adds no route, preserves its
  45) publishes the two members on both starts and on the exec record, the fact
  `confinement.exec-denied-paths`, `withheld`, and the eight codes, each with a vector and the
  fact refusal with one per list. One vector replays a start without the members under its
  `0.28.0` request hash. The vector of `exec.sandbox-place-unserved` is published and not
  executed, because no lane has a volume whose place it refuses. `cargo xtask check-bundle
  0.29.0` holds each value to its wire constant and each refusal to its row
  (`xtask/src/bundle/exec_lists.rs`). The daemon, the SDK and the conformance runner advertise
  `substrate-wire/0.29.0` (inner `bundle.json` sha256
  `65bde1870351dfb72d4431d2dd88f0f641e91cc814007d01e0a5250517539bfd`). A client that validates
  a capability snapshot or an exec record against a bundle's schema needs the schema of
  `0.29.0`.
- The executable lists are served on `xfs` beside `ext4`, `overlay` and `tmpfs`
  (`LISTED_FILE_SYSTEMS`; the probe is design 35 § 3.14). `btrfs` stays refused by name.

### Changed in 0.28.0

- **Breaking (`b10x-substrate-host` adapter API, design 37):** a confinement profile with more
  than 64 `exec_subpaths` entries is refused `confinement.exec-subpaths-exceeded` (422), on
  every adapter, before any fact is read. Such a profile was admitted before.
- **Breaking (`b10x-substrate-wire` and `b10x-substrate-host` public API, design 37):** six
  public Rust types gain a member, and none is `#[non_exhaustive]`: a consumer that writes
  one as a struct literal, or matches an enum without a wildcard, adds a line. In
  `b10x-substrate-wire`: `ConfinementRequest` and `AppliedConfinement` gain `exec_subpaths`
  (`Option<Vec<String>>`) and `exec_denied_paths` (`Vec<String>`), `CapabilitySnapshot` and
  `CapabilityFacts` one member each for `withheld` and the deny-list's fact, and
  `ConfinementProperty` the variant `ExecDeniedPaths`. In `b10x-substrate-host`:
  `ConfinementError` gains `ExecSubpathsExceeded`.
- A metrics scrape carries at most 593 samples, 587 before: the refusal series gain the six
  codes above that are new to the wire crate.
- The request for the pack budget in the published `gix-pack`, which this repository carries
  as `vendor/gix-pack-0.74.2`, is <https://github.com/GitoxideLabs/gitoxide/issues/3244>.

### Fixed in 0.28.0

- A startup probe of the host driver ends when its sandbox has ended. The terminal probe behind
  `sessions.pty` and the egress aperture probe answer that the mechanism is not proved once the
  backend they started is gone. Their deadlines, 10 s and 5 s, are unchanged and bound a sandbox
  that stays and says nothing. No fact changes: a backend that proved a mechanism before proves
  it now. Held by `a_sandbox_that_ends_at_once_ends_the_probe_before_its_deadline` in
  `pty::tests` and in `egress::tests`, each through a backend that exits at once and with half
  the deadline as its bound.
