---
format: aep.planning-md/1
id: story:backend-recheck-hashes-only-on-change
kind: story
status: draft
title: Exec admission rechecks the backend binary without re-hashing it when its metadata is unchanged
summary: recheck_backend re-reads and SHA-256s the bwrap binary on every exec admission (process.rs:1753, probe.rs:267).
tags:
- review
relations:
- decomposes: epic:review-2026-09-03-findings
scope:
- confidence: cited
  path: crates/substrate-host/src/probe.rs
- confidence: cited
  path: crates/substrate-host/src/process.rs
revision: 5
---
# Story: Backend recheck hashes only on change

## Context

Every exec admission calls `recheck_backend`, which calls `backend_binding`, which reads the
whole bubblewrap binary and computes its SHA-256 (`crates/substrate-host/src/process.rs:1753`;
`probe.rs:267`). The binding already records device, inode and size. On a busy daemon this is one
file read and hash per exec start on the admission path.

## Acceptance

An exec admission against a bubblewrap whose device, inode, size and modification time match the probed binding completes without opening the binary, proven by a test that counts the opens.

## Notes

Keep the full hash in the startup probe; the recheck compares metadata first and hashes only when it differs. The existing `exec.capability-stale` refusal for a replaced binary must keep holding; it is already covered by `stale_snapshot_refuses_before_backend_access` and is not this story's acceptance.

## Parallel work

This story shares `crates/substrate-host/src/probe.rs` with story:backend-recheck-hashes-only-on-change, story:confined-processes-cannot-nest-user-namespaces, story:daemon-image-serves-exec-or-says-it-cannot, story:exec-oom-kills-the-whole-tree and story:seccomp-denies-af-vsock; three of them also share `crates/substrate-host/src/process.rs`. Work them one at a time, or in one wave by one implementor; `aep artifact waves` sequences them.
