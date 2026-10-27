---
format: aep.planning-md/3
id: review-result:adr-0109-u6-security-pass-1
kind: review-result
status: active
title: Security review pass 1 on story:oci-adapter
relations:
- reviews: story:oci-adapter
revision: 1
---
# Security review pass 1: story:oci-adapter

Verdict: needs change. Read at the unit head bf4d486 (integration 15cbdba); 5 cases added in
`crates/substrate-host/tests/security_review_oci.rs`, 4 red. Lane run against Docker 29.7.2.

## Findings

- F1 `crates/substrate-host/src/confinement/oci.rs:609` (blocker): any canonical host directory,
  `/` included, is bound writable as `/workspace`; there is no operator workspace root.
- F2 `oci.rs:861` (warning): the environment check refuses only `=` and NUL; linux's
  `validate_environment` also refuses secret-like and non-uppercase names, counts and sizes.
- F3 `oci.rs:511` (warning): on a runtime without limit control a profile with no bounds runs
  with no pids, memory or CPU bound.
- F4 `oci.rs:623` (warning): a read-only root whose host directory holds the runtime socket is
  bound into the container.
- F5 `oci.rs:334` (warning, infeasible today): a root embedder runs the container as `--user 0:0`.
- F6 `oci.rs:1015` (warning): the timeout and CPU budget stop holding when the embedder dies; open
  does not sweep orphaned labelled containers.
- F7 `docs/design/04-security-and-isolation.md:32` (note): the bind-path race is acceptable as
  recorded; `-v` creates a missing source as root where `--mount type=bind` refuses it.
- F8 `oci.rs:983` (note): refusals at start or render time write no audit record.

## Verified to hold

Runtime path only from `OciRequest`, argv-only; image admission needs 64 lowercase hex after
`@sha256:` and comes after `--`; `image_digest_pinned` enforced at open; open fails closed without
seccomp; `--network none`, `--cap-drop ALL`, `no-new-privileges`, `--read-only`, no host namespace
or device flag; containers removed after wait, kill and timeout; output bounded; writable roots
operator-declared; with `SUBSTRATE_OCI_LANE=1` and no runtime the lane fails, never ABSENT.

## Decisions taken by the coordinator

F1: `OciRequest` gains an operator workspace root; the workspace must sit directly beneath it.
F2: oci applies linux's environment check. F3: a run never starts without pids and memory bounds;
otherwise a named refusal. F4: a root that is or contains the runtime socket is refused by name.
F5: a uid 0 embedder is refused `confinement.adapter-unserved`. F6: containers carry the
embedder's identity and `open` removes and reports orphans. F7: binds use `--mount type=bind`.
F8: follows S0's start-time audit fix.
