---
format: aep.planning-md/1
id: story:driver-port-carries-no-host-types
kind: story
status: draft
title: No substrate-host type crosses the driver port
summary: Plan 03 entry criterion 2 and invariant 4, proven by a structural test now; unblocked by phase order because it tests existing code.
owner: substrate
tags:
- daemon
- host
relations:
- decomposes: epic:container-driver-entry
revision: 2
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
