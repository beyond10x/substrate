---
format: aep.planning-md/3
id: decision-blocker:wire-0-18-0-wave-approval
kind: decision-blocker
status: cleared
title: Approve the wire 0.18.0 wave
relations:
- blocks: story:exec-usage-without-io-controller
- blocks: story:read-only-host-file-bind
revision: 3
transitions:
- {from: "open", to: "cleared", at: "2026-10-11T19:54:15Z", actor: "human:timo", revision: 3}
---
# Decision blocker: approve the wire 0.18.0 wave

## Question

Approve one wave, integration branch `wave/wire-0-18-0`, one pull request, then a release.

- u1 `story:exec-usage-without-io-controller`: ADR 0032; cut `xtask/bundle-source/0.18.0/` and
  `contracts/substrate-wire/0.18.0/` from `0.17.0` with `block_io` a boolean and the I/O counters
  optional; add it to `scripts/gate.sh`; probe without `io.stat`; daemon and SDK advertise
  `0.18.0`.
- u2 `story:read-only-host-file-bind`: ADR 0033; `read_only_files` on exec and raw-pipe start in
  the same `0.18.0` bundle, `MAX_READ_ONLY_FILES` fact, `--ro-bind`, named refusals. Adversary
  review (confinement change).

Units run in sequence on the integration branch: both write the same bundle source, wire crate and
gate script, so they cannot run in parallel.

- A: approve both units.
- B: approve u1 only; u2 stays draft.
- C: hold.

Recommendation: A.

## Decision

A (operator, 2026-10-11): both units, u1 then u2 in sequence on `wave/wire-0-18-0`, one adversary pass on u2, harness untouched, release 0.7.13 after the release suite is green. Each unit builds at most 10G (du) and discards its target before the next.
