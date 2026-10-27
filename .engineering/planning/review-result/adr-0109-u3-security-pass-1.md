---
format: aep.planning-md/3
id: review-result:adr-0109-u3-security-pass-1
kind: review-result
status: active
title: Security review pass 1 on story:confinement-contract-on-linux-and-local
relations:
- reviews: story:confinement-contract-on-linux-and-local
revision: 1
---
# Security review pass 1: story:confinement-contract-on-linux-and-local

Verdict: needs change. Read at integration 493d72a; 9 cases added in
`crates/substrate-host/tests/security_review_confinement.rs`, 5 red.

## Findings

- S1 `crates/substrate-host/src/process.rs:2422` (warning): `render` skips the environment, argv
  and limit checks `start` makes, so it returns an argv carrying a credential-named `--setenv`
  value for a command `start` refuses `exec.environment-refused`.
- S2 `confinement/local.rs:272`, `confinement/linux.rs:346` (warning): a start refused after
  admission writes no audit record, against atlas ADR 0109 decision 2.
- S3 `confinement/local.rs:278` (warning): `local` passes a credential-named environment variable
  to an unconfined host process that `linux` refuses.
- S4 `confinement/local.rs:201` (note): a symlinked workdir is followed out of the workspace.
- S5 `confinement/facts.rs:69` (note): a cloned `AdapterFacts` with `local_permitted` added admits
  a profile that a refusing `local` adapter then starts.

## Verified to hold

`linux` starts go through `HostDriver::start_exec` and `ProcessRuntime::admit` with every member
of the isolation floor; `render` builds its argv with the same `command()` and no shell; linux
workspace escapes are refused; no subject, tenant or uid is read from request data; an absent
family or fact is a named refusal; `delegated-lane.sh` fails when the linux lane is absent.

## Decision taken by the coordinator

S3: `local` applies the same environment shape check as `linux`.
