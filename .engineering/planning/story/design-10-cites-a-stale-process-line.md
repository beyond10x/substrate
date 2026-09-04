---
format: aep.planning-md/1
id: story:design-10-cites-a-stale-process-line
kind: story
status: draft
title: Design 10's egress evidence row cites the argv where it is
scope:
- confidence: cited
  path: docs/design/10-destination-bound-egress.md
revision: 2
---
# Story: Design 10's egress evidence row cites the argv where it is

## Context

`docs/design/10-destination-bound-egress.md:34` cites
`crates/substrate-host/src/process.rs:901-905` as the evidence for the sandbox argv. The argv is at
`process.rs:1902-1928`. The row is byte-identical at `617bbed`, so the citation was already wrong
before the 2026-09-04 security wave; it was found by that wave's adversarial passes over
`story:confined-processes-cannot-nest-user-namespaces`
(`review-result:adversary-u4-pass-1`, `-pass-2`).

The same row states the posture as "`--unshare-net` with `--unshare-user/ipc/pid/uts`". That is no
longer the whole argv: the exec and all seven other bubblewrap argv lists now also carry
`--disable-userns`, spliced from `crates/substrate-host/src/process.rs`'s `USER_NAMESPACE_ARGV`.

## Acceptance

`docs/design/10-destination-bound-egress.md`'s evidence row cites the line range where the sandbox
argv actually is and states the posture the code builds, checked by `cargo xtask check-links` and by
reading.

## Notes

`cargo xtask check-links` verifies that a repository-relative target exists; it does not verify that
a cited line number says what the citing document claims. A citation that drifts is invisible to the
gate, which is why this needed an adversary to find and why a line-range citation in a design
document is a liability worth reviewing more widely than this one row.
