---
format: aep.planning-md/3
id: story:workspace-expiry-refuses-on-unreadable-execs
kind: story
status: draft
title: Workspace lease expiry refuses when its execs cannot be read
relations:
- informed_by: review-result:adversary-waveb-u1-pass-1
revision: 1
---
# Workspace lease expiry refuses when its execs cannot be read

## Defect

Adversary review `review-result:adversary-waveb-u1-pass-1`, finding 3, still open on main: the
workspace branch of lease expiry in `crates/substrate-daemon/src/app/service.rs` reads the execs
holding the workspace with `if let Ok(execs)` and no else. A failed store read falls through to
destroying the workspace without signalling its running execs and without a named failure,
which invariant 3 forbids.

## Acceptance

- A failed read of the workspace's execs fails that expiry round the way a failed exec write
  does: the workspace stays, nothing is destroyed, and the lease is retried by the next sweep.
- Once the read succeeds again, the next sweep signals the execs and destroys the workspace.
- One daemon test holds both halves; it runs once, in seconds.
