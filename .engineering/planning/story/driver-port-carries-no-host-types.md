---
format: aep.planning-md/1
id: story:driver-port-carries-no-host-types
kind: story
status: implemented
title: No substrate-host type crosses the driver port
summary: Plan 03 entry criterion 2 and invariant 4, proven by a structural test now; unblocked by phase order because it tests existing code.
owner: substrate
tags:
- daemon
- host
relations:
- decomposes: epic:container-driver-entry
revision: 6
---
# Story: No substrate-host type crosses the driver port

## Outcome

Plan 03's second entry criterion — "driver ports contain no host library types" — is a test that
fails on the first offending `use`, not a claim. A second driver can then be written against the
port without the port having quietly become the host driver's shape.

## Context

`docs/plan/03-container-driver.md` § *Entry criteria*; `AGENTS.md` invariant 4 ("clients do not
branch on driver internals"). This half of the Docker entry gate tests code that already exists
and does not wait for phase 4; the review of 2026-08-29 split it out of
`story:docker-driver-entry-gate`, which keeps the phase-5-bound half.

## Acceptance

A test in `crates/substrate-daemon` fails when any item reachable from the driver port module
resolves to a `substrate-host` type, and passes at HEAD.

Evidence that satisfies it:

- the test (`driver_port_has_no_host_types`, or a `cargo` dependency-graph assertion if the port
  is its own crate — the story records which shape was chosen and why);
- verified failing-first by adding one `substrate_host::` import to the port and watching it fail;
- `bash scripts/gate.sh` exits 0.

## Re-scoped on the finding — 2026-08-29

The port trait is a **host** type: `pub trait Driver` is `crates/substrate-host/src/lib.rs:171`,
and its signature forces six more `substrate-host` types on every caller — `DriverError`
(`lib.rs:95`), `DriverErrorClass` (`:84`), `DispatchOutcome` (`:103`),
`WorkspaceDestroyProgress` (`:111`), `ExecObservation` (re-export `:26`), `PipeStream`
(`process.rs:41`). "Driver ports contain no host library types" therefore cannot be read
literally at HEAD. The checkable claim, and the one the test enforces:

> Outside the composition root (`src/main.rs`, `src/runtime.rs`) and the in-crate test harness
> (`src/app/tests.rs`), the daemon names nothing from `substrate_host` except the port trait and
> the types its signature forces.

Concrete host types (`HostDriver`, `HostConfig`) appear only at `src/runtime.rs:21,355,360` at
HEAD. Moving `Driver` and its vocabulary into a port crate is follow-up work, not this story.

## Implemented — 2026-08-29

- `crates/substrate-daemon/tests/driver_port.rs`: `driver_port_has_no_host_types` plus two
  parser self-tests so the walker cannot pass vacuously. Stdlib only; `Cargo.lock` unchanged.
- `cargo test -p substrate-daemon --test driver_port --locked` → `3 passed; 0 failed`.
- Failing-first: `use substrate_host::{HostConfig, HostDriver};` injected into
  `src/app/execs.rs` → exit 101 naming `execs.rs:9: substrate_host::HostConfig` /
  `…HostDriver`; file restored, absent from `git diff --stat`.
- `cargo clippy -p substrate-daemon --all-targets --locked -- -D warnings` → exit 0.
