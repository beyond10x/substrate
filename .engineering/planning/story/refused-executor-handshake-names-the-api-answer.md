---
format: aep.planning-md/3
id: story:refused-executor-handshake-names-the-api-answer
kind: story
status: active
title: A refused executor handshake names the API's answer at once
relations:
- decomposes: epic:confinement-contract-family
- informed_by: approval-record:d342-substrate-wave-2026-11-01
scope:
- confidence: cited
  path: crates/substrate-kubernetes-client/src/exec.rs
- confidence: inferred
  path: crates/substrate-kubernetes/src/namespace/probe_tests.rs
- confidence: cited
  path: crates/substrate-kubernetes/src/namespace/workspace.rs
- confidence: cited
  path: xtask/src/namespace_lane_run.rs
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-11-01T05:07:15Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-01T05:07:15Z", actor: "human:timo", revision: 3}
---
# Story: a refused executor handshake names the API's answer at once

## Observation

The 0.18.1 namespace lane at `669c5bb8` (2026-11-01, 04:50-04:58Z) gave no record: the case
"S6 Role without pods/exec create" saw only the code `kubernetes.executor-handshake-failed` within
120 s, and no `WARN` line after the first probe carried `cannot create resource "pods/exec"`, which
`story:namespace-lane-verb-case-accepts-only-its-own-refusal` requires. The probe's handshake
(`await_handshake`, `crates/substrate-kubernetes/src/namespace/workspace.rs`) retries every refusal
until `CREATE_BOUND` (60 s) and the exec upgrade's refusal is built from the response body
(`crates/substrate-kubernetes-client/src/exec.rs`). Inferred, not verified: the API's text does not
reach the warning, either because the upgrade response's body is not read or because the bounded
message cuts it.

## Acceptance

- A red case first: `FakeCluster` refuses the exec upgrade with 403 and a `Status` body whose
  message says `cannot create resource "pods/exec"`; the probe's warning carries that text.
- An exec upgrade refused 4xx (other than 408 and 429) ends the handshake at once with the API's
  answer, not after `CREATE_BOUND`; other failures keep retrying as now.
- The namespace lane keeps the daemon log of a case that gave no record, so the next failure can be
  read.
