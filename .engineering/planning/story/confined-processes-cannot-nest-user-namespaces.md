---
format: aep.planning-md/1
id: story:confined-processes-cannot-nest-user-namespaces
kind: story
status: active
title: The sandbox passes --disable-userns and the probe asserts it
summary: bwrap argv lacks --disable-userns although bwrap 0.11.2 supports it (process.rs:1813-1838).
tags:
- review
relations:
- decomposes: epic:review-2026-09-03-findings
scope:
- confidence: cited
  path: crates/substrate-host/src/egress.rs
- confidence: cited
  path: crates/substrate-host/src/probe.rs
- confidence: cited
  path: crates/substrate-host/src/process.rs
- confidence: cited
  path: crates/substrate-host/src/pty.rs
revision: 7
---
# Story: Confined processes cannot nest user namespaces

## Context

The exec argv passes `--unshare-user` and not `--disable-userns`
(`crates/substrate-host/src/process.rs:1813-1838`); the three probe argv lists in `probe.rs`,
`pty.rs` and `egress.rs` match it. The installed bubblewrap 0.11.2 lists `--disable-userns` and
`--assert-userns-disabled` (`bwrap --help`). A confined process can therefore create nested user
namespaces, which is the entry point of most unprivileged kernel privilege escalations. The
current seccomp profile does not block `unshare` or `clone` with `CLONE_NEWUSER`
(`crates/substrate-host/src/seccomp.rs:70-99`).

## Acceptance

Inside an admitted exec on this host, `unshare -U` fails, observed by a delegated-lane case.

## Notes

Invariant 3: a bubblewrap too old for `--disable-userns` is a named refusal, never a sandbox without it. The probe therefore passes `--assert-userns-disabled` and withholds the exec facts when it fails; that path is delivered in this story and checked by its own probe case, separate from the acceptance above.

## Parallel work

This story shares `crates/substrate-host/src/probe.rs` with story:backend-recheck-hashes-only-on-change, story:confined-processes-cannot-nest-user-namespaces, story:daemon-image-serves-exec-or-says-it-cannot, story:exec-oom-kills-the-whole-tree and story:seccomp-denies-af-vsock; three of them also share `crates/substrate-host/src/process.rs`. Work them one at a time, or in one wave by one implementor; `aep artifact waves` sequences them.
