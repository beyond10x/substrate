---
format: aep.planning-md/1
id: story:seccomp-denies-af-vsock
kind: story
status: implemented
title: The seccomp profile refuses AF_VSOCK sockets and a probe proves it
summary: AF_VSOCK is not netns-confined and the profile allows it (seccomp.rs:70-99); inferred, untested on a vsock host.
tags:
- review
relations:
- decomposes: epic:review-2026-09-03-findings
scope:
- confidence: cited
  path: crates/substrate-host/src/probe.rs
- confidence: cited
  path: crates/substrate-host/src/process.rs
- confidence: cited
  path: crates/substrate-host/src/seccomp.rs
- confidence: cited
  path: crates/substrate-host/tests/alg_family_host_state.rs
- confidence: cited
  path: crates/substrate-host/tests/qrtr_family_confinement.rs
revision: 9
---
# Story: seccomp denies AF_VSOCK

## Context

The seccomp profile denies `io_uring_setup`, `socket(AF_UNIX, …)` and
`socketpair(AF_UNIX, SOCK_DGRAM)` and allows every other socket family
(`crates/substrate-host/src/seccomp.rs:70-99`). `AF_VSOCK` is not confined by a network namespace,
so on a virtual-machine host that exposes a vsock transport a confined process can open a socket to
the hypervisor side while `--unshare-net` holds. The observed development nodes are EKS virtual
machines (`STATUS.md` § Current state). Inferred from the kernel's vsock namespace model; not
tested on such a host.

## Acceptance

`socket(AF_VSOCK, SOCK_STREAM, 0)` inside an admitted exec fails with `EACCES`, the probe's sentinel
case includes it, and `seccomp::tests` carries the same case beside the existing `AF_UNIX` ones.

## Notes

One more `jump` against `libc::AF_VSOCK` in `filters`. Consider the same for `AF_NETLINK` and
`AF_PACKET` and record in the story why each is or is not refused.

## Parallel work

This story shares `crates/substrate-host/src/probe.rs` with story:backend-recheck-hashes-only-on-change, story:confined-processes-cannot-nest-user-namespaces, story:daemon-image-serves-exec-or-says-it-cannot, story:exec-oom-kills-the-whole-tree and story:seccomp-denies-af-vsock; three of them also share `crates/substrate-host/src/process.rs`. Work them one at a time, or in one wave by one implementor; `aep artifact waves` sequences them.
