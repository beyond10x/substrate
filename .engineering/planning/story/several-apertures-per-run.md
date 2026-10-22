---
format: aep.planning-md/3
id: story:several-apertures-per-run
kind: story
status: implemented
title: One run reaches several declared egress apertures
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-22T04:37:18Z", actor: "agent:b10x-chef-orchestrator", revision: 3, executor: "agent:substrate-wave-20261022", correlation: "wave-2026-10-22-confinement-gaps"}
- {from: "proposed", to: "active", at: "2026-10-22T04:37:18Z", actor: "agent:b10x-chef-orchestrator", revision: 4, executor: "agent:substrate-wave-20261022", correlation: "wave-2026-10-22-confinement-gaps"}
- {from: "active", to: "implemented", at: "2026-10-22T08:41:43Z", actor: "agent:b10x-chef-orchestrator", revision: 5, decided_on: {"recorded":{"test_result":1,"review_outcome":1}}}
---
# One run reaches several declared apertures

takt's bot-route push needs `api.github.com` (token mint) and `github.com` (push) in one run; Loom
0.33.0 allows one aperture per run because Substrate's `ConfinementRequest.aperture` is one name.
Design `docs/design/26-working-directory-and-several-apertures.md` § 2 adds `apertures`, bundle
`0.21.0`. ADR 0013 fixes no count; design 10 § 9 decision 5 limited only the first slice.

## Acceptance

- A start with `apertures: ["a", "b"]`, both declared on port 443, reaches each through its own
  forwarder: `a` on `127.0.0.1`, `b` on `127.0.0.2`, each name mapped in the generated `/etc/hosts`.
- The applied network is an array of two applied apertures, each with its own bytes, ceiling and `listen`.
- A start with `aperture: "a"` reports the single applied object without `listen`, as on `0.20.0`.
- Both fields present: `exec.aperture-mode-mismatch`; one name in `apertures`: `exec.apertures-too-few`;
  five names: `exec.apertures-too-many`; a repeated name: `exec.aperture-listed-twice`. All 422, before dispatch.
- An undeclared name among several is refused naming that name; nothing is installed.
- A failure installing the second aperture kills the first forwarder; no forwarder outlives the refusal.
- One aperture reaching its ceiling ends the run as on `0.20.0`.

## Scope

`crates/substrate-wire/src/lib.rs`, `crates/substrate-host/src/egress.rs`,
`crates/substrate-host/src/process.rs`, the daemon admission path, bundle `0.21.0`, the delegated
lane's aperture cases.
