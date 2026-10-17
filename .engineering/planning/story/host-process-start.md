---
format: aep.planning-md/3
id: story:host-process-start
kind: story
status: draft
title: Start a host process under the explicit host profile
relations:
- decomposes: epic:host-process-supervision
- informed_by: architecture-decision-record:host-processes-are-a-supervised-resource-family
revision: 1
---
# Story: start a host process under the explicit host profile

## Context

A caller needs a long-lived process in its own repository directory with the network and host ports, and today has only confined exec, which has no egress. Slices S1 and S2 are one story: a start without the `host` profile is refused and `host` is the only profile, so neither can be accepted without the other.

Decided by `adr/0036-host-processes-are-a-supervised-resource-family.md` (S1, S2). Specified in `spec/host-processes/`; the specification change lands first and its conformance suite drives the implementation.

## Scope

- `crates/substrate-wire/src/lib.rs`: the start request, the process record, `profile`/`confinement`, the `process.host` fact and the start refusals.
- `crates/substrate-daemon/src/app/`: a new processes module serving `POST /v1/processes` and `GET /v1/processes/{process_id}`; posture and peer-uid admission.
- `crates/substrate-host/src/`: the unconfined spawn (direct argv, `openat2` cwd, exact environment).
- `crates/substrate-store/`: the durable record, storing env names and digest, never values.

- `spec/host-processes/`: the scenarios below.
- `xtask/bundle-source/<successor>/`: the schemas, routes and vectors this slice adds to the successor bundle after 0.18.0.

## Acceptance

The following named conformance scenarios in `spec/host-processes/` pass, the delegated ones in the delegated lane and reported absent where the lane is absent:

- `host-process-start-records-owner-reason-and-lease`
- `host-process-start-runs-in-caller-cwd-with-exact-env`
- `host-process-record-reports-confinement-none`
- `host-process-env-values-are-never-persisted`
- `host-process-start-without-profile-is-refused` (`process.profile-required`)
- `host-process-unknown-profile-is-refused` (`process.profile-unknown`)
- `host-process-invalid-argv-cwd-env-owner-are-refused` (`process.argv-invalid`, `process.cwd-invalid`, `process.env-invalid`, `process.owner-invalid`)
- `host-process-start-without-reason-is-refused` (`process.reason-required`)
- `host-process-duplicate-owner-name-is-refused` (`process.name-conflict`)
- `host-process-refused-over-tcp-bearer-and-hosted-postures` (`process.host-posture-refused`)
- `host-process-refused-for-a-foreign-peer-uid` (`process.host-subject-mismatch`)
- `host-process-unserved-without-delegated-cgroup` (`process.host-unserved`)
- `exec-sandbox-unavailable-never-becomes-a-host-process`
- `host-process-start-interrupted-before-dispatch-is-abandoned`
